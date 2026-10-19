// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メニュー（デザイン `2:130`）。中身は [`VIEW`] と [`BAR`] のテーブルだけで決まる。メニューバー・右クリックメニュー・
//! キーが同じテーブルを参照するので、足すときは [`Command`]・テーブル・`Deck::run` の一行ずつ。

pub mod egui;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod native;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use native::Prepared;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Debug)]
pub struct Prepared {
    _nothing: (),
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Prepared {
    pub fn present(&self, _window: &winit::window::Window) -> bool {
        false
    }
}

pub const EMPTY: &str = "（まだありません）";

#[derive(Debug)]
pub struct Menus {
    bar: Bar,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    popup: native::Popup,
}

#[derive(Debug)]
#[cfg_attr(target_os = "macos", allow(clippy::large_enum_variant))]
enum Bar {
    InWindow,
    #[cfg(target_os = "macos")]
    Native(native::Native),
}

impl Menus {
    pub const fn none() -> Self {
        Self {
            bar: Bar::InWindow,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            popup: native::Popup::none(),
        }
    }

    pub fn attach() -> Self {
        #[allow(unused_mut)]
        let mut menus = Self::none();
        #[cfg(target_os = "macos")]
        if let Some(native) = native::Native::attach() {
            menus.bar = Bar::Native(native);
        }
        menus
    }

    pub fn in_window(&self) -> bool {
        matches!(self.bar, Bar::InWindow)
    }

    pub fn show(&self, ui: &mut ::egui::Ui, filling: &Filling) -> Option<egui::Shown> {
        match &self.bar {
            Bar::InWindow => egui::show(ui, filling, Self::has_native_context()),
            #[cfg(target_os = "macos")]
            Bar::Native(_) => None,
        }
    }

    pub fn prepare_bar(
        &mut self,
        index: usize,
        filling: &Filling,
        dark: bool,
        at: (f64, f64),
    ) -> Option<Prepared> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let (_, items) = BAR.get(index)?;
            self.popup.prepare_items(items, filling, dark, Some(at))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (index, filling, dark, at);
            None
        }
    }

    pub const fn has_native_context() -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }

    pub fn prepare_items(
        &mut self,
        items: &'static [Item],
        filling: &Filling,
        dark: bool,
    ) -> Option<Prepared> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.popup.prepare_items(items, filling, dark, None)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (items, filling, dark);
            None
        }
    }

    pub fn prepare_context(&mut self, filling: &Filling, dark: bool) -> Option<Prepared> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.popup.prepare(filling, dark)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (filling, dark);
            None
        }
    }

    #[cfg(target_os = "windows")]
    pub fn refill_open(&mut self, hmenu: isize, filling: &Filling) -> bool {
        self.popup.refill(hmenu, filling)
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn update_open(&mut self, filling: &Filling) -> native::Updated {
        self.popup.update_open(filling)
    }

    #[cfg(target_os = "windows")]
    pub fn closed_open(&mut self, hmenu: isize) {
        self.popup.closed(hmenu);
    }

    pub fn keep(&mut self, prepared: Prepared) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.popup.keep(prepared);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = prepared;
    }

    pub fn refill(&mut self, filling: &Filling) {
        match &mut self.bar {
            Bar::InWindow => {
                let _ = filling;
            }
            #[cfg(target_os = "macos")]
            Bar::Native(native) => native.refill(filling),
        }
    }

    pub fn taken(&self) -> Option<Picked> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let event = muda::MenuEvent::receiver().try_recv().ok()?;
            let id: &str = event.id.as_ref();
            if let Some(picked) = self.popup.picked(id) {
                return Some(picked);
            }
            match &self.bar {
                Bar::InWindow => command_of(id).map(Picked::Command),
                #[cfg(target_os = "macos")]
                Bar::Native(native) => native.picked(id),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            None
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn id_of(command: Command) -> String {
    format!("{command:?}")
}

/// 識別子から命令へ。テーブルから探す（一覧を別に持つと足し忘れる）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn command_of(id: &str) -> Option<Command> {
    every_item().find_map(|item| match item {
        Item::Command { command, .. } if id_of(*command) == id => Some(*command),
        _ => None,
    })
}

