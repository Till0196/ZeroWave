// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 別ウィンドウのスレッド（再生スレッドでレンダリングすると、操作している間じゅう映像のループに割り込む）。
//! 共有するのは wgpu のデバイスとスナップショットだけ。ウィンドウは winit の都合でメインスレッドが開く。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel};
use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::EventLoopProxy;
use winit::window::{Window, WindowId};
use zwcore::guide::{Band, Guide, Key, Moment};
use zwcore::output::video::gpu::Gpu;

use crate::stage::configure_surface;
use crate::threads::link::{Owner, Request, Wake};
use crate::ui::{self, Face};
use crate::window::sub::{Happened, Paint, Showing, Sub, Which};
use zwcore::data::{Latest, ToBroadcast};
use zwcore::guide::keeper::{Ask, ToGuide};

mod detail;
mod input;
mod paint;
mod screen;

/// 別ウィンドウを再レンダリングする最短の間隔（30 fps）。60 だとコンポジターで映像の提示が落ちる。
const FRAME: Duration = Duration::from_millis(33);

const REPAINT_EVERY: Duration = Duration::from_millis(250);

const IDLE_WAIT: Duration = Duration::from_millis(200);

const HOURS: u32 = 24;

pub enum ToPanes {
    Opened(
        Arc<Window>,
        Option<crate::window::sub::Paint>,
        Arc<crate::threads::link::Presented>,
    ),
    /// 要求したウィンドウが開けなかった。要求を一つ取り下げる（残すと次のウィンドウが別の用途に付く）。
    NotOpened,
    Event(WindowId, WindowEvent),
    /// 次に来るファイルのドロップが、そのウィンドウのどこに落ちたか（論理座標）。
    DropAt(WindowId, Option<(f32, f32)>),
    Open(Which),
    Detail(Key, Box<zwcore::guide::Programme>),
    /// 視聴中の局のいまの番組の説明を開く。このウィンドウは局と番組に追従する。
    WatchingDetail,
    OpenSettings(ui::panes::settings::Page),
    CaptureLayers(Vec<(String, String)>),
    Guide(Arc<Guide>),
    Watching(Option<Key>),
    /// いまのソースで選べる局。`None` は絞らない。
    Receivable(Option<Arc<std::collections::BTreeSet<Key>>>),
    Info(Box<ui::panes::stream::StreamInfo>),
    Decoding(Box<ui::panes::decoder::Decoding>),
    Plugins(Box<crate::plugin_ui::Shared>),
    Panel(Box<ui::guide::panel::Feed>),
    /// 切り離した下のパネルのウィンドウを閉じる（ユーザーが閉じたのではない）。
    ShutPanel,

    Mode(ui::theme::Mode),
    ShutOneSeg,
    Repaint,
    Profile(String),
    /// デバイスが作り直された。開いているウィンドウは閉じて開き直す（サーフェスは前のインスタンスの物）。
    Gpu(Gpu),
    /// デバイスを失った。前のデバイスへの参照を全部手放す（D3D12 では同じアダプターのデバイスが生きている間は
    /// 新しく作れない）。開いていたウィンドウは閉じ、次の [`ToPanes::Gpu`] で開き直す。
    DropGpu,
    Quit,
}

impl std::fmt::Debug for ToPanes {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Opened(..) => "Opened",
            Self::NotOpened => "NotOpened",
            Self::DropAt(..) => "DropAt",
            Self::Event(..) => "Event",
            Self::Open(_) => "Open",
            Self::Detail(..) => "Detail",
            Self::WatchingDetail => "WatchingDetail",
            Self::OpenSettings(_) => "OpenSettings",
            Self::CaptureLayers(_) => "CaptureLayers",
            Self::Guide(_) => "Guide",
            Self::Watching(_) => "Watching",
            Self::Receivable(_) => "Receivable",
            Self::Info(_) => "Info",
            Self::Decoding(_) => "Decoding",
            Self::Plugins(_) => "Plugins",
            Self::Panel(_) => "Panel",
            Self::ShutPanel => "ShutPanel",

