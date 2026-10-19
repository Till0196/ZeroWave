// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 設定ウィンドウ。`zerowave.toml` のコピー `edited` のうち、`loaded` と違うキーだけを「適用」で書く。

use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

mod channels;
mod codec;
mod form;
mod keys;
mod pans;
mod plain;
mod scan;
#[cfg(test)]
mod snapshot;
mod source;
mod status_items;

/// 別ウィンドウ（プラグインの設定・チューナーの状態・係数）。
pub mod coaxd;
pub mod favourites;
pub mod font_pick;
pub mod plugin;
mod plugin_bound;
mod plugin_list;
pub mod plugin_page;
pub mod scan_results;
pub mod surround;
pub mod tuners;

pub use channels::with_default_networks;
use form::Form;
pub(crate) use scan::{PLANS, ask_boards, line_of, offered};

use crate::config::File;
use crate::ui::theme::settings as skin;
use crate::ui::theme::settings::Skin as Look;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Display,
    Osd,
    Operation,
    Keys,
    Source,
    Scan,
    Channels,
    ChannelList,
    Video,
    Audio,
    Playback,
    Caption,
    Codec,
    Epg,
    Recording,
    Capture,
    Plugins,
    Log,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::General => "一般",
            Self::Display => "表示",
            Self::Osd => "OSD",
            Self::Operation => "操作",
            Self::Keys => "キー割り当て",
            Self::Source => "ソース",
            Self::Scan => "チャンネルスキャン",
            Self::Channels => "チャンネル設定",
            Self::ChannelList => "チャンネル一覧",
            Self::Video => "映像",
            Self::Audio => "音声",
            Self::Playback => "再生",
            Self::Caption => "字幕",
            Self::Codec => "メディアコーデック",
            Self::Epg => "番組表・番組情報",
            Self::Recording => "録画",
            Self::Capture => "キャプチャ",
            Self::Plugins => "プラグイン",
            Self::Log => "ログ",
        }
    }

    fn name(self) -> String {
        String::from(self.label())
    }
}

const PAGES: &[(&str, &[Page])] = &[
    (
        "基本",
        &[
            Page::General,
            Page::Display,
            Page::Osd,
            Page::Operation,
            Page::Keys,
        ],
    ),
    (
        "チューナー",
        &[Page::Source, Page::Scan, Page::Channels, Page::ChannelList],
    ),
    (
        "再生",
        &[
            Page::Video,
            Page::Audio,
            Page::Playback,
            Page::Caption,
            Page::Codec,
        ],
    ),
    ("番組表", &[Page::Epg]),
    ("録画", &[Page::Recording, Page::Capture]),
    ("拡張", &[Page::Plugins, Page::Log]),
];

