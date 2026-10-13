// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 再生スレッド。デコードと時計とレンダリングを、イベントループとは別のスレッドで回す
//! （Windows はメニューやウィンドウの移動の間ループを返さず、音声が途切れ時計が止まる）。

use std::sync::Arc;

use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};

use std::time::{Duration, Instant};

use zwcore::engine::clock::HZ;

use zwcore::filters::{Kind, Stream};

use zwcore::guide::{Band, Guide, Moment};

use zwcore::input::source::Channel;

use zwcore::output::video::gpu::Onto;

use crate::choice::{Choices, Hdr};

use winit::event_loop::EventLoopProxy;

use zwcore::output::video::gpu::Gpu;

use zwcore::output::video::{Space, Transfer};

use zwcore::guide::keeper::{self as guide, ToGuide};

use crate::threads::link::{Act, FromStage, Presented, Request, ToStage, Wake};
use zwcore::data::ToBroadcast;

use crate::draw::overlay;

use crate::threads::panes::{Chose, Panes, ToPanes};

use zwcore::player::{self, Ask, Player};

use crate::draw::scene;

use zwcore::screen::{Canvas, Painter};

use crate::draw::shown;

use zwcore::player::supply::{self, Answer, Supply};

use crate::ui::{self, Status};

mod capture;
mod catalog;
mod channels;
mod compose;
mod plugging;
use plugging::Noticed;
mod reception;
mod recover;
mod run;
mod serve;
mod setup;
mod sources;
mod surface;
#[cfg(test)]
mod tests;
pub(crate) mod tuning;
mod verify;

pub(crate) use catalog::{listed_asked, listed_at_hand};
pub(crate) use compose::shown_size;
use compose::stream_info;
pub use surface::configure_surface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Listed {
    band: Band,
    from: Moment,
    hours: u32,
    watching: Option<zwcore::guide::Key>,
    guide: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    First,
    /// 引数で指定された局。指定された番号が全部合う最初の局。
    Named(Named),
    /// `state.toml` の最後の局。
    Exact(zwcore::guide::Key),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Named {
    pub network: Option<u16>,
    pub transport: Option<u16>,
    pub service: Option<u16>,
}

impl Named {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn fits(&self, channel: &Channel) -> bool {
        let agrees = |want: Option<u16>, have: Option<u16>| want.is_none() || want == have;
        agrees(self.network, channel.key.map(|key| key.network))
            && agrees(self.transport, channel.transport)
            && agrees(self.service, channel.key.map(|key| key.service))
    }

    pub(crate) fn text(&self) -> String {
        [
            ("NID", self.network),
            ("TSID", self.transport),
            ("SID", self.service),
        ]
        .into_iter()
        .filter_map(|(name, id)| Some(format!("{name}: {}", id_text(id?))))
        .collect::<Vec<_>>()
        .join("  ")
    }
}

impl Pick {
    pub(crate) fn from<'a>(&self, channels: &'a [Channel]) -> Option<&'a Channel> {
        match self {
            Self::Named(named) => channels.iter().find(|channel| named.fits(channel)),
            Self::Exact(key) => channels
                .iter()
                .find(|channel| channel.key == Some(*key))
                .or_else(|| channels.first()),
            Self::First => channels.first(),
        }
    }
}

/// まだ接続していないソース。ウィンドウを先に開き、接続できなくても理由を出せるようバックグラウンドで接続する。
#[derive(Debug, Clone)]
pub enum Starting {
    Coaxd {
        endpoint: String,
        priority: i32,
        tuners: Vec<String>,
        profile: String,
        pick: Pick,
    },
    Mirakurun {
        url: String,
        by: zwcore::input::source::mirakurun::By,
        pick: Pick,
    },
}

