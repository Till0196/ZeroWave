// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面（メインスレッド）へ渡すスナップショットをビルドし、ステータスバーや OSD に出す文を作る。

use super::*;
use zwcore::engine::DualMono;
use zwcore::guide::Described;

impl Stage {
    pub(super) fn compose(&mut self, drew: bool) {
        // 毎回要求するとデコードの側がスナップショット作りに時間を取られる。
        if self.asked_tables.elapsed() >= TABLES_EVERY {
            self.asked_tables = Instant::now();
            self.player.ask_tables();
        }
        if self.polled.elapsed() >= POLL_EVERY {
            self.polled = Instant::now();
            self.status.bitrate = self.player.bitrate();
        }
        let told = self.player.told();
        // 大きさが変わったときは待たない（等倍の表示がウィンドウの大きさに直に結び付いている）。
        let area = self.area();
        if self.scene_at.elapsed() >= SCENE_EVERY || self.scened != Some((told, area)) {
            self.scene_at = Instant::now();
            self.scened = Some((told, area));
            collect(&self.player, &self.guide, &mut self.status, area);
            self.judge_reception();
            self.settle_errors();
            self.retitle();
        }
        self.refresh_plugins();
        let seen = self.player.tables().watching.map(|key| {
            (
                key,
                self.player
                    .tables()
                    .now
                    .as_ref()
                    .map(|programme| programme.event)
                    .unwrap_or_default(),
            )
        });
        if seen != self.seen {
            let settling = self.seek_settling;
            if settling && seen.is_some_and(|(_, event)| event != 0) {
                self.seek_settling = false;
            }
            // 局の変化は選局の側が出す。イベント ID が 0 から入ったのは「番組が分かった」だけ。
            let boundary = match (self.seen, seen) {
                (Some((was_key, was_event)), Some((key, event))) => {
                    was_key == key && was_event != 0 && event != 0
                }
                _ => false,
            };
            if boundary && !self.seeking && !settling {
                tracing::info!(
                    "番組の切り替わり: イベント {:?} → {:?}",
                    self.seen.map(|(_, event)| event),
                    seen.map(|(_, event)| event)
                );
                self.osd
                    .show_on(self.osd_auto, ui::overlay::osd::Why::Event);
                // 薄れ切っている間はビルドし直さないので、前の中身が一瞬出るのを防ぐ。
                self.osded = None;
            }
            self.seen = seen;
        }
        // 濃さに引きずられてビルドし直さない。選局中はテーブルが前の局のままなので比較用のキーに入れる。
        let tuning = self
            .live
            .as_ref()
            .and_then(|live| live.supply.tuning())
            .map(|tuning| tuning.ticket);
        let osded = Some((told, tuning, self.guide_version, self.said.clone()));
        if self.osd.is_shown() && self.osded != osded {
            self.osded = osded;
            self.osd_version += 1;
            let tuning = self.live.as_ref().and_then(|live| live.supply.tuning());
            self.osd_shown = osd_of(
                &self.player,
                &self.guide,
                &self.status,
                self.said.as_deref(),
                tuning,
                // 録画の再生中は要求した局で調べない（録画の OSD に前の局の名前が出る）。
                if self.player.is_recording() {
                    None
                } else {
                    self.asked_key
                },
            );
        }
        // ウィンドウが閉じていても OSD が SDT のロゴ・名前を使う。
        let every = if self.mini || self.wants_guide {
            GUIDE_EVERY
        } else {
            GUIDE_EVERY_IDLE
        };
        if self.asked_guide.elapsed() >= every {
            self.asked_guide = Instant::now();
            self.player.ask_guide();
        }
        // 置き換えない（全局分はここにしかない）。
        if let Some(guide) = self.player.taken_guide() {
            let _ = self.guide_to.send(ToGuide::Heard(Box::new(guide)));
        }
        let backdrop = self.backdrop(drew);
        let opacity = self.osd.opacity();
        self.tell_captures();
        self.follow_sound_change();
        self.follow_picture_change();
        self.tell_scene(backdrop, opacity, told);
    }

    /// 黒はこのスレッドが映像と同じフレームに塗る。画面側にレンダリングさせると、モーダルループの間
    /// 黒が新しい局の映像の上に乗り続ける。
    pub(super) fn backdrop(&self, drew: bool) -> scene::Backdrop {
        if self.player.audio_only() || self.player.data_only() {
            scene::Backdrop::Black
        } else if self.player_mode && !self.player.is_recording() {
            scene::Backdrop::Waiting
        } else if self.player.between() || (!drew && self.player.blank()) {
            if self.player.ever_drawn() {
                scene::Backdrop::Black
            } else {
                scene::Backdrop::Waiting
            }
        } else {
            scene::Backdrop::None
        }
    }

    pub(super) fn retitle(&mut self) {
        let tables = self.player.tables();
        let channel = self.status.channel.clone().unwrap_or_default();
        let mut event = String::new();
        if let Some(now) = &tables.now {
            if let Some(start) = now.start {
                event.push_str(&zwcore::guide::airtime(start, now.ends()));
                if now.ends().is_none() {
                    event.push_str("(終了未定)");
                }
            }
            if !now.title.is_empty() {
                if !event.is_empty() {
                    event.push(' ');
                }
                event.push_str(&ui::text::badges::bracketed(&now.title));
            }
        }
        let mut title = titled(&self.title_pattern, &channel, &event);
        if title.is_empty() {
            title.push_str("ZeroWave");
        } else {
            title.push_str(" - ZeroWave");
        }
        if self.title.as_deref() != Some(title.as_str()) {
            self.title = Some(title.clone());
            let _ = self.requests.send(Request::Title(title));
            let _ = self.proxy.send_event(Wake);
        }
    }

