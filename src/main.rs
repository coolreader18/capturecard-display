use clap::Parser;
use egui::util::cache;
use ordered_float::OrderedFloat;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Vec2;

mod audio;
mod video;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    window_title: Option<String>,
    #[clap(long)]
    vendor_id: Option<i32>,
    #[clap(long)]
    product_id: Option<i32>,
    #[clap(long)]
    serial_number: Option<String>,
}

fn main() {
    let args = Args::parse();
    let (done_tx, done_rx) = flume::bounded(0);
    let (finished_tx, finished_rx) = flume::bounded(0);
    let done_ch = (finished_tx, done_rx);
    std::thread::spawn(|| audio::audio_loop(done_ch));
    eframe::run_native(
        "CCDisplay",
        Default::default(),
        Box::new(|cc| Box::new(CCDisplay::new(cc, args))),
    );
    let _ = done_tx.send(());
    let _ = finished_rx.recv();
}

struct CCDisplay {
    texture: egui::TextureHandle,
    window_title: Option<String>,
    ctrl_c: Arc<AtomicBool>,
    display_size_cache: cache::FrameCache<Vec2, DisplaySizeComputer>,
    hover_state: HoverState,
}

enum HoverState {
    NotHovering,
    StartedHovering(Instant),
    Hidden,
}

const TEXTURE_FILTER: egui::TextureFilter = egui::TextureFilter::Linear;

impl CCDisplay {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let texture =
            cc.egui_ctx
                .load_texture("display", egui::ColorImage::example(), TEXTURE_FILTER);
        video::CameraActor {
            texture: texture.clone(),
            ctx: cc.egui_ctx.clone(),
            vendor_id: args.vendor_id,
            product_id: args.product_id,
            serial_number: args.serial_number,
        }
        .run();
        let ctrl_c = Arc::new(AtomicBool::new(false));
        let flag = ctrl_c.clone();
        let _ = ctrlc::set_handler(move || flag.store(true, Relaxed));
        Self {
            texture,
            window_title: args.window_title,
            ctrl_c,
            display_size_cache: Default::default(),
            hover_state: HoverState::NotHovering,
        }
    }
}

impl eframe::App for CCDisplay {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // do the texture rendering right away and everything else after. idk how
        // egui works but maybe this reduces latency?
        let response = egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let window_size = frame.info().window_info.size;
                let texture_size = self.texture.size_vec2();
                let display_size = self
                    .display_size_cache
                    .get((window_size.into(), texture_size.into()));
                ui.add(egui::Image::new(self.texture.id(), display_size))
            });
        if let Some(title) = self.window_title.take() {
            frame.set_window_title(&title);
        }
        if self.ctrl_c.load(Relaxed) || ctx.input().key_pressed(egui::Key::Escape) {
            frame.close();
        }
        match self.hover_state {
            HoverState::NotHovering => {
                if response.response.hovered() && ctx.input().pointer.is_still() {
                    self.hover_state = HoverState::StartedHovering(Instant::now());
                }
            }
            _ if !response.response.hovered() || ctx.input().pointer.is_moving() => {
                self.hover_state = HoverState::NotHovering
            }
            HoverState::StartedHovering(inst) => {
                if inst.elapsed() >= Duration::from_secs(3) {
                    self.hover_state = HoverState::Hidden
                }
            }
            HoverState::Hidden => ctx.output().cursor_icon = egui::CursorIcon::None,
        }
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