            Self::Mode(_) => "Mode",
            Self::ShutOneSeg => "ShutOneSeg",
            Self::Repaint => "Repaint",
            Self::Profile(_) => "Profile",
            Self::Gpu(_) => "Gpu",
            Self::DropGpu => "DropGpu",
            Self::Quit => "Quit",
        })
    }
}

#[derive(Debug)]
pub enum Chose {
    Tune(Key),
    /// いま何が開いているか。再生スレッドがデコードの側へ何を要求するかがこれで決まる。
    Wants {
        guide: bool,
        info: bool,
        decoder: bool,
    },
}

#[derive(Debug)]
pub struct Panes {
    to: Sender<ToPanes>,
    from: Receiver<Chose>,
    boot: Option<(Receiver<ToPanes>, Sender<Chose>)>,
    broadcast: Option<(SyncSender<ToBroadcast>, Latest)>,
    file: Option<crate::config::File>,
    /// 別ウィンドウのスレッド。ウィンドウが隠れているとコンポジターが保持するので、タイムアウト付きで終了する。
    thread: Option<crate::window::ending::Thread>,
}

impl Panes {
    /// チャネルだけを作る。メインスレッドも書く側を持つ（別ウィンドウのイベントは直接来る）。
    pub fn new(
        to_broadcast: SyncSender<ToBroadcast>,
        shown: Latest,
        file: crate::config::File,
    ) -> (Self, Sender<ToPanes>) {
        let (to, events) = channel();
        let (say, from) = channel();
        (
            Self {
                to: to.clone(),
                from,
                boot: Some((events, say)),
                broadcast: Some((to_broadcast, shown)),
                file: Some(file),
                thread: None,
            },
            to,
        )
    }

    /// スレッドを起動する。GLES 2.0 のバックエンドでは wgpu のデバイスは無い（`None`）。
    pub fn hand(
        &mut self,
        gpu: Option<Gpu>,
        requests: Sender<Request>,
        proxy: EventLoopProxy<Wake>,
        guide_to: Sender<ToGuide>,
        listed: Receiver<Box<ui::guide::epg::Listing>>,
    ) {
        let Some((events, say)) = self.boot.take() else {
            return;
        };
        let Some((to_broadcast, shown)) = self.broadcast.take() else {
            return;
        };
        let file = self.file.take().unwrap_or_default();
        let screen = Screen {
            drop_at: std::collections::HashMap::new(),
            to: self.to.clone(),
            to_broadcast,
            shown,
            drawn_shown: None,
            popup_area: None,
            textures: ui::panes::databroadcast::Textures::default(),
            gpu,
            reopen: Vec::new(),
            guide_to,
            listed,
            asked: None,
            events,
            say,
            requests,
            proxy,
            subs: BTreeMap::new(),
            pending: VecDeque::new(),
            guide: Arc::new(Guide::new()),
            version: 0,
            band: None,
            watching: None,
            theme: ui::theme::Theme::of(ui::theme::Mode::default()),
            stream: ui::panes::stream::State::default(),
            scanning: std::sync::Arc::new(std::sync::Mutex::new(crate::scan::Scanning::default())),
            about: None,
            scan_stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scan_keep: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            info: None,
            decoding: None,
            loaded: ui::panes::settings::with_default_networks(file.clone()),
            edited: ui::panes::settings::with_default_networks(file),
            file_seen: crate::config::modified(),
            file_checked: None,
            settings_page: ui::panes::settings::State {
                profile: crate::config::load_state().profile,
                page: if crate::config::first_run() {
                    ui::panes::settings::Page::Source
                } else {
                    ui::panes::settings::Page::General
                },
                ..ui::panes::settings::State::default()
            },
            remote: ui::panes::remote::State::default(),
            listing: ui::guide::epg::Listing::default(),
            from: None,
            jump: None,
            jump_to: None,
            waiting: None,
            finding: ui::guide::found::State::default(),
            wanted: (false, false, false),
            panel: None,
            panel_rows: None,
            panel_size: None,
            panel_quiet: false,
            panel_dropped: false,
        };
        match crate::window::ending::Thread::spawn("panes", "別ウィンドウ", move |_told| {
            screen.run()
        }) {
            Ok(thread) => self.thread = Some(thread),
            Err(error) => tracing::warn!("別ウィンドウのスレッドを起動できない: {error}"),
        }
    }

