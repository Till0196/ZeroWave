// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 回のループの流れ。映像の位置を決め、処理を進め、レンダリングし、ウィンドウへ伝える。

use super::*;

impl Stage {
    /// ウィンドウの大きさが変わった直後のずれに合わせて切り詰める。
    fn video_area(&self, size: (u32, u32)) -> Onto {
        if self.tall == 0 || self.wide == 0 {
            return Onto::all(size);
        }
        let left = self.left.min(size.0);
        let top = self.top.min(size.1);
        Onto {
            left,
            top,
            // バーの高さより低いウィンドウでも、サーフェスの大きさは 0 にできない。
            width: self.wide.min(size.0.saturating_sub(left)).max(1),
            height: self.tall.min(size.1.saturating_sub(top)).max(1),
        }
    }

    pub(super) fn area(&self) -> (u32, u32) {
        let onto = self.video_area(self.size);
        (onto.width, onto.height)
    }

    pub(super) fn split(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.overlay_to.is_some()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    pub(super) fn turn(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last);
        self.last = now;
        let ticks = (elapsed.as_secs_f64() * HZ as f64) as i64;

        let took = self
            .player
            .feed(self.painter.as_ref().expect("レンダリング先"));
        self.save_volume_when_quiet();
        self.next_notice();
        self.take_started();
        self.take_switched();
        self.take_catalog();
        self.take_data_tune();
        self.take_fetched();
        self.take_merged();
        self.check_verifying();
        self.seed_roster();
        self.tell_recording();
        self.tell_receivable();
        self.watch_oneseg();
        self.refresh_listing();
        self.tell_panes();
        self.settle_format();
        // 映像が来ていなくても取る（画面は選局より先に出ている）。隠れている・掴まれている間は取らない。
        let tick = if self.surface.covered() {
            self.player.idle(ticks)
        } else {
            self.paint(ticks)
        };
        let settled = self.surface.settle();
        let wanted = self.hdr_wanted();
        if tick.reconfigure || settled || self.surface.due(wanted) {
            let (width, height) = self.size;
            self.size = (0, 0);
            self.configure(width, height);
        }
        // 音のスレッドはこれを待って時計を合わせる。
        if self.player.has_picture_on_gpu() && !self.surface.due(wanted) {
            self.player.screen_ready();
        }
        if !tick.more {
            if !self.ended {
                self.ended = true;
                if self.player.is_recording() {
                    self.player.set_paused(true);
                    tracing::info!("再生が終了した");
                } else {
                    self.tell("ストリームが終わった");
                }
            }
        } else {
            self.ended = false;
        }
        self.compose(tick.did);
        self.spent.say();
        // Asking again at once spins a core and takes the sound callback's lock; wait a moment.
        // Also once the stream is over: the GLES 2.0 path has no surface to wait on.
        if !took && !tick.did {
            let nap = if self.surface.hidden() || !tick.more {
                15
            } else {
                1
            };
            std::thread::sleep(Duration::from_millis(nap));
        }
        if self.choices.present == crate::choice::Present::Immediate {
            let period =
                Duration::from_secs_f64(1000.0 / f64::from(self.refresh.clamp(24_000, 480_000)));
            if let Some(left) = period.checked_sub(self.painted.elapsed()) {
                std::thread::sleep(left);
            }
        }
    }