/// 選んだフォントを適用前からこのウィンドウに入れる。フォントを読むのは重いので、変わったときだけ入れ直す。
fn preview_fonts(context: &egui::Context, edited: &File) {
    use crate::ui::text::font;
    let id = egui::Id::unique("settings-font-preview");
    let wanted = font::Choice::of(edited);
    let had: Option<font::Choice> = context.data(|data| data.get_temp(id));
    if had.unwrap_or_else(font::chosen) != wanted {
        font::install_with(context, &wanted);
        context.data_mut(|data| data.insert_temp(id, wanted));
        context.request_repaint();
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub page: Page,
    pub capture_layers: Vec<(String, String)>,
    pub font_pick: Option<font_pick::Pick>,
    /// 「ソース」「スキャン」「チャンネル一覧」で選んでいるプロファイル。空は「all」。
    pub profile: String,
    pub active: String,
    pub checks: Checks,
    pub coaxd: coaxd::Coaxd,
    pub capturing: Option<crate::keys::Action>,
    pub channel_tab: zwcore::guide::Band,
    pub scroll: f32,
    pub side_scroll: f32,
    /// 前のフレームで測った中身の高さ。表示してから戻すと跳ね返るのが見えるので、送る前に末尾で止める。
    pub content: f32,
    pub side_content: f32,
    pub scanning: crate::scan::Progress,
    /// 方式のキー → スキャンするか。書いていない方式は、受けるチューナーがあればスキャンする。
    pub scan_on: std::collections::BTreeMap<String, bool>,
    /// coaxd を起動して待つことがあるのでウィンドウのスレッドでは問い合わせない。無ければ問い合わせ中。
    pub boards: Boards,
    pub boards_asked: bool,
    pub plugin_scroll: std::collections::BTreeMap<String, (f32, f32)>,
    pub drops: Vec<Dropped>,
    pub choosing: Option<Dropped>,
    /// 「参照…」で選ばれた物（プラグイン、キー、パス）。本体の保存先はプラグインが空。
    pub browsed: std::sync::Arc<std::sync::Mutex<Vec<(String, String, std::path::PathBuf)>>>,
    pub plugin_told: Told,
    pub plugin_lists: plugin_list::Lists,
    pub channel_mark: Option<zwcore::guide::Key>,
    /// いまのソースで選べる局。`None` は絞らない。
    pub receivable: Option<std::sync::Arc<std::collections::BTreeSet<zwcore::guide::Key>>>,
    pub listed: Listed,
    pub five_one: Option<surround::Draft>,
    pub favourites: Option<favourites::Draft>,
    pub twenty_two: Option<surround::Draft>,
    pub reach: Option<Reach>,
    pub read_for: Option<String>,
    pub results: Option<(
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeMap<String, String>,
    )>,
    pub scan_delete: Option<scan_results::Deleting>,
    pub scan_copy: Option<scan_results::Copying>,
    pub channel_line: String,
    /// チャンネルマップ。多重フレーム（TSMF）を見せるためだけで、ここからは書かない。
    pub channels: Option<crate::config::channels::Channels>,
    pub gpus: Option<Vec<String>>,
    pub devices: Option<Vec<String>>,
    pub codec_tab: usize,
    /// ウィンドウのスレッドで問い合わせると止まる（デバイスを作り DLL を読む）ので別のスレッドが書く。
    pub offers: std::sync::Arc<std::sync::Mutex<Option<codec::Found>>>,
    pub offers_asked: bool,
    pub sound_tab: usize,
    pub plugin_ui: std::sync::Arc<std::sync::Mutex<crate::plugin_ui::Host>>,
    pub plugin_drafts: std::collections::BTreeMap<String, plugin_bound::Draft>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            page: Page::General,
            capture_layers: Vec::new(),
            font_pick: None,
            profile: String::new(),
            active: String::new(),
            checks: Checks::default(),
            coaxd: coaxd::Coaxd::default(),
            capturing: None,
            channel_tab: zwcore::guide::Band::default(),
            channel_mark: None,
            scanning: std::sync::Arc::new(std::sync::Mutex::new(crate::scan::Scanning::default())),
            scan_on: std::collections::BTreeMap::new(),
            boards: Boards::default(),
            boards_asked: false,
            plugin_scroll: std::collections::BTreeMap::new(),
            drops: Vec::new(),
            choosing: None,
            browsed: std::sync::Arc::default(),
            plugin_told: Told::default(),
            plugin_lists: plugin_list::Lists::default(),
            channels: None,
            reach: None,
            read_for: None,
            results: None,
            scan_delete: None,
            scan_copy: None,
            channel_line: String::new(),
            receivable: None,
            listed: Listed::default(),
            five_one: None,
            favourites: None,
            twenty_two: None,
            scroll: 0.0,
            side_scroll: 0.0,
            content: 0.0,
            side_content: 0.0,
            gpus: None,
            devices: None,
            codec_tab: 0,
            offers: std::sync::Arc::default(),
            offers_asked: false,
            sound_tab: 0,
            plugin_ui: std::sync::Arc::default(),
            plugin_drafts: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dropped {
    pub plugin: String,
    pub path: std::path::PathBuf,
    /// 論理座標。OS が返さなければ `None`。
    pub at: Option<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pressed {
    Apply,
    Ok,
    Cancel,
    Profile(String),
    Check,
    AddPlugin,
    PutCoaxd,
    /// 「更新する」が真。
    UpdateCoaxd(bool),
    Scan,
    AskDeleteScan(String),
    /// 「削除する」が真。
    DeleteScan(bool),
    OpenScanCopy,
    /// 「複製する」が真。
    CopyScan(bool),
    /// 設定ファイルに書く表記（`0x7fe6:1024`）をクリップボードへ。
    CopyKey(String),
    Reveal(std::path::PathBuf),
    OpenTuners,
    OpenFiveOne,
    OpenTwentyTwo,
    Boards,
    Close,
    OpenPlugin(String),
    /// プラグインの設定の「参照…」。`folder` ならフォルダーを選ぶ。
    Browse {
        plugin: String,
        key: String,
        label: String,
        folder: bool,
    },
    PickFont {
        target: font_pick::Target,
        place: String,
        current: String,
    },
    /// 書き先は `State::font_pick` が持つ。
    Font(font_pick::Done),
    /// キーは `record.folder` か `capture.folder`、添えるのはいまの欄の中身。
    BrowseFolder(&'static str, String),
    /// すぐ効かせ、保存の対象に数えない。
    CaptionShown(String),
    OpenPluginPage(String, String),
    /// 真なら stream の hook を持つ物で、選局し直して効かせる。
    Replugged(bool),
    /// プラグインのページの「OK」。反映してウィンドウを閉じる。
    Settled(bool),
}

pub type Listed = std::sync::Arc<std::sync::Mutex<ListedFor>>;

#[derive(Debug, Default)]
pub struct ListedFor {
    pub profile: Option<String>,
    pub channels: Vec<zwcore::input::source::Channel>,
    pub asking: bool,
}

pub type Told = std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>;

/// `Ok` はチューナー、`Err` は理由。無ければ問い合わせ中。
pub type Boards = std::sync::Arc<
    std::sync::Mutex<Option<Result<Vec<zwcore::input::source::coaxd::Board>, String>>>,
>;

/// 経路（`[line.<profile>.<plan>]` の `<plan>`）→ その経路で受ける局。
pub type Reach = std::collections::BTreeMap<String, Vec<zwcore::guide::Key>>;

/// 自分のスキャン結果と、無い方式の分の all の結果での経路ごとの局。
pub(crate) fn reach_of(profile: &str) -> Reach {
    reach_in(&crate::config::load_channels_used(profile))
}

pub(crate) fn reach_in(channels: &crate::config::channels::Channels) -> Reach {
    let state = crate::config::load_state();
    channels
        .lines
        .iter()
        .map(|(label, line)| {
            let keys = line
                .streams
                .keys()
                .filter_map(|stream| state.streams.get(stream).map(|known| (stream, known)))
                .flat_map(|(stream, known)| {
                    known.services.keys().map(|service| zwcore::guide::Key {
                        network: stream.network,
                        service: *service,
                    })
                })
                .collect();
            (label.clone(), keys)
        })
        .collect()
}

/// `Ok` はバージョン、`Err` は理由。無ければまだ。
pub type Checks =
    std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Result<String, String>>>>;

fn pick_profile(form: &mut Form<'_, '_>, edited: &File, profile: &str) -> (Option<Pressed>, Rect) {
    let names: Vec<String> = std::iter::once(ALL.to_owned())
        .chain(edited.profiles.iter().map(|profile| profile.name.clone()))
        .collect();
    let shown = if profile.is_empty() { ALL } else { profile };
    let mut chosen = shown.to_owned();
    let picked: Vec<(&str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), name.as_str()))
        .collect();
    let rect = form.select_row(
        "プロファイル",
        ("profile.pick", 0),
        &mut chosen,
        &picked,
        180.0_f32
            .min(form.fit(skin::SELECT_WIDTH) - 80.0)
            .max(100.0),
    );
    let pressed = (chosen != shown).then(|| {
        Pressed::Profile(if chosen == ALL {
            String::new()
        } else {
            chosen.clone()
        })
    });
    (pressed, rect)
}

/// 違うプロファイルならすぐ出せる物で埋め、別のスレッドで問い合わせ直す。
fn listed_for(listed: &Listed, profile: &str, file: &File) -> ListedFor {
    let Ok(mut now) = listed.lock() else {
        return ListedFor::default();
    };
    if now.profile.as_deref() != Some(profile) {
        now.profile = Some(profile.to_owned());
        now.channels = crate::stage::listed_at_hand(file, profile);
        now.asking = true;
        let into = std::sync::Arc::clone(listed);
        let (file, profile) = (file.clone(), profile.to_owned());
        let spawned = std::thread::Builder::new()
            .name(String::from("listed"))
            .spawn(move || {
                let channels = crate::stage::listed_asked(&file, &profile);
                if let Ok(mut now) = into.lock()
                    // 問い合わせている間に別のプロファイルへ移っていれば捨てる。
                    && now.profile.as_deref() == Some(profile.as_str())
                {
                    now.channels = channels;
                    now.asking = false;
                }
            });
        if spawned.is_err() {
            now.asking = false;
        }
    }
    ListedFor {
        profile: now.profile.clone(),
        channels: now.channels.clone(),
        asking: now.asking,
    }
}

/// 暗黙のプロファイルの名前。ファイルには書かない。
pub const ALL: &str = "all";

/// `edited` は直に書き換わる。`run_ui` は `CentralPanel` を置かないので背景を自分で塗る。
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    edited: &mut File,
    loaded: &File,
    guide: &zwcore::guide::Guide,
    overridden: &[String],
    look: &Look,
) -> Option<Pressed> {
    preview_fonts(ui.ctx(), edited);
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, look.body);
    let below = Rect::from_min_max(
        full.min,
        pos2(full.right(), full.bottom() - skin::FOOT_HEIGHT),
    );
    let side_width = skin::SIDE.min((full.width() * 0.25).max(140.0));
    let side = Rect::from_min_max(below.min, pos2(below.left() + side_width, below.bottom()));
    let body = Rect::from_min_max(pos2(side.right(), below.top()), below.max);
    let before = state.page;
    state.page = pages(ui, look, side, state, edited, loaded);
    if state.page != before {
        state.scroll = 0.0;
    }
    let in_page = self_page(ui, look, body, state, edited, loaded, guide, overridden);
    if edited.caption.shown != loaded.caption.shown {
        return Some(Pressed::CaptionShown(edited.caption.shown.clone()));
    }
    let changes = if edited == loaded {
        0
    } else {
        crate::config::changes(loaded, edited).len().max(1)
    };
    foot(ui, look, full, changes).or(in_page)
}

