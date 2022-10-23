use std::cmp;
use std::sync::atomic::{
    AtomicBool,
    Ordering::{Acquire, Release},
};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use egui::Color32;
use rusb::UsbContext;

#[derive(Clone)]
pub struct CameraActor {
    pub texture: egui::TextureHandle,
    pub ctx: egui::Context,
    pub vendor_id: Option<i32>,
    pub product_id: Option<i32>,
    pub serial_number: Option<String>,
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

impl CameraActor {
    pub fn run(mut self) {
        std::thread::spawn(move || {
            let usb_ctx = rusb::Context::new().expect("couldn't create context");
            let ctx = uvc::Context::from_usb_ctx(&usb_ctx).expect("couldn't create context");

            let conn_flag = Arc::new(AtomicBool::new(false));

            let mut hotplug = rusb::HotplugBuilder::new();
            hotplug.enumerate(true);
            if let Some(vid) = self.vendor_id {
                hotplug.vendor_id(vid as u16);
            }
            if let Some(pid) = self.product_id {
                hotplug.product_id(pid as u16);
            }
            struct Hotplug {
                conn_flag: Arc<AtomicBool>,
            }
            impl rusb::Hotplug<rusb::Context> for Hotplug {
                fn device_arrived(&mut self, _: rusb::Device<rusb::Context>) {
                    self.conn_flag.store(true, Release);
                }

                fn device_left(&mut self, _: rusb::Device<rusb::Context>) {
                    self.conn_flag.store(false, Release);
                }
            }
            let callback = Hotplug {
                conn_flag: conn_flag.clone(),
            };
            let _reg = hotplug
                .register(&usb_ctx, Box::new(callback))
                .expect("register failed");

            loop {
                if let Err(e) = poll_until(&usb_ctx, &conn_flag, true) {
                    eprintln!("{e}");
                    return;
                }
                let res = self.go(&ctx, &usb_ctx, &conn_flag);
                self.set_texture(egui::ColorImage::example());
                if let Err(e) = res {
                    eprintln!("error!! {e}");
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        });
    }

    fn set_texture(&mut self, texture: egui::ColorImage) {
        self.texture.set(texture, crate::TEXTURE_FILTER);
        self.ctx.request_repaint();
    }

    fn go(
        &self,
        ctx: &uvc::Context,
        usb_ctx: &rusb::Context,
        conn_flag: &AtomicBool,
    ) -> anyhow::Result<()> {
        let device = ctx.find_device(
            self.vendor_id,
            self.product_id,
            self.serial_number.as_deref(),
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

        let mut actor = self.clone();
        let stream = streamh.start_stream(move |frame| actor.handle_frame(frame))?;

        poll_until(usb_ctx, conn_flag, false)?;
        stream.stop();
        // aborts if we try to drop devicehandle while the device is disconnected
        std::mem::forget(devh);
        Ok(())
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

fn poll_until(usb_ctx: &rusb::Context, flag: &AtomicBool, v: bool) -> anyhow::Result<()> {
    loop {
        if flag.load(Acquire) == v {
            return Ok(());
        }
        usb_ctx
            .handle_events(None)
            .context("failed to handle events")?;
    }
}
