// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! キー・ホイール・リモコンの入力と、設定の読み直し。

use super::*;

impl Deck {
    /// 押されたキーを拾う。
    pub(super) fn keys(
        &mut self,
        ctx: &egui::Context,
        window: &Arc<Window>,
        strip_rect: Option<egui::Rect>,
    ) {
        // 止めるのは文字の欄に入力している間だけ（egui は Tab でボタンにもフォーカスを渡すので、
        // 「何かにフォーカスがある」で止めると全部のキーが効かなくなる）。
        if ctx.text_edit_focused() {
            return;
        }
        let pressed: Vec<(egui::Key, egui::Modifiers, bool)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers, *repeat)),
                    _ => None,
                })
                .collect()
        });
        self.wheel(ctx, strip_rect);
        let keys_taken = self.keys_taken();
        let takes = |group: &str| keys_taken.iter().any(|g| g == group);
        for (key, modifiers, repeat) in pressed {
            // 全画面の Esc はウィンドウ表示に戻す。割り当てより先、文書より先。
            if key == egui::Key::Escape && !repeat && window.fullscreen().is_some() {
                window.set_fullscreen(None);
                continue;
            }
            let remote = ui::panes::databroadcast::remote_key(key, modifiers, &takes);
            if let Some(remote) = remote {
                if !repeat {
                    if remote == zwsdk::layer::Key::DataButton {
                        self.asked_for_data();
                    }
                    let _ = self.to_broadcast.try_send(ToBroadcast::Key(remote));
                }
                continue;
            }
            let chord = crate::keys::Chord::of(key, modifiers);
            let Some(action) = self.bindings.action(chord) else {
                continue;
            };
            let holds = matches!(
                action,
                crate::keys::Action::VolumeUp | crate::keys::Action::VolumeDown
            );
            if repeat && !holds {
                continue;
            }
            self.act_on(action, window);
        }
    }

    pub(super) fn plugin_asked(&mut self, window: &Arc<Window>) {
        for asked in crate::config::plugins::take_asked() {
            match asked {
                crate::config::plugins::Asked::Act(action) => self.act_on(action, window),
                crate::config::plugins::Asked::Tune(key) => self.say(ToStage::Act(Act::Tune(key))),
            }
        }
    }

    fn act_on(&mut self, action: crate::keys::Action, window: &Arc<Window>) {
        use crate::keys::Action;
        let seekable = self.scene.progress.is_some();
        let step = self.knobs.volume_step as i32;
        let act = match action {
            Action::Menu(command) => {
                self.run(command, window);
                return;
            }
            Action::Caption => {
                self.cycle_captions();
                return;
            }
            Action::Remote(number) => {
                let band = self.scene.menu.band;
                self.remote(ui::panes::remote::Pressed::Number { band, number }, window);
                return;
            }
            Action::SeekBack if seekable => {
                self.show_bottom();
                Act::SeekBy(-ui::theme::transport::STEP_MS)
            }
            Action::SeekForward if seekable => {
                self.show_bottom();
                Act::SeekBy(ui::theme::transport::STEP_MS)
            }
            Action::SeekBack | Action::SeekForward => return,
            Action::ChannelDown => Act::StepChannel(false),
            Action::ChannelUp => Act::StepChannel(true),
            Action::VolumeUp => Act::VolumeBy(step),
            Action::VolumeDown => Act::VolumeBy(-step),
            Action::Mute => Act::ToggleMute,
            Action::NextSound => Act::NextSound,
            Action::NextPicture => Act::NextPicture,
            Action::Pause if seekable => Act::Pause,
            Action::Pause | Action::Osd => Act::ShowOsd,
        };
        self.say(ToStage::Act(act));
    }

    /// ホイール。何をするかは設定（`[operation] wheel` など）。
    fn wheel(&mut self, ctx: &egui::Context, strip_rect: Option<egui::Rect>) {
        if ctx.egui_wants_pointer_input() {
            return;
        }
        let over_strip = ctx
            .input(|input| input.pointer.latest_pos())
            .is_some_and(|at| strip_rect.is_some_and(|rect| rect.contains(at)));
        if over_strip {
            return;
        }
        let notches: Vec<(bool, bool, bool)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel {
                        delta, modifiers, ..
                    } if delta.y != 0.0 => {
                        Some((delta.y > 0.0, modifiers.shift, modifiers.command))
                    }
                    _ => None,
                })
                .collect()
        });
        for (up, shift, ctrl) in notches {
            let does = if ctrl {
                self.knobs.wheel_ctrl
            } else if shift {
                self.knobs.wheel_shift
            } else {
                self.knobs.wheel
            };
            let act = match does {
                WheelDoes::Volume => {
                    let step = self.knobs.volume_step as i32;
                    Act::VolumeBy(if up { step } else { -step })
                }
                WheelDoes::Channel => Act::StepChannel(up),
                WheelDoes::Nothing => continue,
            };
            self.say(ToStage::Act(act));
        }
    }

    pub fn forget_textures(&mut self) {
        self.layer_textures.clear();
        self.broadcast_textures = ui::panes::databroadcast::Textures::default();
    }

    pub fn settings(&mut self, file: Box<crate::config::File>) {
        match crate::config::apply::knobs(&file) {
            Ok(knobs) => self.knobs = knobs,
            Err(why) => tracing::warn!("設定: {why}"),
        }
        self.bindings = crate::keys::Bindings::of(&file.keys);
        crate::coaxd::launch_or_not(file.general.launch_coaxd);
        let wanted = shown_number(&file.caption.shown);
        if wanted != self.captions_wanted {
            self.captions_wanted = wanted;
            let _ = self.to_broadcast.try_send(ToBroadcast::Captions(wanted));
        }
        if file.caption.superimpose != self.file.caption.superimpose {
            let wanted = shown_number(&file.caption.superimpose);
            let _ = self.to_broadcast.try_send(ToBroadcast::Superimpose(wanted));
        }
        if (file.caption.forced, file.caption.superimpose_forced)
            != (
                self.file.caption.forced,
                self.file.caption.superimpose_forced,
            )
        {
            let _ = self.to_broadcast.try_send(forced_of(&file.caption));
        }
        self.file = (*file).clone();
        self.say(ToStage::Settings(file));
    }

    pub fn replugged(&mut self, streams: bool) {
        self.say(ToStage::Replugged(streams));
    }

    /// データ放送の文書が使うと宣言しているキーの組（`used-key-list`）。
    fn keys_taken(&self) -> Vec<String> {
        if !self.broadcast_on {
            return Vec::new();
        }
        self.shown_broadcast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|shown| !shown.invisible || self.one_seg_popup)
            .map(|shown| shown.keys.clone())
            .unwrap_or_default()
    }

    fn cycle_captions(&mut self) {
        let (_, languages) = self.captions();
        let current = self.captions_wanted;
        let next = match current {
            0 => 1,
            1 if languages.len() >= 2 => 2,
            _ => 0,
        };
        self.set_captions(next);
    }

    pub fn asks_before_quit(&self) -> bool {
        self.file.record.confirm_exit && zwcore::record::state().is_some()
    }

    pub fn remote(&mut self, pressed: ui::panes::remote::Pressed, window: &Arc<Window>) {
        use ui::panes::remote::Pressed;
        let keys_taken = self.keys_taken();
        let takes = |group: &str| keys_taken.iter().any(|g| g == group);
        let numeric = takes("numeric-tuning");
        let seekable = self.scene.progress.is_some();
        let to_document = |key: zwsdk::layer::Key| ToBroadcast::Key(key);
        let act = match pressed {
            Pressed::Band(_) => return,
            Pressed::Number { band, number } => {
                if numeric {
                    // 文書へは 0〜9。10 は 0、11 と 12 は文書に無い。
                    let digit = match number {
                        10 => 0,
                        11 | 12 => return,
                        n => n,
                    };
                    let _ = self
                        .to_broadcast
                        .try_send(to_document(zwsdk::layer::Key::Digit(digit)));
                    return;
                }
                Act::TuneRemote { band, number }
            }
            Pressed::VolumeUp => Act::VolumeBy(self.knobs.volume_step as i32),
            Pressed::VolumeDown => Act::VolumeBy(-(self.knobs.volume_step as i32)),
            Pressed::Mute => Act::ToggleMute,
            Pressed::Record => {
                self.run(menu::Command::StartStopRecording, window);
                return;
            }
            Pressed::Capture => {
                self.status_clicked(Some(ui::overlay::status::Clicked::Capture));
                return;
            }
            Pressed::Stop => {
                tracing::debug!("リモコンの {pressed:?} は未実装");
                return;
            }
            Pressed::PageUp | Pressed::PageDown => return,
            Pressed::MiniGuide => {
                self.status_clicked(Some(ui::overlay::status::Clicked::Channel));
                return;
            }
            Pressed::Guide => {
                self.run(menu::Command::ProgrammeGuide, window);
                return;
            }
            Pressed::ChannelUp => Act::StepChannel(true),
            Pressed::ChannelDown => Act::StepChannel(false),
            Pressed::Back10 if seekable => Act::SeekBy(-10_000),
            Pressed::Forward30 if seekable => Act::SeekBy(30_000),
            Pressed::Back10 | Pressed::Forward30 => return,
            // 録画の再生で文書が出ていないときだけ、左右は飛ばし。
            Pressed::Left if seekable && !self.page_shown() => {
                self.show_bottom();
                Act::SeekBy(-ui::theme::transport::STEP_MS)
            }
            Pressed::Right if seekable && !self.page_shown() => {
                self.show_bottom();
                Act::SeekBy(ui::theme::transport::STEP_MS)
            }
            // 矢印・決定・戻るはいつも文書へ。
            Pressed::Up
            | Pressed::Down
            | Pressed::Left
            | Pressed::Right
            | Pressed::Enter
            | Pressed::Back => {
                let key = match pressed {
                    Pressed::Up => zwsdk::layer::Key::Up,
                    Pressed::Down => zwsdk::layer::Key::Down,
                    Pressed::Left => zwsdk::layer::Key::Left,
                    Pressed::Right => zwsdk::layer::Key::Right,
                    Pressed::Enter => zwsdk::layer::Key::Enter,
                    _ => zwsdk::layer::Key::Back,
                };
                let _ = self.to_broadcast.try_send(to_document(key));
                return;
            }
            // 色ボタンと d は文書が出ていなくても渡す（d が文書を起動する）。
            Pressed::Blue => {
                let _ = self
                    .to_broadcast
                    .try_send(to_document(zwsdk::layer::Key::Blue));
                return;
            }
            Pressed::Red => {
                let _ = self
                    .to_broadcast
                    .try_send(to_document(zwsdk::layer::Key::Red));
                return;
            }
            Pressed::Green => {
                let _ = self
                    .to_broadcast
                    .try_send(to_document(zwsdk::layer::Key::Green));
                return;
            }
            Pressed::Yellow => {
                let _ = self
                    .to_broadcast
                    .try_send(to_document(zwsdk::layer::Key::Yellow));
                return;
            }
            Pressed::Data => {
                self.asked_for_data();
                let _ = self
                    .to_broadcast
                    .try_send(to_document(zwsdk::layer::Key::DataButton));
                return;
            }
            Pressed::Captions => {
                self.cycle_captions();
                return;
            }
            Pressed::NextSound => Act::NextSound,
            Pressed::Rewind if seekable => Act::Pace(0.5),
            Pressed::FastForward if seekable => Act::Pace(2.0),
            Pressed::PlayPause | Pressed::Pause if seekable => Act::Pause,
            Pressed::SkipStart if seekable => Act::Seek(0.0),
            Pressed::SkipEnd if seekable => Act::Seek(1.0),
            Pressed::Rewind
            | Pressed::FastForward
            | Pressed::PlayPause
            | Pressed::Pause
            | Pressed::SkipStart
            | Pressed::SkipEnd => return,
        };
        if seekable && matches!(pressed, Pressed::Back10 | Pressed::Forward30) {
            self.show_bottom();
        }
        self.say(ToStage::Act(act));
    }
}