fn set_font(file: &mut File, key: &str, name: String) {
    let slot = match key {
        "ui.font" => &mut file.ui.font,
        "osd.font" => &mut file.osd.font,
        "caption.font" => &mut file.caption.font,
        "caption.superimpose_font" => &mut file.caption.superimpose_font,
        "guide.font" => &mut file.guide.font,
        "ui.status_font" => &mut file.ui.status_font,
        _ => return,
    };
    *slot = name;
}

fn page_changed(page: Page, edited: &File, loaded: &File) -> bool {
    match page {
        Page::General => edited.general != loaded.general,
        Page::Operation => {
            edited.operation != loaded.operation
                || edited.audio.volume_step != loaded.audio.volume_step
        }
        Page::Audio => {
            // デコーダーは「メディアコーデック」のページのもの。
            let mut a = edited.audio.clone();
            let mut b = loaded.audio.clone();
            a.volume_step = 0;
            b.volume_step = 0;
            a.decoder = b.decoder.clone();
            a != b
        }
        Page::Playback => edited.playback != loaded.playback,
        Page::Video => {
            let mut a = edited.video.clone();
            let mut b = loaded.video.clone();
            a.decoder = b.decoder.clone();
            b.decoder = a.decoder.clone();
            a != b
        }
        Page::Codec => {
            edited.video.decoder != loaded.video.decoder
                || edited.audio.decoder != loaded.audio.decoder
        }
        Page::Source => edited.sources != loaded.sources || edited.profiles != loaded.profiles,
        Page::Keys => edited.keys != loaded.keys,
        Page::Channels => edited.networks != loaded.networks,
        Page::Scan => edited.lines != loaded.lines,
        Page::ChannelList => edited.channel != loaded.channel,
        Page::Display => {
            edited.ui != loaded.ui
                || edited.video.frame != loaded.video.frame
                || edited.video.pans != loaded.video.pans
                || edited.general.hide_cursor != loaded.general.hide_cursor
                || edited.general.title != loaded.general.title
        }
        Page::Osd => edited.osd != loaded.osd,
        Page::Caption => edited.caption != loaded.caption,
        Page::Epg => edited.guide != loaded.guide,
        Page::Recording => edited.record != loaded.record,
        Page::Capture => edited.capture != loaded.capture,
        Page::Plugins => edited.data != loaded.data,
        Page::Log => edited.log != loaded.log,
    }
}

