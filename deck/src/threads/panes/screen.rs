// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 別ウィンドウのスレッドの 1 回のループ：待つ、設定の読み直し、要求を処理する、ウィンドウを開く。

use super::*;

impl Screen {
    pub(super) fn band(&self) -> Band {
        self.band
            .or_else(|| self.watching.map(|key| Band::of(key.network)))
            .unwrap_or_default()
    }

    pub(super) fn run(mut self) {
        loop {
            let wait = self.wait();
            match self.events.recv_timeout(wait) {
                Ok(ToPanes::Quit) => return,
                // 失ったデバイスで開き直したウィンドウの用意（egui-wgpu）が panic してもスレッドは落とさない。
                Ok(what) => {
                    let device = self.device();
                    crate::draw::through_loss_on(device.as_ref(), || self.take(what));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            loop {
                match self.events.try_recv() {
                    Ok(ToPanes::Quit) => return,
                    Ok(what) => {
                        let device = self.device();
                        crate::draw::through_loss_on(device.as_ref(), || self.take(what));
                    }
                    Err(_) => break,
                }
            }
            self.watch();
            self.refresh();
            let device = self.device();
            crate::draw::through_loss_on(device.as_ref(), || self.paint());
        }
    }

    /// 今使っている wgpu のデバイス（失ったかを直接問い合わせるため）。
    fn device(&self) -> Option<wgpu::Device> {
        self.gpu.as_ref().map(|gpu| gpu.device.clone())
    }

    /// `zerowave.toml` が外で直されたら読み直す（一秒に一度、更新時刻だけ見る）。
    fn watch(&mut self) {
        let now = Instant::now();
        if self
            .file_checked
            .is_some_and(|checked| now.duration_since(checked) < Duration::from_secs(1))
        {
            return;
        }
        self.file_checked = Some(now);
        let modified = crate::config::modified();
        if modified == self.file_seen {
            return;
        }
        self.file_seen = modified;
        let fresh = ui::panes::settings::with_default_networks(crate::config::load_plain());
        if fresh == self.loaded {
            return;
        }
        let screen_only = crate::config::changes(&self.loaded, &fresh)
            .iter()
            .all(|change| screen_only(&change.path));
        self.edited = crate::config::merge_reload(&self.loaded, &self.edited, &fresh);
        self.loaded = fresh;
        // メニューで変えた物は画面の側がもう持っているので、再生側へは送り直さない。
        if screen_only {
            return;
        }
        tracing::info!("設定ファイルの変更を検出し、再読み込みした");
        let _ = self
            .requests
            .send(Request::Settings(Box::new(self.loaded.clone())));
        let _ = self.proxy.send_event(Wake);
    }

    fn wait(&self) -> Duration {
        let wait = self
            .subs
            .values()
            .map(|sub| sub.after(FRAME, REPAINT_EVERY))
            .min()
            .unwrap_or(IDLE_WAIT);
        // ワンセグのウィンドウは、変化を通知してくるものが無いので、こまめに確認する。
        if self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::OneSeg))
        {
            return wait.min(FRAME);
        }
        wait
    }

