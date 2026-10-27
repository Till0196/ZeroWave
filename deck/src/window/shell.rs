// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! イベントループ。ウィンドウを開き、イベントを egui に渡し、画面をビルドする。サーフェスは持たない。
//! 次に何かをする時刻まで待つ（`ControlFlow::WaitUntil`）。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::audio::device as sound;
use crate::deck::Deck;
use crate::draw::overlay::Drawn;
use crate::stage::{self, Stage};
use crate::threads::link::{Act, Caught, Place, Presented, Request, ToStage, Wake};
use crate::threads::panes::ToPanes;
use crate::ui;
use crate::window::ending;

const KEEP_UP: Duration = Duration::from_millis(100);
const UNKEPT_FOR: Duration = Duration::from_secs(1);

pub struct Boot {
    pub stage: stage::Boot,
    pub instance: Option<wgpu::Instance>,
}

pub struct Shell {
    boot: Option<Boot>,
    deck: Deck,
    to_stage: Sender<ToStage>,
    presented: Arc<Presented>,
    unkept_until: Option<Instant>,
    /// 別ウィンドウへ渡すチャネル。再生スレッド経由だと、そちらが詰まっている間番組表が固まる。
    to_panes: Sender<ToPanes>,
    requests: Receiver<Request>,
    stage: Option<ending::Thread>,
    /// 番組表のスレッド。終了するときの番組表の保存を待つ。
    guide: Option<ending::Thread>,
    window: Option<Arc<Window>>,
    panes_windows: std::collections::HashMap<WindowId, Arc<Window>>,
    panes_presented: std::collections::HashMap<WindowId, Arc<Presented>>,
    panes_kept: std::collections::HashMap<WindowId, &'static str>,
    #[cfg(target_os = "linux")]
    wayland_drop: Option<crate::window::wayland_drop::WaylandDrop>,
    pane_unkept_until: Option<Instant>,
    texts: std::collections::HashMap<WindowId, crate::window::text::Text>,
    context: egui::Context,
    fonts_seen: u64,
    state: Option<egui_winit::State>,
    viewport: egui::ViewportInfo,
    next: Option<Instant>,
    instance: Option<wgpu::Instance>,
    keep: sound::Keep,
    done: bool,
    /// ウィンドウが閉じられてから再生スレッドの終了を待つ期限。ループを回したまま待つ（macOS で `join` すると
    /// ウィンドウに触る別ウィンドウのスレッドが止まり、再生スレッドも終了できなくなる）。
    closing: Option<Instant>,
    placed: Option<crate::config::state::Window>,
    deferred: Vec<Request>,
    /// OS が返す明暗（macOS の `Window::theme` は置いた物を返すので別に持つ）。
    os_theme: Option<winit::window::Theme>,
    worn: Option<ui::theme::Mode>,
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    ticking: bool,
    gles: bool,
    built: (u32, Instant),
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Shell").finish_non_exhaustive()
    }
}

impl Shell {
    pub fn new(
        boot: Boot,
        deck: Deck,
        to_stage: Sender<ToStage>,
        to_panes: Sender<ToPanes>,
        requests: Receiver<Request>,
        keep: sound::Keep,
        guide: Option<ending::Thread>,
    ) -> Self {
        let context = egui::Context::default();
        ui::text::font::install_or_say(&context);
        ui::style(&context, ui::theme::Mode::default());
        let presented = boot.stage.presented.clone();
        Self {
            boot: Some(boot),
            deck,
            to_stage,
            presented,
            unkept_until: None,
            to_panes,
            requests,
            stage: None,
            guide,
            window: None,
            panes_windows: std::collections::HashMap::new(),
            panes_presented: std::collections::HashMap::new(),
            panes_kept: std::collections::HashMap::new(),
            #[cfg(target_os = "linux")]
            wayland_drop: None,
            pane_unkept_until: None,
            texts: std::collections::HashMap::new(),
            context,
            fonts_seen: ui::text::font::generation(),
            state: None,
            viewport: egui::ViewportInfo::default(),
            next: Some(Instant::now()),
            instance: None,
            keep,
            done: false,
            closing: None,
            placed: None,
            deferred: Vec::new(),
            os_theme: None,
            worn: None,
            ticking: false,
            gles: false,
            built: (0, Instant::now()),
        }
    }

    pub fn close(self) {
        let terminator = ending::watch(ending::WATCHDOG);
        let guide;
        // 後始末もウォッチドッグの中で（ここで止まると、終わらず、再起動もしない）。
        {
            #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
            let mut shell = self;
            #[cfg(target_os = "linux")]
            drop(shell.wayland_drop.take());
            let Self {
                to_stage,
                stage,
                keep,
                guide: kept_guide,
                ..
            } = shell;
            guide = kept_guide;
            drop(to_stage);
            terminator.at("再生スレッドを終了しているところ");
            if let Some(mut stage) = stage {
                stage.join_within(ending::PATIENCE);
            }
            terminator.at("音声出力を閉じているところ");
            // 音声出力はここまで残す（再生スレッドが最後に書いた音声の出力先）。
            drop(keep);
            terminator.at("ウィンドウと画面を片付けているところ");
        }
        terminator.stood_down();
        // 番組表のスレッドは、チャネルを持つ物が全部片付いてから最後の保存をして終わる。
        // 待たずにプロセスを終えると、保存の途中で打ち切られて書きかけの分が捨てられる。
        // HDD では数秒かかるので、ウィンドウを閉じた後に別の期限で待つ。
        // その間に開き直せるよう、二重起動防止のロックは先に放す。
        if let Some(mut guide) = guide {
            crate::config::release_single();
            let terminator = ending::watch(ending::GUIDE_SAVING + ending::WATCHDOG);
            terminator.at("番組表を保存しているところ");
            guide.join_within(ending::GUIDE_SAVING);
            terminator.stood_down();
        }
    }