fn pages(
    ui: &mut Ui,
    look: &Look,
    rect: Rect,
    state: &mut State,
    edited: &File,
    loaded: &File,
) -> Page {
    let chosen = state.page;
    ui.painter().rect_filled(rect, 0.0, look.dark);
    state.side_scroll = scrolled(ui, rect, state.side_scroll, state.side_content);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    ui.set_clip_rect(rect);
    let ui = &mut ui;
    let mut picked = chosen;
    let mut at = rect.top() - state.side_scroll;
    for (group, pages) in PAGES {
        ui.painter().text(
            pos2(rect.left() + 14.0, at + 10.0),
            egui::Align2::LEFT_TOP,
            group,
            FontId::proportional(skin::GROUP_SIZE),
            look.group,
        );
        at += 26.0;
        for page in pages.iter() {
            let item = Rect::from_min_size(
                pos2(rect.left() + 6.0, at + 1.0),
                vec2(rect.width() - 12.0, skin::ITEM_HEIGHT),
            );
            if item.bottom() < rect.top() || item.top() > rect.bottom() {
                at = item.bottom() + 1.0;
                continue;
            }
            let response = ui.interact(
                item,
                ui.scope_id().with(("設定の頁", format!("{page:?}"))),
                Sense::click(),
            );
            let painter = ui.painter();
            let colour = if *page == chosen {
                painter.rect_filled(item, skin::ITEM_ROUNDING, look.chosen_fill);
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(item.left(), item.center().y - 10.0),
                        vec2(skin::MARK, 20.0),
                    ),
                    1.0,
                    look.chosen_mark,
                );
                look.chosen_text
            } else {
                if response.hovered() {
                    painter.rect_filled(item, skin::ITEM_ROUNDING, look.button_fill);
                }
                look.item
            };
            painter.text(
                pos2(item.left() + 14.0, item.center().y),
                egui::Align2::LEFT_CENTER,
                page.name(),
                FontId::proportional(skin::ITEM_SIZE),
                colour,
            );
            if page_changed(*page, edited, loaded) {
                painter.circle_filled(
                    pos2(item.right() - 12.0, item.center().y),
                    3.0,
                    look.changed,
                );
            }
            if response.clicked() {
                picked = *page;
            }
            at = item.bottom() + 1.0;
        }
    }
    state.side_content = at + state.side_scroll - rect.top() + 6.0;
    scroll_bar(ui, look, rect, state.side_scroll, state.side_content);
    ui.painter().vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(1.0, look.seam),
    );
    picked
}

