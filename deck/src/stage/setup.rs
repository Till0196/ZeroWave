// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 再生スレッドを作り、動かし始めるところ。レンダリング先（wgpu）を開き、開き直す。

use super::*;

impl Stage {
    pub fn new(boot: Boot) -> Self {
        let mut notices: std::collections::VecDeque<String> = crate::config::take_notices().into();
        let notice = notices.pop_front();
        let telling_since = notice.as_ref().map(|_| Instant::now());
        Self {
            choices: boot.choices,
            painter: boot.painter,
            player: boot.player,
            mixer: None,
            drawn: None,
            #[cfg(all(unix, not(target_os = "macos")))]
            layer: Default::default(),
            fresh: false,
            shown: shown::Shown::default(),
            size: boot.size,
            surface: surface::Surface::new(),
            left: 0,
            top: 0,
            wide: 0,
            tall: 0,
            refresh: boot.refresh,
            panes: boot.panes,
            requests: boot.requests,
            title: None,
            proxy: boot.proxy,
            listed: Some(boot.listed),
            instance: boot.instance,
            handed: boot.surface,
            #[cfg(target_os = "windows")]
            overlay: boot.overlay,
            #[cfg(target_os = "windows")]
            overlay_to: None,
            events: boot.events,
            out: boot.out,
            presented: boot.presented,
            broadcast: boot.broadcast,
            told_broadcast: None,
            told_clock: None,
            told_scene: None,
            status: Status::default(),
            wants_guide: false,
            wants_info: false,
            wants_decoder: false,
            asked_decoder: Instant::now(),
            mini: false,
            mini_rows: super::MINI_HOURS,
            mini_shift: 0,
            told: None,
            done: false,
            guide: Arc::new(Guide::new()),
            guide_to: boot.guide_to,
            guide_back: boot.guide_back,
            asked_guide: Instant::now(),
            asked_tables: Instant::now(),
            asked_info: Instant::now(),
            live: boot.live,
            player_mode: false,
            leaving: None,
            profile_pending: false,
            live_key: None,
            ended: false,
            polled: Instant::now(),
            osd: ui::overlay::osd::State::default(),
            osd_auto: ui::overlay::osd::Auto::default(),
            osd_shown: ui::overlay::osd::Osd::default(),
            scene_at: Instant::now(),
            scened: None,
            osded: None,
            seen: None,
            plugin_ui: crate::plugin_ui::Instances::default(),
            plugin_shared: crate::plugin_ui::Shared::default(),
            plugin_shared_told: Arc::default(),
            plugin_menus: Vec::new(),
            plugin_menus_serial: 0,
            plugin_checked: Instant::now(),
            plugin_late: std::collections::VecDeque::new(),
            noticed: super::Noticed::default(),
            filter_latency: 0.0,
            formatted: None,
            seeking: false,
            seek_settling: false,
            band: None,
            asked_key: None,
            listing: Arc::new(ui::guide::epg::Listing::default()),
            listed_mark: None,
            said: None,
            toast: notice.map(|text| Toast::of(&text, 1, 0)),
            toasts: 0,
            toast_behind: None,
            telling_since,
            notices,
            osd_version: 0,
            volume_moved: None,
            volume_shown: None,
            volume_hold: 3.0,
            capture_mixer: None,
            capture_carry: None,
            #[cfg(all(unix, not(target_os = "macos")))]
            capture_layer: crate::draw::layer::GlesLayer::synchronous(),
            #[cfg(all(unix, not(target_os = "macos")))]
            capture_reading: Vec::new(),
            capture_convert: None,
            capture_warm: None,
            capture_pending: Vec::new(),
            frame: crate::config::load_plain().video.frame,
            errors_base: zwcore::filters::Counts::default(),
            errors_reset_due: None,
            reset_errors_on_tune: crate::config::load_plain().playback.reset_errors_on_tune,
            title_pattern: crate::config::load_plain().general.title,
            audio_latency: crate::config::load_plain().audio.latency,
            audio_delay: 0,
            stepping: super::stepping::Stepping::default(),
            reset_audio_delay: crate::config::load_plain().audio.reset_delay_on_change,
            inset_on: false,
            history: channels::History::default(),
            favourites: channels::Loaded::default(),
            recent_serial: 0,
            capture_said: std::sync::mpsc::channel(),
            confirmed: std::sync::mpsc::channel(),
            asking: Arc::default(),
            switching: None,
            started: None,
            starting_since: None,
            hidden: Vec::new(),
            plugins_seen: None,
            oneseg: None,
            reception: reception::Judge::default(),
            said_unreadable: None,
            yielded: None,
            roster_seeded: false,
            buffering_told: None,
            catalog: Vec::new(),
            catalog_back: None,
            catalog_due: true,
            tried: Vec::new(),
            tried_ways: Vec::new(),
            tuning_way: None,
            verifying: None,
            watching_nit: None,
            unconfirmed: Vec::new(),
            switching_to: None,
            profiles: Vec::new(),
            profile_shown: String::new(),
            sound_asked: None,
            picture_asked: None,
            guide_version: 0,
            logos_written: None,
            menu_filling: Arc::new(crate::window::menu::Filling::default()),
            menu_mark: None,
            receivable: None,
            recording_guide: false,
            no_sleep: false,
            awake: crate::awake::Awake::default(),
            awake_at: Instant::now(),
            awake_read: 0,
            last: Instant::now(),
            painted: Instant::now(),
            spent: Outside::default(),
            gpu_late: GpuLate::default(),
            recovery: recover::Recovery::from_env(),
        }
    }