    fn fitting(&self, size: (f32, f32)) -> (f32, f32) {
        // タイトルバーとタスクバーの分。大きく取ると高さの無い画面でリモコンが小さくなりすぎる。
        const MARGIN: f64 = 80.0;
        let Some(monitor) = self
            .window
            .as_ref()
            .and_then(|window| window.current_monitor())
        else {
            return size;
        };
        let scale = monitor.scale_factor().max(0.5);
        let room = monitor.size().to_logical::<f64>(scale);
        let (wide, tall) = (room.width - MARGIN, room.height - MARGIN);
        if wide <= 0.0 || tall <= 0.0 {
            return size;
        }
        let shrink = (wide / f64::from(size.0))
            .min(tall / f64::from(size.1))
            .min(1.0);
        (
            (f64::from(size.0) * shrink).floor() as f32,
            (f64::from(size.1) * shrink).floor() as f32,
        )
    }

    fn wear(&mut self) {
        let mode = ui::theme::Mode::of(self.os_theme);
        if self.worn == Some(mode) {
            return;
        }
        let first = self.worn.is_none();
        self.worn = Some(mode);
        tracing::info!(
            "テーマ {}{}",
            match mode {
                ui::theme::Mode::Dark => "ダーク",
                ui::theme::Mode::Light => "ライト",
            },
            if first { "（起動時）" } else { "" }
        );
        self.deck.wear(mode, &self.context);
        let theme = window_theme(mode);
        for window in self.window.iter().chain(self.panes_windows.values()) {
            window.set_theme(Some(theme));
        }
        let _ = self.to_panes.send(ToPanes::Mode(mode));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn os_theme_changed(&mut self, theme: winit::window::Theme) {
        if self.os_theme == Some(theme) {
            return;
        }
        self.os_theme = Some(theme);
        self.wear();
    }

    fn tell_monitor(&self, window: &Window) {
        if let Some(monitor) = window.current_monitor() {
            let size = monitor.size();
            zwcore::decode::set_display_size(size.width, size.height);
        }
        let Some(name) = window.current_monitor().and_then(|monitor| monitor.name()) else {
            return;
        };
        if let Some(monitor) = crate::draw::monitor::of(&name) {
            let _ = self.to_stage.send(ToStage::Monitor(monitor));
        }
    }

    fn compose(&mut self) {
        let (Some(window), Some(state)) = (self.window.clone(), self.state.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        if self.presented.drawn_waiting() {
            self.next = Some(Instant::now() + Duration::from_millis(16));
            return;
        }
        let mut input = state.take_egui_input(&window);
        egui_winit::update_viewport_info(&mut self.viewport, &self.context, &window, false);
        input
            .viewports
            .insert(egui::ViewportId::ROOT, self.viewport.clone());
        let output = self
            .context
            .run_ui(input, |root| self.deck.build(root, &window));
        self.built.0 += 1;
        if self.built.1.elapsed() >= Duration::from_secs(5) {
            let causes: Vec<String> = self
                .context
                .repaint_causes()
                .iter()
                .map(|cause| format!("{}:{}", cause.file, cause.line))
                .collect();
            tracing::debug!(
                "画面の再構築 {} 回／5 秒、要求元 {}",
                self.built.0,
                causes.join(" ")
            );
            self.built = (0, Instant::now());
        }
        let popup = self.deck.flush(&window);
        let viewport = output.viewport_output.get(&egui::ViewportId::ROOT);
        self.next = viewport.and_then(|out| Instant::now().checked_add(out.repaint_delay));
        if let Some(out) = viewport {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.context,
                &mut self.viewport,
                out.commands.clone(),
                &window,
                &mut actions,
            );
        }
        state.handle_platform_output(&window, output.platform_output);
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let drawn = Drawn::new(
            jobs,
            output.textures_delta,
            output.pixels_per_point,
            (size.width, size.height),
        );
        // 数えてから送る（送った直後に再生スレッドが受け取ると、後から数えた 1 が残って二度とビルドしない）。
        self.presented.drawn_sent();
        if self.to_stage.send(ToStage::Drawn(Box::new(drawn))).is_err() {
            self.presented.drawn_taken();
        }
        if let Some(popup) = popup {
            self.present(popup, &window);
        }
    }

    fn keep_up(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let now = Instant::now();
        if self.unkept_until.is_some_and(|until| now < until) {
            return;
        }
        self.unkept_until = None;
        self.compose();
        if self.gles {
            return;
        }
        if self.presented.wait_for(size, KEEP_UP) == Caught::Late {
            self.unkept_until = Some(now + UNKEPT_FOR);
        }
    }

    /// 新しい一枚が出るまで `WM_SIZE` から戻らない（戻ると OS が引き伸ばして見せる）。Windows だけ
    /// （macOS では別ウィンドウのスレッドがウィンドウに触るたびにメインスレッドへ回され詰まる）。
    fn keep_pane_up(&mut self, id: WindowId, size: (u32, u32)) {
        if cfg!(not(target_os = "windows")) || size.0 == 0 || size.1 == 0 {
            return;
        }
        let now = Instant::now();
        if self.pane_unkept_until.is_some_and(|until| now < until) {
            return;
        }
        self.pane_unkept_until = None;
        let Some(presented) = self.panes_presented.get(&id) else {
            return;
        };
        if presented.wait_for(size, KEEP_UP) == Caught::Late {
            self.pane_unkept_until = Some(now + UNKEPT_FOR);
        }
    }

    /// OS のメニューを出す。閉じるまで戻らない。その間はタイマーで一枚ビルドする
    /// （`Shell::tick_while_modal`）。出している間 `self` の借用は生きておらず、タイマーは生のポインタで触る。
    fn present(&mut self, popup: crate::window::menu::Prepared, window: &Arc<Window>) {
        // ここからは `self` を直に使わない（`&mut self` と生のポインタを混ぜると借用の規則に反する）。
        let this: *mut Self = self;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let ticking = modal::start(this, window);
        // SAFETY: `this` は生きている `self` で、タイマーはこのスレッドでメニューの間にだけ鳴る。
        unsafe { (*this).deck.menu_showing(true) };
        popup.present(window);
        // SAFETY: 同上。
        unsafe { (*this).deck.menu_showing(false) };
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        drop(ticking);
        // SAFETY: 同上。タイマーはもう止まっている。
        unsafe { (*this).deck.keep_menu(popup) };
    }

    #[cfg(target_os = "windows")]
    fn init_menu_popup(&mut self, hmenu: isize) {
        let (_, done) = self.deck.pump(&self.to_panes);
        if done {
            self.done = true;
        }
        self.deck.refill_open_menu(hmenu);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn tick_while_modal(&mut self) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        self.take_requests(None);
        let fresh = self.pump();
        if fresh {
            use crate::window::menu::native::Updated;
            match self.deck.update_open_menu() {
                Updated::Same => {}
                Updated::Redrawn => {
                    #[cfg(target_os = "windows")]
                    modal::refresh_popups(false);
                }
                Updated::Resized => {
                    #[cfg(target_os = "windows")]
                    modal::refresh_popups(true);
                }
            }
        }
        let told = self.deck.due();
        let next = match (self.next, told) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.next = next;
        let due = next.is_some_and(|next| next <= Instant::now());
        if fresh || due || self.deck.layers_changed() {
            self.compose();
        }
        self.ticking = false;
    }

    fn pump(&mut self) -> bool {
        let (fresh, done) = self.deck.pump(&self.to_panes);
        if done {
            self.done = true;
        }
        fresh
    }

    fn owner_window(&self, owner: crate::threads::link::Owner) -> Option<Arc<Window>> {
        use crate::threads::link::Owner;
        match owner {
            Owner::None => None,
            Owner::Main => self.window.clone(),
            Owner::Pane(id) => self.panes_windows.get(&id).cloned(),
        }
    }

    fn take_requests(&mut self, event_loop: Option<&ActiveEventLoop>) {
        let mut waiting: Vec<Request> = std::mem::take(&mut self.deferred);
        waiting.extend(self.requests.try_iter());
        for request in waiting {
            match request {
                Request::Window {
                    title,
                    size,
                    buttons,
                    resizable,
                    min_size,
                    place,
                    owner,
                } => {
                    let Some(event_loop) = event_loop else {
                        self.deferred.push(Request::Window {
                            title,
                            size,
                            buttons,
                            resizable,
                            min_size,
                            place,
                            owner,
                        });
                        continue;
                    };
                    // 形を保ったまま縮める（OS が高さだけ切ると形が崩れる）。
                    let size = self.fitting(size);
                    let attributes = Window::default_attributes()
                        .with_theme(self.worn.map(window_theme))
                        .with_title(title)
                        .with_enabled_buttons(buttons)
                        .with_resizable(resizable)
                        .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1));
                    let mut attributes = with_icon(attributes);
                    if let Some((width, height)) = min_size {
                        attributes = attributes.with_min_inner_size(winit::dpi::LogicalSize::new(
                            width.min(size.0),
                            height.min(size.1),
                        ));
                    }
                    // 位置は隠したまま決めてから表示する（外枠の大きさはウィンドウを作るまで分からない）。
                    let kept = match place {
                        Place::Kept(name) => crate::config::load_state()
                            .panes
                            .get(name)
                            .copied()
                            .filter(|placed| placed.w > 0 && placed.h > 0),
                        Place::Centred => None,
                    };
                    if let Some(placed) = kept {
                        attributes = attributes
                            .with_inner_size(winit::dpi::PhysicalSize::new(placed.w, placed.h));
                    }
                    attributes = attributes.with_visible(false);
                    if let Some(owner) = self.owner_window(owner) {
                        attributes = with_owner(attributes, &owner);
                    }
                    match event_loop.create_window(attributes) {
                        Ok(window) => {
                            self.place_pane(event_loop, &window, place, kept);
                            x11_buttons(&window, buttons, resizable);
                            if let Place::Kept(name) = place {
                                self.panes_kept.insert(window.id(), name);
                            }
                            window.set_visible(true);
                            if kept.is_some_and(|placed| placed.maximized) {
                                window.set_maximized(true);
                            }
                            let window = Arc::new(window);
                            let surface =
                                self.instance.as_ref().and_then(|instance| {
                                    match instance.create_surface(window.clone()) {
                                        Ok(surface) => Some(crate::window::sub::Paint::Wgpu {
                                            surface,
                                            target: wgpu::TextureFormat::Bgra8UnormSrgb,
                                        }),
                                        Err(error) => {
                                            tracing::warn!("サーフェスを作成できない: {error}");
                                            None
                                        }
                                    }
                                });
                            // 別ウィンドウも EGL でレンダリングする。垂直同期は待たない（フレームごとに待つと番組表が固まる）。
                            #[cfg(all(unix, not(target_os = "macos")))]
                            let surface = surface.or_else(|| {
                                self.gles
                                    .then(|| crate::draw::gles::open_gles(&window, false))
                                    .flatten()
                                    .map(crate::window::sub::Paint::Gles)
                            });
                            self.panes_windows.insert(window.id(), Arc::clone(&window));
                            let presented = Arc::new(Presented::default());
                            self.panes_presented
                                .insert(window.id(), Arc::clone(&presented));
                            let _ = self
                                .to_panes
                                .send(ToPanes::Opened(window, surface, presented));
                        }
                        Err(error) => {
                            tracing::warn!("ウィンドウを開けない: {error}");
                            let _ = self.to_panes.send(ToPanes::NotOpened);
                        }
                    }
                }
                Request::Owner { window, owner } => {
                    #[cfg(windows)]
                    if let Some(pane) = self.panes_windows.get(&window) {
                        let owner = self.owner_window(owner);
                        crate::window::win32::set_owner(pane, owner.as_deref());
                    }
                    #[cfg(not(windows))]
                    let _ = (window, owner);
                }
                Request::Title(title) => {
                    if let Some(window) = &self.window {
                        window.set_title(&title);
                    }
                }
                Request::Text { window, shown } => {
                    if shown.is_empty() {
                        self.texts.remove(&window);
                        continue;
                    }
                    let Some(pane) = self.panes_windows.get(&window).cloned() else {
                        continue;
                    };
                    let text = self.texts.entry(window).or_default();
                    text.show(&pane, &shown);
                }
                Request::Closed(window) => {
                    self.keep_pane_place(window);
                    self.texts.remove(&window);
                    self.panes_windows.remove(&window);
                    self.panes_presented.remove(&window);
                }
                Request::Remote(pressed) => {
                    if let Some(window) = self.window.clone() {
                        self.deck.remote(pressed, &window);
                    }
                }
                Request::Panel(said) => self.deck.panel_said(said),
                Request::Settings(file) => {
                    ui::text::font::choose(ui::text::font::Choice::of(&file));
                    ui::text::font::refresh(&self.context, &mut self.fonts_seen);
                    ui::theme::choose(&file.ui.theme);
                    self.deck.settings(file);
                    self.wear();
                }
                Request::SystemTheme(theme) => self.os_theme_changed(theme),
                Request::Raise => {
                    if let Some(window) = &self.window {
                        window.set_minimized(false);
                        window.focus_window();
                    }
                }
                Request::Captions(shown) => self.deck.captions_from_settings(&shown),
                Request::Replugged(streams) => self.deck.replugged(streams),
                Request::YieldTuner(yielding) => self.deck.say(ToStage::YieldTuner(yielding)),
                Request::Scanned(services) => self.deck.say(ToStage::Scanned(services)),
                Request::Reinstance(backends) => {
                    let Some(window) = self.window.clone() else {
                        continue;
                    };
                    let instance = crate::draw::instance_of(backends);
                    let surface = match instance.create_surface(window.clone()) {
                        Ok(surface) => Some(surface),
                        Err(error) => {
                            tracing::warn!("サーフェスを作成できない: {error}");
                            None
                        }
                    };
                    self.instance = Some(instance.clone());
                    // 画面の土台も作り直す（フォントやテクスチャは前のコンテキストにあり、egui は差分しか渡さない）。
                    self.context = egui::Context::default();
                    self.deck.forget_textures();
                    ui::text::font::install_or_say(&self.context);
                    let mode = ui::theme::Mode::of(self.os_theme);
                    self.deck.wear(mode, &self.context);
                    self.state = Some(egui_winit::State::new(
                        self.context.clone(),
                        egui::ViewportId::ROOT,
                        &window,
                        Some(window.scale_factor() as f32),
                        None,
                        None,
                    ));
                    egui_winit::update_viewport_info(
                        &mut self.viewport,
                        &self.context,
                        &window,
                        true,
                    );
                    let _ = self
                        .to_stage
                        .send(ToStage::Reinstanced { instance, surface });
                }
            }
        }
    }
}

