use std::cmp;
use std::ops::ControlFlow;
use std::time::Duration;

use anyhow::Context;
use egui::Color32;
use rusb::UsbContext;

use crate::DeviceId;

pub(crate) struct CameraParams {
    pub texture: egui::TextureHandle,
    pub ctx: egui::Context,
    pub devid_rx: flume::Receiver<DeviceId>,
    pub devid: DeviceId,
}

struct CameraActor {
    texture: EguiTexture,
    devid_rx: flume::Receiver<DeviceId>,
    devid: DeviceId,
    usb_ctx: rusb::Context,
    conn_tx: flume::Sender<UsbUpdate>,
    conn_rx: flume::Receiver<UsbUpdate>,
}

pub(crate) fn run(args: CameraParams) {
    let (conn_tx, conn_rx) = flume::bounded(16);
    CameraActor {
        texture: EguiTexture {
            texture: args.texture,
            ctx: args.ctx,
        },
        devid_rx: args.devid_rx,
        devid: args.devid,
        usb_ctx: rusb::Context::new().expect("couldn't create context"),
        conn_tx,
        conn_rx,
    }
    .run()
}

macro_rules! prefer {
    ($f:expr) => {
        |a, b| {
            let f: fn(&uvc::StreamFormat) -> _ = $f;
            f(&a).cmp(&f(&b))
        }
    };
}
const TARGET_RATIO: f32 = 16.0 / 9.0;
const FORMAT_PREFERENCES: &[fn(uvc::StreamFormat, uvc::StreamFormat) -> cmp::Ordering] = &[
    prefer!(|x| x.fps),
    prefer!(|x| x.format == uvc::FrameFormat::Uncompressed),
    prefer!(|x| {
        let ratio = x.width as f32 / x.height as f32;
        let diff = TARGET_RATIO - ratio;
        cmp::Reverse(ordered_float::OrderedFloat(diff))
    }),
];

#[derive(PartialEq, Eq)]
enum UsbUpdate {
    Connected,
    Disconnected,
}

impl CameraActor {
    fn run(mut self) {
        std::thread::spawn(move || {
            let usb_ctx = self.usb_ctx.clone();
            let ctx = uvc::Context::from_usb_ctx(&usb_ctx).expect("couldn't create context");

            let mut reg = self.hotplug();

            let usb_ctx = self.usb_ctx.clone();
            std::thread::spawn(move || loop {
                if let Err(e) = usb_ctx.handle_events(None) {
                    eprintln!("libusb error? {e}")
                }
            });

            loop {
                while self.conn_rx.recv().unwrap() != UsbUpdate::Connected {}
                let res = self.go(&ctx);
                self.texture.set_texture(egui::ColorImage::example());
                match res {
                    Ok(switch_dev) => {
                        if switch_dev {
                            drop(reg);
                            reg = self.hotplug();
                        }
                    }
                    Err(e) => {
                        eprintln!("error!! {e}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        });
    }

    fn hotplug(&self) -> rusb::Registration<rusb::Context> {
        let mut hotplug = rusb::HotplugBuilder::new();
        hotplug.enumerate(true);
        if let Some(vid) = self.devid.vendor_id {
            hotplug.vendor_id(vid);
        }
        if let Some(pid) = self.devid.product_id {
            hotplug.product_id(pid);
        }
        struct Hotplug {
            tx: flume::Sender<UsbUpdate>,
        }
        impl rusb::Hotplug<rusb::Context> for Hotplug {
            fn device_arrived(&mut self, _: rusb::Device<rusb::Context>) {
                let _ = self.tx.send(UsbUpdate::Connected);
            }

            fn device_left(&mut self, _: rusb::Device<rusb::Context>) {
                let _ = self.tx.send(UsbUpdate::Disconnected);
            }
        }
        let callback = Box::new(Hotplug {
            tx: self.conn_tx.clone(),
        });
        hotplug
            .register(&self.usb_ctx, callback)
            .expect("register failed")
    }

    fn go(&mut self, ctx: &uvc::Context) -> anyhow::Result<bool> {
        let device = ctx.find_device(
            self.devid.vendor_id.map(Into::into),
            self.devid.product_id.map(Into::into),
            self.devid.serial_number.as_deref(),
        )?;

        let devh = device.open()?;

        let format = devh
            .get_preferred_format(|a, b| {
                for pref in FORMAT_PREFERENCES {
                    match pref(a, b) {
                        cmp::Ordering::Less => return b,
                        cmp::Ordering::Greater => return a,
                        cmp::Ordering::Equal => continue,
                    }
                }
                a
            })
            .context("no preferred formats")?;

        // let format = uvc::StreamFormat {
        //     width: 1280,
        //     height: 720,
        //     fps: 60,
        //     format: uvc::FrameFormat::Any,
        // };
        eprintln!("using {format:?}");

        let mut streamh = devh.get_stream_handle_with_format(format)?;

        let mut texture = self.texture.clone();
        let stream = streamh.start_stream(move |frame| texture.handle_frame(frame))?;

        loop {
            let res = flume::Selector::new()
                .recv(&self.conn_rx, |upd| {
                    if upd.unwrap() == UsbUpdate::Disconnected {
                        ControlFlow::Break((true, false))
                    } else {
                        ControlFlow::Continue(())
                    }
                })
                .recv(&self.devid_rx, |devid| {
                    if let Ok(devid) = devid {
                        let switch = devid != self.devid;
                        self.devid = devid;
                        ControlFlow::Break((false, switch))
                    } else {
                        // the process is gonna end really soon anyway
                        ControlFlow::Continue(())
                    }
                })
                .wait();
            if let ControlFlow::Break((forget_dev, switch_dev)) = res {
                stream.stop();
                if forget_dev {
                    // aborts if we try to drop devicehandle while the device is disconnected
                    std::mem::forget(devh);
                }
                return Ok(switch_dev);
            }
        }
    }
}

#[derive(Clone)]
struct EguiTexture {
    texture: egui::TextureHandle,
    ctx: egui::Context,
}
impl EguiTexture {
    fn set_texture(&mut self, texture: egui::ColorImage) {
        self.texture.set(texture, crate::TEXTURE_FILTER);
        self.ctx.request_repaint();
    }
    fn handle_frame(&mut self, frame: &uvc::Frame) {
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        let mut rgba = vec![Color32::TRANSPARENT; width * height];

        let rgb = match frame.to_rgb() {
            Ok(rgb) => rgb,
            Err(e) => {
                eprintln!("bad rgb {e}");
                return;
            }
        };
        for (rgba, rgb) in rgba.iter_mut().zip(rgb.to_bytes().chunks_exact(3)) {
            *rgba = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        }

        self.set_texture(egui::ColorImage {
            pixels: rgba,
            size: [width, height],
        });
    }
}
