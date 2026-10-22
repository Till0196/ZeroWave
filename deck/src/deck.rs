// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面の中身とウィンドウへの要求。ビルドするのはメインスレッド、表示するのは再生スレッド（`stage`）。
//! 映像はここではレンダリングしない（先にレンダリングされた映像の上に `LoadOp::Load` で重ねる）。

mod build;
mod commands;
mod input;
mod layers;
mod panel;
mod size;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};
use std::time::{Duration, Instant};
use zwcore::guide::language_name;

use winit::window::{Fullscreen, Window};

use crate::choice::{Knobs, WheelDoes};
use crate::draw::scene::{Backdrop, Scene};
use crate::threads::link::{Act, FromStage, ToStage};
use crate::threads::panes::ToPanes;
use crate::ui;
use crate::window;
use crate::window::menu;
use crate::window::sub::Which;
use zwcore::data::{Shown, ToBroadcast};

const DATA_ASKED_FOR: Duration = Duration::from_secs(20);

pub struct Deck {
    to_stage: Sender<ToStage>,
    from_stage: Receiver<FromStage>,
    to_panes: Sender<ToPanes>,
    scene: Scene,
    mini: bool,
    seek: ui::overlay::transport::State,
    bottom_until: Option<Instant>,
    top_until: Option<Instant>,
    pointer_seen: Option<Instant>,
    cursor_hidden: bool,
    on_top: bool,
    /// ウィンドウに当てた前面表示。
    told_on_top: Option<bool>,
    full_screen: bool,
    /// 再生スレッドへ知らせた全画面表示の状態。
    told_full: bool,
    bar: f32,
    told_area: Option<(u32, u32, u32, u32)>,
    to_broadcast: SyncSender<ToBroadcast>,
    shown_broadcast: zwcore::data::Latest,
    shown_captions: zwcore::data::LatestCaptions,
    shown_layers: zwcore::layers::Latest,
    /// 絵の並びのレイヤーを再生スレッドが GPU で重ねられるか。
    present: zwcore::layers::Present,
    layer_textures: std::collections::BTreeMap<String, (usize, egui::TextureHandle)>,
    /// 字幕の言語の希望：0 切、1 第 1 言語、2 第 2 言語。
    captions_wanted: u8,
    file: crate::config::File,
    copy_wanted: Option<String>,
    capture_wanted: Option<bool>,
    status_height: f32,
    capture_at: Vec<std::time::Instant>,
    settings_at_start: Option<ui::panes::settings::Page>,
    favourites_at_start: bool,
    about_at_start: bool,
    composer: Option<crate::capture::Composer>,
    status_menu: Option<&'static [menu::Item]>,
    status_menu_at: Option<egui::Pos2>,
    told_layers: Option<Vec<(String, String)>>,
    broadcast_textures: ui::panes::databroadcast::Textures,
    told_inset: Option<Option<[u32; 5]>>,
    told_page_area: Option<(u32, u32)>,
    broadcast_on: bool,
    data_asked: Option<(Instant, Option<String>)>,
    one_seg_popup: bool,
    drawn_broadcast: Option<usize>,
    drawn_layers: Option<usize>,
    theme: ui::theme::Theme,
    fitted: bool,
    knobs: Knobs,
    bindings: crate::keys::Bindings,
    strip_rect: Option<egui::Rect>,
    passing: std::cell::Cell<bool>,
    pending: std::cell::RefCell<Vec<ToStage>>,
    pending_picks: Vec<menu::Picked>,
    context_wanted: bool,
    /// OS のメニューが出ている。その間はポインタを消さない。
    menu_open: bool,
    bar_wanted: Option<(usize, (f64, f64))>,
    sized_at: Option<Instant>,
    sizing_from: Option<(u32, u32)>,
    size_was: Option<(u32, u32)>,
    /// 大きさを変えている間に Ctrl が押された（`keep_aspect` を逆にする）。
    ctrl_sizing: bool,
    moving: Option<Instant>,
    grabbed: bool,
    menus: menu::Menus,
    panel: crate::config::state::Panel,
    panel_changed: Option<Instant>,
    window_rows: Option<u32>,
    told_rows: Option<u32>,
    panel_tabs: Option<(usize, Vec<ui::guide::panel::Tab>)>,
    plugin_ui: crate::plugin_ui::Host,
    plugin_scrolls: std::collections::BTreeMap<String, (f32, f32)>,
    plugin_told: usize,
}