fn window_theme(mode: ui::theme::Mode) -> winit::window::Theme {
    match mode {
        ui::theme::Mode::Dark => winit::window::Theme::Dark,
        ui::theme::Mode::Light => winit::window::Theme::Light,
    }
}

/// macOS は `AppleInterfaceStyle` を読む（バンドルでない実行ファイルでは `ThemeChanged` が届かず、ウィンドウに問い合わせると設定した値が返る）。
#[cfg(target_os = "macos")]
fn system_theme() -> Option<winit::window::Theme> {
    use objc2_core_foundation::{
        CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFString,
        kCFPreferencesAnyApplication,
    };
    // SAFETY: 静的な名前。
    let any = unsafe { kCFPreferencesAnyApplication };
    CFPreferencesAppSynchronize(any);
    let key = CFString::from_str("AppleInterfaceStyle");
    Some(if CFPreferencesCopyAppValue(&key, any).is_some() {
        winit::window::Theme::Dark
    } else {
        winit::window::Theme::Light
    })
}

/// Linux はポータルの `color-scheme`（1 が暗い、2 が明るい）を `gdbus`（無ければ `busctl`）で問い合わせる。
/// winit は Linux では値を返さない。
#[cfg(target_os = "linux")]
fn system_theme() -> Option<winit::window::Theme> {
    const ARGS: [&str; 2] = ["org.freedesktop.appearance", "color-scheme"];
    let asked = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // gdbus は「(<<uint32 1>>,)」、busctl は「v u 1」。どちらも最後の数が値。
    let said = asked(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.Read",
            ARGS[0],
            ARGS[1],
        ],
    )
    .or_else(|| {
        asked(
            "busctl",
            &[
                "--user",
                "call",
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
                "Read",
                "ss",
                ARGS[0],
                ARGS[1],
            ],
        )
    })?;
    scheme_of(&said)
}

