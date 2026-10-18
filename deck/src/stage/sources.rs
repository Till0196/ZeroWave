// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 設定とプロファイルの持ち替え、ソースの繋ぎ替え、通知、音量の保存。

use super::*;

impl Stage {
    pub(super) fn settings(&mut self, file: &crate::config::File) {
        self.frame = file.video.frame.clone();
        self.reset_errors_on_tune = file.playback.reset_errors_on_tune;
        let applied = match crate::config::apply::choices(file) {
            Ok(applied) => applied,
            Err(why) => {
                self.toast(&said_with("設定を保存できません。", &why));
                return;
            }
        };
        let options = applied.choices.options;
        self.player.set_mix(options.mix);
        if options.dual_mono != self.player.dual_mono() {
            self.player.set_dual_mono(options.dual_mono);
            self.player.ask(Ask::DualMono(options.dual_mono));
        }
        self.choices.knobs = applied.choices.knobs;
        self.apply_channel_prefs(file);
        self.volume_hold = file.osd.fade.max(1) as f32;
        self.no_sleep = file.general.no_sleep;
        // 他のプロファイルの Mirakurun からもサービス一覧を取得してキャッシュする。
        self.catalog_due = true;
        let active: Vec<String> = openings(file, &crate::config::load_state().profile)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        catalog::refresh_mirakurun(file, &active);
        crate::pcsc::apply(file);
        let undecoded = crate::config::apply::undecoded(file);
        let was = *self.player.options();
        let replanned = zwcore::decode::plan() != applied.plan
            || zwcore::decode::sound::sound_plan() != applied.sound_plan;
        zwcore::decode::set_plan(applied.plan.clone());
        zwcore::decode::sound::set_sound_plan(applied.sound_plan.clone());
        let redecode = replanned
            || was.decoder != options.decoder
            || was.api != options.api
            || was.deinterlace != options.deinterlace
            || was.deinterlace_by != options.deinterlace_by
            || was.video != options.video;
        self.player.set_decoding(&options);
        let recreate = self.choices.present != applied.choices.present
            || self.choices.hdr != applied.choices.hdr;
        self.choices.present = applied.choices.present;
        self.choices.hdr = applied.choices.hdr;
        if recreate {
            let (width, height) = self.size;
            self.configure(width, height);
        }
        let plugins_changed = self
            .plugins_seen
            .as_ref()
            .is_some_and(|(folder, disabled)| {
                *folder != file.data.plugins || *disabled != file.data.disabled
            });
        if plugins_changed {
            let moved = self
                .plugins_seen
                .as_ref()
                .is_some_and(|(folder, _)| *folder != file.data.plugins);
            if moved {
                crate::config::plugins::forget();
            }
            self.plugins_seen = Some((file.data.plugins.clone(), file.data.disabled.clone()));
            let streams = crate::plugins::reconfigure(&file.data.plugins, &file.data.disabled);
            self.replugged(streams);
            tracing::info!("プラグインを再読み込みした");
        }
        // レンダリングのバックエンド（wgpu ↔ GLES）はイベントループごと違うので起動し直す。auto のままなら、
        // 起動したときに決めた方のまま（auto で GLES を選んでいれば GLES。GPU を名指しされたら wgpu）。
        let now_gles = self.choices.draws_gles();
        let then_gles = crate::choice::Choices {
            auto_gles: self.choices.auto_gles && applied.choices.gpu.is_none(),
            ..applied.choices.clone()
        }
        .draws_gles();
        let rebuild = !now_gles
            && !then_gles
            && (self.choices.backend != applied.choices.backend
                || self.choices.gpu != applied.choices.gpu);
        let restart = now_gles != then_gles;
        if rebuild && !restart {
            self.choices.backend = applied.choices.backend;
            self.choices.gpu = applied.choices.gpu.clone();
            self.choices.render = applied.choices.render;
            tracing::info!("映像のレンダラーを切り替える");
            self.rebuild_gpu(applied.choices.backend);
            return;
        }
        if restart {
            self.toast("設定を反映するため再起動します。");
            crate::config::request_restart();
            self.save_volume();
            self.quit();
            return;
        }
        // いまのソースがプロファイルのどれかなら替えない（最初の局へ連れ戻さない）。
        let profile = crate::config::load_state().profile;
        let now = self.live.as_ref().and_then(|live| live.url.clone());
        if !uses_source(file, &profile, now.as_deref())
            && let Some(wanted) = first_source(file, &profile)
        {
            self.switch_source(wanted);
            return;
        }
        if redecode || undecoded {
            self.reopen_current();
        }
        tracing::info!("設定を保存した");
    }