fn scrolled(ui: &Ui, rect: Rect, scroll: f32, content: f32) -> f32 {
    let most = (content - rect.height()).max(0.0);
    let scroll = (scroll - wheeled(ui, rect)).clamp(0.0, most);
    dragged(ui, rect, scroll, content)
}

fn bar_track(rect: Rect) -> Rect {
    Rect::from_min_max(
        pos2(rect.right() - 12.0, rect.top() + 4.0),
        pos2(rect.right(), rect.bottom() - 4.0),
    )
}

fn bar_thumb(track: Rect, rect: Rect, scroll: f32, content: f32) -> (f32, f32) {
    let most = (content - rect.height()).max(0.0);
    let height = (track.height() * rect.height() / content).max(18.0);
    let top = track.top() + (track.height() - height) * (scroll / most);
    (height, top)
}

fn bar_id(rect: Rect) -> egui::Id {
    egui::Id::unique(("設定の送り", rect.left() as i32, rect.top() as i32))
}

fn dragged(ui: &Ui, rect: Rect, scroll: f32, content: f32) -> f32 {
    let most = (content - rect.height()).max(0.0);
    if most <= 0.0 {
        return scroll;
    }
    let id = bar_id(rect);
    let track = bar_track(rect);
    let response = ui.interact(track, id, Sense::click_and_drag());
    let held: Option<f32> = ui.data(|data| data.get_temp(id));
    if !response.is_pointer_button_down_on() {
        if held.is_some() {
            ui.data_mut(|data| data.remove::<f32>(id));
        }
        return scroll;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return scroll;
    };
    let (height, top) = bar_thumb(track, rect, scroll, content);
    let grab = held.unwrap_or_else(|| {
        let grab = if (top..top + height).contains(&pointer.y) {
            pointer.y - top
        } else {
            height / 2.0
        };
        ui.data_mut(|data| data.insert_temp(id, grab));
        grab
    });
    let travel = (track.height() - height).max(1.0);
    ((pointer.y - grab - track.top()) / travel * most).clamp(0.0, most)
}