#[cfg(target_os = "linux")]
fn scheme_of(said: &str) -> Option<winit::window::Theme> {
    let value = said
        .split(|c: char| !c.is_ascii_digit())
        .rfind(|part| !part.is_empty())?;
    match value {
        "1" => Some(winit::window::Theme::Dark),
        "2" => Some(winit::window::Theme::Light),
        _ => None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_theme() -> Option<winit::window::Theme> {
    None
}

/// OS のダーク／ライト設定を別のスレッドで定期的に問い合わせる（macOS は待たされ、Linux は外部プログラムに問い合わせるため）。
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn watch_system_theme(
    requests: Sender<Request>,
    proxy: winit::event_loop::EventLoopProxy<Wake>,
) {
    let spawned = std::thread::Builder::new()
        .name(String::from("theme"))
        .spawn(move || {
            let mut seen = system_theme();
            let every = if cfg!(target_os = "linux") {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(1)
            };
            loop {
                std::thread::sleep(every);
                let now = system_theme();
                if now == seen {
                    continue;
                }
                seen = now;
                let Some(theme) = now else { continue };
                if requests.send(Request::SystemTheme(theme)).is_err() {
                    return;
                }
                let _ = proxy.send_event(Wake);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("OS のテーマを監視するスレッドを起動できない: {error}");
    }
}

/// 保存した位置を使う最小のサイズ（物理ピクセル）。これより小さいのは壊れた値と見る。
const MIN_PLACED: (u32, u32) = (320, 180);

fn on_a_monitor(event_loop: &ActiveEventLoop, placed: &crate::config::state::Window) -> bool {
    let (x, y) = (placed.x + 40, placed.y + 20);
    event_loop.available_monitors().any(|monitor| {
        let at = monitor.position();
        let size = monitor.size();
        x >= at.x && y >= at.y && x < at.x + size.width as i32 && y < at.y + size.height as i32
    })
}

impl Shell {
    fn note_placed(&mut self, window: &Window) {
        if window.fullscreen().is_some() || window.is_minimized() == Some(true) {
            return;
        }
        let maximized = window.is_maximized();
        let mut placed = self.placed.unwrap_or_default();
        placed.maximized = maximized;
        if !maximized {
            let size = window.inner_size();
            if size.width >= MIN_PLACED.0 && size.height >= MIN_PLACED.1 {
                placed.w = size.width;
                placed.h = size.height;
            }
            if let Ok(at) = window.outer_position() {
                placed.x = at.x;
                placed.y = at.y;
            }
        }
        if placed.w >= MIN_PLACED.0 && placed.h >= MIN_PLACED.1 {
            self.placed = Some(placed);
        }
    }

    fn place_pane(
        &self,
        event_loop: &ActiveEventLoop,
        window: &Window,
        place: Place,
        kept: Option<crate::config::state::Window>,
    ) {
        use crate::window::place::{self as placing, Rect};
        let size = window.outer_size();
        let size = (size.width as i32, size.height as i32);
        let main = self.window.as_deref();
        let at = match (place, kept) {
            (Place::Kept(_), Some(placed)) => {
                let wanted = Rect {
                    x: placed.x,
                    y: placed.y,
                    w: size.0,
                    h: size.1,
                };
                placing::monitor_for(event_loop, wanted).map_or((wanted.x, wanted.y), |monitor| {
                    placing::inside(wanted, monitor)
                })
            }
            (Place::Kept(_), None) => {
                let Some(at) = main.and_then(|main| main.inner_position().ok()) else {
                    return;
                };
                let wanted = Rect {
                    x: at.x,
                    y: at.y,
                    w: size.0,
                    h: size.1,
                };
                placing::monitor_for(event_loop, wanted).map_or((wanted.x, wanted.y), |monitor| {
                    placing::inside(wanted, monitor)
                })
            }
            (Place::Centred, _) => {
                let Some(owner) = main.and_then(placing::outer) else {
                    return;
                };
                let work = main.and_then(placing::work_area).unwrap_or(owner);
                placing::centred(owner, size, work)
            }
        };
        window.set_outer_position(winit::dpi::PhysicalPosition::new(at.0, at.1));
    }

    fn keep_pane_place(&mut self, id: WindowId) {
        let Some(name) = self.panes_kept.remove(&id) else {
            return;
        };
        let Some(window) = self.panes_windows.get(&id) else {
            return;
        };
        if window.is_minimized() == Some(true) {
            return;
        }
        let maximized = window.is_maximized();
        let size = window.inner_size();
        let at = window.outer_position().ok();
        crate::config::update_state(move |state| {
            let placed = state.panes.entry(name.to_owned()).or_default();
            placed.maximized = maximized;
            if !maximized && let Some(at) = at {
                placed.x = at.x;
                placed.y = at.y;
                placed.w = size.width;
                placed.h = size.height;
            }
        });
    }

    fn dropped(&self, path: &std::path::Path) {
        if crate::window::opening::playable(path) {
            let _ = self.to_stage.send(ToStage::Act(Act::Open(path.to_owned())));
        } else {
            tracing::info!(
                "{} は TS でも MMT/TLV でもない（内容で判定）",
                path.display()
            );
        }
    }

    fn save_placed(&mut self, window: &Window) {
        self.note_placed(window);
        if let Some(placed) = self.placed {
            crate::config::update_state(move |state| state.window = Some(placed));
        }
    }
}

#[cfg(windows)]
fn with_owner(
    attributes: winit::window::WindowAttributes,
    main: &Window,
) -> winit::window::WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    match crate::window::win32::raw_hwnd(main) {
        Some(hwnd) => attributes.with_owner_window(hwnd),
        None => attributes,
    }
}

#[cfg(not(windows))]
fn with_owner(
    attributes: winit::window::WindowAttributes,
    _main: &Window,
) -> winit::window::WindowAttributes {
    attributes
}

/// X11 は winit が出すボタンを選べないので、`_MOTIF_WM_HINTS` の functions で伝える（表示する前に）。
/// 従うかはウィンドウマネージャー次第。decorations など他の欄は winit が書いた物を残す。
#[cfg(target_os = "linux")]
fn x11_buttons(window: &Window, buttons: winit::window::WindowButtons, resizable: bool) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::WindowButtons;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    const HINTS_FUNCTIONS: u32 = 1;
    const FUNC_RESIZE: u32 = 1 << 1;
    const FUNC_MOVE: u32 = 1 << 2;
    const FUNC_MINIMIZE: u32 = 1 << 3;
    const FUNC_MAXIMIZE: u32 = 1 << 4;
    const FUNC_CLOSE: u32 = 1 << 5;
    if buttons == WindowButtons::all() {
        return;
    }
    let id = match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Xlib(handle)) => handle.window as u32,
        Ok(RawWindowHandle::Xcb(handle)) => handle.window.get(),
        _ => return,
    };
    let set = || -> Result<(), Box<dyn std::error::Error>> {
        let (connection, _) = x11rb::connect(None)?;
        let atom = connection
            .intern_atom(false, b"_MOTIF_WM_HINTS")?
            .reply()?
            .atom;
        let had = connection
            .get_property(false, id, atom, atom, 0, 5)?
            .reply()?;
        let mut hints = [0u32; 5];
        for (slot, value) in hints.iter_mut().zip(had.value32().into_iter().flatten()) {
            *slot = value;
        }
        let mut functions = FUNC_MOVE;
        for (button, function) in [
            (WindowButtons::CLOSE, FUNC_CLOSE),
            (WindowButtons::MINIMIZE, FUNC_MINIMIZE),
            (WindowButtons::MAXIMIZE, FUNC_MAXIMIZE),
        ] {
            if buttons.contains(button) {
                functions |= function;
            }
        }
        if resizable {
            functions |= FUNC_RESIZE;
        }
        hints[0] |= HINTS_FUNCTIONS;
        hints[1] = functions;
        connection.change_property32(PropMode::REPLACE, id, atom, atom, &hints)?;
        connection.flush()?;
        Ok(())
    };
    if let Err(error) = set() {
        tracing::debug!("_MOTIF_WM_HINTS を設定できない: {error}");
    }
}

