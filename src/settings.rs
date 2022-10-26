use std::mem;

use crate::DeviceId;

pub(crate) struct Settings {
    window_title: String,
    pub devid: DeviceId,
    vidname: String,
    pub audname: String,
}
impl Settings {
    pub fn from_storage(storage: &dyn eframe::Storage) -> Self {
        let parse = |s: &str| (!s.is_empty()).then(|| s.parse::<u16>()).transpose();
        let devid = || {
            let (product_id, vendor_id) = (
                storage.get_string("ccdisplay.pid").unwrap_or_default(),
                storage.get_string("ccdisplay.vid").unwrap_or_default(),
            );
            Some(DeviceId {
                product_id: parse(&product_id).ok()?,
                vendor_id: parse(&vendor_id).ok()?,
            })
        };
        Self {
            window_title: storage
                .get_string("ccdisplay.windowtitle")
                .unwrap_or_else(|| "CCDisplay".to_owned()),
            devid: devid().unwrap_or_default(),
            vidname: storage.get_string("ccdisplay.vidname").unwrap_or_default(),
            audname: storage.get_string("ccdisplay.audname").unwrap_or_default(),
        }
    }
    fn save(&self, storage: &mut dyn eframe::Storage) {
        let s = |s: Option<_>| s.as_ref().map(ToString::to_string).unwrap_or_default();
        storage.set_string("ccdisplay.windowtitle", self.window_title.clone());
        storage.set_string("ccdisplay.pid", s(self.devid.product_id));
        storage.set_string("ccdisplay.vid", s(self.devid.vendor_id));
        storage.set_string("ccdisplay.vidname", self.vidname.clone());
        storage.set_string("ccdisplay.audname", self.audname.clone());
    }
}

pub(crate) struct SettingsWindow {
    pub open: bool,
    devid_tx: flume::Sender<DeviceId>,
    audname_tx: flume::Sender<String>,
    settings: Settings,
    first_render: bool,
    vid_list: Option<(Vec<uvc::DeviceDescription>, usize)>,
    audio_list: Option<(Vec<AudioDescr>, usize)>,
}
#[derive(Debug)]
struct AudioDescr {
    name: String,
    desc: Option<String>,
}

impl DeviceId {
    fn matches_device(&self, desc: &uvc::DeviceDescription) -> bool {
        self.vendor_id.map_or(true, |id| desc.vendor_id == id)
            && self.product_id.map_or(true, |id| desc.product_id == id)
    }
}