    pub(super) fn choose_sound(&mut self, at: usize) {
        let tables = self.player.tables();
        let Some(choice) = tables.choices.get(at).cloned() else {
            return;
        };
        let stream_changes = tables.sound_at != Some(choice.stream);
        if let Some(mode) = choice.mode {
            self.player.set_dual_mono(mode);
            self.player.ask(Ask::DualMono(mode));
        }
        if stream_changes {
            self.sound_asked = Some((
                self.player.tables().sound.as_ref().map(|s| s.id),
                Instant::now(),
            ));
            self.player.ask(Ask::Sound(choice.stream));
        } else if choice.mode.is_some() {
            self.toast(&format!("音声：［{}］", choice.label));
        }
    }

    fn follow_sound_change(&mut self) {
        let Some((was, asked)) = self.sound_asked else {
            return;
        };
        let tables = self.player.tables();
        let now = tables.sound.as_ref().map(|s| s.id);
        if now != was && now.is_some() {
            self.sound_asked = None;
            match sound_toast(tables) {
                Some(text) => self.toast(&text),
                None => tracing::info!("音声を切り換えた"),
            }
        } else if asked.elapsed() > Duration::from_secs(1) {
            self.sound_asked = None;
            self.toast("現在音声切換はできません。");
        }
    }

    fn follow_picture_change(&mut self) {
        let Some((was, asked)) = self.picture_asked else {
            return;
        };
        let tables = self.player.tables();
        let now = tables.picture_at;
        if now != was && now.is_some() {
            self.picture_asked = None;
            let name = now.and_then(|at| tables.pictures.get(at)).cloned();
            let text = match name.as_deref() {
                Some(name) if name.ends_with("(降雨対応放送)") => {
                    "降雨対応放送に切り換えました。画質が低下した状態です。".to_owned()
                }
                Some(name) => format!("映像：［{name}］"),
                None => {
                    tracing::info!("映像を切り換えた");
                    return;
                }
            };
            self.toast(&text);
        } else if asked.elapsed() > Duration::from_secs(1) {
            self.picture_asked = None;
        }
    }

    fn bring_back_toast(&mut self) {
        if self.toast_behind.is_none() || self.toast_opacity().is_some() {
            return;
        }
        if let Some(mut toast) = self.toast_behind.take() {
            self.toasts += 1;
            toast.serial = self.toasts;
            toast.at = Instant::now();
            self.toast = Some(toast);
        }
    }

    fn toast_opacity(&self) -> Option<f32> {
        let toast = self.toast.as_ref()?;
        let age = toast.at.elapsed().as_secs_f32();
        if age < toast.hold {
            Some(1.0)
        } else if age < toast.hold + TOAST_FADE {
            Some(1.0 - (age - toast.hold) / TOAST_FADE)
        } else {
            None
        }
    }

    fn volume_opacity(&self) -> Option<f32> {
        let (at, refused) = self.volume_shown?;
        if self.player.muted() && !refused {
            return Some(1.0);
        }
        let age = at.elapsed().as_secs_f32();
        if age < self.volume_hold {
            Some(1.0)
        } else if age < self.volume_hold + TOAST_FADE {
            Some(1.0 - (age - self.volume_hold) / TOAST_FADE)
        } else {
            None
        }
    }