#[cfg(not(target_os = "linux"))]
fn x11_buttons(_window: &Window, _buttons: winit::window::WindowButtons, _resizable: bool) {}

/// ウィンドウのアイコン。Windows は exe に埋め込んだ物（`deck/build.rs`）、X11 は 64px のサイズ。
/// Wayland は .desktop の物が出る。
#[cfg(windows)]
fn with_icon(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use windows::Win32::UI::WindowsAndMessaging::{SM_CXICON, SM_CXSMICON};
    use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
    let sized = |metric| {
        let side = crate::window::win32::system_metric(metric).max(16) as u32;
        winit::window::Icon::from_resource(1, Some(winit::dpi::PhysicalSize::new(side, side))).ok()
    };
    attributes
        .with_window_icon(sized(SM_CXSMICON))
        .with_taskbar_icon(sized(SM_CXICON))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn with_icon(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    static PNG: &[u8] = include_bytes!("../../linux/icons/hicolor/64x64/apps/zerowave.png");
    let icon = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .ok()
        .and_then(|picture| {
            let picture = picture.to_rgba8();
            let (width, height) = picture.dimensions();
            winit::window::Icon::from_rgba(picture.into_raw(), width, height).ok()
        });
    // Wayland・X11 とも .desktop（`linux/zerowave.desktop`）と同じ名前に。
    #[cfg(target_os = "linux")]
    let attributes = {
        use winit::platform::wayland::WindowAttributesExtWayland;
        attributes.with_name("zerowave", "zerowave")
    };
    attributes.with_window_icon(icon)
}

/// macOS は Dock のアイコン。素の実行ファイルでは汎用の物になるので、ここで置く。
#[cfg(target_os = "macos")]
fn with_icon(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    attributes
}

#[cfg(target_os = "macos")]
fn dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    static PNG: &[u8] = include_bytes!("../../icon/dock.png");
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    // SAFETY: メインスレッドで、有効な画像を渡す。
    unsafe { NSApplication::sharedApplication(main).setApplicationIconImage(Some(&image)) };
}

