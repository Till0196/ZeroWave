// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メインウィンドウとは別に開くウィンドウ（番組表、番組説明など）。レンダリングは [`crate::threads::panes`] のスレッドで行う。

use std::sync::Arc;
use std::time::Instant;

use winit::window::Window;
use zwcore::guide::Band;

use crate::ui::{self, Face};

pub enum Which {
    Guide,
    Search,
    Detail(Box<ui::guide::detail::Detail>),
    Stream,
    Decoder,
    Settings,
    OneSeg,
    Remote,
    Scan,
    About,
    Tuners,
    FiveOne,
    TwentyTwo,
    Plugin(String),
    PluginPage(String, String),
    Favourites,
    FontPick,
    CoaxdUpdate,
    ScanDelete,
    ScanCopy,
    Panel((f32, f32)),
}

impl std::fmt::Debug for Which {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Guide => "Guide",
            Self::Search => "Search",
            Self::Detail(_) => "Detail",
            Self::Stream => "Stream",
            Self::Decoder => "Decoder",
            Self::Settings => "Settings",
            Self::OneSeg => "OneSeg",
            Self::Remote => "Remote",
            Self::Scan => "Scan",
            Self::About => "About",
            Self::Tuners => "Tuners",
            Self::FiveOne => "FiveOne",
            Self::TwentyTwo => "TwentyTwo",
            Self::Plugin(_) => "Plugin",
            Self::PluginPage(..) => "PluginPage",
            Self::Favourites => "Favourites",
            Self::FontPick => "FontPick",
            Self::CoaxdUpdate => "CoaxdUpdate",
            Self::ScanDelete => "ScanDelete",
            Self::ScanCopy => "ScanCopy",
            Self::Panel(_) => "Panel",
        })
    }
}