    pub fn say(&self, what: ToPanes) {
        let _ = self.to.send(what);
    }

    pub fn taken(&self) -> Vec<Chose> {
        self.from.try_iter().collect()
    }

    /// スレッドを終了する。メインスレッドも書く側を持つので、チャネルを落とすだけでは終わらない。
    pub fn close(&mut self) {
        let _ = self.to.send(ToPanes::Quit);
        if let Some(mut thread) = self.thread.take() {
            thread.join_within(crate::window::ending::PATIENCE);
        }
    }
}

struct Screen {
    gpu: Option<Gpu>,
    /// デバイスを失ったときに閉じたウィンドウ。次のデバイスで開き直す。
    reopen: Vec<Which>,
    drop_at: std::collections::HashMap<WindowId, Option<(f32, f32)>>,
    to: Sender<ToPanes>,
    to_broadcast: SyncSender<ToBroadcast>,
    shown: Latest,
    drawn_shown: Option<usize>,
    popup_area: Option<(u32, u32)>,
    textures: ui::panes::databroadcast::Textures,
    /// 番組表のスレッドへ一覧のビルドを要求するチャネル（数百 ms 掛かるので、ここではビルドしない）。
    guide_to: Sender<ToGuide>,
    listed: Receiver<Box<ui::guide::epg::Listing>>,
    asked: Option<Ask>,
    events: Receiver<ToPanes>,
    say: Sender<Chose>,
    requests: Sender<Request>,
    proxy: EventLoopProxy<Wake>,
    subs: BTreeMap<WindowId, Sub>,
    /// 開くよう要求して、まだウィンドウが返ってきていないもの。要求した順に返る。
    pending: VecDeque<Which>,
    guide: Arc<Guide>,
    version: u64,
    /// 番組表のウィンドウで選んだ種別。パネルの番組表とは別（連動すると見ていた列が消える）。
    band: Option<Band>,
    watching: Option<Key>,
    theme: ui::theme::Theme,
    stream: ui::panes::stream::State,
    info: Option<ui::panes::stream::StreamInfo>,
    decoding: Option<ui::panes::decoder::Decoding>,
    scanning: crate::scan::Progress,
    about: Option<ui::panes::about::About>,
    /// チャンネルスキャンを止めるフラグ。終了するときに立てないと、チューナーを確保し続ける。
    scan_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scan_keep: std::sync::Arc<std::sync::atomic::AtomicBool>,
    loaded: crate::config::File,
    edited: crate::config::File,
    file_seen: Option<std::time::SystemTime>,
    file_checked: Option<Instant>,
    settings_page: ui::panes::settings::State,
    remote: ui::panes::remote::State,
    listing: ui::guide::epg::Listing,
    from: Option<Moment>,
    jump: Option<Moment>,
    jump_to: Option<Key>,
    /// 形の違う一覧を要求して、まだ届いていない。届くまでスクロールは効かせない（別の時刻へ行く）。
    waiting: Option<Instant>,
    finding: ui::guide::found::State,
    wanted: (bool, bool, bool),
    panel: Option<ui::guide::panel::Feed>,
    panel_rows: Option<u32>,
    panel_size: Option<(u32, u32)>,
    panel_quiet: bool,
    panel_dropped: bool,
}

fn screen_only(path: &[String]) -> bool {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    matches!(
        path.as_slice(),
        ["ui", "status_bar"]
            | ["ui", "menu_bar"]
            | ["osd", "enabled"]
            | ["caption", "shown"]
            | ["capture", "include", ..]
            | ["capture", "size"]
    )
}