    fn paint(&mut self, ticks: i64) -> player::Tick {
        let idle = player::Tick {
            more: true,
            reconfigure: false,
            did: false,
        };
        #[cfg(unix)]
        let hdr = self.hdr_wanted();
        let (Some(Painter::Wgpu { gpu, .. }), Some(surface)) =
            (self.painter.as_ref(), self.handed.as_ref())
        else {
            #[cfg(unix)]
            {
                #[cfg(not(target_os = "macos"))]
                let layer = self
                    .layer
                    .has()
                    .then_some(&self.layer as &dyn zwcore::output::present::Layer);
                #[cfg(target_os = "macos")]
                let layer = None;
                let area = (self.wide != 0 && self.tall != 0).then(|| {
                    let onto = self.video_area(self.size);
                    (onto.left, onto.top, onto.width, onto.height)
                });
                let black = self.backdrop(false) == scene::Backdrop::Black;
                let canvas = Canvas::Gles {
                    layer,
                    area,
                    black,
                    hdr,
                };
                // 映像を出す番でないフレームも、画面が変わっていれば最後の映像の上に重ね直して出す。
                let tick = if self.player.wants_turn(ticks) {
                    self.player.tick(&canvas, 0)
                } else {
                    self.player.redraw(&canvas);
                    idle
                };
                if !self.player.has_screen()
                    && let Some(painter) = self.painter.as_ref()
                    && let Err(error) = painter.paint_blank(layer)
                {
                    tracing::warn!("映像なしで画面をレンダリングできない: {error:?}");
                }
                return tick;
            }
            #[cfg(not(unix))]
            return idle;
        };
        // 垂直同期ごとに 1 フレーム、最新の映像と画面を合成する（映像の時刻で取ると画面の更新が映像のフレームレートに引きずられる）。
        // 取得して提示しないフレームは作らない（DXGI は三つ無駄にすると 1 秒待たせる）。
        let _ = self.player.wants_turn(ticks);
        let began = Instant::now();
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // 作り直してはいけない（ディスプレイがスリープしている間、作り直し続ける）。`Occluded` はウィンドウの状態として記録しない
            // （macOS の wgpu は遅れて更新するので、記録すると戻れなくなる）。
            hidden @ (wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Timeout) => {
                if matches!(hidden, wgpu::CurrentSurfaceTexture::Occluded) {
                    if self.surface.unseen_now() {
                        tracing::info!("サーフェスが Occluded を返した");
                    }
                    self.player.set_hidden(true);
                } else {
                    self.player.surface_timed_out();
                }
                return self.player.idle(ticks);
            }
            other => {
                let lost = matches!(other, wgpu::CurrentSurfaceTexture::Lost);
                if self.recovery.surface_failed(lost) {
                    tracing::warn!("サーフェスを取得できない: {other:?}");
                }
                let (width, height) = self.size;
                self.size = (0, 0);
                self.configure(width, height);
                return idle;
            }
        };
        self.recovery.surface_ok();
        let got = Instant::now();
        self.spent.acquire += got - began;
        self.spent.acquire_most = self.spent.acquire_most.max(got - began);
        if self.surface.seen() {
            tracing::info!("サーフェスを再び取得できた");
            self.player.set_hidden(self.surface.hidden());
        }
        // コンポジターがこのウィンドウを保持していた。測ったものは捨てる。
        if got - began >= STALLED {
            self.shown.forget();
        }
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let size = (frame.texture.width(), frame.texture.height());
        let onto = self.video_area(size);
        let tick = self.player.tick(&Canvas::Wgpu { view: &view, onto }, 0);
        if self.backdrop(tick.did) == scene::Backdrop::Black {
            blackout(&gpu.device, &gpu.queue, &view);
        }
        // 待たない（画面が 1 フレーム遅れても、映像は待たない）。
        if !self.split()
            && let (Some(mixer), Some(drawn)) = (self.mixer.as_mut(), self.drawn.as_mut())
        {
            mixer.draw(&gpu.device, &gpu.queue, &frame.texture, size, drawn);
        }
        if !self.split() {
            self.fresh = false;
        }
        let painted = Instant::now();
        let whole = self.split() || self.drawn.as_ref().is_none_or(|drawn| drawn.size == size);
        // 取ったサーフェスは必ず出す。
        gpu.queue.present(frame);
        self.presented.say(size, whole);
        let presented = Instant::now();
        self.spent.present += presented - painted;
        self.spent.present_most = self.spent.present_most.max(presented - painted);
        self.spent.turns += 1;
        // 映像をレンダリングしたフレームだけ（画面だけのフレームを数えると見積もりが揺れて音声の待たせ方が変わる）。
        if tick.did
            && let Some(ms) = self.shown.presented(surface)
        {
            self.player.set_video_latency(ms);
        }
        self.painted = painted;
        self.watch_gpu(got - began, tick.did);
        tick
    }

    fn tell_panes(&mut self) {
        // 選局の最中も種別を落とさない（`watching` は数秒空で、ウィンドウが地デジへ戻ってしまう）。
        let watching = self
            .player
            .tables()
            .watching
            .or(self.asked_key)
            .or(self.told.flatten());
        if self.told != Some(watching) {
            self.told = Some(watching);
            self.panes.say(ToPanes::Watching(watching));
        }
        let tables = self.player.tables();
        // 番組表の時刻は壁時計をゾーン無しで秒にしたもの。データ放送へは本当の UNIX 秒で渡す。
        let unix = |moment: i64| moment - i64::from(zwcore::guide::JST_OFFSET);
        let broadcast = zwsdk::layer::Broadcast {
            network: tables.identity.map_or(0, |i| i.network),
            transport: tables.identity.map_or(0, |i| i.transport),
            service: watching.map_or(0, |key| key.service),
            event: tables.now.as_ref().map_or(0, |now| now.event),
            start: tables.now.as_ref().and_then(|now| now.start).map(unix),
            duration: tables.now.as_ref().and_then(|now| now.seconds),
            name: tables
                .now
                .as_ref()
                .map_or(String::new(), |now| now.title.clone()),
            next: tables.next.as_ref().map(|next| {
                (
                    next.event,
                    next.start.map(unix),
                    next.seconds,
                    next.title.clone(),
                )
            }),
            audio: tables.sound.as_ref().and_then(|sound| sound.tag),
            captions: tables.carried.iter().any(|(_, program, kind)| {
                watching.is_some_and(|key| key.service == *program)
                    && (kind == "stpp" || kind == "06")
            }),
        };
        // 送れなかったものを送信済みとして記録すると、データ放送が放送の識別情報を受け取らないまま起動しない。
        if self.told_broadcast.as_ref() != Some(&broadcast)
            && self
                .broadcast
                .try_send(ToBroadcast::Broadcast {
                    broadcast: broadcast.clone(),
                    tune: zwcore::data::RETUNES.load(std::sync::atomic::Ordering::SeqCst),
                })
                .is_ok()
        {
            self.told_broadcast = Some(broadcast);
        }
        if let Some(clock) = tables.clock
            && self.told_clock != Some(clock)
        {
            self.told_clock = Some(clock);
            let _ = self
                .broadcast
                // 放送から入った再生モードでも `live` は残っている。
                .try_send(ToBroadcast::Clock(unix(clock), self.player.is_recording()));
            let _ = self.guide_to.send(ToGuide::Clock(Some(clock)));
        }
        if self.wants_info && self.asked_info.elapsed() >= TABLES_EVERY {
            self.asked_info = Instant::now();
            let info = stream_info(self.player.tables());
            self.panes.say(ToPanes::Info(Box::new(info)));
        }
        if self.wants_decoder && self.asked_decoder.elapsed() >= TABLES_EVERY {
            self.tell_decoding();
        }
    }
}

