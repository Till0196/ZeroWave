// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表の一覧を要求し直すことと、別ウィンドウのレンダリング。

use super::*;

impl Screen {
    /// 番組表に出すものを、必要なら要求し直す（検索結果も一覧に載るので、検索のウィンドウだけでも要求する）。
    pub(super) fn refresh(&mut self) {
        if !self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::Guide | Which::Search))
        {
            return;
        }
        let asked = Ask {
            band: self.band(),
            from: self.from,
            hours: HOURS,
            watching: self.watching,
        };
        if self.asked != Some(asked) {
            // 番組表の先頭が変わる要求は、一覧が届くまでスクロールを待つ。
            if self
                .asked
                .is_none_or(|was| was.from != asked.from || was.band != asked.band)
            {
                self.waiting = Some(Instant::now());
            }
            self.asked = Some(asked);
            let _ = self.guide_to.send(ToGuide::List(asked));
        }
        // たまっていたら最後の一つだけ。途中のものは後の追加分に含まれる。
        if let Some(listing) = self.listed.try_iter().last() {
            self.listing = *listing;
            self.waiting = None;
            for sub in self.subs.values_mut() {
                sub.dirty = true;
            }
        }
        self.listing.now = Some(self.guide.now_or_machine());
        self.listing.jump = if self.waiting() { None } else { self.jump };
    }

    /// 要求した一覧をまだ待っているか。返って来ないこともあるので、しばらくで諦める。
    fn waiting(&self) -> bool {
        self.waiting
            .is_some_and(|asked| asked.elapsed() < Duration::from_secs(2))
    }

    /// 「ZeroWave について」が開いたら一度だけ集め、閉じたら捨てる。集めたらウィンドウをその高さに合わせる。
    fn gather_about(&mut self) {
        let Some(sub) = self
            .subs
            .values()
            .find(|sub| matches!(sub.what, Which::About))
        else {
            self.about = None;
            return;
        };
        if self.about.is_some() {
            return;
        }
        let mut about = ui::panes::about::About::gather(self.gpu.as_ref());
        about.displays = crate::machine::displays(&sub.window);
        let before = sub.window.inner_size().height as f64;
        let after = about.height() as f64 * sub.window.scale_factor();
        if let Ok(at) = sub.window.outer_position() {
            let y = at.y + ((before - after) / 2.0).round() as i32;
            sub.window
                .set_outer_position(winit::dpi::PhysicalPosition::new(at.x, y));
        }
        let _ = sub.window.request_inner_size(winit::dpi::LogicalSize::new(
            ui::panes::about::WIDTH,
            about.height(),
        ));
        self.about = Some(about);
    }

    pub(super) fn paint(&mut self) {
        if self.subs.is_empty() {
            self.about = None;
            return;
        }
        let one_seg = self.refresh_one_seg();
        let panel_rows = self.panel_rows();
        let band = self
            .watching
            .map(|key| Band::of(key.network))
            .unwrap_or_default();
        let scanning = self
            .scanning
            .lock()
            .map(|scanning| scanning.clone())
            .unwrap_or_default();
        self.gather_about();
        // 番組表の一覧は開くまでビルドされないので、局一覧と選べる局から直接取る。
        let bands = zwcore::guide::keeper::bands_of(
            &self.stations,
            band,
            self.settings_page.receivable.as_deref(),
        );
        let gpu = &self.gpu;
        let waiting = self.waiting();
        let mut showing = Showing {
            listing: &self.listing,
            jump_to: if waiting { None } else { self.jump_to },
            finding: &mut self.finding,
            stream: &mut self.stream,
            info: self.info.as_ref(),
            decoding: self.decoding.as_ref(),
            edited: &mut self.edited,
            loaded: &self.loaded,
            stations: &self.stations,
            settings_page: &mut self.settings_page,
            skins: self.theme,
            broadcast: one_seg.as_deref(),
            textures: &mut self.textures,
            text: &mut Vec::new(),
            remote: &mut self.remote,
            band,
            bands: &bands,
            scan: &scanning,
            about: self.about.as_ref(),
            panel: self.panel.as_ref(),
            panel_rows,
        };
        let mut copying = None;
        let mut panel_pressed = None;
        let mut scanned = None;
        let mut happened = None;
        let mut followed = None;
        let mut found = None;
        let mut guide_painted = false;
        let mut remote = None;
        let mut pressed = None;
        let mut settings_pressed = None;
        let mut popup_area = None;
        let mut closing = Vec::new();
        for (id, sub) in &mut self.subs {
            if !sub.due(FRAME, REPAINT_EVERY) {
                continue;
            }
            sub.hurry = false;
            // 出すものが届いていないウィンドウは、サーフェスも取らない（何もレンダリングしていない物を出してしまう）。
            if (matches!(sub.what, Which::Stream) && showing.info.is_none())
                || (matches!(sub.what, Which::Decoder) && showing.decoding.is_none())
            {
                sub.painted = Instant::now();
                continue;
            }
            // ウィンドウの大きさはウィンドウに問い合わせる。`Resized` を取りこぼすと古い大きさのまま残る。
            let now = sub.window.inner_size();
            // 最小化されていてサーフェスは作れない。`dirty` を立てたままだとループを回し続ける。
            if now.width == 0 || now.height == 0 {
                sub.painted = Instant::now();
                continue;
            }
            let resized = sub.size != (now.width, now.height);
            sub.size = (now.width, now.height);
            if matches!(sub.what, Which::OneSeg) {
                popup_area = Some((now.width, now.height));
            }
            sub.dirty = false;
            sub.painted = Instant::now();
            let what = &sub.what;
            let mut said = None;
            let drawn = match &mut sub.paint {
                Paint::Wgpu { surface, target } => {
                    let Some(gpu) = gpu.as_ref() else {
                        continue;
                    };
                    // 失ったデバイスには何も頼まない（wgpu は書き込みで panic する）。開き直したデバイスが届いたら
                    // 用意し直してレンダリングする。
                    if zwcore::output::video::gpu::lost().is_some() {
                        sub.size = (0, 0);
                        sub.dirty = true;
                        continue;
                    }
                    if resized {
                        configure_surface(gpu, surface, target, now.width, now.height);
                    }
                    let frame = match surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(frame)
                        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                        other => {
                            tracing::warn!("別ウィンドウのサーフェスを取得できない: {other:?}");
                            // 次のループで用意し直す。放っておくと最後にレンダリングしたフレームのまま止まる。
                            sub.size = (0, 0);
                            sub.dirty = true;
                            continue;
                        }
                    };
                    let view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let size = (frame.texture.width(), frame.texture.height());
                    guide_painted |= matches!(what, Which::Guide);
                    let input = sub.face.take_input(&sub.window);
                    // 一つのウィンドウのレンダリングが落ちてもスレッドは落とさない（別ウィンドウが全部固まる）。
                    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        sub.face
                            .draw(input, &gpu.device, &gpu.queue, &view, size, |root| {
                                said = what.show(root, &mut showing)
                            })
                    }));
                    drawn.map(|output| (output, Some(frame), size))
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                Paint::Gles(painter) => {
                    // Wayland の EGL サーフェスはウィンドウに追従しないので、大きさはレンダリングする側から伝える。
                    if resized {
                        painter.resize(now.width, now.height);
                    }
                    let window = match painter.gles_window() {
                        Ok(window) => window,
                        Err(error) => {
                            tracing::warn!("別ウィンドウで EGL を開けない: {error:?}");
                            closing.push(*id);
                            continue;
                        }
                    };
                    guide_painted |= matches!(what, Which::Guide);
                    let input = sub.face.take_input(&sub.window);
                    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        sub.face
                            .draw_gles(input, &window, |root| said = what.show(root, &mut showing))
                    }));
                    drawn.map(|output| (output, None, window.size()))
                }
            };
            let Ok((output, frame, size)) = drawn else {
                // GPU のデバイスを失った（egui-wgpu はバッファを作れずに panic する）。ウィンドウは閉じず、
                // 再生スレッドが開き直して新しいデバイスを渡すのを待つ（そのとき開き直す）。
                if zwcore::output::video::gpu::lost().is_some() {
                    sub.dirty = false;
                    continue;
                }
                tracing::warn!(
                    "別ウィンドウ「{}」のレンダリング中にパニックしたため閉じた",
                    sub.what.title(band)
                );
                closing.push(*id);
                continue;
            };
            let window = Arc::clone(&sub.window);
            sub.face.hand_output(&window, output);
            if let Some(frame) = frame
                && let Some(gpu) = gpu.as_ref()
                && zwcore::output::video::gpu::lost().is_none()
            {
                gpu.queue.present(frame);
            }
            // メインスレッドはウィンドウの縁がドラッグされたあと、この通知を待っている（`Shell::keep_pane_up`）。
            sub.presented.say(size, true);
            let scale = window.scale_factor() as f32;
            let px = |v: f32| (v * scale).round() as i32;
            let wanted: Vec<crate::window::text::Shown> = std::mem::take(showing.text)
                .into_iter()
                .map(|piece| {
                    let detail = &self.theme.detail;
                    let [text, dim, back, ground] = piece.colours.unwrap_or([
                        detail.text,
                        detail.dim,
                        if piece.scroll {
                            detail.box_fill
                        } else {
                            detail.background
                        },
                        detail.background,
                    ]);
                    crate::window::text::Shown {
                        rect: (
                            px(piece.rect.left()),
                            px(piece.rect.top()),
                            px(piece.rect.width()),
                            px(piece.rect.height()),
                        ),
                        sections: piece.sections,
                        marks: piece
                            .marks
                            .iter()
                            .map(|(label, c)| (label.clone(), [c.r(), c.g(), c.b()]))
                            .collect(),
                        scroll: piece.scroll,
                        dark: self.theme.mode == ui::theme::Mode::Dark,
                        colour: over(text, ground),
                        dim: over(dim, ground),
                        back: over(back, ground),
                        font_pt: piece.size * 0.75,
                        mono: piece.mono,
                        face: (!piece.mono)
                            .then(crate::ui::text::font::native_face)
                            .flatten(),
                        tail: piece.tail,
                    }
                })
                .collect();
            if wanted != sub.text {
                sub.text = wanted.clone();
                let _ = self.requests.send(Request::Text {
                    window: *id,
                    shown: wanted,
                });
                let _ = self.proxy.send_event(Wake);
            }

            match said {
                Some(Happened::Close) => closing.push(*id),
                Some(Happened::Copy(written)) => copying = Some((*id, written)),
                Some(Happened::Scan(what)) => scanned = Some((*id, what)),
                Some(Happened::Epg(action)) => happened = Some((*id, action)),
                Some(Happened::Found(what)) => found = Some((*id, what)),
                Some(Happened::Follow(follow)) => followed = Some(follow),
                Some(Happened::Remote(key)) => remote = Some(key),
                Some(Happened::Pressed(what)) => pressed = Some(what),
                Some(Happened::Panel(what)) => panel_pressed = Some(what),
                Some(Happened::Settings(what)) => {
                    if matches!(
                        what,
                        ui::panes::settings::Pressed::Ok
                            | ui::panes::settings::Pressed::Cancel
                            | ui::panes::settings::Pressed::Close
                            | ui::panes::settings::Pressed::Settled(_)
                    ) {
                        closing.push(*id);
                    }
                    settings_pressed = Some(what);
                }
                None => {}
            }
        }
        for id in closing {
            self.shut(id);
        }
        if let Some(what) = panel_pressed {
            self.panel_pressed(what);
        }
        if let Some(pressed) = settings_pressed {
            self.settings_pressed(pressed);
        }
        if popup_area.is_some() && popup_area != self.popup_area {
            self.popup_area = popup_area;
            let _ = self.to_broadcast.try_send(ToBroadcast::Popup(popup_area));
        }
        if let Some(key) = remote {
            let _ = self.to_broadcast.try_send(ToBroadcast::Key(key));
        }
        if let Some(what) = pressed {
            let _ = self.requests.send(Request::Remote(what));
            let _ = self.proxy.send_event(Wake);
        }
        // チャンネルスキャンのスレッドは止められないので停止フラグだけ立てる。完了は残し、キャンセルは捨てる。
        if let Some((id, what)) = scanned {
            match what {
                ui::panes::scan::Pressed::Done => {
                    self.scan_keep
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    self.scan_stop
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    self.shut(id);
                }
                ui::panes::scan::Pressed::Stop => {
                    self.scan_keep
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    self.scan_stop
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut scanning) = self.scanning.lock() {
                        scanning.log.push(String::from("スキャンを停止しています…"));
                    }
                }
            }
        }
        if let Some((id, written)) = copying
            && let Some(sub) = self.subs.get_mut(&id)
        {
            sub.face.copy_text(written);
        }
        // スクロールは一度だけ（残すとユーザーがスクロールした分が毎回戻る）。消すのは番組表がそのスクロール位置でレンダリングできてから。
        if guide_painted && !waiting {
            self.jump = None;
            self.jump_to = None;
            self.listing.jump = None;
        }
        if let Some(follow) = followed
            && let Some(detail) = self.watching_detail(follow)
        {
            self.show_detail(detail, false, None);
        }
        let (guide_window, happened) = happened.unzip();
        match happened {
            Some(ui::guide::epg::Action::Open(chosen)) => {
                self.open_detail(chosen.key, chosen.programme, guide_window);
            }
            Some(ui::guide::epg::Action::Tune(key)) => {
                let _ = self.say.send(Chose::Tune(key));
            }
            Some(ui::guide::epg::Action::Band(band)) => {
                self.band = Some(band);
                for sub in self.subs.values() {
                    sub.window.set_title(&sub.what.title(band));
                }
            }
            // スクロールできるのは今日の始まりから七日先の終わりまで（`ui::guide::epg::DAYS`）。
            Some(ui::guide::epg::Action::Shift(seconds)) => {
                let now = self.guide.now_or_machine();
                let first = ui::guide::epg::day_start(now) - 7 * 86_400;
                let last =
                    first + i64::from(ui::guide::epg::DAYS) * 86_400 - i64::from(HOURS) * 3600;
                self.from = Some((self.listing.from + seconds).clamp(first, last.max(first)));
            }
            Some(ui::guide::epg::Action::Now) => {
                self.from = None;
                self.jump = Some(self.guide.now_or_machine());
            }
            Some(ui::guide::epg::Action::Day(n)) => {
                let now = self.guide.now_or_machine();
                let day = ui::guide::epg::day_start(now) + i64::from(n) * 86_400;
                self.from = Some(day);
                self.jump = Some(day);
            }
            Some(ui::guide::epg::Action::Search(query)) => {
                self.finding.ask(query.trim());
                self.ask_search();
                self.open_from(Which::Search, guide_window);
            }
            None => {}
        }
        let (search_window, found) = found.unzip();
        match found {
            Some(ui::guide::found::Pressed::Ask) => self.ask_search(),
            Some(ui::guide::found::Pressed::Open(chosen)) => {
                self.open_detail(chosen.key, chosen.programme, search_window);
            }
            Some(ui::guide::found::Pressed::Jump(key, start)) => {
                let band = Band::of(key.network);
                self.band = Some(band);
                for sub in self.subs.values() {
                    sub.window.set_title(&sub.what.title(band));
                }
                // 開くと「いま」へスクロールするので、スクロール先はその後に置く。
                self.open(Which::Guide);
                self.from = Some(ui::guide::epg::day_start(start));
                self.jump = Some(start);
                self.jump_to = Some(key);
            }
            None => {}
        }
    }

    fn panel_rows(&mut self) -> u32 {
        let size = self
            .subs
            .values()
            .find(|sub| matches!(sub.what, Which::Panel(_)))
            .map(|sub| {
                sub.window
                    .inner_size()
                    .to_logical::<f32>(sub.window.scale_factor())
            });
        let Some(size) = size.filter(|size| size.width > 0.0 && size.height > 0.0) else {
            return self.panel_rows.unwrap_or(2);
        };
        let height = size.height;
        let whole = (size.width.round() as u32, size.height.round() as u32);
        if self.panel_size != Some(whole) {
            self.panel_size = Some(whole);
            let _ = self
                .requests
                .send(Request::Panel(ui::guide::panel::Said::Size(whole)));
            let _ = self.proxy.send_event(Wake);
        }
        let rows = ui::guide::panel::rows_in_window(height);
        if self.panel_rows != Some(rows) {
            self.panel_rows = Some(rows);
            let _ = self
                .requests
                .send(Request::Panel(ui::guide::panel::Said::Rows(rows)));
            let _ = self.proxy.send_event(Wake);
        }
        rows
    }

    fn panel_pressed(&mut self, pressed: ui::guide::panel::Pressed) {
        use ui::guide::panel::Pressed;
        match pressed {
            Pressed::Strip(ui::guide::mini::Pressed::Detail(key, programme)) => {
                self.open_detail(key, *programme, None);
            }
            Pressed::OpenPage(stem, page) => self.open(Which::PluginPage(stem, page)),
            Pressed::Tab(key) => {
                if let Some(feed) = self.panel.as_mut() {
                    feed.tab.clone_from(&key);
                }
                let _ = self
                    .requests
                    .send(Request::Panel(ui::guide::panel::Said::Pressed(
                        Pressed::Tab(key),
                    )));
                let _ = self.proxy.send_event(Wake);
            }
            other => {
                let _ = self
                    .requests
                    .send(Request::Panel(ui::guide::panel::Said::Pressed(other)));
                let _ = self.proxy.send_event(Wake);
            }
        }
    }

    fn ask_search(&mut self) {
        let asked = &self.finding.asked;
        let _ = self.guide_to.send(ToGuide::Search(
            (!asked.query.trim().is_empty()).then(|| asked.clone()),
        ));
        for sub in self.subs.values_mut() {
            if matches!(sub.what, Which::Search) {
                sub.dirty = true;
            }
        }
    }
}