/// auto で GLES を選んだときは EGL を一度開いてみて、開けなければ wgpu に戻る（ソフトウェアのアダプターでも描ける）。
/// `--render gles` と指定されたときは戻らない。GLES で描くなら真。
#[cfg(all(unix, not(target_os = "macos")))]
fn gles_or_wgpu(window: &Arc<Window>, boot: &mut Boot) -> bool {
    if !boot.stage.choices.auto_gles {
        return true;
    }
    match crate::draw::gles::probe(window) {
        Ok(()) => true,
        Err(why) => {
            tracing::warn!("GLES 2.0 を開けないので、wgpu でレンダリングする: {why}");
            boot.stage.choices.auto_gles = false;
            let instance = crate::draw::instance_of(boot.stage.choices.backend);
            boot.stage.instance = Some(instance.clone());
            boot.instance = Some(instance);
            false
        }
    }
}

impl ApplicationHandler<Wake> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = with_icon(
            Window::default_attributes()
                .with_title("ZeroWave")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0)),
        );
        #[cfg(target_os = "macos")]
        dock_icon();
        let remembered = crate::config::load_state()
            .window
            .filter(|placed| placed.w >= MIN_PLACED.0 && placed.h >= MIN_PLACED.1);
        if let Some(placed) = remembered {
            attributes = attributes
                .with_inner_size(winit::dpi::PhysicalSize::new(placed.w, placed.h))
                .with_maximized(placed.maximized);
            if on_a_monitor(event_loop, &placed) {
                attributes =
                    attributes.with_position(winit::dpi::PhysicalPosition::new(placed.x, placed.y));
            }
            self.placed = Some(placed);
        }
        // `--render gles`（auto で GLES を選んだときも）。Wayland では映像をサブサーフェスに載せ、ウィンドウを透明にする。
        // 不透明のウィンドウだとコンポジターが抜いた所も黒で埋める。
        #[cfg(all(unix, not(target_os = "macos")))]
        let gles = self
            .boot
            .as_ref()
            .is_some_and(|boot| boot.instance.is_none() && boot.stage.choices.draws_gles());
        #[cfg(target_os = "linux")]
        let attributes = {
            use winit::platform::wayland::ActiveEventLoopExtWayland;
            attributes.with_transparent(gles && event_loop.is_wayland())
        };
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                tracing::warn!("ウィンドウを開けない: {error}");
                event_loop.exit();
                return;
            }
        };
        crate::window::place::set_main(&window);
        #[cfg(all(unix, not(target_os = "macos")))]
        let gles = gles
            && self
                .boot
                .as_mut()
                .is_none_or(|boot| gles_or_wgpu(&window, boot));
        #[cfg(target_os = "linux")]
        {
            self.wayland_drop = crate::window::wayland_drop::WaylandDrop::of(&window);
        }
        let refresh = window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz());
        match refresh {
            Some(hz) => tracing::info!("リフレッシュレート {:.3} Hz", f64::from(hz) / 1000.0),
            None => tracing::info!("リフレッシュレートが不明"),
        }
        let _ = self.to_stage.send(ToStage::Display(refresh.unwrap_or(0)));
        self.tell_monitor(&window);
        // OS のダーク／ライト設定はウィンドウの外観を設定する前に問い合わせる（設定したあとは設定した値を返す）。
        self.os_theme = system_theme().or(window.theme());
        // GLES 2.0 の環境は大きなテクスチャを扱えない。フォントのテクスチャはこのサイズに収める。
        #[cfg(all(unix, not(target_os = "macos")))]
        let max_texture_side = gles.then_some(2048);
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let max_texture_side = None;
        self.state = Some(egui_winit::State::new(
            self.context.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            max_texture_side,
        ));
        egui_winit::update_viewport_info(&mut self.viewport, &self.context, &window, true);
        self.window = Some(window.clone());
        self.wear();
        self.deck.attach_menus();
        let size = window.inner_size();
        let _ = self
            .to_stage
            .send(ToStage::Resized((size.width, size.height)));
        let Some(mut boot) = self.boot.take() else {
            return;
        };
        boot.stage.size = (size.width, size.height);
        boot.stage.refresh = refresh.unwrap_or(0);
        if let Some(instance) = boot.instance.take() {
            match instance.create_surface(window.clone()) {
                Ok(surface) => boot.stage.surface = Some(surface),
                Err(error) => tracing::warn!("サーフェスを作成できない: {error}"),
            }
            // 画面を別のスワップチェーンへ（`ZW_OVERLAY=1` のときだけ）。合成のビジュアルを使うと映像が
            // 独立フリップから DWM の合成に落ちるため。
            #[cfg(target_os = "windows")]
            if std::env::var_os("ZW_OVERLAY").is_some()
                && let Some(hwnd) = crate::window::win32::raw_hwnd(&window)
            {
                match crate::draw::composition::Overlay::open(&instance, hwnd) {
                    Ok(overlay) => boot.stage.overlay = Some(overlay),
                    Err(error) => {
                        tracing::warn!("画面用の別のスワップチェーンを開けない: {error}")
                    }
                }
            }
            self.instance = Some(instance);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if gles {
            let vsync = boot.stage.choices.present != crate::choice::Present::Immediate;
            match crate::draw::gles::open_gles(&window, vsync) {
                Some(painter) => {
                    boot.stage.painter = Some(painter);
                    self.gles = true;
                }
                None => {
                    event_loop.exit();
                    return;
                }
            }
        }
        let stage = Stage::new(boot.stage);
        match ending::Thread::spawn("stage", "再生", move |_told| stage.play()) {
            Ok(handle) => self.stage = Some(handle),
            Err(error) => {
                tracing::warn!("再生スレッドを起動できない: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            // 閉じた別ウィンドウの参照を手放す（持ったままだとウィンドウが消えない）。テキストコントロールが先。
            if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
                self.keep_pane_place(id);
                self.texts.remove(&id);
                self.panes_windows.remove(&id);
                self.panes_presented.remove(&id);
            }
            // ドロップした位置を先に通知する（ドラッグしている間はカーソルの動きが届かない）。
            if matches!(event, WindowEvent::DroppedFile(_)) {
                let at = self
                    .panes_windows
                    .get(&id)
                    .and_then(|pane| crate::window::cursor::inside(pane));
                let _ = self.to_panes.send(ToPanes::DropAt(id, at));
            }
            let resized = match event {
                WindowEvent::Resized(size) => Some((size.width, size.height)),
                _ => None,
            };
            let _ = self.to_panes.send(ToPanes::Event(id, event));
            if let Some(size) = resized {
                self.keep_pane_up(id, size);
            }
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(state) = self.state.as_mut() {
            // 消費されたかは見ない（`Deck::keys` と `Ui::interact` が自分で判じる）。
            let _ = state.on_window_event(&window, &event);
        }
        match &event {
            WindowEvent::Resized(size) => {
                let _ = self
                    .to_stage
                    .send(ToStage::Resized((size.width, size.height)));
                self.keep_up((size.width, size.height));
                self.note_placed(&window);
            }
            WindowEvent::Occluded(hidden) => {
                let _ = self.to_stage.send(ToStage::Hidden(*hidden));
            }
            // macOS では受け取らない（外観を設定するたびに winit が出す）。
            #[cfg(not(target_os = "macos"))]
            WindowEvent::ThemeChanged(theme) => self.os_theme_changed(*theme),
            WindowEvent::CloseRequested => {
                if self.closing.is_none() && self.deck.asks_before_quit() {
                    let _ = self.to_stage.send(ToStage::Quit);
                    return;
                }
                if self.closing.is_none() {
                    self.save_placed(&window);
                    let open: Vec<WindowId> = self.panes_kept.keys().copied().collect();
                    for id in open {
                        self.keep_pane_place(id);
                    }
                    let _ = self.to_stage.send(ToStage::Quit);
                    // 閉じたように見せて、終了処理は裏で（[`Shell::closing`]）。
                    window.set_visible(false);
                    let by = Instant::now() + ending::PATIENCE * 2;
                    self.closing = Some(by);
                    event_loop.set_control_flow(ControlFlow::WaitUntil(by));
                }
                return;
            }
            // 掴んで動かしている間、コンポジターはサーフェスを返さない。別のモニターへ移ったかもしれないのでモニターも問い合わせ直す。
            WindowEvent::Moved(_) => {
                self.deck.moved(&self.to_stage);
                self.tell_monitor(&window);
                self.note_placed(&window);
            }
            WindowEvent::Focused(true) => self.tell_monitor(&window),
            WindowEvent::DroppedFile(path) => self.dropped(path),
            _ => {}
        }
        self.compose();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: Wake) {
        self.about_to_wait(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.take_requests(Some(event_loop));
        #[cfg(target_os = "linux")]
        if let Some(first) = self
            .wayland_drop
            .as_mut()
            .and_then(|drop| drop.take().into_iter().next())
        {
            self.dropped(&first);
        }
        self.keep.follow_default();
        let pressed = match self.window.clone() {
            Some(window) => self.deck.take_menus(&window),
            None => false,
        };
        let fresh =
            self.pump() || pressed || self.deck.broadcast_changed() || self.deck.layers_changed();
        if self.done {
            event_loop.exit();
            return;
        }
        if let Some(by) = self.closing {
            if Instant::now() >= by {
                tracing::info!(
                    "再生スレッドが {:?} 以内に終了しない。先にウィンドウを閉じる",
                    ending::PATIENCE * 2
                );
                event_loop.exit();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(by));
            }
            return;
        }
        let settled = self.deck.settle(&self.to_stage);
        let told = self.deck.due();
        let next = match (self.next, told) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.next = next;
        let due = next.is_some_and(|next| next <= Instant::now());
        if fresh || due || settled {
            self.compose();
        }
        event_loop.set_control_flow(match self.next {
            Some(next) => ControlFlow::WaitUntil(next),
            None => ControlFlow::Wait,
        });
    }
}

#[cfg(target_os = "windows")]
mod modal {
    use std::cell::Cell;
    use std::sync::Arc;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        KillTimer, SetTimer, WM_INITMENUPOPUP, WM_UNINITMENUPOPUP,
    };
    use winit::window::Window;

    use super::Shell;
    use crate::window::win32::{self, Original};

    const EVERY_MS: u32 = 16;
    const ID: usize = 0x5a57;

    thread_local! {
        static SHELL: Cell<*mut Shell> = const { Cell::new(std::ptr::null_mut()) };
        static ORIGINAL: Cell<Option<Original>> = const { Cell::new(None) };
    }

    unsafe extern "system" fn hook(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_INITMENUPOPUP {
            let shell = SHELL.with(Cell::get);
            if !shell.is_null() {
                // SAFETY: `tick` と同じ。
                unsafe { (*shell).init_menu_popup(wparam.0 as isize) };
            }
        }
        if message == WM_UNINITMENUPOPUP {
            let shell = SHELL.with(Cell::get);
            if !shell.is_null() {
                // SAFETY: `tick` と同じ。
                unsafe { (*shell).deck.menu_closed(wparam.0 as isize) };
            }
        }
        match ORIGINAL.with(Cell::get) {
            Some(original) => original.call(hwnd, message, wparam, lparam),
            None => LRESULT(0),
        }
    }

    unsafe extern "system" fn tick(_: HWND, _: u32, _: usize, _: u32) {
        let shell = SHELL.with(Cell::get);
        if shell.is_null() {
            return;
        }
        // SAFETY: `start` が置き、`Ticking` が落ちるまで生きている `Shell`。`&mut Shell` の借用は他に無い。
        unsafe { (*shell).tick_while_modal() };
    }

    /// このスレッドで表示中のポップアップメニューを再描画する。`resize` なら先にサイズを測り直させる。
    /// `MN_SIZEWINDOW`（0x01E2）は文書化されていない。閉じて開き直すと強調が先頭へ飛ぶので採らない。
    pub fn refresh_popups(resize: bool) {
        use windows::Win32::Graphics::Gdi::{
            RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowExW, GetWindowThreadProcessId, IsWindowVisible, SendMessageW,
        };
        use windows::core::{PCWSTR, w};
        const MN_SIZEWINDOW: u32 = 0x01e2;
        let shell = SHELL.with(Cell::get);
        if shell.is_null() {
            return;
        }
        // SAFETY: 出している最中の `Shell`（`tick` と同じ）。
        let Some(window) = (unsafe { (*shell).window.clone() }) else {
            return;
        };
        let Some(own) = win32::hwnd(&window) else {
            return;
        };
        // SAFETY: ウィンドウの列挙と、サイズの再計算・再描画の要求だけ。
        unsafe {
            let thread = GetWindowThreadProcessId(own, None);
            let mut after = HWND::default();
            while let Ok(found) = FindWindowExW(None, Some(after), w!("#32768"), PCWSTR::null()) {
                if found.0.is_null() {
                    break;
                }
                if GetWindowThreadProcessId(found, None) == thread
                    && IsWindowVisible(found).as_bool()
                {
                    if resize {
                        let _ = SendMessageW(found, MN_SIZEWINDOW, None, None);
                    }
                    let _ = RedrawWindow(
                        Some(found),
                        None,
                        None,
                        RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW,
                    );
                }
                after = found;
            }
        }
    }

    pub struct Ticking {
        hwnd: HWND,
    }

    impl Drop for Ticking {
        fn drop(&mut self) {
            SHELL.with(|shell| shell.set(std::ptr::null_mut()));
            if let Some(original) = ORIGINAL
                .with(Cell::take)
                .filter(|original| original.is_set())
            {
                original.restore(self.hwnd);
            }
            // SAFETY: 自分で掛けたタイマーを、同じウィンドウ・同じ ID で外す。
            let _ = unsafe { KillTimer(Some(self.hwnd), ID) };
        }
    }

    pub fn start(shell: *mut Shell, window: &Arc<Window>) -> Option<Ticking> {
        let hwnd = win32::hwnd(window)?;
        SHELL.with(|slot| slot.set(shell));
        // SAFETY: 有効なウィンドウの HWND に、このモジュールの関数でタイマーを掛ける。
        let set = unsafe { SetTimer(Some(hwnd), ID, EVERY_MS, Some(tick)) };
        if set == 0 {
            SHELL.with(|slot| slot.set(std::ptr::null_mut()));
            return None;
        }
        // メニューの表示中だけウィンドウプロシージャを差し替える。
        let original = win32::subclass(hwnd, hook);
        ORIGINAL.with(|slot| slot.set(Some(original)));
        Some(Ticking { hwnd })
    }
}