/// 中身の上に乗るよう、中身の後に呼ぶ。
fn scroll_bar(ui: &Ui, look: &Look, rect: Rect, scroll: f32, content: f32) {
    if content <= rect.height() {
        return;
    }
    let track = bar_track(rect);
    let (height, top) = bar_thumb(track, rect, scroll, content);
    let hot = ui.rect_contains_pointer(track)
        || ui.data(|data| data.get_temp::<f32>(bar_id(rect)).is_some());
    let width = if hot { 6.0 } else { 3.0 };
    let left = track.right() - 3.0 - width;
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(left, track.top()), pos2(left + width, track.bottom())),
        width / 2.0,
        look.select_fill,
    );
    ui.painter().rect_filled(
        Rect::from_min_size(pos2(left, top), vec2(width, height)),
        width / 2.0,
        look.hover,
    );
}

/// この矩形の上のホイールの回転量（論理ピクセル）。`smooth_scroll_delta` は何フレームかに分けて渡し、
/// イベントのときしか再描画しないウィンドウには最初の一部しか届かないので、生のイベントを読む。
fn wheeled(ui: &Ui, rect: Rect) -> f32 {
    if !ui.rect_contains_pointer(rect) {
        return 0.0;
    }
    let line = ui
        .ctx()
        .options(|options| options.input_options.line_scroll_speed);
    ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * line,
                    egui::MouseWheelUnit::Page => delta.y * rect.height(),
                }),
                _ => None,
            })
            .sum()
    })
}

