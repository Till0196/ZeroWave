// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メニューの操作：選ばれた項目を実行し、字幕・拡大などを保存する。

use super::*;

impl Deck {
    pub(super) fn shown_on_bar(&mut self, shown: Option<menu::egui::Shown>, window: &Arc<Window>) {
        match shown {
            Some(menu::egui::Shown::Picked(picked)) => self.pick(picked, window),
            Some(menu::egui::Shown::Open(index, at)) => {
                self.bar_wanted = Some((index, (f64::from(at.x), f64::from(at.y))));
            }
            None => {}
        }
    }

    pub(super) fn pick(&mut self, picked: menu::Picked, window: &Arc<Window>) {
        use menu::Picked;
        // パスの途中ならビルドし終えてから（全画面の切り替えは二度届くと元に戻る）。
        if self.passing.get() {
            self.pending_picks.push(picked);
            return;
        }
        match picked {
            Picked::Command(command) => self.run(command, window),
            Picked::Tune(key) => self.say(ToStage::Act(Act::Tune(key))),
            Picked::Zoom(percent) => self.zoom(window, percent),
            Picked::Sound(at) => self.say(ToStage::Act(Act::Sound(at))),
            Picked::Picture(at) => self.say(ToStage::Act(Act::Picture(at))),
            Picked::Caption(language) => self.set_captions(language),
            Picked::CaptureSize(at) => {
                if let Some((spelled, _)) = menu::CAPTURE_SIZES.get(at) {
                    self.file.capture.size = (*spelled).to_owned();
                    self.remember(
                        &["capture", "size"],
                        toml::Value::String((*spelled).to_owned()),
                    );
                }
            }
            Picked::Aspect(at) => {
                if let Some((spelled, _)) = menu::ASPECTS.get(at) {
                    self.file.video.frame.aspect = (*spelled).to_owned();
                    self.remember(
                        &["video", "frame", "aspect"],
                        toml::Value::String((*spelled).to_owned()),
                    );
                    self.say(ToStage::Act(Act::Aspect((*spelled).to_owned())));
                }
            }
            Picked::CaptureLayer(at) => {
                let rows = crate::capture::layer_rows(&self.plugin_layers());
                if let Some((key, _)) = rows.get(at) {
                    let on = !self.file.capture.includes(key);
                    self.file.capture.include.insert(key.clone(), on);
                    self.remember(&["capture", "include", key], toml::Value::Boolean(on));
                }
            }
            Picked::Profile(at) => {
                let name = self
                    .scene
                    .menu
                    .profiles
                    .get(at)
                    .cloned()
                    .unwrap_or_default();
                let name = if name == crate::ui::panes::settings::ALL {
                    String::new()
                } else {
                    name
                };
                self.say(ToStage::Act(Act::Profile(name)));
            }
            Picked::Plugin { plugin, item } => {
                let Some(menu) = self.scene.menu.plugins.get(usize::from(plugin)) else {
                    return;
                };
                match menu.items.get(usize::from(item)) {
                    Some(zwsdk::ui::MenuItem::Item { id, .. }) => {
                        self.say(ToStage::Act(Act::PluginPick {
                            stem: menu.stem.clone(),
                            id: id.clone(),
                        }));
                    }
                    Some(zwsdk::ui::MenuItem::Open {
                        open: zwsdk::ui::Open::Panel(id),
                        ..
                    }) => {
                        let (stem, id) = (menu.stem.clone(), id.clone());
                        self.open_plugin_panel(&stem, &id);
                    }
                    Some(zwsdk::ui::MenuItem::Open { open, .. }) => {
                        let what = match open {
                            zwsdk::ui::Open::Settings => {
                                match crate::plugin_ui::settings_page_of(&menu.stem) {
                                    Some(page) => {
                                        ToPanes::Open(Which::PluginPage(menu.stem.clone(), page))
                                    }
                                    None => ToPanes::Open(Which::Plugin(menu.stem.clone())),
                                }
                            }
                            zwsdk::ui::Open::Page(page) => {
                                ToPanes::Open(Which::PluginPage(menu.stem.clone(), page.clone()))
                            }
                            zwsdk::ui::Open::Panel(_) => return,
                        };
                        let _ = self.to_panes.send(what);
                    }
                    Some(zwsdk::ui::MenuItem::Separator) | None => {}
                }
            }
        }
    }