    pub(super) fn refresh_one_seg(&mut self) -> Option<Arc<zwcore::data::Shown>> {
        if !self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::OneSeg))
        {
            return None;
        }
        let shown = self.shown.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let now = shown.as_ref().map(|shown| Arc::as_ptr(shown) as usize);
        if now != self.drawn_shown {
            self.drawn_shown = now;
            for sub in self.subs.values_mut() {
                if matches!(sub.what, Which::OneSeg) {
                    sub.dirty = true;
                }
            }
        }
        shown
    }

    fn take(&mut self, what: ToPanes) {
        match what {
            ToPanes::Opened(window, paint, presented) => {
                self.attach(window, paint, presented);
            }
            ToPanes::NotOpened => {
                self.pending.pop_front();
            }
            ToPanes::DropAt(id, at) => {
                self.drop_at.insert(id, at);
            }
            ToPanes::Event(id, event) => self.event(id, event),
            ToPanes::Open(what) => self.open(what),
            ToPanes::CaptureLayers(layers) => {
                self.settings_page.capture_layers = layers;
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::Settings) {
                        sub.dirty = true;
                    }
                }
            }
            ToPanes::OpenSettings(page) => {
                self.settings_page.page = page;
                self.settings_page.scroll = 0.0;
                self.open(Which::Settings);
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::Settings) {
                        sub.dirty = true;
                    }
                }
            }
            ToPanes::Detail(key, programme) => self.open_detail(key, *programme, None),
            ToPanes::WatchingDetail => {
                if let Some(detail) = self.watching_detail(ui::guide::detail::Follow::Now) {
                    self.show_detail(detail, true, None);
                }
            }
            ToPanes::Guide(guide) => {
                self.guide = guide;
                self.version += 1;
                self.asked = None;
                self.follow_watching();
            }
            ToPanes::Watching(watching) => {
                self.watching = watching;
                self.follow_watching();
            }
            ToPanes::Receivable(keys) => {
                self.settings_page.receivable = keys;
                for sub in self.subs.values_mut() {
                    sub.dirty = true;
                }
            }
            ToPanes::Info(info) => self.info = Some(*info),
            ToPanes::Decoding(decoding) => {
                self.decoding = Some(*decoding);
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::Decoder) {
                        sub.dirty = true;
                    }
                }
            }
            ToPanes::Plugins(shared) => {
                self.settings_page
                    .plugin_ui
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .tell(*shared);
                for sub in self.subs.values_mut() {
                    if matches!(
                        sub.what,
                        Which::Settings | Which::PluginPage(..) | Which::Panel(_)
                    ) {
                        sub.dirty = true;
                    }
                }
            }
            ToPanes::Panel(feed) => {
                self.panel = Some(*feed);
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::Panel(_)) {
                        sub.dirty = true;
                    }
                }
            }
            ToPanes::ShutPanel => {
                // 開く要求がまだ返っていなければ、届いたらすぐ閉じる（要求の順は飛ばさない）。
                if self
                    .pending
                    .iter()
                    .any(|what| matches!(what, Which::Panel(_)))
                {
                    self.panel_dropped = true;
                }
                if let Some(id) = self
                    .subs
                    .iter()
                    .find(|(_, sub)| matches!(sub.what, Which::Panel(_)))
                    .map(|(id, _)| *id)
                {
                    self.panel_quiet = true;
                    self.shut(id);
                    self.panel_quiet = false;
                }
            }
            ToPanes::Mode(mode) => self.wear(mode),
            ToPanes::ShutOneSeg => {
                if let Some(id) = self
                    .subs
                    .iter()
                    .find(|(_, sub)| matches!(sub.what, Which::OneSeg))
                    .map(|(id, _)| *id)
                {
                    self.shut(id);
                }
            }
            ToPanes::Repaint => {
                for sub in self.subs.values_mut() {
                    sub.dirty = true;
                }
            }
            ToPanes::Profile(name) => {
                self.settings_page.profile = name.clone();
                self.settings_page.active = name;
                for sub in self.subs.values_mut() {
                    sub.dirty = true;
                }
            }
            ToPanes::Gpu(gpu) => {
                let mut open = std::mem::take(&mut self.reopen);
                open.extend(self.close_all());
                let _ = self.proxy.send_event(Wake);
                self.gpu = Some(gpu);
                self.about = None;
                for what in open {
                    self.open(what);
                }
            }
            ToPanes::DropGpu => {
                let closed = self.close_all();
                self.reopen.extend(closed);
                self.gpu = None;
                self.about = None;
            }
            ToPanes::Quit => {}
        }
    }

    /// 開いているウィンドウを全部閉じ、開き直すための種類を返す（サーフェスは前のデバイスとインスタンスの物）。
    fn close_all(&mut self) -> Vec<Which> {
        let ids: Vec<WindowId> = self.subs.keys().copied().collect();
        let mut open = Vec::new();
        for id in ids {
            self.drop_at.remove(&id);
            if let Some(sub) = self.subs.remove(&id).map(Sub::release) {
                let _ = self.requests.send(Request::Closed(id));
                match sub.what {
                    Which::Guide => open.push(Which::Guide),
                    Which::Search => open.push(Which::Search),
                    Which::Stream => open.push(Which::Stream),
                    Which::Decoder => open.push(Which::Decoder),
                    Which::Settings => open.push(Which::Settings),
                    Which::Remote => open.push(Which::Remote),
                    Which::OneSeg => open.push(Which::OneSeg),
                    Which::Scan => open.push(Which::Scan),
                    // デバイスが替わると GPU の名前も替わるので、開き直して集め直す。
                    Which::About => open.push(Which::About),
                    Which::Tuners => open.push(Which::Tuners),
                    Which::FiveOne => open.push(Which::FiveOne),
                    Which::Favourites => open.push(Which::Favourites),
                    Which::FontPick => open.push(Which::FontPick),
                    Which::CoaxdUpdate => open.push(Which::CoaxdUpdate),
                    Which::ScanDelete => open.push(Which::ScanDelete),
                    Which::ScanCopy => open.push(Which::ScanCopy),
                    Which::TwentyTwo => open.push(Which::TwentyTwo),
                    Which::Plugin(name) => open.push(Which::Plugin(name)),
                    Which::PluginPage(stem, page) => {
                        open.push(Which::PluginPage(stem, page));
                    }
                    Which::Panel(size) => open.push(Which::Panel(size)),
                    Which::Detail(_) => {}
                }
            }
        }
        open
    }

    pub(super) fn open(&mut self, what: Which) {
        self.open_from(what, None);
    }

    /// `from` は開いたウィンドウ。番組表などから開いたらそれをオーナーにする（本体を手前に出さない）。
    pub(super) fn open_from(&mut self, what: Which, from: Option<WindowId>) {
        if matches!(what, Which::Panel(_)) {
            self.panel_dropped = false;
        }
        let owner = self.owner_of(what.owned(), from);
        if let Some(already) = self.subs.values().find(|sub| sub.what.same(&what)) {
            self.reown(&already.window, owner);
            already.window.focus_window();
            return;
        }
        if self.pending.iter().any(|known| known.same(&what)) {
            return;
        }
        if matches!(what, Which::Settings) {
            let active = crate::config::load_state().profile;
            self.settings_page.profile = active.clone();
            self.settings_page.active = active;
        }
        if matches!(what, Which::Guide) {
            self.jump = Some(self.guide.now_or_machine());
        }
        let mut title = what.title(self.band());
        let mut size = what.size();
        if let Which::PluginPage(stem, page) = &what
            && let Ok(page) = self
                .settings_page
                .plugin_ui
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .page(stem, page, &self.guide)
        {
            if !page.title.is_empty() {
                title = page.title;
            }
            if let Some([width, height]) = page.size {
                size = (width, height);
            }
        }
        let buttons = what.buttons();
        let resizable = what.resizable();
        let min_size = what.min_size();
        let place = what.place();
        self.pending.push_back(what);
        let _ = self.requests.send(Request::Window {
            title,
            size,
            buttons,
            resizable,
            min_size,
            place,
            owner,
        });
        let _ = self.proxy.send_event(Wake);
    }

    pub(super) fn owner_of(&self, owned: bool, from: Option<WindowId>) -> Owner {
        if !owned {
            return Owner::None;
        }
        match from.filter(|id| self.subs.contains_key(id)) {
            Some(id) => Owner::Pane(id),
            None => Owner::Main,
        }
    }

    /// 開いているウィンドウを手前に出す前に、オーナーを替える。
    pub(super) fn reown(&self, window: &Window, owner: Owner) {
        let _ = self.requests.send(Request::Owner {
            window: window.id(),
            owner,
        });
        let _ = self.proxy.send_event(Wake);
    }

    /// 要求してあったウィンドウが届いた。サーフェスは映像と同じインスタンスから作ってある。
    fn attach(
        &mut self,
        window: Arc<Window>,
        paint: Option<crate::window::sub::Paint>,
        presented: Arc<crate::threads::link::Presented>,
    ) {
        let Some(what) = self.pending.pop_front() else {
            tracing::info!("要求していないウィンドウが作成された");
            return;
        };
        let Some(paint) = paint else {
            tracing::info!("別ウィンドウのサーフェスが無いため開けない");
            return;
        };
        let size = window.inner_size();
        let mode = self.theme.mode;
        let paper = what.paper();
        let face = match &paint {
            crate::window::sub::Paint::Wgpu { .. } => {
                let Some(gpu) = self.gpu.as_ref() else {
                    tracing::info!("wgpu のデバイスが無いため、別ウィンドウを開けない");
                    return;
                };
                let dress = |target| {
                    if paper {
                        Face::on_paper(&window, &gpu.device, target, mode)
                    } else {
                        Face::new(&window, &gpu.device, target)
                    }
                };
                #[allow(irrefutable_let_patterns)]
                let crate::window::sub::Paint::Wgpu { surface, .. } = &paint else {
                    return;
                };
                let mut target = wgpu::TextureFormat::Bgra8UnormSrgb;
                configure_surface(gpu, surface, &mut target, size.width, size.height);
                // サーフェスの形式が決まってから画面を用意する（別の形式のレンダラーではレンダリングできない）。
                let face = dress(target);
                (face, Some(target))
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            crate::window::sub::Paint::Gles(_) => {
                (Face::on_gles(&window, paper.then_some(mode)), None)
            }
        };
        let (face, target) = face;
        let mut paint = paint;
        if let (crate::window::sub::Paint::Wgpu { target: was, .. }, Some(target)) =
            (&mut paint, target)
        {
            *was = target;
        }
        let sub = Sub {
            face,
            window: window.clone(),
            paint,
            size: (size.width, size.height),
            what,
            dirty: true,
            painted: Instant::now(),
            moving_until: None,
            resized_at: None,
            hurry: false,
            presented,
            text: Vec::new(),
        };
        let id = window.id();
        let dropped =
            matches!(sub.what, Which::Panel(_)) && std::mem::take(&mut self.panel_dropped);
        self.subs.insert(id, sub);
        if dropped {
            self.panel_quiet = true;
            self.shut(id);
            self.panel_quiet = false;
        }
        self.tell_wants();
    }
}