    pub(super) fn choose_profile(&mut self, name: String) {
        let file = crate::config::load_plain();
        let remembered = name.clone();
        crate::config::update_state(move |state| state.profile = remembered);
        self.panes.say(ToPanes::Profile(name.clone()));
        self.apply_channel_prefs(&file);
        self.catalog_due = true;
        let now = self.live.as_ref().and_then(|live| live.url.clone());
        let wanted =
            first_source(&file, &name).filter(|_| !uses_source(&file, &name, now.as_deref()));
        let named = if name.is_empty() {
            crate::ui::panes::settings::ALL
        } else {
            &name
        };
        self.profile_pending = false;
        match wanted {
            // 再生モード中はソースに触らない（替えると放送へ連れ戻す）。
            Some(wanted) if Some(wanted.came()) != now && self.player_mode => {
                self.profile_pending = true;
                self.toast(&format!(
                    "プロファイル：{named}\nソースは再生モードを終えたときに切り替えます。"
                ));
            }
            Some(wanted) if Some(wanted.came()) != now => self.switch_source(wanted),
            Some(_) => self.toast(&format!("プロファイル：{named}")),
            None => self.toast("このプロファイルで使えるソースがありません。"),
        }
    }

    pub(crate) fn replugged(&mut self, streams: bool) {
        let loaded = crate::plugins::loaded();
        if let Err(why) = self.broadcast.send(ToBroadcast::Plugins(loaded)) {
            tracing::warn!("放送のスレッドにプラグインの変更を通知できない: {why}");
        }
        if streams
            && !self.player_mode
            && let Some(key) = self.asked_key
        {
            tracing::info!("ストリームを処理するプラグインが変わったため再選局する");
            self.tune(key);
        }
    }

    pub(super) fn apply_channel_prefs(&mut self, file: &crate::config::File) {
        self.profile_changed();
        let profile = crate::config::load_state().profile;
        self.profiles = std::iter::once(crate::ui::panes::settings::ALL.to_owned())
            .chain(file.profiles.iter().map(|p| p.name.clone()))
            .collect();
        self.profile_shown = if profile.is_empty() {
            crate::ui::panes::settings::ALL.to_owned()
        } else {
            profile.clone()
        };
        // ここから下はプロファイルの上書きを重ねた物で（`[profile.channel]`）。
        let file = &crate::config::layered(file, &profile);
        zwcore::region::set_postal(&file.general.zipcode);
        crate::config::plugins::set_receiver(
            "receiver.zipcode",
            zwcore::region::digits(&file.general.zipcode),
        );
        for (key, value) in [
            ("receiver.maker_id", &file.data.maker_id),
            ("receiver.maker_oui", &file.data.maker_oui),
            ("receiver.model_id", &file.data.model_id),
        ] {
            let value = value.trim();
            crate::config::plugins::set_receiver(
                key,
                (!value.is_empty()).then(|| value.to_owned()),
            );
        }
        zwcore::input::source::coaxd::set_tuner_settings(
            file.sources
                .iter()
                .filter(|source| source.kind == "coaxd")
                .flat_map(|source| &source.tuner_settings)
                .filter(|(_, setting)| !setting.is_empty())
                .map(|(id, setting)| (id.clone(), setting.to_core()))
                .collect(),
        );
        zwcore::guide::Band::set_networks(crate::config::apply::networks(file, &profile));
        zwcore::guide::Detailed::set(crate::config::apply::extended(file));
        let _ = self
            .guide_to
            .send(ToGuide::Prefs(crate::config::apply::prefs(file)));
        self.hidden = file
            .channel
            .hide
            .iter()
            .filter_map(|text| crate::config::key::KeyPattern::parse(text))
            .collect();
        self.listed_mark = None;
        self.menu_mark = None;
    }