    pub(super) fn captions(&self) -> (u8, Vec<[u8; 3]>) {
        self.shown_captions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| (c.language, c.languages.clone()))
            .unwrap_or((self.captions_wanted, Vec::new()))
    }

    pub(super) fn filling(&self) -> Arc<menu::Filling> {
        let mut filling = (*self.scene.menu).clone();
        let (_, languages) = self.captions();
        let (captions, at) = caption_items(self.captions_wanted, &languages);
        filling.captions = captions;
        filling.caption_at = Some(at);
        filling.full_screen = self.full_screen;
        filling.on_top = self.on_top;
        filling.menu_bar = self.file.ui.menu_bar;
        filling.status_bar = self.file.ui.status_bar;
        filling.capture_size = self.file.capture.size.clone();
        filling.aspect = self.file.video.frame.aspect.clone();
        filling.osd = self.file.osd.enabled;
        filling.capture_layers = crate::capture::layer_rows(&self.plugin_layers())
            .into_iter()
            .map(|(key, name)| {
                let on = self.file.capture.includes(&key);
                (name, on)
            })
            .collect();
        Arc::new(filling)
    }

    fn plugin_layers(&self) -> Vec<(String, String)> {
        self.shown_layers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|layer| (crate::capture::plugin_key(&layer.name), layer.name.clone()))
            .chain(crate::capture::video_filter_row(&self.scene.video_filters))
            .collect()
    }

    /// メニューで変えた設定を `zerowave.toml` に書き戻す。
    fn remember(&self, path: &[&str], value: toml::Value) {
        let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
        let is_default = toml::Table::try_from(crate::config::File::default())
            .ok()
            .and_then(|table| {
                let (leaf, sections) = path.split_last()?;
                let mut at = &table;
                for section in sections {
                    at = at.get(section)?.as_table()?;
                }
                at.get(leaf).cloned()
            })
            .is_some_and(|default| default == value);
        let change = crate::config::Change {
            path,
            value,
            is_default,
        };
        if let Err(why) = crate::config::save_changes(&[change]) {
            tracing::warn!("設定を書けない: {why}");
        }
    }

    pub(super) fn set_captions(&mut self, next: u8) {
        let (_, languages) = self.captions();
        let next = next.min(2);
        self.captions_wanted = next;
        let _ = self.to_broadcast.try_send(ToBroadcast::Captions(next));
        let spelled = shown_spelling(next);
        if self.file.caption.shown != spelled {
            self.file.caption.shown = spelled.to_owned();
            self.remember(
                &["caption", "shown"],
                toml::Value::String(spelled.to_owned()),
            );
        }
        let said = match next {
            0 => String::from("字幕表示：［切］"),
            // 第2言語が無ければ第1言語を表示する。
            2 if languages.len() == 1 => {
                format!("字幕表示：［{}］", caption_label(1, &languages))
            }
            n => format!("字幕表示：［{}］", caption_label(n, &languages)),
        };
        self.say(ToStage::Act(Act::Toast(said)));
    }

    pub fn captions_from_settings(&mut self, shown: &str) {
        self.set_captions(shown_number(shown));
    }

    fn zoom(&mut self, window: &Arc<Window>, percent: u32) {
        if window.fullscreen().is_some() {
            return;
        }
        let Some((width, height)) = self.scene.picture else {
            return;
        };
        let scale = f64::from(percent) / 100.0;
        let points = window.scale_factor().max(0.01);
        let want_w = f64::from(width) * scale / points;
        let want_h = f64::from(height) * scale / points + f64::from(self.chrome());
        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(want_w, want_h));
    }

    pub(super) fn run(&mut self, command: menu::Command, window: &Arc<Window>) {
        use menu::Command;

        if self.passing.get() {
            self.pending_picks.push(menu::Picked::Command(command));
            return;
        }
        match command {
            Command::AlwaysOnTop => {
                self.on_top = !self.on_top;
                window.set_window_level(if self.on_top {
                    winit::window::WindowLevel::AlwaysOnTop
                } else {
                    winit::window::WindowLevel::Normal
                });
            }
            Command::Close => {
                self.say(ToStage::Quit);
                if !self.asks_before_quit() {
                    let _ = self.to_panes.send(ToPanes::Quit);
                }
            }
            // ダイアログを出している間もこのスレッドは止めない（`window::opening`）。
            Command::Open => window::opening::ask(self.to_stage.clone()),
            Command::PlayerMode => self.say(ToStage::Act(Act::PlayerMode)),
            Command::FullScreen => {
                let full = window.fullscreen().is_none();
                window.set_fullscreen(full.then_some(Fullscreen::Borderless(None)));
                self.full_screen = full;
            }
            Command::StreamInformation => {
                let _ = self.to_panes.send(ToPanes::Open(Which::Stream));
            }
            Command::DecoderInformation => {
                let _ = self.to_panes.send(ToPanes::Open(Which::Decoder));
            }
            Command::About => {
                let _ = self.to_panes.send(ToPanes::Open(Which::About));
            }
            Command::PanelWindow => {
                self.status_clicked(Some(ui::overlay::status::Clicked::Channel))
            }
            Command::Settings => {
                let _ = self.to_panes.send(ToPanes::Open(Which::Settings));
            }
            Command::ProgrammeGuide => {
                let _ = self.to_panes.send(ToPanes::Open(Which::Guide));
            }
            Command::ProgrammeDetail => {
                let _ = self.to_panes.send(ToPanes::WatchingDetail);
            }
            Command::Remote => {
                let _ = self.to_panes.send(ToPanes::Open(Which::Remote));
            }
            Command::VolumeUp => {
                self.say(ToStage::Act(Act::VolumeBy(self.knobs.volume_step as i32)));
            }
            Command::VolumeDown => {
                self.say(ToStage::Act(Act::VolumeBy(
                    -(self.knobs.volume_step as i32),
                )));
            }
            Command::Mute => self.say(ToStage::Act(Act::ToggleMute)),
            Command::StartStopRecording => {
                self.say(ToStage::Act(Act::Record { status_bar: false }));
            }
            Command::SaveImage => self.capture_wanted = Some(false),
            Command::CopyImage => self.capture_wanted = Some(true),
            Command::OpenRecordFolder => {
                let folder = crate::capture::folder_of(
                    &self.file.record.folder,
                    crate::capture::Folder::Videos,
                );
                let _ = std::fs::create_dir_all(&folder);
                window::opening::reveal(&folder);
            }
            Command::OpenCaptureFolder => {
                let folder = crate::capture::folder_of(
                    &self.file.capture.folder,
                    crate::capture::Folder::Pictures,
                );
                let _ = std::fs::create_dir_all(&folder);
                window::opening::reveal(&folder);
            }
            Command::RecordSettings => {
                let _ = self
                    .to_panes
                    .send(ToPanes::OpenSettings(ui::panes::settings::Page::Recording));
            }
            Command::Reset => self.say(ToStage::Act(Act::Reset)),
            Command::ResetViewer => self.say(ToStage::Act(Act::ResetViewer)),
            Command::RebuildPlayer => self.say(ToStage::Act(Act::RebuildPlayer)),
            Command::ResetErrorCount => self.say(ToStage::Act(Act::ResetErrorCount)),
            Command::PlaybackOff => self.say(ToStage::Act(Act::PlaybackOff)),
            Command::AddFavourite => self.say(ToStage::Act(Act::AddFavourite)),
            Command::OrganizeFavourites => {
                let _ = self
                    .to_panes
                    .send(ToPanes::Open(window::sub::Which::Favourites));
            }
            Command::ClearRecent => self.say(ToStage::Act(Act::ClearRecent)),
            Command::ChannelBack => self.say(ToStage::Act(Act::ChannelBack)),
            Command::ChannelForward => self.say(ToStage::Act(Act::ChannelForward)),
            Command::ChannelPrevious => self.say(ToStage::Act(Act::ChannelPrevious)),
            Command::CaptureSettings => {
                let _ = self
                    .to_panes
                    .send(ToPanes::OpenSettings(ui::panes::settings::Page::Capture));
            }
            Command::CopyProgramme => {
                let line = self.scene.now_playing.clone();
                if line.is_empty() {
                    self.say(ToStage::Act(Act::Toast(String::from(
                        "コピーできる番組情報がありません。",
                    ))));
                } else {
                    self.copy_wanted = Some(line);
                    self.say(ToStage::Act(Act::Toast(String::from(
                        "番組情報をクリップボードにコピーしました。",
                    ))));
                }
            }
            Command::MenuBar => {
                self.file.ui.menu_bar = !self.file.ui.menu_bar;
                self.remember(
                    &["ui", "menu_bar"],
                    toml::Value::Boolean(self.file.ui.menu_bar),
                );
                self.told_area = None;
            }
            Command::StatusBar => {
                self.file.ui.status_bar = !self.file.ui.status_bar;
                self.remember(
                    &["ui", "status_bar"],
                    toml::Value::Boolean(self.file.ui.status_bar),
                );
                self.told_area = None;
            }
            Command::Osd => {
                self.file.osd.enabled = !self.file.osd.enabled;
                self.remember(
                    &["osd", "enabled"],
                    toml::Value::Boolean(self.file.osd.enabled),
                );
            }
        }
    }
}
