use std::cmp;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use egui::Color32;

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
        let ctx = uvc::Context::new().expect("couldn't create context");
        std::thread::spawn(move || loop {
            if let Err(e) = self.go(&ctx) {
                eprintln!("error!! {e}");
                self.set_texture(egui::ColorImage::example())
            }
            std::thread::sleep(Duration::from_millis(500));
        });
    }

    fn set_texture(&mut self, texture: egui::ColorImage) {
        self.texture.set(texture, crate::TEXTURE_FILTER);
        self.ctx.request_repaint();
    }

    fn go(&self, ctx: &uvc::Context) -> anyhow::Result<()> {
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

        let counter = Arc::new(AtomicU64::new(0));
        let counter2 = counter.clone();
        let stream = streamh.start_stream(
            move |frame, actor| {
                counter2.fetch_add(1, Relaxed);
                actor.handle_frame(frame)
            },
            self.clone(),
        )?;

        let mut cache = counter.load(Relaxed);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let prev = cache;
            cache = counter.load(Relaxed);
            if prev == cache {
                if let Err(uvc::Error::NoDevice) = devh.exposure_abs() {
                    stream.stop();
                    // aborts if we try to drop devicehandle while the device is disconnected
                    std::mem::forget(devh);
                    return Ok(());
                }
            }
        }
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