impl std::fmt::Debug for Deck {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Deck")
            .field("mini", &self.mini)
            .field("grabbed", &self.grabbed)
            .finish_non_exhaustive()
    }
}

impl Deck {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        to_stage: Sender<ToStage>,
        from_stage: Receiver<FromStage>,
        to_panes: Sender<ToPanes>,
        to_broadcast: SyncSender<ToBroadcast>,
        shown_broadcast: zwcore::data::Latest,
        shown_captions: zwcore::data::LatestCaptions,
        shown_layers: zwcore::layers::Latest,
        present: zwcore::layers::Present,
        fit_window: bool,
        knobs: Knobs,
    ) -> Self {
        let file = crate::config::load_plain();
        let captions_wanted = shown_number(&file.caption.shown);
        let _ = to_broadcast.try_send(ToBroadcast::Captions(captions_wanted));
        let _ = to_broadcast.try_send(ToBroadcast::Superimpose(shown_number(
            &file.caption.superimpose,
        )));
        let _ = to_broadcast.try_send(forced_of(&file.caption));
        let on_top = file.general.always_on_top;
        Self {
            knobs,
            bindings: crate::keys::Bindings::of(&file.keys),
            file,
            copy_wanted: None,
            capture_wanted: None,
            status_height: ui::theme::status::HEIGHT,
            capture_at: capture_times(),
            favourites_at_start: std::env::var("ZW_OPEN_SETTINGS").as_deref() == Ok("favourites"),
            about_at_start: std::env::var("ZW_OPEN_SETTINGS").as_deref() == Ok("about"),
            settings_at_start: match std::env::var("ZW_OPEN_SETTINGS").as_deref() {
                Ok("caption") => Some(ui::panes::settings::Page::Caption),
                Ok("capture") => Some(ui::panes::settings::Page::Capture),
                Ok("recording") => Some(ui::panes::settings::Page::Recording),
                _ => None,
            },
            composer: None,
            status_menu: None,
            status_menu_at: None,
            told_layers: None,
            strip_rect: None,
            to_stage,
            from_stage,
            to_panes,
            to_broadcast,
            shown_broadcast,
            shown_captions,
            shown_layers,
            present,
            layer_textures: std::collections::BTreeMap::new(),
            captions_wanted,
            broadcast_textures: ui::panes::databroadcast::Textures::default(),
            told_inset: None,
            told_page_area: None,
            broadcast_on: true,
            data_asked: None,
            one_seg_popup: false,
            drawn_broadcast: None,
            drawn_layers: None,
            scene: Scene::default(),
            mini: false,
            seek: ui::overlay::transport::State::default(),
            bottom_until: None,
            top_until: None,
            pointer_seen: None,
            cursor_hidden: false,
            on_top,
            told_on_top: None,
            full_screen: false,
            told_full: false,
            bar: 0.0,
            told_area: None,
            theme: ui::theme::Theme::of(ui::theme::Mode::default()),
            fitted: !fit_window,
            passing: std::cell::Cell::new(false),
            pending: std::cell::RefCell::new(Vec::new()),
            pending_picks: Vec::new(),
            context_wanted: false,
            menu_open: false,
            bar_wanted: None,
            sized_at: None,
            sizing_from: None,
            size_was: None,
            ctrl_sizing: false,
            moving: None,
            grabbed: false,
            menus: menu::Menus::none(),
            panel: crate::config::load_state().panel,
            panel_changed: None,
            window_rows: None,
            told_rows: None,
            panel_tabs: None,
            plugin_ui: crate::plugin_ui::Host::default(),
            plugin_scrolls: std::collections::BTreeMap::new(),
            plugin_told: 0,
        }
    }

    pub fn attach_menus(&mut self) {
        self.menus = menu::Menus::attach();
    }

    pub fn take_menus(&mut self, window: &Arc<Window>) -> bool {
        let mut pressed = false;
        while let Some(picked) = self.menus.taken() {
            self.pick(picked, window);
            pressed = true;
        }
        pressed
    }

    pub(crate) fn say(&self, what: ToStage) {
        // egui は一回のパスを捨ててビルドし直すことがあり、捨てたパスでも `clicked()` は真なので
        // ここでは送らない。最後のパスの分だけ [`Deck::flush`] が送る。
        if self.passing.get() {
            self.pending.borrow_mut().push(what);
            return;
        }
        let _ = self.to_stage.send(what);
    }

    pub fn flush(&mut self, window: &Arc<Window>) -> Option<menu::Prepared> {
        self.passing.set(false);
        for what in self.pending.borrow_mut().drain(..) {
            let _ = self.to_stage.send(what);
        }
        let picks: Vec<menu::Picked> = std::mem::take(&mut self.pending_picks);
        for picked in picks {
            self.pick(picked, window);
        }
        if std::mem::take(&mut self.context_wanted) {
            let filling = self.filling();
            let dark = self.theme.mode == ui::theme::Mode::Dark;
            return self.menus.prepare_context(&filling, dark);
        }
        if menu::Menus::has_native_context()
            && let Some(items) = self.status_menu.take()
        {
            let filling = self.filling();
            let dark = self.theme.mode == ui::theme::Mode::Dark;
            return self.menus.prepare_items(items, &filling, dark);
        }
        if let Some((index, at)) = self.bar_wanted.take() {
            let filling = self.filling();
            let dark = self.theme.mode == ui::theme::Mode::Dark;
            return self.menus.prepare_bar(index, &filling, dark, at);
        }
        None
    }

    #[cfg(target_os = "windows")]
    pub fn refill_open_menu(&mut self, hmenu: isize) -> bool {
        let filling = self.filling();
        self.menus.refill_open(hmenu, &filling)
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn update_open_menu(&mut self) -> menu::native::Updated {
        let filling = self.filling();
        self.menus.update_open(&filling)
    }

    #[cfg(target_os = "windows")]
    pub fn menu_closed(&mut self, hmenu: isize) {
        self.menus.closed_open(hmenu);
    }

    pub fn keep_menu(&mut self, prepared: menu::Prepared) {
        self.menus.keep(prepared);
    }

    pub fn pump(&mut self, _to_panes: &Sender<ToPanes>) -> (bool, bool) {
        let mut fresh = false;
        loop {
            match self.from_stage.try_recv() {
                Ok(FromStage::Scene(scene)) => {
                    self.scene = *scene;
                    self.forward_panel();
                    let filling = self.filling();
                    self.menus.refill(&filling);
                    fresh = true;
                }
                Ok(FromStage::Done) => return (fresh, true),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return (fresh, true),
            }
        }
        (fresh, false)
    }

    pub fn due(&self) -> Option<Instant> {
        let at = self.scene.due(self.scene.now)?;
        let ticks = at.saturating_sub(self.scene.now).max(0);
        Instant::now().checked_add(Duration::from_secs_f64(
            ticks as f64 / zwcore::engine::clock::HZ as f64,
        ))
    }

    pub fn wear(&mut self, mode: ui::theme::Mode, context: &egui::Context) {
        self.theme = ui::theme::Theme::of(mode);
        ui::style(context, mode);
    }

    /// ウィンドウが動いた。[`MOVE_APART`] の内に続けて来たときだけドラッグで動かしていると見る。
    pub fn moved(&mut self, to_stage: &Sender<ToStage>) {
        let now = Instant::now();
        let again = self
            .moving
            .is_some_and(|last| now.duration_since(last) < MOVE_APART);
        self.moving = Some(now);
        if again && !self.grabbed {
            self.grabbed = true;
            let _ = to_stage.send(ToStage::Grabbed(true));
        }
    }

    pub fn settle(&mut self, to_stage: &Sender<ToStage>) -> bool {
        let Some(last) = self.moving else {
            return false;
        };
        if last.elapsed() < SETTLE {
            return false;
        }
        self.moving = None;
        if !self.grabbed {
            return false;
        }
        self.grabbed = false;
        let _ = to_stage.send(ToStage::Grabbed(false));
        true
    }
}