impl Starting {
    fn waiting(&self) -> &'static str {
        match self {
            Self::Coaxd { .. } => "coaxd を起動しています...",
            Self::Mirakurun { .. } => "Mirakurun に接続しています...",
        }
    }

    fn reach(self) -> Result<Started, String> {
        use zwcore::input::source::Source as _;
        match self {
            Self::Coaxd {
                endpoint,
                priority,
                tuners,
                profile,
                pick,
            } => {
                let mut source = zwcore::input::source::coaxd::Coaxd::written(&endpoint, priority)
                    .with_tuners(tuners.clone());
                let version = source
                    .version()
                    .map_err(|why| said_with("coaxd に接続できません。", &why))?;
                let mapped = tuning::mapped_for(&endpoint, priority, &tuners, &profile);
                let channels = if mapped.is_empty() {
                    source.channels().unwrap_or_default()
                } else {
                    mapped
                };
                Ok(Started {
                    said: format!("coaxd バージョン {version}"),
                    channels,
                    pick,
                    guide_from: None,
                })
            }
            Self::Mirakurun { url, by, pick } => {
                let unreached =
                    |why: &dyn std::fmt::Display| said_with("Mirakurun に接続できません。", why);
                let mut source = zwcore::input::source::mirakurun::Mirakurun::at(&url)
                    .map_err(|why| unreached(&why))?;
                source.set_by(by);
                let version = source.version().map_err(|why| unreached(&why))?;
                let channels = source.channels().map_err(|why| {
                    said_with("Mirakurun からチャンネルの一覧を取得できません。", &why)
                })?;
                Ok(Started {
                    said: format!("{} バージョン {version}", source.name()),
                    channels,
                    pick,
                    guide_from: Some(url),
                })
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Started {
    said: String,
    channels: Vec<Channel>,
    pick: Pick,
    guide_from: Option<String>,
}

pub struct Live {
    /// 触るのは向こうのスレッドだけ（`crate::threads::supply`）。
    pub supply: Supply,
    pub channels: Vec<Channel>,
    pub fetched: Option<std::sync::mpsc::Receiver<Guide>>,
    pub url: Option<String>,
    pub starting: Option<Starting>,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Live")
            .field("source", &self.supply.name())
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum Confirmed {
    Act(Act),
    Quit,
}

pub struct Boot {
    pub player: Player,
    pub choices: Choices,
    pub live: Option<Live>,
    /// GLES 2.0 だけは開いたものが渡る。wgpu はこのスレッドで開く。
    pub painter: Option<Painter>,
    /// 作るのはイベントループのスレッド（winit は Windows でウィンドウのハンドルをそちらにしか渡さない）。
    pub surface: Option<wgpu::Surface<'static>>,
    #[cfg(target_os = "windows")]
    pub overlay: Option<crate::draw::composition::Overlay>,
    /// 別ウィンドウのサーフェスも同じインスタンスから作る（別のインスタンスのサーフェスはデバイスから使えない）。
    pub instance: Option<wgpu::Instance>,
    pub panes: Panes,
    pub requests: Sender<Request>,
    pub proxy: EventLoopProxy<Wake>,
    pub guide_to: Sender<ToGuide>,
    pub guide_back: Receiver<Arc<Guide>>,
    pub listed: Receiver<Box<ui::guide::epg::Listing>>,
    pub size: (u32, u32),
    pub refresh: u32,
    pub events: Receiver<ToStage>,
    pub out: Sender<FromStage>,
    pub presented: Arc<Presented>,
    pub broadcast: SyncSender<ToBroadcast>,
}

impl std::fmt::Debug for Boot {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Boot").finish_non_exhaustive()
    }
}

pub struct Stage {
    choices: Choices,
    painter: Option<Painter>,
    player: Player,
    mixer: Option<overlay::Mixer>,
    drawn: Option<overlay::Drawn>,
    #[cfg(all(unix, not(target_os = "macos")))]
    layer: crate::draw::layer::GlesLayer,
    fresh: bool,
    shown: shown::Shown,
    size: (u32, u32),
    surface: surface::Surface,
    left: u32,
    top: u32,
    wide: u32,
    tall: u32,
    /// 千分の一 Hz。
    refresh: u32,
    panes: Panes,
    requests: Sender<Request>,
    title: Option<String>,
    proxy: EventLoopProxy<Wake>,
    listed: Option<Receiver<Box<ui::guide::epg::Listing>>>,
    instance: Option<wgpu::Instance>,
    handed: Option<wgpu::Surface<'static>>,
    /// 画面のスワップチェーン（Windows）。あれば映像のサーフェスには画面を重ねない。
    #[cfg(target_os = "windows")]
    overlay: Option<crate::draw::composition::Overlay>,
    #[cfg(target_os = "windows")]
    overlay_to: Option<std::sync::mpsc::Sender<Box<overlay::Drawn>>>,
    events: Receiver<ToStage>,
    out: Sender<FromStage>,
    presented: Arc<Presented>,
    broadcast: SyncSender<ToBroadcast>,
    told_broadcast: Option<zwsdk::layer::Broadcast>,
    told_clock: Option<zwcore::guide::Moment>,
    told_scene: Option<SceneMark>,
    status: Status,
    wants_guide: bool,
    wants_info: bool,
    wants_decoder: bool,
    asked_decoder: Instant,
    mini: bool,
    mini_rows: u32,
    mini_shift: i32,
    told: Option<Option<zwcore::guide::Key>>,
    done: bool,
    /// ソースが終わった。閉じない（最後のフレームで止まり、シークバーを動かせば動き出す）。
    ended: bool,
    guide: Arc<Guide>,
    guide_to: Sender<ToGuide>,
    guide_back: Receiver<Arc<Guide>>,
    asked_guide: Instant,
    live: Option<Live>,
    player_mode: bool,
    live_key: Option<zwcore::guide::Key>,
    /// 再生モードから放送へ出ようとしてまだ開いていない。開けなければ中身の `asked_key` へ戻す。
    leaving: Option<Option<zwcore::guide::Key>>,
    profile_pending: bool,
    polled: Instant,
    osd: ui::overlay::osd::State,
    osd_shown: ui::overlay::osd::Osd,
    scene_at: Instant,
    scened: Option<(u64, (u32, u32))>,
    osded: Option<(u64, Option<supply::Ticket>, u64, Option<String>)>,
    seen: Option<(zwcore::guide::Key, u16)>,
    plugin_ui: crate::plugin_ui::Instances,
    plugin_shared: crate::plugin_ui::Shared,
    plugin_shared_told: Arc<crate::plugin_ui::Shared>,
    plugin_menus: Vec<(crate::plugin_ui::UiPlugin, Vec<zwsdk::ui::MenuItem>)>,
    plugin_menus_serial: u64,
    plugin_checked: Instant,
    plugin_late: std::collections::VecDeque<(Instant, u64)>,
    /// 通知を出した番組と通知。位置を動かすと同じ通知が来るので、同じ番組の中では出し直さない。
    noticed: Noticed,
    filter_latency: f64,
    /// ラジオは `(0, 0)`。
    formatted: Option<(u32, u32)>,
    seeking: bool,
    /// 位置を動かした先の EIT がまだ届いていない。その一回の番組の変化は OSD にしない。
    seek_settling: bool,
    band: Option<Band>,
    /// 最後に選局を要求した局。`Tables::watching` は次の PAT 等まで変わらないので、送る起点はこちら。
    asked_key: Option<zwcore::guide::Key>,
    listing: Arc<ui::guide::epg::Listing>,
    listed_mark: Option<Listed>,
    said: Option<String>,
    toast: Option<Toast>,
    /// 本体のトーストに押しのけられた進み具合つきの通知。トーストが消えたら出し直す。
    toast_behind: Option<Toast>,
    toasts: u64,
    notices: std::collections::VecDeque<String>,
    telling_since: Option<Instant>,
    osd_version: u64,
    /// 連打で毎回は書かず、一秒静かになってから `state.toml` へ。
    volume_moved: Option<Instant>,
    volume_shown: Option<(Instant, bool)>,
    volume_hold: f32,
    capture_mixer: Option<(wgpu::TextureFormat, Space, overlay::Mixer)>,
    capture_carry: Option<overlay::Drawn>,
    capture_convert: Option<capture::Converter>,
    /// GLES 2.0 のキャプチャのレンダラー。作るのに数百 ms かかるので使い回す。
    #[cfg(all(unix, not(target_os = "macos")))]
    capture_layer: crate::draw::layer::GlesLayer,
    #[cfg(all(unix, not(target_os = "macos")))]
    capture_reading: Vec<capture::Reading>,
    capture_warm: Option<Receiver<capture::Warm>>,
    capture_pending: Vec<capture::Pending>,
    errors_base: zwcore::filters::Counts,
    errors_reset_due: Option<Instant>,
    reset_errors_on_tune: bool,
    frame: crate::config::file::Frame,
    inset_on: bool,
    history: channels::History,
    favourites: channels::Loaded,
    recent_serial: u64,
    capture_said: (Sender<String>, Receiver<String>),
    confirmed: (Sender<Confirmed>, Receiver<Confirmed>),
    /// 確認のダイアログを出している間に来た確認の要る操作は捨てる。
    asking: Arc<std::sync::atomic::AtomicBool>,
    switching: Option<Receiver<Result<Live, String>>>,
    started: Option<Receiver<Result<Started, String>>>,
    starting_since: Option<(Instant, Instant, &'static str)>,
    /// 音声の切り替えを要求したときの識別子と時刻。一秒たっても変わらなければ「できません」。
    sound_asked: Option<(Option<u16>, Instant)>,
    picture_asked: Option<(Option<usize>, Instant)>,
    guide_version: u64,
    logos_written: Option<std::collections::BTreeMap<zwcore::guide::Key, zwcore::guide::LogoOf>>,
    menu_filling: Arc<crate::window::menu::Filling>,
    menu_mark: Option<MenuMark>,
    receivable: Option<Arc<std::collections::BTreeSet<zwcore::guide::Key>>>,
    recording_guide: bool,
    hidden: Vec<crate::config::key::KeyPattern>,
    plugins_seen: Option<(String, Vec<String>)>,
    oneseg: Option<(zwcore::guide::Key, zwcore::guide::Key)>,
    reception: reception::Judge,
    said_unreadable: Option<zwcore::guide::Key>,
    yielded: Option<zwcore::guide::Key>,
    roster_seeded: bool,
    buffering_told: Option<std::time::Instant>,
    catalog: Vec<catalog::Offer>,
    catalog_back: Option<Receiver<catalog::Built>>,
    catalog_due: bool,
    tried: Vec<String>,
    tried_ways: Vec<String>,
    tuning_way: Option<String>,
    verifying: Option<verify::Verifying>,
    watching_nit: Option<verify::WatchingNit>,
    unconfirmed: Vec<verify::Unconfirmed>,
    switching_to: Option<String>,
    profiles: Vec<String>,
    profile_shown: String,
    asked_tables: Instant,
    asked_info: Instant,
    last: Instant,
    /// 映像が来なくても時計は進むので時々は出し直す。
    painted: Instant,
    spent: Outside,
    gpu_late: GpuLate,
    recovery: recover::Recovery,
}

/// サーフェスの取得がフレーム間隔の 1.5 倍より遅れたループを数える。時々の遅れが続くのは GPU の
/// レンダリングが間に合っていないので通知する。毎回遅れるのはディスプレイの側が遅いので通知しない。
#[derive(Debug)]
struct GpuLate {
    since: Instant,
    turns: u32,
    late: u32,
    streak: u32,
    warned: bool,
}

impl Default for GpuLate {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            turns: 0,
            late: 0,
            streak: 0,
            warned: false,
        }
    }
}

const GPU_LATE_OVER: Duration = Duration::from_secs(1);
const GPU_LATE_LEAST: u32 = 2;
const GPU_LATE_STREAK: u32 = 3;

/// 速いディスプレイ（165 Hz など）でも垂直同期ではなくこの間隔で遅れを測る。
const OUTPUT_LEAST: Duration = Duration::from_micros(16_683);

impl std::fmt::Debug for Stage {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Stage")
            .field("size", &self.size)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

const TABLES_EVERY: Duration = Duration::from_millis(250);
pub(crate) const TOAST_FOR: f32 = 3.5;
pub(crate) const TOAST_FADE: f32 = 0.35;

const POLL_EVERY: Duration = Duration::from_secs(1);

/// coaxd のタイムアウト（ヘッダーに 30 秒、本文に 20 秒）より長く。
const TUNE_PATIENCE: Duration = Duration::from_secs(45);

/// テーブルの入れ替わりと映像の場所の大きさが変わったときは待たない。
const SCENE_EVERY: Duration = Duration::from_millis(100);

const MINI_HOURS: u32 = 2;

const MINI_SHIFT_MOST: i32 = 7 * 24;

/// 映像のサーフェスを取って出すのに掛かった時間（コンポジターに待たされる分）。
#[derive(Debug)]
struct Outside {
    acquire: Duration,
    acquire_most: Duration,
    present: Duration,
    present_most: Duration,
    turns: u64,
    began: Instant,
}

impl Default for Outside {
    fn default() -> Self {
        Self {
            acquire: Duration::ZERO,
            acquire_most: Duration::ZERO,
            present: Duration::ZERO,
            present_most: Duration::ZERO,
            turns: 0,
            began: Instant::now(),
        }
    }
}

impl Outside {
    fn say(&mut self) {
        let over = self.began.elapsed();
        if over.as_secs() < 5 {
            return;
        }
        let seconds = over.as_secs_f64();
        let ms = |spent: Duration| spent.as_secs_f64() * 1000.0 / seconds;
        let most = |spent: Duration| spent.as_secs_f64() * 1000.0;
        tracing::debug!(
            "映像のサーフェス {:.1} 秒: 表示 {} 枚、1 秒あたり ms — 取得 {:.0}（最大 {:.1}）、提示 {:.0}（最大 {:.1}）",
            seconds,
            self.turns,
            ms(self.acquire),
            most(self.acquire_most),
            ms(self.present),
            most(self.present_most),
        );
        *self = Self::default();
    }
}

/// これより遅いとコンポジターがウィンドウを保持していたと見る。
const STALLED: Duration = Duration::from_millis(100);

/// 「離した」は 120 ms 止まっただけでも来る。
const GRIP_HOLD: Duration = Duration::from_millis(1500);

/// 毎回問い合わせると隠れたウィンドウのために処理が回る。
const UNSEEN_FOR: Duration = Duration::from_millis(250);

type SceneMark = (
    u64,
    u32,
    scene::Backdrop,
    u32,
    u64,
    u64,
    bool,
    Option<u32>,
    Option<(bool, bool, u32)>,
    usize,
    (Option<(u16, usize)>, Option<u64>),
    // タプルは 12 要素までしか比べられないのでまとめてある。
    (Option<(u32, bool, bool, u32)>, Option<(usize, i64, usize)>),
);

#[derive(Debug, Clone)]
struct Toast {
    said: String,
    shown: crate::draw::scene::Toasted,
    at: Instant,
    serial: u64,
    hold: f32,
}

impl Toast {
    /// 一行目を見出し、残りを詳細に。詳細の行の数だけ長く出す。
    fn of(text: &str, repeated: u32, serial: u64) -> Self {
        let mut lines = text.split('\n').filter(|line| !line.is_empty());
        let said = lines.next().unwrap_or_default().to_owned();
        let details: Vec<String> = lines.map(str::to_owned).collect();
        let title = if repeated > 1 {
            format!("{said}（{repeated} 回目）")
        } else {
            said.clone()
        };
        tracing::info!(
            "{}",
            std::iter::once(title.as_str())
                .chain(details.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        let hold = TOAST_FOR + details.len() as f32 * TOAST_PER_LINE;
        Self {
            said,
            shown: crate::draw::scene::Toasted {
                title,
                details,
                ..Default::default()
            },
            at: Instant::now(),
            serial,
            hold,
        }
    }

    fn gone(&self) -> bool {
        self.at.elapsed().as_secs_f32() >= self.hold + TOAST_FADE
    }
}

const TOAST_PER_LINE: f32 = 1.0;

const TOAST_PROGRESS_HOLD: f32 = f32::INFINITY;

/// すぐ済んだときに一瞬で消えないように。
const TOAST_LEAST: f32 = 2.0;

const TOAST_REPEAT_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn said_with(title: &str, why: &dyn std::fmt::Display) -> String {
    let why = why.to_string();
    if why.trim().is_empty() {
        title.to_owned()
    } else {
        format!("{title}\n{why}")
    }
}

pub(crate) fn id_text(value: u16) -> String {
    zwcore::si::hex_id(value, 4)
}

fn repeat_of(before: &Toast, said: &str) -> u32 {
    let said = said.split('\n').next().unwrap_or_default();
    if before.said != said || before.at.elapsed() > TOAST_REPEAT_WITHIN {
        return 1;
    }
    let counted = before
        .shown
        .title
        .strip_prefix(said)
        .and_then(|rest| rest.strip_prefix("（"))
        .and_then(|rest| rest.strip_suffix(" 回目）"))
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(1);
    counted + 1
}

type MenuMark = (
    u64,
    u64,
    Option<usize>,
    Option<zwcore::guide::Key>,
    (Option<zwcore::guide::Key>, u64),
    Option<usize>,
    Option<usize>,
    bool,
    zwcore::engine::DualMono,
    u32,
    (bool, bool),
    // 回数、いまの分（番組名を添えるので、時刻が進めばビルドし直す）。
    (String, (u64, u64, i64)),
);

const GUIDE_EVERY: Duration = Duration::from_secs(3);
const GUIDE_EVERY_IDLE: Duration = Duration::from_secs(10);

fn blackout(device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("blackout"),
    });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("blackout"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    queue.submit([encoder.finish()]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    Mirakurun {
        url: String,
        by: zwcore::input::source::mirakurun::By,
    },
    Coaxd {
        endpoint: String,
        priority: i32,
        tuners: Vec<String>,
        profile: String,
    },
}

impl Opening {
    /// 開き直す必要があるかを比べるためだけ。
    fn came(&self) -> String {
        match self {
            Self::Mirakurun { url, .. } => url.clone(),
            Self::Coaxd {
                endpoint,
                tuners,
                profile,
                ..
            } => tuning::came_of_coaxd(endpoint, tuners, profile),
        }
    }
}

fn fetch_guide(url: String) -> Receiver<Guide> {
    let (send, fetched) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(String::from("guide"))
        .spawn(move || {
            let began = Instant::now();
            let Ok(mut source) = zwcore::input::source::mirakurun::Mirakurun::at(&url) else {
                return;
            };
            match zwcore::input::source::Source::guide(&mut source) {
                Ok(Some(guide)) => {
                    tracing::info!(
                        "全局の番組表 {} 局・{} 番組（{:.1} 秒）",
                        guide.services().count(),
                        guide.programme_count(),
                        began.elapsed().as_secs_f64()
                    );
                    let _ = send.send(guide);
                }
                Ok(None) => {}
                Err(why) => tracing::warn!("全局の番組表を取得できない: {why}"),
            }
        });
    if let Err(why) = spawned {
        tracing::warn!("番組表を取得するスレッドを起動できない: {why}");
    }
    fetched
}

fn live_is_empty(live: Option<&Live>) -> bool {
    live.is_none_or(|live| live.channels.is_empty())
}

fn first_source(file: &crate::config::File, profile: &str) -> Option<Opening> {
    openings(file, profile)
        .into_iter()
        .next()
        .map(|(_, opening)| opening)
}

fn uses_source(file: &crate::config::File, profile: &str, now: Option<&str>) -> bool {
    now.is_some_and(|now| {
        openings(file, profile)
            .iter()
            .any(|(_, opening)| opening.came() == now)
    })
}

pub(crate) fn openings(file: &crate::config::File, profile: &str) -> Vec<(String, Opening)> {
    let profile = file.profile(profile);
    let (entries, _) = file.entries_of(&profile);
    let scanned = crate::config::channels_profile(&profile.name).to_owned();
    entries
        .into_iter()
        .filter_map(|entry| {
            let source = entry.source;
            let opening = match source.kind.as_str() {
                "mirakurun" if !source.url.is_empty() => Opening::Mirakurun {
                    url: source.url.clone(),
                    by: zwcore::input::source::mirakurun::By::parse(&source.by)
                        .unwrap_or(zwcore::input::source::mirakurun::By::Auto),
                },
                "coaxd" => Opening::Coaxd {
                    endpoint: source.endpoint.clone(),
                    priority: source.priority,
                    tuners: entry.tuner.into_iter().collect(),
                    profile: scanned.clone(),
                },
                _ => return None,
            };
            Some((entry.name, opening))
        })
        .collect()
}