impl SettingsWindow {
    pub fn new(
        settings: Settings,
        devid_tx: flume::Sender<DeviceId>,
        audname_tx: flume::Sender<String>,
    ) -> Self {
        Self {
            open: false,
            devid_tx,
            audname_tx,
            settings,
            first_render: true,
            vid_list: None,
            audio_list: None,
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
            .auto_sized()
            .show(ctx, |ui| {
                let settings = &mut self.settings;
                ui.horizontal(|ui| {
                    ui.label("Window title");
                    ui.text_edit_singleline(&mut settings.window_title);
                });
                let (vidlist, v_i) = self.vid_list.get_or_insert_with(|| {
                    let list = uvc::Context::new()
                        .unwrap()
                        .devices()
                        .unwrap()
                        .map(|dev| dev.description().unwrap())
                        .collect::<Vec<_>>();
                    let i = list
                        .iter()
                        .position(|desc| settings.devid.matches_device(desc))
                        .unwrap_or(usize::MAX);
                    (list, i)
                });
                let vidname = |x: &uvc::DeviceDescription| {
                    format!(
                        "{} {}",
                        x.manufacturer.as_deref().unwrap_or(""),
                        x.product.as_deref().unwrap_or("")
                    )
                };
                source_dropdown(ui, "Video source", vidlist, v_i, &settings.vidname, vidname);
                let (audlist, a_i) = self.audio_list.get_or_insert_with(|| {
                    let list = {
                        let rt = super::audio::PaRuntime::new();
                        let mut ctx = rt.make_context("getlist");
                        rt.run(async move {
                            super::audio::connect(&mut ctx).await.unwrap();
                            super::audio::get_source_info_list(&ctx.introspect(), |info| {
                                let name = info.name.as_deref()?;
                                Some(AudioDescr {
                                    name: name.to_owned(),
                                    desc: info.description.as_deref().map(str::to_owned),
                                })
                            })
                            .await
                        })
                    };
                    let i = list
                        .iter()
                        .position(|desc| desc.name == settings.audname)
                        .unwrap_or(usize::MAX);
                    (list, i)
                });
                source_dropdown(ui, "Audio source", audlist, a_i, &settings.audname, |x| {
                    x.desc.clone().unwrap_or_else(|| x.name.clone())
                });
                // let product_id = settings.product_id.show(ui, "Product ID");
                // let vendor_id = settings.vendor_id.show(ui, "Vendor ID");
                // ui.horizontal(|ui| {
                //     ui.label("Serial number");
                //     ui.text_edit_singleline(&mut settings.serial_number);
                // });
                if ui.button("Save").clicked() {
                    if *v_i != usize::MAX {
                        let dev = &vidlist[*v_i];
                        settings.vidname = vidname(dev);
                        settings.devid = DeviceId {
                            vendor_id: Some(dev.vendor_id),
                            product_id: Some(dev.product_id),
                        };
                        let _ = self.devid_tx.try_send(settings.devid.clone());
                    }
                    if *a_i != usize::MAX {
                        let name = audlist[*a_i].name.clone();
                        settings.audname = name.clone();
                        let _ = self.audname_tx.try_send(name);
                    }
                    settings.save(frame.storage_mut().unwrap());
                    close = true;
                }
            });
        if close {
            self.open = false;
        }
        if !self.open {
            self.vid_list = None;
            self.audio_list = None;
        }
    }
}

fn source_dropdown<T>(
    ui: &mut egui::Ui,
    label: &str,
    list: &[T],
    i: &mut usize,
    fallback: &str,
    display: impl Fn(&T) -> String,
) {
    ui.horizontal(|ui| {
        ui.label(label);

        let get = |i| {
            if i == usize::MAX {
                fallback.to_owned()
            } else {
                display(&list[i])
            }
        };
        let boxx = egui::ComboBox::from_id_source(egui::Id::new("srclist").with(label))
            .width(160.0)
            .selected_text({
                let mut text = get(*i);

                let mut charlen = 0usize;
                let char40 = text.char_indices().inspect(|_| charlen += 1).nth(40);
                if let Some((char40, _)) = char40 {
                    text.drain(char40..);
                    text.push('…');
                } else {
                    text.extend(std::iter::repeat(' ').take(40usize - charlen));
                }
                text
            });
        // based off ComboBox::show_index

        let selected = i;
        let mut changed = false;

        let mut response = boxx
            .show_ui(ui, |ui| {
                for i in 0..list.len() {
                    if ui.selectable_label(i == *selected, get(i)).clicked() {
                        *selected = i;
                        changed = true;
                    }
                }
            })
            .response;

        if changed {
            response.mark_changed();
        }
    });
}

// struct SettingStringField<T: FromStr> {
//     s: String,
//     _t: PhantomData<T>,
// }
// impl<T: FromStr> SettingStringField<T> {
//     fn new(s: String) -> Self {
//         Self { s, _t: PhantomData }
//     }
//     fn parse(&self) -> Result<Option<T>, T::Err> {
//         (!self.s.is_empty()).then(|| self.s.parse()).transpose()
//     }
//     fn show(&mut self, ui: &mut egui::Ui, label: &str) -> Result<Option<T>, T::Err>
//     where
//         T::Err: ToString,
//     {
//         let t = self.parse();
//         ui.horizontal(|ui| {
//             ui.label(label);
//             if t.is_err() {
//                 ui.style_mut().visuals.override_text_color = Some(egui::Color32::RED)
//             }
//             ui.text_edit_singleline(&mut self.s);
//             if let Err(e) = &t {
//                 ui.label(e.to_string());
//             }
//         });
//         t
//     }
// }
