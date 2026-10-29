// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 要求の処理。イベントループからの要求を処理し、Act を実行する。

use super::*;

impl Stage {
    pub(super) fn say(&self, what: FromStage) {
        let _ = self.out.send(what);
        let _ = self.proxy.send_event(Wake);
    }

    /// 終わることにする。録画していれば書き出し待ちを書き終えてから。
    pub(super) fn quit(&mut self) {
        let _ = zwcore::record::stop();
        self.save_volume();
        self.done = true;
    }

    fn confirmation(&self, act: &Act) -> Option<(&'static str, &'static str)> {
        zwcore::record::state()?;
        let record = crate::config::load_plain().record;
        let channel = (
            "チャンネル変更の確認",
            "録画中です。チャンネル変更しますか?",
        );
        match act {
            Act::Tune(key) if self.asked_key != Some(*key) && record.confirm_channel_change => {
                Some(channel)
            }
            Act::StepChannel(_) | Act::TuneRemote { .. } | Act::Band(_)
                if record.confirm_channel_change =>
            {
                Some(channel)
            }
            Act::ChannelBack | Act::ChannelForward | Act::ChannelPrevious
                if record.confirm_channel_change =>
            {
                Some(channel)
            }
            Act::Record { status_bar } if record.confirms_stop(*status_bar) => {
                Some(("停止の確認", "録画を停止しますか?"))
            }
            _ => None,
        }
    }