#[allow(clippy::too_many_arguments)]
fn self_page(
    ui: &mut Ui,
    look: &Look,
    rect: Rect,
    state: &mut State,
    edited: &mut File,
    loaded: &File,
    guide: &zwcore::guide::Guide,
    overridden: &[String],
) -> Option<Pressed> {
    let page = state.page;
    ui.painter().rect_filled(rect, 0.0, look.body);
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), 52.0));
    ui.painter().rect_filled(head, 0.0, look.dark);
    ui.painter().hline(
        head.x_range(),
        head.bottom() - 0.5,
        Stroke::new(1.0, look.seam),
    );
    ui.painter().text(
        pos2(head.left() + skin::PAD, head.center().y),
        egui::Align2::LEFT_CENTER,
        page.name(),
        FontId::proportional(skin::HEAD_SIZE),
        look.text,
    );
    let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
    state.scroll = scrolled(ui, body, state.scroll, state.content);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(body));
    inner.set_clip_rect(body);
    let mut at = body.top() + 6.0 - state.scroll;
    let mut form = Form {
        ui: &mut inner,
        look,
        body,
        at: &mut at,
        overridden,
        next_key: None,
    };
    let mut pressed = None;
    if state.read_for.as_deref() != Some(state.profile.as_str()) {
        state.reach = None;
        state.channels = None;
        state.results = None;
        state.read_for = Some(state.profile.clone());
    }
    let profile = state.profile.clone();
    match page {
        Page::General => plain::general(&mut form, edited, loaded),
        Page::Operation => plain::operation(&mut form, edited, loaded),
        Page::Video => {
            let gpus = state.gpus.get_or_insert_with(|| {
                zwcore::output::video::gpu::Gpu::names(&crate::draw::instance_of(None))
            });
            plain::video(&mut form, edited, loaded, gpus)
        }
        Page::Audio => {
            let devices = state
                .devices
                .get_or_insert_with(crate::audio::device::output_names);
            pressed = plain::audio(&mut form, edited, loaded, devices);
        }
        Page::Playback => plain::playback(&mut form, edited, loaded),
        Page::Source => {
            if !state.boards_asked {
                ask_boards(state, edited);
            }
            let reach = state.reach.get_or_insert_with(|| reach_of(&profile));
            pressed = source::source(
                &mut form,
                edited,
                loaded,
                &state.profile,
                &state.active,
                &state.checks,
                &state.boards,
                reach,
                &state.coaxd,
            );
        }
        Page::Keys => keys::keys(&mut form, edited, &mut state.capturing),
        Page::Channels => {
            let reach = state.reach.get_or_insert_with(|| reach_of(&profile));
            pressed = channels::channels(&mut form, edited, reach);
        }
        Page::ChannelList => {
            // チューナーの一覧を問い合わせるのが先。問い合わせると経路のテーブルとチャンネルマップが空に戻る。
            if !state.boards_asked {
                ask_boards(state, edited);
            }
            let map = state
                .channels
                .get_or_insert_with(|| crate::config::load_channels_used(&profile));
            *form.at += 8.0;
            let select;
            (pressed, select) = pick_profile(&mut form, edited, &state.profile);
            let filter_rect = Rect::from_min_size(
                pos2(select.right() + 12.0, select.top()),
                vec2(160.0, select.height()),
            );
            let shown = listed_for(&state.listed, &state.profile, edited);
            // いまのプロファイルなら、受けているストリームの局（一覧に出ないラジオ）も選べる。
            let receivable = (state.profile == state.active)
                .then_some(state.receivable.as_deref())
                .flatten();
            let offered = channels::offered(&shown.channels, receivable);
            let unfiltered = offered.is_empty()
                && !shown.asking
                && state.profile == state.active
                && state.receivable.is_none();
            let boards = scan::boards_answer(&state.boards)
                .and_then(Result::ok)
                .unwrap_or_default();
            let chosen = channels::channel_list(
                &mut form,
                edited,
                guide,
                &mut state.channel_tab,
                &mut state.channel_mark,
                map,
                (!unfiltered).then_some(&offered),
                shown.asking,
                &state.profile,
                state.reach.get_or_insert_with(|| reach_of(&profile)),
                &boards,
                &mut state.channel_line,
                filter_rect,
            );
            pressed = pressed.or(chosen);
        }
        Page::Display => pressed = plain::display(&mut form, edited, loaded),
        Page::Osd => pressed = plain::osd(&mut form, edited, loaded),
        Page::Caption => pressed = plain::caption(&mut form, edited, loaded),
        Page::Codec => {
            if !state.offers_asked {
                state.offers_asked = true;
                let into = std::sync::Arc::clone(&state.offers);
                let _ = std::thread::Builder::new()
                    .name(String::from("デコーダーを数える"))
                    .spawn(move || {
                        let found = codec::Found {
                            video: codec::CODECS.map(|(codec, _)| zwcore::decode::offers(codec)),
                            sound: codec::SOUNDS.map(zwcore::decode::sound::offers),
                        };
                        if let Ok(mut slot) = into.lock() {
                            *slot = Some(found);
                        }
                    });
            }
            let offers = state.offers.lock().ok().and_then(|slot| slot.clone());
            codec::codecs(
                &mut form,
                edited,
                offers.as_ref(),
                &mut state.codec_tab,
                &mut state.sound_tab,
            );
        }
        Page::Epg => pressed = plain::epg(&mut form, edited, loaded),
        Page::Recording => {
            take_folders(state, edited);
            pressed = plain::recording(&mut form, edited, loaded);
        }
        Page::Capture => {
            take_folders(state, edited);
            pressed = plain::capture(&mut form, edited, loaded, &state.capture_layers);
        }
        Page::Scan => {
            if !state.boards_asked {
                ask_boards(state, edited);
            }
            pressed = scan::scan(&mut form, edited, state);
        }
        Page::Plugins => pressed = plugin::plugins(&mut form, edited, loaded, &state.plugin_told),
        Page::Log => pressed = plain::log(&mut form, edited, loaded),
    }
    let content = at + state.scroll - body.top() + 8.0;
    state.content = content;
    scroll_bar(ui, look, body, state.scroll, content);
    pressed
}