const MOVE_APART: Duration = Duration::from_millis(150);

const SETTLE: Duration = Duration::from_millis(120);

const RATIO_SLACK: u32 = 2;

fn shown_number(spelled: &str) -> u8 {
    match spelled {
        "off" => 0,
        "second" => 2,
        _ => 1,
    }
}

/// 「表示しない」のときも強制表示のものを出すか。
fn forced_of(caption: &crate::config::file::Caption) -> ToBroadcast {
    ToBroadcast::Forced {
        caption: caption.forced,
        superimpose: caption.superimpose_forced,
    }
}

fn shown_spelling(number: u8) -> &'static str {
    match number {
        0 => "off",
        2 => "second",
        _ => "first",
    }
}

fn caption_label(n: u8, languages: &[[u8; 3]]) -> String {
    let name = languages
        .get(usize::from(n) - 1)
        .and_then(|iso| std::str::from_utf8(iso).ok())
        .map(language_name)
        .filter(|name| !name.is_empty())
        .map(|name| format!("：{name}"))
        .unwrap_or_default();
    format!("第{n}言語{name}")
}

/// メニューの字幕の項目。言語がわかれば「第1言語：日本語」、わかるまでは「第1言語」。
/// OS のメニューの幅は開いたときのままなので、わからないうちは言語名の分の幅を空白で取っておく。
fn caption_choice(n: u8, languages: &[[u8; 3]]) -> String {
    let label = caption_label(n, languages);
    if label.contains('：') {
        label
    } else {
        format!("{label}　　　　　　　　")
    }
}

