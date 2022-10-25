use std::marker::PhantomData;
use std::mem;
use std::str::FromStr;

use crate::DeviceId;

pub(crate) struct Settings {
    window_title: String,
    product_id: SettingStringField<u16>,
    vendor_id: SettingStringField<u16>,
    serial_number: String,
}
impl Settings {
    pub fn from_storage(storage: &dyn eframe::Storage) -> Self {
        Self {
            window_title: storage
                .get_string("ccdisplay.windowtitle")
                .unwrap_or_else(|| "CCDisplay".to_owned()),
            product_id: SettingStringField::new(
                storage.get_string("ccdisplay.pid").unwrap_or_default(),
            ),
            vendor_id: SettingStringField::new(
                storage.get_string("ccdisplay.vid").unwrap_or_default(),
            ),
            serial_number: storage
                .get_string("ccdisplay.serialnum")
                .unwrap_or_default(),
        }
    }
    fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string("ccdisplay.windowtitle", self.window_title.clone());
        storage.set_string("ccdisplay.pid", self.product_id.s.clone());
        storage.set_string("ccdisplay.vid", self.vendor_id.s.clone());
        storage.set_string("ccdisplay.serialnum", self.serial_number.clone());
    }
    pub fn deviceid(&self) -> Option<DeviceId> {
        Some(DeviceId {
            product_id: self.product_id.parse().ok()?,
            vendor_id: self.vendor_id.parse().ok()?,
            serial_number: (!self.serial_number.is_empty()).then(|| self.serial_number.clone()),
        })
    }
}

pub(crate) struct SettingsWindow {
    pub open: bool,
    devid_tx: flume::Sender<DeviceId>,
    settings: Settings,
    first_render: bool,
}

impl SettingsWindow {
    pub fn new(settings: Settings, devid_tx: flume::Sender<DeviceId>) -> Self {
        Self {
            open: false,
            devid_tx,
            settings,
            first_render: true,
        }
    }
    pub fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if mem::take(&mut self.first_render) {
            frame.set_window_title(&self.settings.window_title);
        }
        if ctx
            .input_mut()
            .consume_key(egui::Modifiers::ALT, egui::Key::S)
        {
            self.open = !self.open;
        }
        if self.open && ctx.input().key_pressed(egui::Key::Escape) {
            self.open = false;
        }
        let mut close = false;
        egui::Window::new("Settings")
            .open(&mut self.open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("hiiii");
                let settings = &mut self.settings;
                ui.horizontal(|ui| {
                    ui.label("Window title");
                    ui.text_edit_singleline(&mut settings.window_title);
                });
                let product_id = settings.product_id.show(ui, "Product ID");
                let vendor_id = settings.vendor_id.show(ui, "Vendor ID");
                ui.horizontal(|ui| {
                    ui.label("Serial number");
                    ui.text_edit_singleline(&mut settings.serial_number);
                });
                let save = egui::Button::new("Save");
                if let (Ok(product_id), Ok(vendor_id)) = (product_id, vendor_id) {
                    if ui.add_enabled(true, save).clicked() {
                        let _ = self.devid_tx.send(DeviceId {
                            vendor_id,
                            product_id,
                            serial_number: (!settings.serial_number.is_empty())
                                .then(|| settings.serial_number.clone()),
                        });
                        settings.save(frame.storage_mut().unwrap());
                        close = true;
                    }
                } else {
                    ui.add_enabled(false, save);
                }
            });
        if close {
            self.open = false;
        }
    }
}

struct SettingStringField<T: FromStr> {
    s: String,
    _t: PhantomData<T>,
}
impl<T: FromStr> SettingStringField<T> {
    fn new(s: String) -> Self {
        Self { s, _t: PhantomData }
    }
    fn parse(&self) -> Result<Option<T>, T::Err> {
        (!self.s.is_empty()).then(|| self.s.parse()).transpose()
    }
    fn show(&mut self, ui: &mut egui::Ui, label: &str) -> Result<Option<T>, T::Err>
    where
        T::Err: ToString,
    {
        let t = self.parse();
        ui.horizontal(|ui| {
            ui.label(label);
            if t.is_err() {
                ui.style_mut().visuals.override_text_color = Some(egui::Color32::RED)
            }
            ui.text_edit_singleline(&mut self.s);
            if let Err(e) = &t {
                ui.label(e.to_string());
            }
        });
        t
    }
}