    pub(super) fn switch_source(&mut self, wanted: Opening) {
        tracing::info!("ソースを {} に切り替える", wanted.came());
        let (send, back) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(String::from("switch"))
            .spawn(move || {
                let opened = match wanted {
                    Opening::Mirakurun { url, by } => tuning::open_mirakurun(&url, by),
                    Opening::Coaxd {
                        endpoint,
                        priority,
                        tuners,
                        profile,
                    } => tuning::open_coaxd(&endpoint, priority, &tuners, &profile),
                };
                let _ = send.send(opened);
            })
            .ok();
        self.switching = Some(back);
    }

    pub(super) fn start_source(&mut self, starting: Starting) {
        let waiting = starting.waiting();
        if !self.telling_notice() {
            self.toast(waiting);
        }
        let now = Instant::now();
        self.starting_since = Some((now, now, waiting));
        let (send, back) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(String::from("reach"))
            .spawn(move || {
                let _ = send.send(starting.reach());
            });
        match spawned {
            Ok(_) => self.started = Some(back),
            Err(why) => {
                self.starting_since = None;
                tracing::warn!("ソースに接続するスレッドを起動できない: {why}");
            }
        }
    }

    pub(super) fn tell_decoding(&mut self) {
        self.asked_decoder = Instant::now();
        let tables = self.player.tables();
        let decoding = crate::ui::panes::decoder::Decoding {
            sound_codec: compose::sound_codec(tables),
            dual_mono: tables.dual_mono,
            video: self.player.video_stats(),
            sound: self.player.sound_stats(),
            painter: self.painter.as_ref().map(Painter::name),
            scaling: self.painter.as_ref().map(|_| self.player.scaling()),
        };
        self.panes.say(ToPanes::Decoding(Box::new(decoding)));
    }

    pub(super) fn next_notice(&mut self) {
        if self.notices.is_empty() {
            return;
        }
        let gone = self.toast.as_ref().is_none_or(Toast::gone);
        if gone && let Some(text) = self.notices.pop_front() {
            self.toast(&text);
            self.telling_since = Some(Instant::now());
        }
    }

    fn telling_notice(&self) -> bool {
        !self.notices.is_empty()
            || self.telling_since.is_some_and(|since| {
                let hold = self.toast.as_ref().map_or(TOAST_FOR, |toast| toast.hold);
                since.elapsed().as_secs_f32() < hold
            })
    }