impl Stage {
    fn watch_gpu(&mut self, acquire: Duration, drew: bool) {
        let period =
            Duration::from_secs_f64(1000.0 / f64::from(self.refresh.clamp(24_000, 480_000)))
                .max(OUTPUT_LEAST);
        let settled = self.player.settled();
        let late = &mut self.gpu_late;
        if !settled {
            *late = GpuLate::default();
            return;
        }
        if late.warned {
            return;
        }
        if drew && acquire < STALLED {
            late.turns += 1;
            if acquire > period * 3 / 2 {
                late.late += 1;
            }
        }
        if late.since.elapsed() < GPU_LATE_OVER {
            return;
        }
        let (turns, count) = (late.turns, late.late);
        let behind = count >= GPU_LATE_LEAST && count * 2 <= turns;
        let streak = if behind { late.streak + 1 } else { 0 };
        *late = GpuLate {
            streak,
            ..GpuLate::default()
        };
        if streak < GPU_LATE_STREAK {
            return;
        }
        self.gpu_late.warned = true;
        tracing::info!(
            "GPU のレンダリングが間に合わない（{GPU_LATE_STREAK} 秒続けて、最後の 1 秒で {count} / {turns} フレーム）"
        );
        self.toast(
            "GPU のレンダリングが間に合っていません。\n拡大・縮小やデインターレースを軽い方式にすると改善することがあります。",
        );
    }
}