fn every_item() -> impl Iterator<Item = &'static Item> {
    BAR.iter()
        .flat_map(|(_, items)| items.iter())
        .chain(CONTEXT.iter())
        .chain(RECORD_MENU.iter())
        .chain(ERRORS_MENU.iter())
        .chain(CAPTURE_MENU.iter())
}

pub type Channel = (
    zwcore::guide::Key,
    String,
    Option<u8>,
    Option<std::sync::Arc<zwcore::guide::Logo>>,
);

#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    pub key: zwcore::guide::Key,
    pub name: String,
    pub programme: String,
    pub logo: Option<std::sync::Arc<zwcore::guide::Logo>>,
}

impl Station {
    fn label(&self) -> String {
        if self.programme.is_empty() {
            self.name.clone()
        } else {
            format!("{}　{}", self.name, self.programme)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Favourite {
    Station(Station),
    Folder(String, Vec<Favourite>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filling {
    pub favourites: Vec<Favourite>,
    pub recent: Vec<Station>,
    pub player_mode: bool,
    pub playback_off: bool,
    pub channels: Vec<(zwcore::guide::Band, Vec<Channel>)>,
    pub watching: Option<zwcore::guide::Key>,
    pub services: Vec<(
        zwcore::guide::Key,
        String,
        Option<std::sync::Arc<zwcore::guide::Logo>>,
    )>,
    pub band: zwcore::guide::Band,
    pub sounds: Vec<String>,
    pub sound_at: Option<usize>,
    pub pictures: Vec<String>,
    pub picture_at: Option<usize>,
    pub captions: Vec<String>,
    pub caption_at: Option<usize>,
    pub volume: u32,
    pub muted: bool,
    pub profiles: Vec<String>,
    pub profile: String,
    pub plugins: Vec<PluginMenu>,
    pub full_screen: bool,
    pub on_top: bool,
    pub menu_bar: bool,
    pub status_bar: bool,
    pub osd: bool,
    pub capture_layers: Vec<(String, bool)>,
    pub capture_size: String,
    pub aspect: String,
}

pub const ASPECTS: &[(&str, &str)] = &[
    ("default", "デフォルト(D)"),
    ("16:9", "16:9(W)"),
    ("letterbox", "16:9 レターボックス(L)"),
    ("double", "16:9 超額縁(S)"),
    ("side_cut", "4:3 サイドカット(C)"),
    ("4:3", "4:3(N)"),
];

pub const CAPTURE_SIZES: &[(&str, &str)] = &[
    ("source", "元の大きさ"),
    ("display", "表示されている大きさ"),
    ("3/4", "75 % (3/4)"),
    ("2/3", "66 % (2/3)"),
    ("1/2", "50 % (1/2)"),
    ("1/3", "33 % (1/3)"),
    ("1/4", "25 % (1/4)"),
    ("1920x1080", "1920 x 1080"),
    ("1440x810", "1440 x 810"),
    ("1280x720", "1280 x 720"),
    ("1024x576", "1024 x 576"),
    ("960x540", "960 x 540"),
    ("800x450", "800 x 450"),
    ("640x360", "640 x 360"),
    ("320x180", "320 x 180"),
    ("1440x1080", "1440 x 1080"),
    ("1280x960", "1280 x 960"),
    ("1024x768", "1024 x 768"),
    ("800x600", "800 x 600"),
    ("720x540", "720 x 540"),
    ("640x480", "640 x 480"),
    ("320x240", "320 x 240"),
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginMenu {
    pub stem: String,
    pub title: String,
    pub items: Vec<zwsdk::ui::MenuItem>,
}

impl Filling {
    pub fn is_on(&self, command: Command) -> bool {
        match command {
            Command::PlayerMode => self.player_mode,
            Command::PlaybackOff => self.playback_off,
            Command::Mute => self.muted,
            Command::FullScreen => self.full_screen,
            Command::AlwaysOnTop => self.on_top,
            Command::MenuBar => self.menu_bar,
            Command::StatusBar => self.status_bar,
            Command::Osd => self.osd,
            _ => false,
        }
    }
}

pub const MAX_ROWS: usize = 24;

pub fn columns(entries: Vec<Entry>) -> Vec<Entry> {
    if entries.len() <= MAX_ROWS {
        return entries;
    }
    let mut out = Vec::with_capacity(entries.len() + entries.len() / MAX_ROWS);
    for (nth, entry) in entries.into_iter().enumerate() {
        if nth > 0 && nth % MAX_ROWS == 0 {
            out.push(Entry::Break);
        }
        out.push(entry);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Picked {
    Command(Command),
    Tune(zwcore::guide::Key),
    Zoom(u32),
    Sound(usize),
    Picture(usize),
    Caption(u8),
    Profile(usize),
    CaptureLayer(usize),
    CaptureSize(usize),
    Aspect(usize),
    Plugin { plugin: u16, item: u16 },
}

pub const ZOOMS: &[u32] = &[25, 33, 50, 75, 100, 150, 200, 300];

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Pick {
        label: String,
        chosen: Option<bool>,
        picked: Picked,
        logo: Option<std::sync::Arc<zwcore::guide::Logo>>,
    },
    Sub {
        label: String,
        entries: Vec<Entry>,
    },
    Off {
        label: String,
    },
    Break,
    Separator,
}

impl Entry {
    fn command(label: impl Into<String>, command: Command) -> Self {
        Self::Pick {
            label: label.into(),
            chosen: None,
            picked: Picked::Command(command),
            logo: None,
        }
    }

    fn choice(label: impl Into<String>, chosen: bool, picked: Picked) -> Self {
        Self::Pick {
            label: label.into(),
            chosen: Some(chosen),
            picked,
            logo: None,
        }
    }

    fn station(
        label: impl Into<String>,
        chosen: bool,
        picked: Picked,
        logo: Option<std::sync::Arc<zwcore::guide::Logo>>,
    ) -> Self {
        Self::Pick {
            label: label.into(),
            chosen: Some(chosen),
            picked,
            logo,
        }
    }
}

fn favourite_entries(filling: &Filling) -> Vec<Entry> {
    fn rows(list: &[Favourite], watching: Option<zwcore::guide::Key>) -> Vec<Entry> {
        list.iter()
            .map(|row| match row {
                Favourite::Station(station) => Entry::station(
                    station.label(),
                    watching == Some(station.key),
                    Picked::Tune(station.key),
                    station.logo.clone(),
                ),
                Favourite::Folder(name, inside) => Entry::Sub {
                    label: name.clone(),
                    entries: rows(inside, watching),
                },
            })
            .collect()
    }
    let mut out = rows(&filling.favourites, filling.watching);
    if out.is_empty() {
        out.push(Entry::Off {
            label: String::from("なし"),
        });
    }
    out.push(Entry::Separator);
    if filling.watching.is_some() && !filling.player_mode {
        out.push(Entry::command("お気に入りに追加(A)", Command::AddFavourite));
    } else {
        out.push(Entry::Off {
            label: String::from("お気に入りに追加(A)"),
        });
    }
    out.push(Entry::command(
        "お気に入りの管理(O)...",
        Command::OrganizeFavourites,
    ));
    out
}

fn recent_entries(filling: &Filling) -> Vec<Entry> {
    let mut out: Vec<Entry> = filling
        .recent
        .iter()
        .map(|station| {
            Entry::station(
                station.label(),
                filling.watching == Some(station.key),
                Picked::Tune(station.key),
                station.logo.clone(),
            )
        })
        .collect();
    let empty = out.is_empty();
    if empty {
        out.push(Entry::Off {
            label: String::from("なし"),
        });
    }
    out.push(Entry::Separator);
    if empty {
        out.push(Entry::Off {
            label: String::from("履歴をクリア(C)"),
        });
    } else {
        out.push(Entry::command("履歴をクリア(C)", Command::ClearRecent));
    }
    out
}

pub fn entries_of(group: Group, filling: &Filling) -> Vec<Entry> {
    match group {
        Group::Zoom => ZOOMS
            .iter()
            .map(|zoom| Entry::choice(format!("{zoom}%"), false, Picked::Zoom(*zoom)))
            .collect(),
        Group::Channel => filling
            .channels
            .iter()
            .map(|(band, channels)| Entry::Sub {
                label: band.name().to_owned(),
                entries: columns(
                    channels
                        .iter()
                        .map(|(key, name, remote, logo)| {
                            let label = match remote {
                                Some(number) => format!("{number}: {name}"),
                                None => name.clone(),
                            };
                            Entry::station(
                                label,
                                filling.watching == Some(*key),
                                Picked::Tune(*key),
                                logo.clone(),
                            )
                        })
                        .collect(),
                ),
            })
            .collect(),
        Group::Service => filling
            .services
            .iter()
            .map(|(key, name, logo)| {
                Entry::station(
                    name.clone(),
                    filling.watching == Some(*key),
                    Picked::Tune(*key),
                    logo.clone(),
                )
            })
            .collect(),
        Group::Profile => filling
            .profiles
            .iter()
            .enumerate()
            .map(|(at, name)| {
                Entry::choice(name.clone(), *name == filling.profile, Picked::Profile(at))
            })
            .collect(),
        Group::Audio => {
            let entries: Vec<Entry> = filling
                .sounds
                .iter()
                .enumerate()
                .map(|(at, sound)| {
                    Entry::choice(
                        sound.clone(),
                        filling.sound_at == Some(at),
                        Picked::Sound(at),
                    )
                })
                .collect();
            entries
        }
        Group::Video => filling
            .pictures
            .iter()
            .enumerate()
            .map(|(at, name)| {
                Entry::choice(
                    name.clone(),
                    filling.picture_at == Some(at),
                    Picked::Picture(at),
                )
            })
            .collect(),
        Group::Caption => filling
            .captions
            .iter()
            .enumerate()
            .map(|(at, name)| {
                Entry::choice(
                    name.clone(),
                    filling.caption_at == Some(at),
                    Picked::Caption(at as u8),
                )
            })
            .collect(),
        Group::Volume => vec![
            Entry::Pick {
                label: format!("音量 {}%", filling.volume),
                chosen: None,
                picked: Picked::Command(Command::VolumeUp),
                logo: None,
            },
            Entry::Pick {
                label: String::from("音量を上げる(U)"),
                chosen: None,
                picked: Picked::Command(Command::VolumeUp),
                logo: None,
            },
            Entry::Pick {
                label: String::from("音量を下げる(D)"),
                chosen: None,
                picked: Picked::Command(Command::VolumeDown),
                logo: None,
            },
            Entry::choice("消音(M)", filling.muted, Picked::Command(Command::Mute)),
            Entry::Separator,
            Entry::command("音ずれ補正 音声を早める", Command::AudioDelayEarlier),
            Entry::command("音ずれ補正 音声を遅らせる", Command::AudioDelayLater),
            Entry::command("音ずれ補正 リセット", Command::AudioDelayReset),
        ],
        Group::Plugins => filling
            .plugins
            .iter()
            .enumerate()
            .map(|(plugin, menu)| Entry::Sub {
                label: menu.title.clone(),
                entries: menu
                    .items
                    .iter()
                    .enumerate()
                    .map(|(item, said)| {
                        let picked = Picked::Plugin {
                            plugin: plugin as u16,
                            item: item as u16,
                        };
                        match said {
                            zwsdk::ui::MenuItem::Item { label, checked, .. } => Entry::Pick {
                                label: label.clone(),
                                chosen: *checked,
                                picked,
                                logo: None,
                            },
                            zwsdk::ui::MenuItem::Open { label, .. } => Entry::Pick {
                                label: label.clone(),
                                chosen: None,
                                picked,
                                logo: None,
                            },
                            zwsdk::ui::MenuItem::Separator => Entry::Separator,
                        }
                    })
                    .collect(),
            })
            .collect(),
        Group::Favourites => favourite_entries(filling),
        Group::Recent => recent_entries(filling),
        Group::Aspect => ASPECTS
            .iter()
            .enumerate()
            .map(|(at, (spelled, name))| {
                Entry::choice(*name, filling.aspect == *spelled, Picked::Aspect(at))
            })
            .collect(),
        // バッファリセットは受信側のバッファがビューアと分かれていないので無い。
        Group::Reset => vec![
            Entry::command("リセット(R)", Command::Reset),
            Entry::command("ビューアリセット(V)", Command::ResetViewer),
            Entry::Separator,
            Entry::command("映像と音声の再初期化(F)", Command::RebuildPlayer),
        ],
        Group::RecentFiles => Vec::new(),
        Group::CaptureSize => CAPTURE_SIZES
            .iter()
            .enumerate()
            .map(|(at, (spelled, name))| {
                Entry::choice(
                    *name,
                    filling.capture_size == *spelled,
                    Picked::CaptureSize(at),
                )
            })
            .collect(),
        Group::CaptureLayers => filling
            .capture_layers
            .iter()
            .enumerate()
            .map(|(at, (name, on))| Entry::Pick {
                label: name.clone(),
                chosen: Some(*on),
                picked: Picked::CaptureLayer(at),
                logo: None,
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Command {
    Open,
    PlayerMode,
    FullScreen,
    AlwaysOnTop,
    StartStopRecording,
    CopyImage,
    SaveImage,
    PlaybackOff,
    Reset,
    ResetViewer,
    RebuildPlayer,
    ResetErrorCount,
    AddFavourite,
    OrganizeFavourites,
    ClearRecent,
    ChannelBack,
    ChannelForward,
    ChannelPrevious,
    PanelWindow,
    ProgrammeGuide,
    ProgrammeDetail,
    CopyProgramme,
    MenuBar,
    StatusBar,
    Osd,
    OpenRecordFolder,
    OpenCaptureFolder,
    RecordSettings,
    CaptureSettings,
    Remote,
    VolumeUp,
    VolumeDown,
    Mute,
    AudioDelayEarlier,
    AudioDelayLater,
    AudioDelayReset,
    Settings,
    StreamInformation,
    DecoderInformation,
    About,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Zoom,
    Aspect,
    Channel,
    Service,
    Profile,
    Favourites,
    Recent,
    Volume,
    Audio,
    Video,
    Caption,
    Reset,
    Plugins,
    RecentFiles,
    CaptureLayers,
    CaptureSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Command {
        label: &'static str,
        command: Command,
        key: Option<&'static str>,
        enabled: bool,
        toggle: bool,
    },
    Group {
        label: &'static str,
        group: Group,
        enabled: bool,
    },
    Separator,
}

impl Item {
    const fn command(label: &'static str, command: Command, key: Option<&'static str>) -> Self {
        Self::Command {
            label,
            command,
            key,
            enabled: true,
            toggle: false,
        }
    }

    const fn toggle(label: &'static str, command: Command, key: Option<&'static str>) -> Self {
        Self::Command {
            label,
            command,
            key,
            enabled: true,
            toggle: true,
        }
    }

    #[allow(dead_code)]
    const fn pending(label: &'static str, command: Command, key: Option<&'static str>) -> Self {
        Self::Command {
            label,
            command,
            key,
            enabled: false,
            toggle: false,
        }
    }

    const fn group(label: &'static str, group: Group) -> Self {
        Self::Group {
            label,
            group,
            enabled: true,
        }
    }

    const fn pending_group(label: &'static str, group: Group) -> Self {
        Self::Group {
            label,
            group,
            enabled: false,
        }
    }
}

pub const FILE: &[Item] = &[
    Item::command("開く(O)...", Command::Open, Some("CmdOrCtrl+O")),
    Item::pending_group("最近開いたファイル(F)", Group::RecentFiles),
    Item::Separator,
    Item::command("録画開始/停止(R)", Command::StartStopRecording, Some("R")),
    Item::Separator,
    Item::command("画像の保存(S)", Command::SaveImage, None),
    Item::group("キャプチャに含めるもの(L)", Group::CaptureLayers),
    Item::Separator,
    Item::command("設定(E)...", Command::Settings, Some("CmdOrCtrl+,")),
    Item::Separator,
    Item::command("閉じる(X)", Command::Close, Some("CmdOrCtrl+W")),
];

pub const EDIT: &[Item] = &[
    Item::command("画像のコピー(C)", Command::CopyImage, None),
    Item::command("番組情報のコピー(I)", Command::CopyProgramme, None),
];

pub const VIEW: &[Item] = &[
    Item::group("倍率(Z)", Group::Zoom),
    Item::group("比率(A)", Group::Aspect),
    Item::toggle("全画面表示(F)", Command::FullScreen, Some("Alt+Enter")),
    Item::Separator,
    Item::group("字幕(J)", Group::Caption),
    Item::Separator,
    Item::toggle("メニューバー(M)", Command::MenuBar, None),
    Item::toggle("ステータスバー(B)", Command::StatusBar, None),
    Item::toggle("OSD(O)", Command::Osd, None),
];

pub const PLAYBACK: &[Item] = &[
    Item::group("チャンネル(C)", Group::Channel),
    Item::group("サービス(S)", Group::Service),
    Item::group("プロファイル(T)", Group::Profile),
    Item::group("お気に入りチャンネル(L)", Group::Favourites),
    Item::group("最近見たチャンネル(H)", Group::Recent),
    Item::Separator,
    Item::group("音量(V)", Group::Volume),
    Item::group("音声(M)", Group::Audio),
    Item::group("映像切替(Q)", Group::Video),
    Item::Separator,
    Item::toggle("再生モード(P)", Command::PlayerMode, None),
    Item::toggle("再生オフ(I)", Command::PlaybackOff, None),
    Item::group("リセット(E)", Group::Reset),
];

pub const WINDOW: &[Item] = &[
    Item::toggle("常に前面に表示(W)", Command::AlwaysOnTop, Some("T")),
    Item::Separator,
    Item::command("パネルウィンドウ(N)", Command::PanelWindow, Some("P")),
    Item::command("EPG番組表(G)", Command::ProgrammeGuide, Some("E")),
    Item::command("番組説明(B)", Command::ProgrammeDetail, None),
    Item::command("リモコン(M)", Command::Remote, None),
    Item::Separator,
    Item::command("ストリームの情報(K)", Command::StreamInformation, None),
    Item::command("デコーダの情報(D)", Command::DecoderInformation, None),
    Item::Separator,
    Item::group("プラグイン(U)", Group::Plugins),
];

const HELP: &[Item] = &[Item::command("バージョン情報(A)...", Command::About, None)];

pub const BAR: &[(&str, &[Item])] = &[
    ("ファイル", FILE),
    ("編集", EDIT),
    ("表示", VIEW),
    ("再生", PLAYBACK),
    ("ウィンドウ", WINDOW),
    ("ヘルプ", HELP),
];

pub const RECORD_MENU: &[Item] = &[
    Item::command("録画開始/停止(R)", Command::StartStopRecording, Some("R")),
    Item::Separator,
    Item::command("録画フォルダを開く(O)", Command::OpenRecordFolder, None),
    Item::command("録画の設定(E)...", Command::RecordSettings, None),
];

pub const ERRORS_MENU: &[Item] = &[Item::command(
    "エラーカウントリセット(R)",
    Command::ResetErrorCount,
    None,
)];

pub const CAPTURE_MENU: &[Item] = &[
    Item::command("画像の保存(S)", Command::SaveImage, None),
    Item::command("画像のコピー(C)", Command::CopyImage, None),
    Item::Separator,
    Item::group("キャプチャに含めるもの(L)", Group::CaptureLayers),
    Item::group("大きさ(Z)", Group::CaptureSize),
    Item::Separator,
    Item::command(
        "保存先のフォルダを開く(O)",
        Command::OpenCaptureFolder,
        None,
    ),
    Item::command("キャプチャの設定(E)...", Command::CaptureSettings, None),
];

pub const CONTEXT: &[Item] = &[
    Item::group("倍率(Z)", Group::Zoom),
    Item::group("比率(A)", Group::Aspect),
    Item::toggle("全画面表示(F)", Command::FullScreen, Some("Alt+Enter")),
    Item::toggle("常に前面に表示(W)", Command::AlwaysOnTop, Some("T")),
    Item::toggle("メニューバー", Command::MenuBar, None),
    Item::Separator,
    Item::group("チャンネル(C)", Group::Channel),
    Item::group("サービス(S)", Group::Service),
    Item::group("プロファイル(T)", Group::Profile),
    Item::group("お気に入りチャンネル(L)", Group::Favourites),
    Item::group("最近見たチャンネル(H)", Group::Recent),
    Item::toggle("再生モード(P)", Command::PlayerMode, None),
    Item::Separator,
    Item::group("音量(V)", Group::Volume),
    Item::group("音声(M)", Group::Audio),
    Item::group("映像切替(Q)", Group::Video),
    Item::group("字幕(J)", Group::Caption),
    Item::Separator,
    Item::command("録画開始/停止(R)", Command::StartStopRecording, Some("R")),
    Item::command("画像のコピー(Y)", Command::CopyImage, None),
    // アクセスキーは 26 文字とも埋まっているので、この二つには付けない。
    Item::command("画像の保存", Command::SaveImage, None),
    Item::group("キャプチャに含めるもの", Group::CaptureLayers),
    Item::Separator,
    Item::toggle("再生オフ(I)", Command::PlaybackOff, None),
    Item::group("リセット(E)", Group::Reset),
    Item::Separator,
    Item::command("パネルウィンドウ(N)", Command::PanelWindow, Some("P")),
    Item::command("EPG番組表(G)", Command::ProgrammeGuide, Some("E")),
    Item::command("番組説明(B)", Command::ProgrammeDetail, None),
    Item::command("リモコン(M)", Command::Remote, None),
    Item::group("プラグイン(U)", Group::Plugins),
    Item::Separator,
    Item::command("設定(O)...", Command::Settings, None),
    Item::command("ストリームの情報(K)", Command::StreamInformation, None),
    Item::command("デコーダの情報(D)", Command::DecoderInformation, None),
    Item::Separator,
    Item::command("閉じる(X)", Command::Close, None),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn every_menu() -> Vec<(&'static str, &'static [Item])> {
        let mut out: Vec<(&str, &[Item])> = BAR.to_vec();
        out.push(("右クリックメニュー", CONTEXT));
        out.push(("録画の欄", RECORD_MENU));
        out.push(("キャプチャの欄", CAPTURE_MENU));
        out.push(("エラーの欄", ERRORS_MENU));
        out
    }

    #[test]
    fn no_separator_stands_alone_at_an_edge_or_beside_another() {
        for (name, items) in every_menu() {
            assert!(!matches!(items.first(), Some(Item::Separator)), "{name}");
            assert!(!matches!(items.last(), Some(Item::Separator)), "{name}");
            for pair in items.windows(2) {
                assert!(
                    !matches!((pair[0], pair[1]), (Item::Separator, Item::Separator)),
                    "{name}: 区切りが二つ続いている",
                );
            }
        }
    }

    #[test]
    fn no_two_items_in_a_menu_share_an_access_key() {
        for (name, items) in every_menu()
            .into_iter()
            .filter(|(name, _)| *name != "右クリックメニュー")
        {
            let mut letters: Vec<char> = items
                .iter()
                .filter_map(|item| match item {
                    Item::Command { label, .. } | Item::Group { label, .. } => {
                        let open = label.find('(')?;
                        label[open + 1..].chars().next()
                    }
                    Item::Separator => None,
                })
                .collect();
            letters.sort_unstable();
            let before = letters.len();
            letters.dedup();
            assert_eq!(before, letters.len(), "{name}: 同じアクセスキーが二つある");
        }
    }

    #[test]
    fn no_two_items_take_the_same_key() {
        for (_, items) in every_menu() {
            let mut keys: Vec<&str> = items
                .iter()
                .filter_map(|item| match item {
                    Item::Command { key, .. } => *key,
                    _ => None,
                })
                .collect();
            keys.sort_unstable();
            let before = keys.len();
            keys.dedup();
            assert_eq!(before, keys.len(), "同じキーが二つの項目に付いている");
        }
    }

    #[test]
    fn every_command_in_the_menu_survives_its_identifier() {
        for item in every_menu().into_iter().flat_map(|(_, items)| items.iter()) {
            if let Item::Command { command, .. } = item {
                assert_eq!(command_of(&id_of(*command)), Some(*command), "{command:?}");
            }
        }
        assert_eq!(command_of("nothing"), None);
    }

    #[test]
    fn the_filling_reaches_every_group_it_belongs_to() {
        let key = zwcore::guide::Key {
            network: 4,
            service: 101,
        };
        let filling = Filling {
            player_mode: false,
            playback_off: false,
            channels: vec![(
                zwcore::guide::Band::of(0x0004),
                vec![(key, "BS 局".to_owned(), Some(1), None)],
            )],
            watching: Some(key),
            services: vec![(key, "BS 局".to_owned(), None)],
            band: zwcore::guide::Band::of(0x0004),
            sounds: vec!["日本語 ステレオ".to_owned()],
            sound_at: Some(0),
            pictures: vec!["映像1".to_owned(), "映像2 (降雨対応放送)".to_owned()],
            picture_at: Some(0),
            captions: vec!["オフ".to_owned(), "第1言語：日本語".to_owned()],
            caption_at: Some(1),
            volume: 80,
            muted: false,
            profiles: vec![String::from("all")],
            profile: String::from("all"),
            plugins: vec![PluginMenu {
                stem: String::from("shaders"),
                title: String::from("映像フィルター"),
                items: vec![
                    zwsdk::ui::MenuItem::Item {
                        id: String::from("auto"),
                        label: String::from("自動選択"),
                        checked: Some(true),
                        enabled: true,
                    },
                    zwsdk::ui::MenuItem::Separator,
                    zwsdk::ui::MenuItem::Open {
                        label: String::from("設定..."),
                        open: zwsdk::ui::Open::Settings,
                    },
                ],
            }],
            full_screen: false,
            on_top: false,
            menu_bar: true,
            status_bar: true,
            osd: true,
            capture_layers: vec![(String::from("字幕"), true)],
            capture_size: String::from("display"),
            ..Default::default()
        };
        assert_eq!(
            entries_of(Group::Plugins, &filling),
            vec![Entry::Sub {
                label: String::from("映像フィルター"),
                entries: vec![
                    Entry::choice("自動選択", true, Picked::Plugin { plugin: 0, item: 0 }),
                    Entry::Separator,
                    Entry::Pick {
                        label: String::from("設定..."),
                        chosen: None,
                        picked: Picked::Plugin { plugin: 0, item: 2 },
                        logo: None,
                    },
                ],
            }]
        );
        assert_eq!(
            entries_of(Group::Caption, &filling),
            vec![
                Entry::choice("オフ", false, Picked::Caption(0)),
                Entry::choice("第1言語：日本語", true, Picked::Caption(1)),
            ]
        );
        assert_eq!(
            entries_of(Group::Video, &filling),
            vec![
                Entry::choice("映像1", true, Picked::Picture(0)),
                Entry::choice("映像2 (降雨対応放送)", false, Picked::Picture(1)),
            ]
        );
        assert_eq!(
            entries_of(Group::Service, &filling),
            vec![Entry::choice("BS 局", true, Picked::Tune(key))]
        );
        assert!(matches!(
            entries_of(Group::Channel, &filling).as_slice(),
            [Entry::Sub { entries, .. }] if entries == &[Entry::choice("1: BS 局", true, Picked::Tune(key))]
        ));
        assert_eq!(entries_of(Group::Audio, &filling).len(), 1);
        assert_eq!(entries_of(Group::Zoom, &filling).len(), ZOOMS.len());
        assert_eq!(
            entries_of(Group::CaptureLayers, &filling),
            vec![Entry::choice("字幕", true, Picked::CaptureLayer(0))]
        );
        assert_eq!(entries_of(Group::Aspect, &filling).len(), ASPECTS.len());
        assert_eq!(entries_of(Group::Reset, &filling).len(), 4);
        assert!(entries_of(Group::RecentFiles, &filling).is_empty());
    }

    #[test]
    fn no_two_items_mean_the_same_thing() {
        for (_, items) in every_menu() {
            let mut commands: Vec<Command> = items
                .iter()
                .filter_map(|item| match item {
                    Item::Command { command, .. } => Some(*command),
                    _ => None,
                })
                .collect();
            commands.sort_unstable();
            let before = commands.len();
            commands.dedup();
            assert_eq!(before, commands.len());
        }
    }
}