    pub(super) fn take_started(&mut self) {
        let Some(back) = self.started.as_ref() else {
            return;
        };
        let answer = match back.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Some((since, said, waiting)) = self.starting_since
                    && said.elapsed() >= Duration::from_secs(3)
                    && !self.telling_notice()
                {
                    self.starting_since = Some((since, Instant::now(), waiting));
                    self.toast(&format!("{waiting}（{} 秒）", since.elapsed().as_secs()));
                }
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.started = None;
                self.starting_since = None;
                return;
            }
        };
        self.started = None;
        self.starting_since = None;
        let started = match answer {
            Ok(started) => started,
            Err(why) => {
                self.complain(&why);
                return;
            }
        };
        tracing::info!("{}  {} 局", started.said, started.channels.len());
        // TS ID を返さないソースでは --tsid を断る（どの局も合わないと表示すると紛らわしい）。
        let unknowable = matches!(started.pick, Pick::Named(named) if named.transport.is_some())
            && started
                .channels
                .iter()
                .all(|channel| channel.transport.is_none());
        let chosen = if unknowable {
            None
        } else {
            started.pick.from(&started.channels).cloned()
        };
        let Some(live) = self.live.as_mut() else {
            return;
        };
        live.channels = started.channels;
        if let Some(url) = started.guide_from {
            live.fetched = Some(fetch_guide(url));
        }
        self.roster_seeded = false;
        self.listed_mark = None;
        self.catalog_due = true;
        match chosen.and_then(|channel| channel.key) {
            Some(key) => {
                self.watched(key);
                self.tune(key);
            }
            None if unknowable => {
                self.complain("このソースは TSID を返さないため、--tsid では選局できません。\n--nid と --sid で指定してください。");
            }
            None if let Pick::Named(named) = started.pick => {
                self.complain(&format!(
                    "指定されたサービスが見つかりません。\n{}",
                    named.text()
                ));
            }
            None if live_is_empty(self.live.as_ref()) => {
                self.complain(
                    "受信できるチャンネルがありません。\nチューナーの接続を確認するか、チャンネルスキャンを行ってください。",
                );
            }
            None => self
                .toast("coaxd に接続しました。\nチャンネルスキャンを行うとチャンネルを選べます。"),
        }
    }

    pub(super) fn take_switched(&mut self) {
        let Some(back) = self.switching.as_ref() else {
            return;
        };
        let opened = match back.try_recv() {
            Ok(opened) => opened,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.switching = None;
                return;
            }
        };
        self.switching = None;
        match opened {
            Ok(live) => {
                self.switching_to = None;
                let key = self
                    .asked_key
                    .filter(|key| {
                        live.channels
                            .iter()
                            .any(|channel| channel.key == Some(*key))
                    })
                    .or_else(|| live.channels.iter().find_map(|channel| channel.key));
                self.toast(&format!("ソースを切り替えました。\n{}", live.supply.name()));
                // 新しいソースのスレッドを起動する（起動しないと選局が「しています」のまま止まる）。
                if let Some(mut old) = self.live.take() {
                    old.supply.close();
                }
                let mut live = live;
                live.supply.hand();
                self.live = Some(live);
                self.roster_seeded = false;
                self.listed_mark = None;
                if let Some(key) = key {
                    self.tune(key);
                }
            }
            Err(why) => {
                if let Some(label) = self.switching_to.take()
                    && !self.tried.contains(&label)
                {
                    self.tried.push(label);
                }
                if let Some(key) = self.asked_key
                    && let Some((label, opening)) = self.offer_for(key)
                {
                    tracing::warn!("ソースを開けない（{why}）。同じ局を出す {label} で開く");
                    self.switching_to = Some(label);
                    self.switch_source(opening);
                    return;
                }
                self.complain_not_tuned(&said_with("ソースを開けません。", &why));
            }
        }
    }

    pub(super) fn reopen_current(&mut self) {
        if self.player.is_recording() {
            let fraction = self
                .player
                .progress()
                .filter(|progress| progress.length > 0)
                .map_or(0.0, |progress| {
                    progress.position as f32 / progress.length as f32
                });
            self.seek_to(fraction);
            return;
        }
        if self.player_mode {
            return;
        }
        if let Some(key) = self.asked_key {
            self.tune(key);
        }
    }

    pub(super) fn save_volume_when_quiet(&mut self) {
        let Some(moved) = self.volume_moved else {
            return;
        };
        if moved.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.save_volume();
    }

    pub(super) fn save_volume(&mut self) {
        if self.volume_moved.take().is_none() {
            return;
        }
        let (volume, mute) = (self.player.volume(), self.player.muted());
        crate::config::update_state(move |state| {
            state.volume = Some(volume);
            state.mute = mute;
        });
    }
}
