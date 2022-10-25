use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

use egui::{util::cache, Vec2};
use ordered_float::OrderedFloat;

mod audio;
mod settings;
mod video;

fn main() {
    eframe::run_native(
        "CCDisplay",
        Default::default(),
        Box::new(|cc| Box::new(CCDisplay::new(cc))),
    );
}

struct CCDisplay {
    texture: egui::TextureHandle,
    ctrl_c: Arc<AtomicBool>,
    display_size_cache: cache::FrameCache<Vec2, DisplaySizeComputer>,
    settings: settings::SettingsWindow,
    done_tx: flume::Sender<()>,
    finished_rx: flume::Receiver<()>,
}

const TEXTURE_FILTER: egui::TextureFilter = egui::TextureFilter::Linear;

#[derive(Clone, Default, PartialEq, Eq)]
struct DeviceId {
    vendor_id: Option<u16>,
    product_id: Option<u16>,
    serial_number: Option<String>,
}

impl CCDisplay {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let texture =
            cc.egui_ctx
                .load_texture("display", egui::ColorImage::example(), TEXTURE_FILTER);
        let settings = settings::Settings::from_storage(cc.storage.unwrap());
        let (devid_tx, devid_rx) = flume::bounded(4);

        video::run(video::CameraParams {
            texture: texture.clone(),
            ctx: cc.egui_ctx.clone(),
            devid_rx,
            devid: settings.deviceid().unwrap_or_default(),
        });

        let (done_tx, done_rx) = flume::bounded(0);
        let (finished_tx, finished_rx) = flume::bounded(0);
        std::thread::spawn(|| audio::audio_loop((finished_tx, done_rx)));

        let ctrl_c = Arc::new(AtomicBool::new(false));
        let flag = ctrl_c.clone();
        let ctx = cc.egui_ctx.clone();
        let _ = ctrlc::set_handler(move || {
            flag.store(true, Relaxed);
            ctx.request_repaint();
        });

        Self {
            texture,
            ctrl_c,
            display_size_cache: Default::default(),
            settings: settings::SettingsWindow::new(settings, devid_tx),
            done_tx,
            finished_rx,
        }
    }
}

impl eframe::App for CCDisplay {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // do the texture rendering right away and everything else after. idk how
        // egui works but maybe this reduces latency?
        let window_info = frame.info().window_info;
        let response = egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let window_size = window_info.size;
                let texture_size = self.texture.size_vec2();
                let display_size = self
                    .display_size_cache
                    .get((window_size.into(), texture_size.into()));
                ui.add(egui::Image::new(self.texture.id(), display_size));
            });
        if self.ctrl_c.load(Relaxed) {
            frame.close();
        }
        if !self.settings.open {
            let input = ctx.input();
            if input.key_pressed(egui::Key::Escape) {
                frame.close();
            }
            if input.key_pressed(egui::Key::F) {
                frame.set_fullscreen(!window_info.fullscreen);
            }
        }
        let hide_cursor = ctx.animate_bool_with_time(
            egui::Id::new("pointerhover"),
            response.response.hovered() && ctx.input().pointer.is_still(),
            3.0,
        );
        if hide_cursor == 1.0 {
            ctx.output().cursor_icon = egui::CursorIcon::None;
        }
        self.settings.update(ctx, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.done_tx.send(());
        let _ = self.finished_rx.recv();
    }
}

#[derive(Default)]
struct DisplaySizeComputer;
#[derive(Copy, Clone, Hash)]
struct OrderedVec2 {
    x: OrderedFloat<f32>,
    y: OrderedFloat<f32>,
}
impl From<Vec2> for OrderedVec2 {
    fn from(v: Vec2) -> Self {
        OrderedVec2 {
            x: OrderedFloat(v.x),
            y: OrderedFloat(v.y),
        }
    }
}
impl From<OrderedVec2> for Vec2 {
    fn from(v: OrderedVec2) -> Self {
        Vec2::new(v.x.0, v.y.0)
    }
}
impl cache::ComputerMut<(OrderedVec2, OrderedVec2), Vec2> for DisplaySizeComputer {
    fn compute(&mut self, (window_size, texture_size): (OrderedVec2, OrderedVec2)) -> Vec2 {
        let (window_size, texture_size): (Vec2, Vec2) = (window_size.into(), texture_size.into());
        let aspect_ratio = |v: Vec2| v.x / v.y.max(1.0);
        let window_ratio = aspect_ratio(window_size);
        let texture_ratio = aspect_ratio(texture_size);
        if window_ratio < texture_ratio {
            // window is thinner than texture
            Vec2 {
                x: window_size.x,
                y: (texture_size.y * window_size.x) / texture_size.x,
            }
        } else {
            // window is wider than texture
            Vec2 {
                x: (texture_size.x * window_size.y) / texture_size.y,
                y: window_size.y,
            }
        }
    }
}