#[cfg(target_os = "macos")]
mod modal {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::Arc;

    use objc2_core_foundation::{
        CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext,
        kCFRunLoopCommonModes,
    };
    use winit::window::Window;

    use super::Shell;

    const EVERY: f64 = 0.016;

    thread_local! {
        static SHELL: Cell<*mut Shell> = const { Cell::new(std::ptr::null_mut()) };
    }

    unsafe extern "C-unwind" fn tick(_: *mut CFRunLoopTimer, _: *mut c_void) {
        let shell = SHELL.with(Cell::get);
        if shell.is_null() {
            return;
        }
        // SAFETY: `start` が置き、`Ticking` が落ちるまで生きている `Shell`。`&mut Shell` の借用は他に無い。
        unsafe { (*shell).tick_while_modal() };
    }

    pub struct Ticking {
        timer: CFRetained<CFRunLoopTimer>,
    }

    impl Drop for Ticking {
        fn drop(&mut self) {
            SHELL.with(|shell| shell.set(std::ptr::null_mut()));
            self.timer.invalidate();
        }
    }

    pub fn start(shell: *mut Shell, _window: &Arc<Window>) -> Option<Ticking> {
        let main = CFRunLoop::main()?;
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        SHELL.with(|slot| slot.set(shell));
        // SAFETY: 関数はこのモジュールにあり、コンテキストは使わない（`Shell` は thread_local）。
        let timer = unsafe {
            CFRunLoopTimer::new(
                None,
                CFAbsoluteTimeGetCurrent() + EVERY,
                EVERY,
                0,
                0,
                Some(tick),
                &raw mut context,
            )
        };
        let Some(timer) = timer else {
            SHELL.with(|slot| slot.set(std::ptr::null_mut()));
            return None;
        };
        // SAFETY: 静的な名前。
        main.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
        Some(Ticking { timer })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_portal_answer_becomes_a_theme() {
        assert_eq!(
            scheme_of("(<<uint32 1>>,)\n"),
            Some(winit::window::Theme::Dark)
        );
        assert_eq!(scheme_of("v u 2\n"), Some(winit::window::Theme::Light));
        assert_eq!(scheme_of("(<<uint32 0>>,)\n"), None);
    }
}