impl Which {
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plugin(one), Self::Plugin(other)) => one == other,
            (Self::PluginPage(a, b), Self::PluginPage(c, d)) => a == c && b == d,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }

    pub fn title(&self, band: Band) -> String {
        match self {
            Self::Guide => band.guide_title().to_owned(),
            Self::Search => "番組検索".to_owned(),
            Self::Detail(_) => "番組説明".to_owned(),
            Self::Stream => "ストリームの情報".to_owned(),
            Self::Decoder => "デコーダの情報".to_owned(),
            Self::Settings => "設定".to_owned(),
            Self::OneSeg => "ワンセグ データ放送".to_owned(),
            Self::Remote => "リモコン".to_owned(),
            Self::Scan => "チャンネルスキャン".to_owned(),
            Self::About => "ZeroWave について".to_owned(),
            Self::Tuners => "チューナーの状態".to_owned(),
            Self::FiveOne => "5.1chの設定".to_owned(),
            Self::TwentyTwo => "22.2chの設定".to_owned(),
            Self::Plugin(name) => {
                let shown = crate::plugins::shown_name(name);
                if shown.ends_with(|c: char| c.is_ascii_alphanumeric()) {
                    format!("{shown} の設定")
                } else {
                    format!("{shown}の設定")
                }
            }
            Self::PluginPage(stem, _) => crate::plugin_ui::title_of(stem),
            Self::Favourites => "お気に入りの管理".to_owned(),
            Self::FontPick => ui::panes::settings::font_pick::TITLE.to_owned(),
            Self::CoaxdUpdate => "coaxd の更新".to_owned(),
            Self::ScanDelete => "スキャン結果の削除".to_owned(),
            Self::ScanCopy => "スキャン結果の複製".to_owned(),
            Self::Panel(_) => ui::guide::panel::WINDOW_TITLE.to_owned(),
        }
    }

    pub fn size(&self) -> (f32, f32) {
        match self {
            Self::Guide => (ui::theme::epg::WIDTH, ui::theme::epg::HEIGHT),
            Self::Search => (ui::theme::epg::FOUND_WIDTH, ui::theme::epg::FOUND_HEIGHT),
            Self::Detail(_) => (ui::theme::detail::WIDTH, ui::theme::detail::HEIGHT),
            Self::Stream => (ui::theme::stream::WIDTH, ui::theme::stream::HEIGHT),
            Self::Decoder => (ui::panes::decoder::WIDTH, ui::panes::decoder::HEIGHT),
            // デザインは OS のウィンドウとして作っているので、要求するサイズからタイトルバーの分を引く。
            Self::Settings => (
                ui::theme::settings::WIDTH,
                ui::theme::settings::HEIGHT - ui::theme::settings::BAR_HEIGHT,
            ),
            Self::OneSeg => (480.0, 960.0),
            Self::Remote => (ui::theme::remote::WIDTH, ui::theme::remote::HEIGHT),
            Self::Scan => (ui::panes::scan::WIDTH, ui::panes::scan::HEIGHT),
            Self::About => (ui::panes::about::WIDTH, ui::panes::about::HEIGHT),
            Self::Tuners => (
                ui::panes::settings::tuners::WIDTH,
                ui::panes::settings::tuners::HEIGHT,
            ),
            Self::TwentyTwo => (
                ui::panes::settings::surround::TWENTY_TWO_WIDTH,
                ui::panes::settings::surround::TWENTY_TWO_HEIGHT,
            ),
            Self::FiveOne => (
                ui::panes::settings::surround::WIDTH,
                ui::panes::settings::surround::HEIGHT,
            ),
            Self::Plugin(_) => (
                ui::panes::settings::plugin::WIDTH,
                ui::panes::settings::plugin::HEIGHT,
            ),
            Self::PluginPage(..) => (
                ui::panes::settings::plugin_page::WIDTH,
                ui::panes::settings::plugin_page::HEIGHT,
            ),
            Self::Favourites => (
                ui::panes::settings::favourites::WIDTH,
                ui::panes::settings::favourites::HEIGHT,
            ),
            Self::FontPick => (
                ui::panes::settings::font_pick::WIDTH,
                ui::panes::settings::font_pick::HEIGHT,
            ),
            Self::CoaxdUpdate => (
                ui::panes::settings::coaxd::WIDTH,
                ui::panes::settings::coaxd::HEIGHT,
            ),
            Self::ScanDelete => (
                ui::panes::settings::scan_results::DELETE_WIDTH,
                ui::panes::settings::scan_results::DELETE_HEIGHT,
            ),
            Self::ScanCopy => (
                ui::panes::settings::scan_results::COPY_WIDTH,
                ui::panes::settings::scan_results::COPY_HEIGHT,
            ),
            Self::Panel(size) => *size,
        }
    }

    pub fn buttons(&self) -> winit::window::WindowButtons {
        use winit::window::WindowButtons;
        // 最大化・最小化は大きくして見る番組表とワンセグだけ。
        match self {
            Self::Guide | Self::Search | Self::OneSeg => WindowButtons::all(),
            _ => WindowButtons::CLOSE,
        }
    }

    pub fn min_size(&self) -> Option<(f32, f32)> {
        match self {
            Self::Settings => Some((600.0, 400.0)),
            Self::Plugin(_) | Self::PluginPage(..) => Some((480.0, 320.0)),
            Self::Remote => Some((
                ui::theme::remote::WIDTH * 0.6,
                ui::theme::remote::HEIGHT * 0.6,
            )),
            _ => None,
        }
    }

    pub fn place(&self) -> crate::threads::link::Place {
        use crate::threads::link::Place;
        match self {
            Self::Settings => Place::Kept("settings"),
            Self::Guide => Place::Kept("guide"),
            Self::Search => Place::Kept("search"),
            Self::Stream => Place::Kept("stream"),
            Self::Decoder => Place::Kept("decoder"),
            Self::Favourites => Place::Kept("favourites"),
            Self::Panel(_) => Place::Kept("panel"),
            Self::Remote => Place::Kept("remote"),
            _ => Place::Centred,
        }
    }

    pub fn owned(&self) -> bool {
        !matches!(self, Self::Guide)
    }

    pub fn resizable(&self) -> bool {
        !matches!(
            self,
            Self::About | Self::CoaxdUpdate | Self::ScanDelete | Self::ScanCopy
        )
    }

    pub fn paper(&self) -> bool {
        true
    }

    pub fn show(&self, root: &mut egui::Ui, showing: &mut Showing<'_>) -> Option<Happened> {
        match self {
            Self::Guide => {
                ui::guide::epg::show(root, showing.listing, showing.jump_to, &showing.skins.epg)
                    .map(Happened::Epg)
            }
            Self::Search => {
                ui::guide::found::show(root, showing.finding, showing.listing, &showing.skins.epg)
                    .map(Happened::Found)
            }
            Self::Detail(detail) => {
                let shown = ui::guide::detail::show(root, detail, &showing.skins.detail);
                *showing.text = shown.native;
                match shown.pressed {
                    Some(ui::guide::detail::Pressed::Close) => Some(Happened::Close),
                    Some(ui::guide::detail::Pressed::Follow(follow)) => {
                        Some(Happened::Follow(follow))
                    }
                    None => None,
                }
            }
            Self::Stream => {
                let info = showing.info?;
                match ui::panes::stream::show(root, showing.stream, info, &showing.skins.stream)? {
                    ui::panes::stream::Pressed::Copy => {
                        Some(Happened::Copy(ui::panes::stream::written(info)))
                    }
                    ui::panes::stream::Pressed::Close => Some(Happened::Close),
                }
            }
            Self::Decoder => {
                let decoding = showing.decoding?;
                match ui::panes::decoder::show(root, decoding, &showing.skins.stream)? {
                    ui::panes::stream::Pressed::Copy => {
                        Some(Happened::Copy(ui::panes::decoder::written(decoding)))
                    }
                    ui::panes::stream::Pressed::Close => Some(Happened::Close),
                }
            }
            Self::Settings => {
                let pressed = ui::panes::settings::show(
                    root,
                    showing.settings_page,
                    showing.edited,
                    showing.loaded,
                    showing.guide,
                    crate::config::overridden(),
                    &showing.skins.settings,
                )?;
                Some(Happened::Settings(pressed))
            }
            Self::OneSeg => {
                let area = root.max_rect();
                match showing.broadcast {
                    Some(shown) => {
                        showing.textures.keep_only(shown);
                        ui::panes::databroadcast::popup(root, area, shown, showing.textures);
                    }
                    None => {
                        root.painter().rect_filled(area, 0.0, egui::Color32::BLACK);
                    }
                }
                let pressed: Vec<(egui::Key, egui::Modifiers)> = root.input(|input| {
                    input
                        .events
                        .iter()
                        .filter_map(|event| match event {
                            egui::Event::Key {
                                key,
                                pressed: true,
                                modifiers,
                                ..
                            } => Some((*key, *modifiers)),
                            _ => None,
                        })
                        .collect()
                });
                let keys = showing
                    .broadcast
                    .map(|shown| shown.keys.clone())
                    .unwrap_or_default();
                let takes = |group: &str| keys.iter().any(|g| g == group);
                pressed
                    .into_iter()
                    .find_map(|(key, modifiers)| {
                        ui::panes::databroadcast::remote_key(key, modifiers, &takes)
                    })
                    .map(Happened::Remote)
            }
            Self::Remote => ui::panes::remote::show(
                root,
                showing.remote,
                showing.band,
                showing.bands,
                &showing.skins.remote,
            )
            .map(Happened::Pressed),
            Self::Scan => {
                let shown = ui::panes::scan::show(root, showing.scan, &showing.skins.settings);
                *showing.text = shown.native;
                shown.pressed.map(Happened::Scan)
            }
            Self::FontPick => {
                let Some(pick) = showing.settings_page.font_pick.as_mut() else {
                    return Some(Happened::Close);
                };
                ui::panes::settings::font_pick::show(root, pick, &showing.skins.settings)
                    .map(|done| Happened::Settings(ui::panes::settings::Pressed::Font(done)))
            }
            Self::CoaxdUpdate => ui::panes::settings::coaxd::show(
                root,
                showing.settings_page,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::ScanDelete => ui::panes::settings::scan_results::show_delete(
                root,
                showing.settings_page,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::ScanCopy => ui::panes::settings::scan_results::show_copy(
                root,
                showing.settings_page,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::Favourites => {
                let draft = showing
                    .settings_page
                    .favourites
                    .get_or_insert_with(ui::panes::settings::favourites::Draft::load);
                let pressed =
                    ui::panes::settings::favourites::show(root, draft, &showing.skins.settings);
                if pressed.is_some() {
                    showing.settings_page.favourites = None;
                }
                pressed.map(Happened::Settings)
            }
            Self::FiveOne => ui::panes::settings::surround::five_one(
                root,
                showing.settings_page,
                showing.edited,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::TwentyTwo => ui::panes::settings::surround::twenty_two(
                root,
                showing.settings_page,
                showing.edited,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::Tuners => ui::panes::settings::tuners::show(
                root,
                showing.settings_page,
                showing.edited,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::Plugin(name) => ui::panes::settings::plugin::show(
                root,
                name,
                showing.settings_page,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::PluginPage(stem, page) => ui::panes::settings::plugin_page::show_window(
                root,
                stem,
                page,
                showing.settings_page,
                showing.guide,
                &showing.skins.settings,
            )
            .map(Happened::Settings),
            Self::Panel(_) => {
                let feed = showing.panel?;
                let tabs = ui::guide::panel::tabs_of(&crate::plugin_ui::panels());
                let rows = showing.panel_rows;
                let shown = ui::guide::panel::Panel {
                    strip: ui::guide::mini::Strip {
                        channels: &feed.banded.listing.channels,
                        from: feed.banded.from,
                        rows,
                        now_when: feed.banded.now_when.clone(),
                        now_title: feed.banded.now_title.clone(),
                        band: feed.banded.band,
                        bands: &feed.banded.bands,
                        watching: feed.banded.watching,
                        moment: Some(feed.banded.moment),
                        theme: showing.skins,
                    },
                    tab: ui::guide::panel::chosen(&tabs, &feed.tab),
                    tabs: &tabs,
                    folded: false,
                    detached: true,
                    detail: feed.banded.detail.as_deref(),
                    most_rows: rows,
                };
                let host = Arc::clone(&showing.settings_page.plugin_ui);
                let mut host = host.lock().unwrap_or_else(|e| e.into_inner());
                let mut plugins = ui::guide::panel::Plugins {
                    host: &mut host,
                    scrolls: &mut showing.settings_page.plugin_scroll,
                    guide: showing.guide,
                    look: &showing.skins.settings,
                };
                ui::guide::panel::show(root, &shown, &mut plugins).map(Happened::Panel)
            }
            Self::About => {
                let about = showing.about?;
                match ui::panes::about::show(root, about, &showing.skins.settings)? {
                    ui::panes::about::Pressed::Copy => Some(Happened::Copy(about.written())),
                    ui::panes::about::Pressed::Close => Some(Happened::Close),
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum Happened {
    Close,
    Copy(String),
    Scan(ui::panes::scan::Pressed),
    Epg(ui::guide::epg::Action),
    Found(ui::guide::found::Pressed),
    Remote(zwsdk::layer::Key),
    Pressed(ui::panes::remote::Pressed),
    Settings(ui::panes::settings::Pressed),
    Follow(ui::guide::detail::Follow),
    Panel(ui::guide::panel::Pressed),
}

pub struct Showing<'a> {
    pub listing: &'a ui::guide::epg::Listing,
    pub jump_to: Option<zwcore::guide::Key>,
    pub finding: &'a mut ui::guide::found::State,
    pub stream: &'a mut ui::panes::stream::State,
    pub info: Option<&'a ui::panes::stream::StreamInfo>,
    pub decoding: Option<&'a ui::panes::decoder::Decoding>,
    pub edited: &'a mut crate::config::File,
    pub guide: &'a zwcore::guide::Guide,
    pub loaded: &'a crate::config::File,
    pub settings_page: &'a mut ui::panes::settings::State,
    pub skins: ui::theme::Theme,
    pub broadcast: Option<&'a zwcore::data::Shown>,
    pub textures: &'a mut ui::panes::databroadcast::Textures,
    pub text: &'a mut Vec<ui::guide::detail::NativeBox>,
    pub remote: &'a mut ui::panes::remote::State,
    pub band: Band,
    pub bands: &'a [Band],
    pub scan: &'a crate::scan::Scanning,
    pub about: Option<&'a ui::panes::about::About>,
    pub panel: Option<&'a ui::guide::panel::Feed>,
    pub panel_rows: u32,
}

pub enum Paint {
    Wgpu {
        surface: wgpu::Surface<'static>,
        target: wgpu::TextureFormat,
    },
    #[cfg(all(unix, not(target_os = "macos")))]
    Gles(zwcore::screen::Painter),
}

impl std::fmt::Debug for Paint {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wgpu { target, .. } => write!(out, "Wgpu({target:?})"),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Gles(_) => write!(out, "Gles"),
        }
    }
}

pub struct Sub {
    pub window: Arc<Window>,
    pub paint: Paint,
    pub face: Face,
    pub size: (u32, u32),
    pub what: Which,
    pub dirty: bool,
    pub painted: Instant,
    /// 人が掴んで動かしている最中。その間はサーフェスを取らない（DXGI の中で待たされる）。
    pub moving_until: Option<Instant>,
    /// 最後に大きさが変わった時刻。大きさを変えている間の `Moved` は掴んで動かしているのではない。
    pub resized_at: Option<Instant>,
    pub hurry: bool,
    pub presented: std::sync::Arc<crate::threads::link::Presented>,
    pub text: Vec<crate::window::text::Shown>,
}

pub const MOVING_HOLD: std::time::Duration = std::time::Duration::from_millis(150);

impl Sub {
    /// GLES 2.0 のウィンドウなら GL のレンダラーを片付ける（そのまま破棄すると egui_glow がテクスチャのリークを警告する）。
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(unused_mut))]
    pub fn release(mut self) -> Self {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Paint::Gles(painter) = &self.paint
            && let Ok(window) = painter.gles_window()
            && window.make_current().is_ok()
        {
            self.face.release_gl();
        }
        self
    }
}

impl std::fmt::Debug for Sub {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Sub")
            .field("what", &self.what)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Sub {
    pub fn due(&self, gap: std::time::Duration, every: std::time::Duration) -> bool {
        if self.moving() {
            return false;
        }
        let since = self.painted.elapsed();
        // 大きさが変わったばかりなら間を空けない（メインスレッドが待っている）。
        if self.hurry {
            return true;
        }
        let asked = self.face.due_by(std::time::Instant::now());
        let wanted = self.dirty || asked || since >= every;
        wanted && since >= gap
    }

    pub fn moving(&self) -> bool {
        self.moving_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub fn after(
        &self,
        gap: std::time::Duration,
        every: std::time::Duration,
    ) -> std::time::Duration {
        let since = self.painted.elapsed();
        let floor = gap.saturating_sub(since);
        if let Some(until) = self.moving_until
            && let Some(left) = until.checked_duration_since(Instant::now())
        {
            return left.max(floor);
        }
        if self.dirty {
            return floor;
        }
        let asked = self.face.due_in(std::time::Instant::now()).unwrap_or(every);
        asked.min(every.saturating_sub(since)).max(floor)
    }
}