pub(super) fn foot(ui: &mut Ui, look: &Look, full: Rect, changes: usize) -> Option<Pressed> {
    let changed = changes > 0;
    let rect = Rect::from_min_size(
        pos2(full.left(), full.bottom() - skin::FOOT_HEIGHT),
        vec2(full.width(), skin::FOOT_HEIGHT),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, look.dark);
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, look.edge));
    let mut left = rect.left() + 18.0;
    if changed {
        painter.circle_filled(pos2(left + 3.0, rect.center().y), 3.0, look.changed);
        left += 12.0;
    }
    painter.text(
        pos2(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if changed {
            format!("保存されていない変更 {changes}件")
        } else {
            String::from("変更はありません")
        },
        FontId::proportional(skin::TEXT_SIZE - 0.5),
        if changed { look.text } else { look.label },
    );
    let mut pressed = None;
    let mut right = rect.right() - 18.0;
    for (label, which, primary, enabled) in [
        ("適用", Pressed::Apply, false, changed),
        ("キャンセル", Pressed::Cancel, false, true),
        ("OK", Pressed::Ok, true, true),
    ] {
        let button =
            Rect::from_min_size(pos2(right - 88.0, rect.center().y - 14.0), vec2(88.0, 28.0));
        if foot_button(ui, look, button, label, primary, enabled) {
            pressed = Some(which);
        }
        right = button.left() - 8.0;
    }
    pressed
}

fn foot_button(
    ui: &mut Ui,
    look: &Look,
    button: Rect,
    label: &str,
    primary: bool,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        button,
        ui.scope_id().with(("設定の釦", label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let painter = ui.painter();
    if primary {
        painter.rect_filled(button, skin::BUTTON_ROUNDING, look.primary);
    } else {
        let fill = if response.hovered() && enabled {
            look.hover
        } else {
            look.button_fill
        };
        painter.rect_filled(button, skin::BUTTON_ROUNDING, fill);
        painter.rect_stroke(
            button,
            skin::BUTTON_ROUNDING,
            Stroke::new(1.0, look.button_edge),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        button.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(skin::TEXT_SIZE),
        if primary {
            Color32::WHITE
        } else if enabled {
            look.text
        } else {
            look.label
        },
    );
    enabled && response.clicked()
}

fn take_folders(state: &mut State, edited: &mut File) {
    let Ok(mut browsed) = state.browsed.lock() else {
        return;
    };
    browsed.retain(|(plugin, key, path)| {
        if !plugin.is_empty() {
            return true;
        }
        let shown = path.to_string_lossy().into_owned();
        match key.as_str() {
            "record.folder" => edited.record.folder = shown,
            "capture.folder" => edited.capture.folder = shown,
            _ => {}
        }
        false
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_page_appears_twice_in_the_list() {
        let mut seen = Vec::new();
        for (_, pages) in PAGES {
            for page in *pages {
                assert!(!seen.contains(page), "{} が二度出ている", page.name());
                seen.push(*page);
            }
        }
    }

    #[test]
    fn every_page_has_a_name() {
        for (_, pages) in PAGES {
            for page in *pages {
                assert!(!page.name().is_empty());
            }
        }
    }

    #[test]
    fn bondriver_is_not_in_the_list() {
        for (_, pages) in PAGES {
            for page in *pages {
                assert!(!page.name().contains("BonDriver"), "{}", page.name());
            }
        }
    }

    #[test]
    fn a_changed_page_is_marked_until_it_is_put_back() {
        let loaded = File::default();
        let mut edited = loaded.clone();
        assert!(!page_changed(Page::Audio, &edited, &loaded));
        edited.audio.downmix = String::from("off");
        assert!(page_changed(Page::Audio, &edited, &loaded));
        assert!(!page_changed(Page::Operation, &edited, &loaded));
        edited.audio.downmix = String::from("auto");
        edited.audio.volume_step = 10;
        assert!(page_changed(Page::Operation, &edited, &loaded));
        assert!(!page_changed(Page::Audio, &edited, &loaded));
    }
}