    pub(super) fn ask(&self, title: &'static str, text: &'static str, then: Confirmed) {
        use std::sync::atomic::Ordering;
        if self.asking.swap(true, Ordering::AcqRel) {
            return;
        }
        let to = self.confirmed.0.clone();
        let asking = Arc::clone(&self.asking);
        let spawned = std::thread::Builder::new()
            .name(String::from("confirm"))
            .spawn(move || {
                if crate::window::ask::ok_cancel(title, text) {
                    let _ = to.send(then);
                }
                asking.store(false, Ordering::Release);
            });
        if let Err(error) = spawned {
            tracing::warn!("確認ダイアログのスレッドを起動できない: {error}");
            self.asking
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    pub(super) fn serve(&mut self) -> bool {
        if self.take_confirmed() {
            return true;
        }
        loop {
            match self.events.try_recv() {
                Ok(ToStage::Drawn(drawn)) => {
                    self.presented.drawn_taken();
                    self.hand(*drawn);
                }
                Ok(ToStage::Resized((width, height))) => self.configure(width, height),
                Ok(ToStage::Area {
                    left,
                    top,
                    width,
                    height,
                }) => {
                    self.left = left;
                    self.top = top;
                    self.wide = width;
                    self.tall = height;
                }
                Ok(ToStage::Inset(inset)) => {
                    // データ放送が映像を一部の領域へ縮めているときだけ（画面いっぱいなら比率は効く）。
                    self.inset_on = inset.is_some_and(|[left, top, wide, tall, _]| {
                        left > 0.001 || top > 0.001 || wide < 0.999 || tall < 0.999
                    });
                    self.player.set_inset(inset);
                }
                Ok(ToStage::BroadcastVisible(visible)) => {
                    self.broadcast_visible = visible;
                }
                Ok(ToStage::Hidden(hidden)) => self.hide(hidden),
                Ok(ToStage::FullScreen(full)) => self.full_screen = full,
                Ok(ToStage::Grabbed(grabbed)) => self.grab(grabbed),
                Ok(ToStage::Display(hz)) => {
                    self.refresh = hz;
                    self.say_video_latency();
                }
                // 用意し直すのは掴まれていない間（境目を跨いで動かす間は結果が何度も入れ替わる）。
                Ok(ToStage::Monitor(monitor)) => self.surface.monitor(monitor),
                Ok(ToStage::Wants { guide, info, mini }) => {
                    let _ = (guide, info);
                    if !mini {
                        self.mini_shift = 0;
                    }
                    self.mini = mini;
                }
                Ok(ToStage::Act(act)) => self.act(act),
                Ok(ToStage::Settings(file)) => self.settings(&file),
                Ok(ToStage::Capture(job)) => self.capture(job),
                Ok(ToStage::Replugged(streams)) => self.replugged(streams),
                Ok(ToStage::YieldTuner(yielding)) => self.yield_tuner(yielding),
                Ok(ToStage::Scanned(services)) => self.scanned(services),
                Ok(ToStage::Reinstanced { instance, surface }) => {
                    self.reinstanced(instance, surface);
                }
                Ok(ToStage::Quit) => {
                    if zwcore::record::state().is_some()
                        && crate::config::load_plain().record.confirm_exit
                    {
                        self.ask(
                            "終了の確認",
                            "現在録画中です。\n終了してもいいですか?",
                            Confirmed::Quit,
                        );
                        continue;
                    }
                    self.quit();
                    return true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        let answers = self
            .live
            .as_ref()
            .map(|live| live.supply.taken())
            .unwrap_or_default();
        for answer in answers {
            self.tuned(answer);
        }
        self.give_up_tuning();
        for chose in self.panes.taken() {
            match chose {
                Chose::Tune(key) => self.tune(key),
                Chose::Wants {
                    guide,
                    info,
                    decoder,
                } => {
                    if info && !self.wants_info {
                        let tables = stream_info(self.player.tables());
                        self.panes.say(ToPanes::Info(Box::new(tables)));
                        self.asked_info = Instant::now();
                    }
                    if decoder && !self.wants_decoder {
                        self.tell_decoding();
                    }
                    self.wants_guide = guide;
                    self.wants_info = info;
                    self.wants_decoder = decoder;
                }
            }
        }
        true
    }

    pub(super) fn hand(&mut self, mut drawn: overlay::Drawn) {
        #[cfg(target_os = "windows")]
        if let Some(to) = self.overlay_to.as_ref() {
            if to.send(Box::new(drawn)).is_err() {
                self.overlay_to = None;
            }
            return;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if matches!(self.painter, Some(zwcore::screen::Painter::Gles { .. })) {
            self.layer.hand(drawn);
            self.fresh = true;
            return;
        }
        if let Some(mut old) = self.drawn.take() {
            drawn.fold_into(&mut old);
        }
        self.drawn = Some(drawn);
        self.fresh = true;
    }

    fn act(&mut self, act: Act) {
        if let Some((title, text)) = self.confirmation(&act) {
            self.ask(title, text, Confirmed::Act(act));
            return;
        }
        self.act_now(act);
    }

    fn take_confirmed(&mut self) -> bool {
        while let Ok(confirmed) = self.confirmed.1.try_recv() {
            match confirmed {
                Confirmed::Act(act) => self.act_now(act),
                Confirmed::Quit => {
                    self.quit();
                    return true;
                }
            }
        }
        false
    }

    fn act_now(&mut self, act: Act) {
        match act {
            // 二重音声は主・副・主＋副の三つとして並んでいる。
            Act::NextSound => {
                let tables = self.player.tables();
                if tables.choices.is_empty() {
                    return;
                }
                let next = tables
                    .choice_at
                    .map_or(0, |at| (at + 1) % tables.choices.len());
                self.choose_sound(next);
            }
            Act::NextPicture => {
                self.picture_asked = Some((self.player.tables().picture_at, Instant::now()));
                self.player.ask(Ask::NextPicture);
            }
            Act::Sound(at) => self.choose_sound(at),
            Act::Picture(at) => {
                self.picture_asked = Some((self.player.tables().picture_at, Instant::now()));
                self.player.ask(Ask::Picture(at));
            }
            Act::StepChannel(forward) => self.step_channel(forward),
            Act::TuneRemote { band, number } => self.tune_remote(band, number),
            Act::ShowOsd => self.osd.show(self.osd_prefs),
            Act::Toast(text) => self.toast(&text),
            Act::Record { .. } => self.record(),
            Act::AddFavourite => self.add_favourite(),
            Act::Aspect(aspect) => self.frame.aspect = aspect,
            Act::Reset => {
                self.reopen_current();
                tracing::info!("リセットした");
            }
            Act::ResetViewer => {
                self.player.reset_viewer();
                tracing::info!("ビューアをリセットした");
            }
            Act::RebuildPlayer => {
                self.player.rebuild_screen();
                self.reopen_current();
                tracing::info!("映像と音声を再初期化した");
            }
            Act::ResetErrorCount => self.reset_error_count(),
            Act::PlaybackOff => {
                let on = !self.player.playback();
                self.player.set_playback(on);
                if on {
                    tracing::info!("再生をオンにした");
                } else {
                    self.toast("再生をオフにしました。\n受信と録画は継続しています。");
                }
            }
            Act::ClearRecent => self.clear_recent(),
            Act::ChannelBack => self.go_back(),
            Act::ChannelForward => self.go_forward(),
            Act::ChannelPrevious => self.go_previous(),
            Act::VolumeBy(by) => {
                if self.player.passes_sound_through() {
                    self.volume_shown = Some((Instant::now(), true));
                    return;
                }
                let volume = zwcore::sound::volume::stepped(self.player.volume(), by);
                self.player.set_volume(volume);
                if self.player.muted() && by > 0 {
                    self.player.set_muted(false);
                }
                self.volume_shown = Some((Instant::now(), false));
                self.volume_moved = Some(Instant::now());
            }
            Act::SetVolume(volume) => {
                if self.player.passes_sound_through() {
                    self.volume_shown = Some((Instant::now(), true));
                    return;
                }
                let volume = volume.min(zwcore::sound::volume::MAX);
                self.player.set_volume(volume);
                if self.player.muted() {
                    self.player.set_muted(false);
                }
                self.volume_shown = Some((Instant::now(), false));
                self.volume_moved = Some(Instant::now());
            }
            Act::AudioDelay(by) => self.audio_delay(by),
            Act::ToggleMute => {
                if self.player.passes_sound_through() {
                    self.volume_shown = Some((Instant::now(), true));
                    return;
                }
                let muted = !self.player.muted();
                self.player.set_muted(muted);
                self.volume_shown = Some((Instant::now(), false));
                self.volume_moved = Some(Instant::now());
            }
            Act::Tune(key) => self.tune(key),
            Act::Open(path) => self.play_file(&path),
            Act::PlayerMode => self.toggle_player_mode(),
            Act::Band(band) => self.choose_band(band),
            Act::StripShift(hours) => {
                self.mini_shift =
                    (self.mini_shift + hours).clamp(-MINI_SHIFT_MOST, MINI_SHIFT_MOST);
            }
            Act::StripRows(rows) => {
                self.mini_rows = rows.clamp(1, ui::theme::mini::MOST_ROWS);
            }
            Act::Profile(name) => self.choose_profile(name),
            Act::PluginPick { stem, id } => self.plugin_pick(&stem, &id),
            Act::Seek(fraction) => self.seek_to(fraction),
            Act::Pause => {
                let paused = !self.player.paused();
                self.player.set_paused(paused);
            }
            Act::Pace(pace) => {
                let now = self.player.pace();
                let wanted = if (now - pace).abs() < 1e-6 { 1.0 } else { pace };
                self.player.set_pace(wanted);
            }
            Act::SeekBy(ms) => {
                let Some(progress) = self.player.progress() else {
                    return;
                };
                if progress.length <= 0 {
                    return;
                }
                let target = (progress.position + ms * HZ / 1000).clamp(0, progress.length);
                self.seek_to(target as f32 / progress.length as f32);
            }
        }
    }

    pub(super) fn seek_to(&mut self, fraction: f32) {
        if !self.player.is_recording() {
            return;
        }
        if self.ended {
            self.ended = false;
            self.player.set_paused(false);
        }
        if self.player.seek_to(fraction) {
            self.seeking = true;
            self.seek_settling = true;
            let _ = self.broadcast.try_send(ToBroadcast::Sought);
            // 放送の時計は動かした先の TOT まで分からない。
            self.told_clock = None;
            let _ = self.guide_to.send(ToGuide::Clock(None));
        }
    }
}