/// テーマの色を、背景の上に置いた不透明の sRGB に（前乗算の半透明なので、そのままだと黒になる）。
fn over(colour: egui::Color32, background: egui::Color32) -> [u8; 3] {
    let rest = u32::from(255 - colour.a());
    let blend = |c: u8, b: u8| ((u32::from(c) * 255 + u32::from(b) * rest) / 255).min(255) as u8;
    [
        blend(colour.r(), background.r()),
        blend(colour.g(), background.g()),
        blend(colour.b(), background.b()),
    ]
}

impl Screen {
    /// チェックの付いた受信方式を設定から組み立てる。受けるチューナーが無い方式はスキャンしない。
    fn scan_legs(&self) -> Vec<crate::scan::Leg> {
        let Some(source) = self
            .edited
            .sources
            .iter()
            .find(|source| source.kind == "coaxd")
        else {
            return Vec::new();
        };
        let patterns = self
            .edited
            .coaxd_tuners(&self.edited.profile(&self.settings_page.profile));
        crate::ui::panes::settings::PLANS
            .iter()
            .filter(|plan| *self.settings_page.scan_on.get(**plan).unwrap_or(&false))
            .filter_map(|plan| {
                let (_, system) = crate::ui::panes::settings::offered(plan);
                let tuner = zwcore::scan::serving_tuner(
                    &source.endpoint,
                    source.priority,
                    system,
                    &patterns,
                )?;
                let line = crate::ui::panes::settings::line_of(&self.edited, plan);
                Some(crate::scan::Leg {
                    label: (*plan).to_owned(),
                    endpoint: source.endpoint.clone(),
                    priority: source.priority.saturating_sub(1),
                    spec: zwcore::scan::Spec {
                        plan: line.plan.clone(),
                        bands: line.bands.clone(),
                        only: line.only.clone(),
                        satellites: line.satellites.clone(),
                        modulations: line.modulations.clone(),
                    },
                    tuner: Some(tuner),
                })
            })
            .collect()
    }
}

fn check_source(source: &crate::config::file::Source) -> Result<String, String> {
    match source.kind.as_str() {
        "mirakurun" => {
            if source.url.is_empty() {
                return Err(String::from("URL が無い"));
            }
            let mirakurun = zwcore::input::source::mirakurun::Mirakurun::at(&source.url)
                .map_err(|why| why.to_string())?;
            mirakurun
                .version()
                .map(|version| format!("Mirakurun {version}"))
                .map_err(|why| format!("接続できません: {why}"))
        }
        "coaxd" => zwcore::input::source::coaxd::Coaxd::written(&source.endpoint, source.priority)
            .version()
            .map(|version| format!("coaxd {version}"))
            .map_err(|why| format!("接続できません: {why}")),
        other => Err(format!("kind「{other}」は不明")),
    }
}

/// 保存で名前が変わったプロファイル（前、後）。並びの同じ位置で見分け、長さが変われば見分けない。
fn renamed_profiles(
    before: &[crate::config::file::Profile],
    after: &[crate::config::file::Profile],
) -> Vec<(String, String)> {
    if before.len() != after.len() {
        return Vec::new();
    }
    before
        .iter()
        .zip(after)
        .filter(|(was, now)| {
            was.name != now.name
                && !before.iter().any(|profile| profile.name == now.name)
                && !after.iter().any(|profile| profile.name == was.name)
        })
        .map(|(was, now)| (was.name.clone(), now.name.clone()))
        .collect()
}

/// 保存で使っているプロファイルの名前が変わったなら新しい名前、消されたなら all（空）。
fn follow_active(
    active: &str,
    before: &[crate::config::file::Profile],
    after: &[crate::config::file::Profile],
) -> Option<String> {
    if active.is_empty() || after.iter().any(|profile| profile.name == active) {
        return None;
    }
    let at = before.iter().position(|profile| profile.name == active)?;
    if before.len() == after.len()
        && let Some(renamed) = after.get(at)
        && !before.iter().any(|profile| profile.name == renamed.name)
    {
        return Some(renamed.name.clone());
    }
    Some(String::new())
}

#[cfg(test)]
mod follow_tests;