    pub(super) fn tell_scene(
        &mut self,
        backdrop: scene::Backdrop,
        opacity: Option<f32>,
        told: u64,
    ) {
        let shade = (opacity.unwrap_or(0.0) * 1000.0) as u32;
        self.bring_back_toast();
        let toast = self.toast_opacity();
        let toast_shade = (toast.unwrap_or(0.0) * 1000.0) as u32;
        self.player.refresh_recording();
        let progress = self.player.progress().or_else(|| {
            (self.player_mode && !self.player.is_recording()).then_some(zwcore::player::Progress {
                fraction: 0.0,
                position: 0,
                length: 0,
                paused: true,
                buffering: false,
                pace: 1.0,
            })
        });
        let buffering = progress.is_some_and(|p| p.buffering);
        match (buffering, self.buffering_told) {
            (true, told)
                if told.is_none_or(|at| at.elapsed().as_secs_f32() >= super::TOAST_FOR) =>
            {
                self.buffering_told = Some(std::time::Instant::now());
                self.toast("読み込んでいます...");
            }
            (false, Some(_)) => self.buffering_told = None,
            _ => {}
        }
        let second = progress.map(|p| (p.position / zwcore::engine::clock::HZ) as u32);
        let playing = progress.map(|p| (p.paused, p.buffering, (p.pace * 100.0) as u32));
        // メニューの中身も比較用のキーに入れる（何も流れていない間に古いまま残らないように）。
        let menu = self.menu_filling();
        let volume = self.volume_opacity().map(|opacity| {
            (
                scene::Volume {
                    volume: self.player.volume(),
                    muted: self.player.muted(),
                    passthrough: self.volume_shown.is_some_and(|(_, refused)| refused),
                },
                opacity,
            )
        });
        let volume_mark = volume.map(|(shown, opacity)| {
            (
                shown.volume,
                shown.muted,
                shown.passthrough,
                (opacity * 1000.0) as u32,
            )
        });
        let trouble = self.trouble();
        let idle = self.idle().filter(|_| trouble.is_none());
        let idle_mark = (
            idle.as_ref()
                .map(|idle| (idle.now.programme.event, idle.now.programme.detail.len())),
            trouble.as_ref().map(scene::Trouble::mark),
        );
        let mark = (
            told,
            shade,
            backdrop,
            toast_shade,
            self.toast.as_ref().map_or(0, |toast| toast.serial),
            self.osd_version,
            self.surface.space.is_hdr(),
            second,
            playing,
            Arc::as_ptr(&menu) as usize,
            idle_mark,
            (
                volume_mark,
                self.mini.then(|| {
                    (
                        Arc::as_ptr(&self.listing) as usize,
                        self.guide.now_or_machine().div_euclid(60),
                        Arc::as_ptr(&self.plugin_shared_told) as usize,
                    )
                }),
            ),
        );
        if self.told_scene == Some(mark) {
            return;
        }
        self.told_scene = Some(mark);
        let strip = self.mini.then(|| {
            let tables = self.player.tables();
            let moment = self.guide.now_or_machine();
            let (now_when, now_title) = now_text(tables);
            let detail = tables.watching.and_then(|key| {
                let programme = self
                    .guide
                    .at(key, moment)
                    .cloned()
                    .or_else(|| tables.now.clone())?;
                let now = ui::guide::detail::of(&self.guide, key, programme, None);
                let band = band_label(key, now.remote_key);
                let next = self
                    .guide
                    .after(key, moment)
                    .cloned()
                    .or_else(|| tables.next.clone());
                Some(Box::new(scene::Idle { now, band, next }))
            });
            scene::Banded {
                listing: Arc::clone(&self.listing),
                now_when,
                now_title,
                band: self.band(),
                bands: self.bands(),
                watching: tables.watching,
                from: self.listing.from,
                moment,
                detail,
                guide: Arc::clone(&self.guide),
                plugins: Arc::clone(&self.plugin_shared_told),
            }
        });
        // 形が分からない間は OSD を出さない（ウィンドウの幅で出てから映像の幅へ縮む）。
        let format = self.player.picture_format();
        let picture = format.as_ref().map(shown_size);
        let source_ratio = format.as_ref().map(|format| {
            let (w, h) = shown_size(format);
            let (num, den) = format.aspect;
            if num == 0 || den == 0 {
                w as f32 / h as f32
            } else {
                num as f32 / den as f32
            }
        });
        let pan_scan = format
            .as_ref()
            .filter(|_| !self.inset_on)
            .map(|format| self.frame.pan_scan((format.width, format.height)))
            .unwrap_or_default();
        if pan_scan != self.player.pan_scan() {
            tracing::info!(
                "アスペクト比 {}（{pan_scan:?}）",
                if self.inset_on {
                    "データ放送の表示位置を優先"
                } else {
                    self.frame.aspect.as_str()
                }
            );
        }
        self.player.set_pan_scan(pan_scan);
        let ratio = source_ratio.map(|source| pan_scan.shape(source));
        let crop = source_ratio.map_or([0.0; 4], |source| pan_scan.visible(source));
        let scene = scene::Scene {
            menu,
            now: self.player.now().unwrap_or_default(),
            status: crate::ui::overlay::status::Status {
                profile: self.live.is_some().then(|| self.profile_shown.clone()),
                ..self.status.clone()
            },
            osd: opacity
                .filter(|_| {
                    picture.is_some() || self.player.audio_only() || self.player.data_only()
                })
                .map(|opacity| (self.osd_shown.clone(), opacity)),
            strip,
            picture,
            ratio,
            backdrop,
            toast: toast.and_then(|opacity| {
                self.toast
                    .as_ref()
                    .map(|toast| (toast.shown.clone(), opacity))
            }),
            hdr: self.surface.space.is_hdr(),
            crop,
            video_filters: self
                .plugin_shared
                .report
                .as_ref()
                .map(|report| {
                    report
                        .stages
                        .iter()
                        .filter(|stage| !stage.deinterlace)
                        .map(|stage| stage.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            progress,
            now_playing: now_playing(&self.osd_shown),
            event: self.osd_shown.title.name.clone(),
            idle: idle.map(Arc::new),
            trouble: trouble.map(Arc::new),
            volume,
        };
        self.say(FromStage::Scene(Box::new(scene)));
    }

    fn judge_reception(&mut self) {
        let judging = !self.player_mode && !self.player.is_recording() && self.switching.is_none();
        // Mirakurun は TS が届くまで応答しない。coaxd は電波が無ければすぐエラーを返すので、待ちはチューナーの順番。
        let waiting = self.live.as_ref().and_then(|live| {
            let tuning = live.supply.tuning()?;
            let coaxd = live
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("coaxd:"));
            Some((!coaxd).then_some(tuning.began))
        });
        let Some(key) = self.asked_key.filter(|_| judging && self.live.is_some()) else {
            self.reception.pause();
            self.status.signal_alarm = false;
            return;
        };
        match waiting {
            Some(Some(asked)) => {
                let verdict = self.reception.waiting(Instant::now(), asked);
                self.status.signal_alarm = verdict.is_some_and(|verdict| verdict.trouble.alarms());
                return;
            }
            Some(None) => {
                self.reception.pause();
                self.status.signal_alarm = false;
                return;
            }
            None => {}
        }
        let tables = self.player.tables();
        let program = tables
            .programs
            .iter()
            .find(|program| program.number == key.service);
        let kind_in_network = tables
            .networks
            .iter()
            .filter(|network| network.own)
            .flat_map(|network| &network.multiplexes)
            .filter(|multiplex| {
                tables
                    .identity
                    .is_none_or(|identity| identity.transport == multiplex.transport)
            })
            .flat_map(|multiplex| &multiplex.services)
            .find(|(service, _)| *service == key.service)
            .map(|(_, kind)| *kind);
        let has_picture = !self.player.audio_only() && !self.player.data_only();
        let look = reception::Look {
            bytes: self.player.read_bytes(),
            errors: tables.counts.errors,
            dropped: tables.counts.dropped,
            others: tables
                .pids
                .iter()
                .filter(|(pid, _)| *pid != 0x1fff)
                .map(|(_, counted)| counted.arrived)
                .sum(),
            listed: tables.listed.clone(),
            mapped: program.is_some(),
            kind_in_network,
            stalled: has_picture && self.player.stalled(),
            unsupported: program.is_some_and(|program| !reception::unplayable(program).is_empty()),
        };
        let verdict = self.reception.look(Instant::now(), look);
        self.status.signal_alarm = verdict.is_some_and(|verdict| verdict.trouble.alarms());
    }

    fn trouble(&self) -> Option<scene::Trouble> {
        use super::reception::Trouble as Why;
        let verdict = self.reception.shown().filter(|verdict| verdict.covers)?;
        let tables = self.player.tables();
        let key = self.asked_key?;
        let service = self.guide.service(key);
        let name = service
            .map(|service| service.name.clone())
            .or_else(|| self.status.channel.clone())
            .unwrap_or_else(|| format!("サービス {}", super::id_text(key.service)));
        let remote_key = service.and_then(|service| service.remote_key);
        let station = scene::Station {
            channel: name.clone(),
            badge: service.map_or_else(
                || ui::guide::epg::badge_of(&name),
                zwcore::guide::Service::badge,
            ),
            badge_colour: ui::guide::epg::badge_colour(&name),
            logo: self.guide.logo_of(key).cloned(),
            band: band_label(key, remote_key),
        };
        let moment = self.guide.now_or_machine();
        let signal = format!(
            "{}　{:.2} Mbps",
            self.status.cnr.map_or_else(
                || String::from("CNR -- dB"),
                |cnr| format!("CNR {cnr:.1} dB")
            ),
            self.status.bitrate
        );
        let now = || {
            self.guide.at(key, moment).and_then(|programme| {
                let start = programme.start?;
                Some((
                    String::from("放送中"),
                    format!(
                        "{}　{}",
                        zwcore::guide::airtime(start, programme.ends()),
                        ui::text::badges::titled(&programme.title, &programme.detail).name
                    ),
                ))
            })
        };
        let row = |heading: &str, value: String| (heading.to_owned(), value);
        let (rows, below) = match verdict.trouble {
            Why::NoSignal => (
                vec![
                    row(
                        "チューナー",
                        self.status
                            .tuner
                            .clone()
                            .unwrap_or_else(|| String::from("—")),
                    ),
                    row("信号", signal),
                    row("状態", String::from("受信できません")),
                ],
                now(),
            ),
            Why::Suspended => {
                let next = self
                    .guide
                    .after(key, moment)
                    .and_then(|programme| Some((programme.start?, programme)));
                let rows = vec![
                    row("状態", String::from("放送休止中（正常に受信しています）")),
                    row("受信状態", String::from("良好")),
                ];
                let below = next.map(|(start, programme)| {
                    (
                        String::from("次"),
                        format!(
                            "{}　{}",
                            clock_text(start),
                            ui::text::badges::titled(&programme.title, &programme.detail).name
                        ),
                    )
                });
                (rows, below)
            }
            Why::Weak => (
                vec![
                    row("信号", signal),
                    row("エラー", grouped(tables.counts.errors)),
                    row("ドロップ", grouped(tables.counts.dropped)),
                ],
                now(),
            ),
            Why::Ended => {
                let kind = tables
                    .networks
                    .iter()
                    .flat_map(|network| &network.multiplexes)
                    .flat_map(|multiplex| &multiplex.services)
                    .find(|(service, _)| *service == key.service)
                    .map(|(_, kind)| *kind)
                    .or(service.map(|service| service.kind));
                let named = match kind {
                    Some(0xa1) => "臨時映像サービス",
                    Some(0xa3) => "臨時データサービス",
                    _ => "臨時サービス",
                };
                (
                    vec![
                        row(
                            "サービス",
                            format!("{named}（SID {}）", super::id_text(key.service)),
                        ),
                        row("状態", String::from("放送は終了しました")),
                    ],
                    None,
                )
            }
            Why::Unsupported => {
                let rows = tables
                    .programs
                    .iter()
                    .find(|program| program.number == key.service)
                    .map(|program| {
                        super::reception::unplayable(program)
                            .into_iter()
                            .map(|(what, name)| row(what, format!("{name}（対応していません）")))
                            .collect()
                    })
                    .unwrap_or_default();
                (rows, now())
            }
        };
        Some(scene::Trouble {
            code: verdict.trouble.code(),
            title: verdict.trouble.title(),
            guide: verdict.trouble.guide(),
            station: Some(station),
            rows,
            below,
            dim: verdict.trouble == Why::Weak,
        })
    }

    fn idle(&self) -> Option<scene::Idle> {
        if !self.player.audio_only() {
            return None;
        }
        let tables = self.player.tables();
        let key = tables.watching?;
        let moment = self.guide.now_or_machine();
        // 録画の再生では EIT の現在と次だけ（PC の時計は録った時ではない）。
        let recorded = self.player.is_recording();
        let programme = if recorded {
            tables.now.clone()?
        } else {
            self.guide.at(key, moment)?.clone()
        };
        let service = self.guide.service(key);
        let name = service
            .map(|service| service.name.clone())
            .or_else(|| self.status.channel.clone())
            .unwrap_or_default();
        let remote_key = service.and_then(|service| service.remote_key);
        Some(scene::Idle {
            now: ui::guide::detail::Detail {
                programme,
                channel: name.clone(),
                remote_key,
                badge: service.map_or_else(
                    || ui::guide::epg::badge_of(&name),
                    zwcore::guide::Service::badge,
                ),
                badge_colour: ui::guide::epg::badge_colour(&name),
                logo: self.guide.logo_of(key).cloned(),
                shared: Vec::new(),
                follow: None,
                now: (!recorded).then_some(moment),
            },
            band: band_label(key, remote_key),
            next: if recorded {
                tables.next.clone()
            } else {
                self.guide.after(key, moment).cloned()
            },
        })
    }
}

fn sound_toast(tables: &player::Tables) -> Option<String> {
    if let Some(choice) = tables.choice_at.and_then(|at| tables.choices.get(at)) {
        return Some(format!("音声：［{}］", choice.label));
    }
    let sound = tables.sound.as_ref()?;
    let mut mode = sound
        .component
        .as_ref()
        .map(Described::arrangement)
        .unwrap_or_default();
    if let Some(language) = sound
        .component
        .as_ref()
        .map(|c| zwcore::guide::language_name(&c.language))
        .filter(|name| !name.is_empty())
    {
        if !mode.is_empty() {
            mode.push('　');
        }
        mode.push_str(language);
    }
    if mode.is_empty() {
        return None;
    }
    let program = tables
        .carried
        .iter()
        .find(|(pid, _, _)| *pid == sound.id)
        .map(|(_, p, _)| *p);
    let sounds: Vec<u16> = tables
        .carried
        .iter()
        .filter(|(_, p, kind)| Some(*p) == program && is_sound_kind(kind))
        .map(|(pid, _, _)| *pid)
        .collect();
    let number = if sounds.len() > 1 {
        sounds
            .iter()
            .position(|pid| *pid == sound.id)
            .map(|i| format!("音声{} ", i + 1))
            .unwrap_or_default()
    } else {
        String::new()
    };
    Some(format!("音声：［{number}{mode}］"))
}

fn is_sound_kind(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "0f" | "11" | "03" | "04" | "mp4a"
    )
}

fn now_playing(osd: &ui::overlay::osd::Osd) -> String {
    if osd.channel.is_empty() {
        return String::new();
    }
    [
        osd.channel.as_str(),
        osd.title.name.as_str(),
        osd.airtime.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("　")
}

fn plugin_menus(
    said: &[(crate::plugin_ui::UiPlugin, Vec<zwsdk::ui::MenuItem>)],
) -> Vec<crate::window::menu::PluginMenu> {
    use zwsdk::ui::{MenuItem, Open};
    let settings = || MenuItem::Open {
        label: String::from("設定…"),
        open: Open::Settings,
    };
    let opens_settings = |items: &[MenuItem]| {
        items.iter().any(|item| {
            matches!(
                item,
                MenuItem::Open {
                    open: Open::Settings,
                    ..
                }
            )
        })
    };
    let loaded = crate::plugins::loaded();
    let has_settings = |stem: &str| {
        loaded.all().iter().any(|plugin| {
            crate::plugins::stem_of(plugin) == stem
                && (!plugin.manifest().config.is_empty() || plugin.manifest().ui.settings.is_some())
        })
    };
    let mut menus: Vec<crate::window::menu::PluginMenu> = said
        .iter()
        .map(|(plugin, items)| {
            let mut items = items.clone();
            if has_settings(&plugin.stem) && !opens_settings(&items) {
                if !items.is_empty() {
                    items.push(MenuItem::Separator);
                }
                items.push(settings());
            }
            crate::window::menu::PluginMenu {
                stem: plugin.stem.clone(),
                title: plugin.title.clone(),
                items,
            }
        })
        .collect();
    for plugin in loaded.all() {
        let stem = crate::plugins::stem_of(plugin);
        let manifest = plugin.manifest();
        if menus.iter().any(|menu| menu.stem == stem)
            || (manifest.config.is_empty() && manifest.ui.settings.is_none())
        {
            continue;
        }
        let title = if manifest.ui.title.is_empty() {
            manifest.name.clone()
        } else {
            manifest.ui.title.clone()
        };
        menus.push(crate::window::menu::PluginMenu {
            stem,
            title,
            items: vec![settings()],
        });
    }
    menus
}

fn band_label(key: zwcore::guide::Key, remote_key: Option<u8>) -> String {
    use zwcore::guide::Band;
    let name = Band::of(key.network).name();
    let terrestrial = Band::is_terrestrial(key.network);
    let number = match (terrestrial, remote_key) {
        (true, Some(remote)) => {
            format!(
                "{:03}",
                u32::from(remote) * 10 + u32::from(key.service & 0x07) + 1
            )
        }
        (true, None) => String::new(),
        _ => format!("{:03}", key.service),
    };
    if number.is_empty() {
        name
    } else {
        format!("{name} {number}")
    }
}

fn clock_text(moment: zwcore::guide::Moment) -> String {
    let (_, _, _, hour, minute, _) = zwcore::guide::civil(zwcore::guide::shown(moment));
    format!("{hour}:{minute:02}")
}

fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (nth, digit) in digits.chars().enumerate() {
        if nth > 0 && (digits.len() - nth).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn airtime_text(programme: Option<&zwcore::guide::Programme>) -> String {
    let Some(programme) = programme else {
        return String::new();
    };
    let Some(start) = programme.start else {
        return String::new();
    };
    let mut said = zwcore::guide::hhmm(start);
    if let Some(ends) = programme.ends() {
        said.push_str(" - ");
        said.push_str(&zwcore::guide::hhmm(ends));
    }
    said
}

pub(super) fn now_text(tables: &player::Tables) -> (String, String) {
    let Some(programme) = &tables.now else {
        return (String::new(), String::new());
    };
    let when = programme
        .start
        .map(|start| zwcore::guide::airtime(start, programme.ends()))
        .unwrap_or_default();
    (when, ui::text::badges::bracketed(&programme.title))
}

/// 映像を出す大きさ（1440×1080 の 16:9 は 1920×1080）。`aspect` が 0 ならコーデック上の大きさ。
/// フィールド構造ピクチャーだけのストリームは縦を倍に。
pub fn shown_size(format: &mediadecoder::es::Format) -> (u32, u32) {
    let height = if format.field_pictures {
        format.height * 2
    } else {
        format.height
    };
    let (num, den) = format.aspect;
    let width = if num == 0 || den == 0 {
        format.width
    } else {
        (f64::from(height) * f64::from(num) / f64::from(den)).round() as u32
    };
    (width.max(1), height.max(1))
}

/// TS は PMT のデータ符号化方式記述子（字幕の 0x0008 は数えない、ARIB STD-B10 第2部 6.2.20）。
/// MMT の字幕のアセットも MH-データ符号化方式記述子で記述されるので、字幕のストリームは数えない。
/// MMT は MH のアプリケーションサービス記述子が示す MH-AIT に、アプリケーションが載っているか。
/// 記述子はデータ放送の無い局の MPT にも載るので、それだけでは分からない。MH-AIT がまだ届いて
/// いなければ、有るとして扱う。
fn has_data_broadcast(program: &zwcore::filters::Program, applications: &[(u16, usize)]) -> bool {
    const CAPTION: u16 = 0x0008;
    if let Some(service) = &program.application {
        return applications
            .iter()
            .find(|(packet_id, _)| *packet_id == service.ait)
            .is_none_or(|(_, count)| *count > 0);
    }
    program.streams.iter().any(|stream| {
        stream.kind != zwcore::filters::Kind::Caption
            && stream.data.as_ref().is_some_and(|data| data.id != CAPTION)
    })
}

/// フリー関数なのは `player` と `guide` を借りたまま `status` を渡すため。
pub(super) fn collect(player: &Player, guide: &Guide, status: &mut Status, size: (u32, u32)) {
    status.picture = player.picture_size().map(|(width, height)| {
        let zoom = (size.0 as f32 / width as f32).min(size.1 as f32 / height as f32);
        (width, height, zoom)
    });
    let tables = player.tables();
    if let Some(watching) = tables.watching
        && let Some(service) = guide.service(watching)
        && !service.name.is_empty()
    {
        status.channel = Some(service.name.clone());
    }
    if let Some(choice) = tables.choice_at.and_then(|at| tables.choices.get(at)) {
        status.sound = Some(choice.label.clone());
    } else if let Some(sound) = &tables.sound {
        status.sound = Some(sound_text(
            sound,
            tables.dual_mono.then(|| player.dual_mono()),
        ));
    }
    status.data = tables.watching.and_then(|watching| {
        tables
            .programs
            .iter()
            .find(|program| program.number == watching.service)
            .map(|program| has_data_broadcast(program, &tables.applications))
    });
    status.counts = tables.counts;
    status.clock = clock_now();
    status.volume = player.volume();
    status.muted = player.muted();
    let recording = zwcore::record::state();
    status.recording = recording.is_some();
    status.recorded_for = recording.map_or(0, |state| state.started.elapsed().as_secs());
    status.fixed = player.passes_sound_through();
    status.emergency = tables
        .emergency
        .iter()
        .find(|warning| warning.started)
        .map(|warning| {
            tables
                .services
                .iter()
                .find(|service| service.service == warning.service_id)
                .map(|service| service.name.clone())
                .unwrap_or_else(|| format!("サービス {}", warning.service_id))
        });
}

pub(super) fn sound_text(stream: &Stream, dual_mono: Option<DualMono>) -> String {
    let mode_word = |mode: DualMono| match mode {
        DualMono::Main => "主",
        DualMono::Sub => "副",
        DualMono::Both => "主／副",
    };
    let Some(component) = &stream.component else {
        return match dual_mono {
            Some(mode) => mode_word(mode).to_owned(),
            None => format!("0x{:04x}", stream.id),
        };
    };
    let arrangement = match dual_mono {
        Some(mode) => mode_word(mode).to_owned(),
        None => component.arrangement(),
    };
    let called = component.with_assist(component.who());
    if called.is_empty() {
        arrangement
    } else {
        format!("{called} {arrangement}")
    }
}

pub(super) fn osd_of(
    player: &Player,
    guide: &Guide,
    status: &Status,
    said: Option<&str>,
    tuning: Option<&supply::Tuning>,
    asked: Option<zwcore::guide::Key>,
) -> ui::overlay::osd::Osd {
    let tables = player.tables();
    let clock = status.clock.get(..5).unwrap_or_default().to_owned();
    if let Some(tuning) = tuning {
        let service = guide.service(tuning.key);
        // EIT を待つと数秒空になるので、ためてある番組表と PC の時計から。
        let now = guide.at(tuning.key, guide.now_or_machine());
        return ui::overlay::osd::Osd {
            title: now
                .map(|programme| ui::text::badges::titled(&programme.title, &programme.detail))
                .unwrap_or_default(),
            clock,
            remote_key: service.and_then(|service| service.remote_key),
            badge: service.map_or_else(
                || ui::guide::epg::badge_of(&tuning.name),
                zwcore::guide::Service::badge,
            ),
            badge_colour: ui::guide::epg::badge_colour(&tuning.name),
            logo: guide.logo_of(tuning.key).cloned(),
            band: band_label(tuning.key, service.and_then(|service| service.remote_key)),
            channel: tuning.name.clone(),
            airtime: airtime_text(now),
            info: String::new(),
            said: said.map(str::to_owned),
        };
    }
    let key = tables.watching.or(asked);
    let service = key.and_then(|key| guide.service(key));
    let name = service
        .map(|service| service.name.clone())
        .or_else(|| status.channel.clone())
        .unwrap_or_default();
    let codec = tables.video.and_then(|kind| match kind {
        Kind::Video(mediadecoder::es::Codec::Mpeg2) => Some("MPEG-2"),
        Kind::Video(mediadecoder::es::Codec::H264) => Some("H.264"),
        Kind::Video(mediadecoder::es::Codec::Hevc) => Some("HEVC"),
        _ => None,
    });
    let picture = player
        .picture_format()
        .map(|format| (format.width, format.height, format.progressive));
    let sound = tables.sound.as_ref().map(|stream| {
        let said = stream
            .component
            .as_ref()
            .map(Described::arrangement)
            .unwrap_or_default();
        // TS は ADTS、MMT は LATM、MMT を TS に詰めたものは LOAS。
        let codec = codec_of(stream.kind, tables.he_aac);
        let label = tables
            .choice_at
            .and_then(|at| tables.choices.get(at))
            .map(|choice| choice.label.clone());
        match label {
            Some(label) => format!("{label} / {codec}"),
            None if said.is_empty() => codec,
            None => format!("{codec} {said}"),
        }
    });
    let now = tables.now.as_ref().or_else(|| {
        if player.is_recording() {
            return None;
        }
        key.and_then(|key| guide.at(key, guide.now_or_machine()))
    });
    ui::overlay::osd::Osd {
        title: now
            .map(|programme| ui::text::badges::titled(&programme.title, &programme.detail))
            .unwrap_or_default(),
        clock,
        remote_key: service.and_then(|service| service.remote_key),
        badge: service.map_or_else(
            || ui::guide::epg::badge_of(&name),
            zwcore::guide::Service::badge,
        ),
        badge_colour: ui::guide::epg::badge_colour(&name),
        logo: key.and_then(|key| guide.logo_of(key).cloned()),
        band: key
            .map(|key| band_label(key, service.and_then(|service| service.remote_key)))
            .unwrap_or_default(),
        channel: name,
        airtime: airtime_text(now),
        info: ui::overlay::osd::info_line(
            if player.audio_only() {
                Some("ラジオ")
            } else {
                codec
            },
            picture,
            sound.as_deref(),
            tables.sound_rate.or_else(|| player.heard_rate()),
        ),
        said: said.map(str::to_owned),
    }
}

impl Stage {
    fn menu_filling(&mut self) -> Arc<crate::window::menu::Filling> {
        self.refresh_favourites();
        let tables = self.player.tables();
        let mark = (
            tables_mark(tables),
            self.guide_version,
            self.receivable
                .as_ref()
                .map(|keys| Arc::as_ptr(keys) as usize),
            tables.watching,
            (self.asked_key, self.plugin_menus_serial),
            tables.sound_at,
            tables.picture_at,
            tables.dual_mono,
            self.player.dual_mono(),
            self.player.volume(),
            (self.player.muted(), self.player.playback()),
            (
                self.profile_shown.clone(),
                (
                    self.favourites.serial,
                    self.recent_serial,
                    self.guide.now_or_machine() / 60,
                ),
            ),
        );
        if self.menu_mark.as_ref() == Some(&mark) {
            return Arc::clone(&self.menu_filling);
        }
        self.menu_mark = Some(mark);
        let channels = self
            .bands()
            .into_iter()
            .map(|band| {
                let keys = self.keys_of(band);
                let listed = keys
                    .into_iter()
                    .filter_map(|key| {
                        let service = self.guide.service(key)?;
                        Some((
                            key,
                            service.name.clone(),
                            service.remote_key,
                            self.guide.logo_of(key).cloned(),
                        ))
                    })
                    .collect();
                (band, listed)
            })
            .collect();
        let tables = self.player.tables();
        // 並べるのは視聴の対象、データサービス（0xC0）、部分受信（PMT 0x1FC8〜0x1FCF）、
        // SDT がまだなら映像か音声を持つもの。
        let network = tables
            .identity
            .map(|identity| identity.network)
            .or_else(|| {
                let live = self.live.as_ref()?;
                live.channels
                    .iter()
                    .find(|channel| live.supply.carries(channel))
                    .and_then(|channel| channel.key)
                    .map(|key| key.network)
            });
        let services = match network {
            None => Vec::new(),
            Some(network) => tables
                .programs
                .iter()
                .enumerate()
                .filter_map(|(nth, program)| {
                    let named = tables
                        .services
                        .iter()
                        .find(|service| service.service == program.number);
                    let played = program.streams.iter().any(|stream| {
                        matches!(
                            stream.kind,
                            zwcore::filters::Kind::Video(_) | zwcore::filters::Kind::Audio(_)
                        )
                    });
                    let listed = match named {
                        Some(service) => {
                            service.watchable() || service.kind == 0xc0 || program.one_seg()
                        }
                        None => played || program.one_seg(),
                    };
                    if !listed {
                        return None;
                    }
                    let mut name = named
                        .map(|service| service.name.clone())
                        .filter(|name| !name.is_empty())
                        // OS のメニューの幅は開いたときのままなので、仮の名前で幅を取っておく。
                        .unwrap_or_else(|| format!("サービス{}　　　　　　　　", nth + 1));
                    if program.one_seg() && !name.contains("ワンセグ") {
                        name.push_str("（ワンセグ）");
                    }
                    let key = zwcore::guide::Key {
                        network,
                        service: program.number,
                    };
                    Some((key, name, self.guide.logo_of(key).cloned()))
                })
                .collect(),
        };
        let watching = if self.player_mode {
            tables.watching
        } else {
            self.asked_key.or(tables.watching)
        };
        self.menu_filling = Arc::new(crate::window::menu::Filling {
            favourites: self.favourite_rows(),
            recent: self.recent_rows(),
            player_mode: self.player_mode,
            playback_off: !self.player.playback(),
            channels,
            watching,
            services,
            band: self.band(),
            sounds: tables
                .choices
                .iter()
                .map(|choice| choice.label.clone())
                .collect(),
            sound_at: tables.choice_at,
            pictures: tables.pictures.clone(),
            picture_at: tables.picture_at,
            captions: Vec::new(),
            caption_at: None,
            volume: self.player.volume(),
            muted: self.player.muted(),
            profiles: self.profiles.clone(),
            profile: self.profile_shown.clone(),
            full_screen: false,
            on_top: false,
            menu_bar: true,
            status_bar: true,
            osd: true,
            capture_layers: Vec::new(),
            capture_size: String::new(),
            aspect: String::new(),
            plugins: plugin_menus(&self.plugin_menus),
        });
        Arc::clone(&self.menu_filling)
    }
}

fn service_of(service: &zwcore::guide::Service) -> ui::panes::stream::Service {
    ui::panes::stream::Service {
        id: service.service,
        name: service.name.clone(),
        kind: service.kind,
        network: service.network,
        transport: service.transport,
        scrambled: service.scrambled,
        lines: Vec::new(),
    }
}

fn neighbour_of(service: &zwcore::si::ServiceInfo) -> ui::panes::stream::Service {
    ui::panes::stream::Service {
        id: service.service,
        name: service.name.clone(),
        kind: service.kind,
        network: service.network,
        transport: service.transport,
        scrambled: service.scrambled,
        lines: zwcore::si::service_lines(service),
    }
}

pub(super) fn stream_info(tables: &player::Tables) -> ui::panes::stream::StreamInfo {
    let carried: Vec<(u16, u16, &str)> = tables
        .carried
        .iter()
        .map(|(pid, service, declared)| (*pid, *service, declared.as_str()))
        .collect();
    ui::panes::stream::StreamInfo {
        identity: tables.identity,
        services: tables.services.iter().map(service_of).collect(),
        neighbours: tables.neighbours.iter().map(neighbour_of).collect(),
        programs: tables.programs.clone(),
        frame_header: tables.frame_header.clone(),
        networks: tables.networks.clone(),
        broadcasters: tables.broadcasters.clone(),
        emergency: tables.emergency.clone(),
        emm: tables.emm.clone(),
        downloads: tables.downloads.clone(),
        logos: tables.logos.clone(),
        partial: tables.partial.clone(),
        packages: tables.packages.clone(),
        layouts: tables.layouts.clone(),
        counts: tables.counts,
        pids: tables
            .pids
            .iter()
            .map(|(pid, counted)| ui::panes::stream::Row {
                pid: *pid,
                counted: *counted,
                what: ui::panes::stream::what_carries(tables.container, *pid, &carried),
            })
            .collect(),
    }
}

fn codec_of(kind: Kind, he_aac: bool) -> String {
    let aac = if he_aac { "HE-AAC" } else { "AAC-LC" };
    match kind {
        Kind::Audio(zwcore::filters::Audio::Adts) => format!("{aac} ADTS"),
        Kind::Audio(zwcore::filters::Audio::Latm | zwcore::filters::Audio::Loas) => {
            format!("{aac} LOAS/LATM")
        }
        Kind::Audio(zwcore::filters::Audio::Mpeg) => String::from("MPEG"),
        _ => aac.to_owned(),
    }
}

pub(super) fn sound_codec(tables: &player::Tables) -> Option<String> {
    tables
        .sound
        .as_ref()
        .map(|stream| codec_of(stream.kind, tables.he_aac))
}

/// 設定の `general.title` の `{channel}`・`{event}` を置き換える。空の所の前後の空白は詰める。
pub(super) fn titled(pattern: &str, channel: &str, event: &str) -> String {
    pattern
        .replace("{channel}", channel)
        .replace("{event}", event)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// PC の時計で、放送の時刻ではない。
pub(super) fn clock_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let Ok(since) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return String::from("--:--");
    };
    let moment = since.as_secs() as i64 + i64::from(zwcore::guide::shown_offset());
    let (_, _, _, hour, minute, _) = zwcore::guide::civil(moment);
    format!("{hour:02}:{minute:02}")
}

/// テーブルは 250 ms ごとに届くので、届いた数ではビルドし直さない。
fn tables_mark(tables: &zwcore::player::Tables) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tables
        .identity
        .map(|identity| identity.network)
        .hash(&mut hasher);
    for program in &tables.programs {
        program.number.hash(&mut hasher);
        program.one_seg().hash(&mut hasher);
        program.streams.len().hash(&mut hasher);
    }
    for service in &tables.services {
        (service.service, &service.name, service.kind).hash(&mut hasher);
    }
    for choice in &tables.choices {
        choice.label.hash(&mut hasher);
    }
    tables.pictures.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_empty_entry_means_no_data_broadcast() {
        let program = zwcore::filters::Program {
            number: 0x0101,
            map: 0,
            streams: Vec::new(),
            pcr: 0,
            ecm: Vec::new(),
            application: Some(isdb::descriptors::ApplicationService {
                resolution: 0,
                ait: 0x8000,
                data: None,
                emts: Vec::new(),
            }),
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        };
        assert!(super::has_data_broadcast(&program, &[]));
        assert!(super::has_data_broadcast(&program, &[(0x8000, 1)]));
        assert!(!super::has_data_broadcast(&program, &[(0x8000, 0)]));
        assert!(super::has_data_broadcast(&program, &[(0x8001, 0)]));
    }

    #[test]
    fn a_caption_asset_is_not_a_data_broadcast() {
        let caption = zwcore::filters::Stream {
            id: 0xf130,
            kind: zwcore::filters::Kind::Caption,
            framing: mediadecoder::es::Framing::Lengths,
            declared: String::from("stpp"),
            tag: Some(0x0030),
            component: None,
            name: None,
            hierarchy: None,
            data: Some(isdb::bxml::DataComponent {
                id: 0x0020,
                additional: Vec::new(),
            }),
            decode: None,
        };
        let program = zwcore::filters::Program {
            number: 0x00a1,
            map: 0,
            streams: vec![caption],
            pcr: 0,
            ecm: Vec::new(),
            application: None,
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        };
        assert!(!super::has_data_broadcast(&program, &[]));
    }
}