    pub fn play(mut self) {
        self.start();
        while !self.done {
            if !self.serve() {
                break;
            }
            if self.painter.is_none() {
                self.retry_gpu();
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            if self.watch_lost() {
                continue;
            }
            // wgpu はデバイスを失った後の書き込み（`Queue::write_buffer` など）で panic する。失ったと分かるのが
            // そのときのこともあるので、失っていたときの panic だけ止め、次の `watch_lost` で開き直す。
            let device = self.device();
            if crate::draw::unless_lost_on(device.as_ref(), || self.turn()).is_none() {
                continue;
            }
            // パススルーできない音声は黙って PCM に落とさずに終わる。
            if let Some(why) = self.player.fault() {
                tracing::warn!("{why}");
                crate::leave(2);
            }
        }
        // ソースを先に切る（ネットから読む `read` は相手が応答しないと待ち続け、抜けられない）。
        if let Some(live) = self.live.as_mut() {
            live.supply.close();
        }
        self.panes.close();
        let _ = self.out.send(FromStage::Done);
        let _ = self.proxy.send_event(Wake);
        self.player.report();
    }

    pub(super) fn start(&mut self) {
        let file = crate::config::load_plain();
        self.plugins_seen = Some((file.data.plugins.clone(), file.data.disabled.clone()));
        self.apply_channel_prefs(&file);
        self.volume_hold = file.osd.fade.max(1) as f32;
        self.osd_auto = ui::overlay::osd::Auto::of(&file.osd);
        self.no_sleep = file.general.no_sleep;
        let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.open_painter()));
        let opened = opened.unwrap_or_else(|panic| {
            crate::draw::settle_panic(panic, self.device().as_ref());
            None
        });
        let Some(beside) = opened else {
            return;
        };
        // 別ウィンドウのスレッドはデバイスが開いてから起動する。GLES 2.0 のバックエンドでは別ウィンドウはそれぞれ EGL でレンダリングする。
        if let Some(listed) = self.listed.take() {
            self.panes.hand(
                beside,
                self.requests.clone(),
                self.proxy.clone(),
                self.guide_to.clone(),
                listed,
            );
            if crate::config::first_run() {
                self.panes
                    .say(ToPanes::Open(crate::window::sub::Which::Settings));
            }
        }
        if let Some(live) = self.live.as_mut() {
            live.supply.hand();
        }
        if let Some(starting) = self.live.as_mut().and_then(|live| live.starting.take()) {
            self.start_source(starting);
        }
        // Only now: which decoder a coding gets depends on what was just opened.
        self.player.start();
    }

    /// wgpu で開く。返るのは別ウィンドウ用のデバイス。開けなければ終わることにして `None`。
    fn open_painter(&mut self) -> Option<Option<Gpu>> {
        if self.painter.is_none() {
            let opened = match self.recovery.injected_failure() {
                Some(error) => Err(error),
                None => self.open_wgpu(),
            };
            match opened {
                Ok(painter) => self.painter = Some(painter),
                Err(error) if self.recovery.active => {
                    self.reopen_failed(&error);
                    return None;
                }
                Err(error) => {
                    crate::log::fatal(&format!(
                        "GPU を使えないため、ZeroWave を起動できません。\n\
                         GPU のドライバーが正しくインストールされているか確認してください。\n\n\
                         詳細: {error}"
                    ));
                    self.quit();
                    return None;
                }
            }
        }
        let Some(painter) = self.painter.as_ref() else {
            self.quit();
            return None;
        };
        tracing::info!("レンダラー {}", painter.name());
        self.recovery.opened();
        // サーフェスはデバイスより先に用意する（重ね合わせのパイプラインはサーフェスの形式でビルドされる）。
        let size = self.size;
        self.size = (0, 0);
        self.configure(size.0, size.1);
        let Some(Painter::Wgpu { gpu, .. }) = self.painter.as_ref() else {
            return Some(None);
        };
        self.mixer = Some(overlay::Mixer::new(
            &gpu.device,
            self.surface.target,
            self.surface.space,
            self.surface.sdr_white,
        ));
        self.capture_warm = capture::warm(
            gpu.device.clone(),
            self.surface.target,
            self.surface.space,
            self.surface.sdr_white,
        );
        #[cfg(target_os = "windows")]
        if let Some(overlay) = self.overlay.take() {
            self.overlay_to = crate::draw::composition::spawn(
                overlay,
                gpu.adapter.clone(),
                gpu.device.clone(),
                gpu.queue.clone(),
            );
        }
        // キューを分ければ別ウィンドウの処理が放送の映像の提示の前に入らない。分けられない環境では共有する。
        let beside = match gpu.beside() {
            Ok(beside) => beside,
            Err(error) => {
                tracing::warn!("別ウィンドウ用のデバイスを開けない（映像と共有する）: {error}");
                Gpu {
                    instance: gpu.instance.clone(),
                    adapter: gpu.adapter.clone(),
                    device: gpu.device.clone(),
                    queue: gpu.queue.clone(),
                }
            }
        };
        Some(Some(beside))
    }

    /// バックエンドを替える：デバイスとサーフェスを捨て、メインスレッドにインスタンスを作り直してもらう。
    pub(super) fn rebuild_gpu(&mut self, backends: Option<wgpu::Backends>) {
        self.drop_gpu();
        let _ = self.requests.send(Request::Reinstance(backends));
        let _ = self.proxy.send_event(Wake);
    }

    /// 今のデバイスで作った物と、デバイスへの参照を全部手放す（D3D12 では同じアダプターのデバイスが一つでも
    /// 生きている間は、作り直しても失ったそのデバイスが返る）。
    pub(super) fn drop_gpu(&mut self) {
        self.panes.say(ToPanes::DropGpu);
        #[cfg(windows)]
        zwcore::decode::d3d12::dx::forget();
        self.mixer = None;
        self.capture_mixer = None;
        self.capture_convert = None;
        self.capture_warm = None;
        // 前のデバイスのリソースを新しいデバイスで使わない。
        self.player.rebuild_screen();
        self.painter = None;
        self.handed = None;
        self.instance = None;
        #[cfg(target_os = "windows")]
        {
            self.overlay_to = None;
        }
    }

    pub(super) fn reinstanced(
        &mut self,
        instance: wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
    ) {
        // 開いてすぐまた失うことがある。そのときは次の `watch_lost` が数えて開き直すかやめる。
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.reinstanced_now(instance, surface)
        }));
        if let Err(panic) = done {
            crate::draw::settle_panic(panic, self.device().as_ref());
        }
    }

    /// 今レンダリングに使っている wgpu のデバイス。
    pub(super) fn device(&self) -> Option<wgpu::Device> {
        match self.painter.as_ref() {
            Some(Painter::Wgpu { gpu, .. }) => Some(gpu.device.clone()),
            _ => None,
        }
    }

    fn reinstanced_now(
        &mut self,
        instance: wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
    ) {
        self.instance = Some(instance);
        self.handed = surface;
        // メインスレッドは egui を作り直すので、次に届く物にテクスチャが全部入っている。
        self.drawn = None;
        let Some(beside) = self.open_painter() else {
            return;
        };
        if let Some(beside) = beside {
            self.panes.say(ToPanes::Gpu(beside));
        }
        self.reopen_current();
        if self.reopened() {
            self.toast("GPU を開き直しました。");
        } else {
            self.toast("映像レンダラを切り替えました。");
        }
    }

    fn open_wgpu(&mut self) -> Result<Painter, String> {
        let instance = self
            .instance
            .clone()
            .ok_or_else(|| String::from("wgpu のインスタンスが無い"))?;
        let surface = self.handed.as_ref().ok_or_else(|| {
            String::from("レンダリング用のサーフェスを作れなかった（メインスレッドからの通知）")
        })?;
        // The device is opened on the instance the surface came from.
        let mut complaint = None;
        let gpu = match self.choices.gpu.as_deref() {
            Some(name) => match Gpu::open_named(instance.clone(), Some(surface), Some(name)) {
                Ok(gpu) => gpu,
                Err(error) => {
                    complaint = Some(said_with(
                        "指定された GPU が見つかりません。\n自動選択で起動しています。",
                        &error,
                    ));
                    Gpu::open_on(instance, Some(surface)).map_err(|error| format!("{error}"))?
                }
            },
            None => Gpu::open_on(instance, Some(surface)).map_err(|error| format!("{error}"))?,
        };
        if let Some(complaint) = complaint {
            self.toast(&complaint);
        }
        Ok(Painter::Wgpu {
            gpu,
            target: wgpu::TextureFormat::Bgra8UnormSrgb,
            space: Space::Srgb,
            white: zwcore::output::video::colour::SDR_WHITE,
            peak: zwcore::output::video::colour::HLG_PEAK,
        })
    }
}