/// メニューの字幕の項目と、チェックを付ける位置。言語の項目は放送が示す言語の数だけ出す（届くまでは
/// 第1言語だけ）。選んだ言語が無ければもう一方を表示するので、チェックもそちらに付ける（設定は変えない）。
fn caption_items(wanted: u8, languages: &[[u8; 3]]) -> (Vec<String>, usize) {
    let count = languages.len().clamp(1, 2) as u8;
    let items = std::iter::once(String::from("表示しない"))
        .chain((1..=count).map(|n| caption_choice(n, languages)))
        .collect();
    (items, usize::from(wanted.min(count)))
}

fn capture_times() -> Vec<std::time::Instant> {
    let started = std::time::Instant::now();
    std::env::var("ZW_CAPTURE_AT")
        .unwrap_or_default()
        .split(',')
        .filter_map(|seconds| seconds.trim().parse::<f64>().ok())
        // `from_secs_f64` は大きすぎる秒で panic する。
        .filter_map(|seconds| std::time::Duration::try_from_secs_f64(seconds.max(0.0)).ok())
        .filter_map(|after| started.checked_add(after))
        .collect()
}

#[cfg(test)]
mod caption_tests {
    use super::*;

    #[test]
    fn a_caption_choice_names_the_language_once_it_is_known() {
        assert_eq!(caption_choice(1, &[]), "第1言語　　　　　　　　");
        assert_eq!(caption_choice(1, &[*b"jpn"]), "第1言語：日本語");
        assert_eq!(caption_choice(2, &[*b"jpn"]), "第2言語　　　　　　　　");
        assert_eq!(caption_choice(2, &[*b"jpn", *b"eng"]), "第2言語：英語");
    }

    #[test]
    fn the_caption_menu_has_as_many_languages_as_the_broadcast() {
        // 届く前は第1言語だけ。
        assert_eq!(
            caption_items(1, &[]),
            (
                vec![
                    String::from("表示しない"),
                    String::from("第1言語　　　　　　　　")
                ],
                1
            )
        );
        // 一つなら第1言語だけ。第2言語を選んでいても、表示している第1言語にチェック。
        assert_eq!(
            caption_items(2, &[*b"jpn"]),
            (
                vec![String::from("表示しない"), String::from("第1言語：日本語")],
                1
            )
        );
        assert_eq!(caption_items(0, &[*b"jpn"]).1, 0);
        // 二つなら両方。
        assert_eq!(
            caption_items(2, &[*b"jpn", *b"eng"]),
            (
                vec![
                    String::from("表示しない"),
                    String::from("第1言語：日本語"),
                    String::from("第2言語：英語")
                ],
                2
            )
        );
    }
}
