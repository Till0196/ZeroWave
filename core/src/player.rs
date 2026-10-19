// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一つの放送を再生する。読み取り・分離・デコードのスレッドと音声のスレッドを持つ。
//! 画面のスレッドが [`Player::tick`] をループの一回ごとに呼ぶ。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use mediadecoder::es::Format;

use crate::data::{Feed, ToBroadcast};
use crate::decode::{Decoded, DeinterlaceBy, Prefer};
use crate::engine::clock::HZ;
use crate::engine::decoding::Decoding;
use crate::engine::{DualMono, Engine, Out};
use crate::filters::{Container, Counted, Counts, Kind, Stream};
use crate::guide::{Guide, Programme, Service};
use crate::input::growing::Growing;
use crate::output::video::Error as VideoError;
use crate::output::video::{Deinterlace, Settings};
use crate::plugins::Loaded;
use crate::screen::{Canvas, Painter, Screen, Spent};
use crate::si::{BroadcasterInfo, Identity, NetworkInfo, ServiceInfo};
use crate::sound::Device;
use crate::sound::mix::Mix;
use crate::sound::sounding::{Say, Shared};

pub use crate::decode::{AHEAD, HANDOVER, PREROLL_PICTURES};
pub use meter::Decoder;
pub use stats::{CodedStructure, SoundStats, VideoStats, structure_of};

/// 再生の設定のうち、ウィンドウとレンダリングに関わらないもの。既定はどれも `auto`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub decoder: Option<Prefer>,
    pub api: crate::decode::Api,
    pub deinterlace: Option<Deinterlace>,
    pub deinterlace_by: DeinterlaceBy,
    pub telecine: Option<bool>,
    /// 1 から数える。`None` は番組の先頭で、「1」とは違う。
    pub audio: Option<usize>,
    pub dual_mono: DualMono,
    pub video: Option<usize>,
    /// デバイスが報告する分に加える出力の遅延（ミリ秒）。負なら映像を遅らせる。
    pub audio_latency: i32,
    pub mix: Mix,
    pub volume: u32,
    pub muted: bool,
    pub scaling: crate::output::video::Scaling,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            decoder: None,
            api: crate::decode::Api::Auto,
            deinterlace: None,
            deinterlace_by: DeinterlaceBy::default(),
            telecine: None,
            audio: None,
            dual_mono: DualMono::default(),
            video: None,
            audio_latency: 0,
            mix: Mix::default(),
            volume: crate::sound::volume::MAX,
            scaling: crate::output::video::Scaling::default(),
            muted: false,
        }
    }
}

impl Options {
    pub fn prefer(&self) -> Prefer {
        self.decoder.unwrap_or_default()
    }
}

const CHUNK: usize = 64 << 10;

const BLOCKS_RECORDING: usize = 1024;

/// 放送のリングバッファに持つ時間の既定（ミリ秒）。
const HOLD_MS: u32 = 2_000;

/// 録画の読み込みが詰まったとみなす条件と、再開する先読みの量。
const STARVED_LEAST: std::time::Duration = std::time::Duration::from_millis(100);
const LEAD_LOW: i64 = HZ / 2;
const LEAD_RESUME: i64 = 2 * HZ;

/// 表示に間に合わないフレームがこれより多くたまっていたら、一回でまとめて飛ばす（既定）。
const PILE_MOST: usize = 3;

/// 持ちすぎるとデコーダーのリングバッファで古いフレームが上書きされ、映像が前後に跳ぶ。
const _: () = assert!(
    HANDOVER + AHEAD <= crate::decode::HELD_BY_PLAYER,
    "the player holds more decoded pictures than the decoders' rings are cut for"
);

/// 次のフレームの時刻をこれ以上過ぎていなければ、現在のフレームは捨てずにレンダリングする。
/// 59.94 fps の映像を 60 Hz で出すとフレームごとの揺らぎで一枚捨てがちになるので、一フレーム分待つ。
const BEHIND_SLACK: i64 = HZ * 16 / 1000;

/// The one deep buffer in the player (five seconds at 60/s).
const ACCESS: usize = 300;

/// 符号化データのチャネルにバッファする時間の上限。ライブ再生で自然にたまる量（二秒近く）より十分大きく取る。
const SPAN_MOST: i64 = 7 * HZ / 2;

/// 録画を読むとき、表示している時刻よりどれだけ先まで読むか。[`SPAN_MOST`] の一秒下。
pub(crate) const AHEAD_MOST: i64 = 5 * HZ / 2;

/// 一フレーム分より短く（長いとチャネルが一時的に空になる）。
const PACE_NAP: Duration = Duration::from_millis(5);

const PACE_MOST: Duration = Duration::from_millis(500);

/// 番組の音声はすべて届いて本数で分け合うので、四本まで [`SPAN_MOST`] を越える容量にする。
const REST: usize = 1024;

/// 映像がこれだけ遅れたら映像も読み飛ばす。選局直後の一秒近い遅れでは発動しない値にする。
const BEHIND_MOST: i64 = 2 * HZ;

const BEHIND_APART: Duration = Duration::from_secs(2);

/// 選局直後だけ [`BEHIND_MOST`] の代わりに使う（選局直後は二秒を越えてたまることがある）。
const SETTLING_BEHIND: i64 = HZ / 2;

const SETTLING: Duration = Duration::from_secs(5);

const HEAD: usize = 4 << 20;

/// The most the wall clock may move the stream's clock in one turn (three pictures).
const MOST_ADVANCE: i64 = HZ / 10;

mod away;
mod meter;
mod names;
mod run;
mod stats;
pub mod supply;
mod tune;
mod turn;

use meter::{Decodes, Filling, First, Meter, Reading};
use run::{Stamped, Wanted, decode, fill, read};

struct Due {
    at: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct Tick {
    pub more: bool,
    pub reconfigure: bool,
    /// Without it the loop asks millions of times a second, taking the sound callback's lock.
    pub did: bool,
}

#[derive(Debug)]
pub struct Recording {
    path: String,
    bytes: u64,
    span: Option<crate::input::span::Span>,
    opened_at: i64,
    program: Option<u16>,
    measured: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub fraction: f32,
    /// 先頭から、90 kHz。
    pub position: i64,
    pub length: i64,
    pub paused: bool,
    /// 録画の読み込みが詰まって一時停止しているか（ユーザーが止めたのではない）。
    pub buffering: bool,
    pub pace: f64,
}

pub struct Player {
    engine: Option<Engine>,
    container: Container,
    file: Option<BufReader<Growing>>,
    recording: Option<Recording>,
    live: Option<crate::input::source::Bytes>,
    live_source: bool,
    pictures: Option<Receiver<Out>>,
    say: Option<Sender<Say>>,
    shared: Arc<Shared>,
    has_sound: bool,
    first_picture: Option<Instant>,
    passthrough: crate::sound::Passthrough,
    rate: u32,
    tables: Option<Receiver<Tables>>,
    last_tables: Tables,
    paused: bool,
    display_mhz: u32,
    told_speed: f64,
    buffering: bool,
    pace: f64,
    carry: Wanted,
    guides: Option<Receiver<Guide>>,
    asking: Option<std::sync::mpsc::Sender<Ask>>,
    ready_to_start: Option<(Engine, crate::input::source::Bytes)>,
    video: Option<Screen>,
    format: Option<Format>,
    settings: Settings,
    options: Options,
    plugins: Option<Arc<Loaded>>,
    said_no_hardware: bool,
    skip_to: Arc<AtomicI64>,
    /// 最初にデコードできたフレームの時刻。音声は映像より早く届くことがあるので、音声のスレッドが参照する。
    video_first: Arc<AtomicI64>,
    video_decoded: Arc<AtomicU64>,
    video_expected: Arc<AtomicBool>,
    video_ready: Arc<AtomicBool>,
    /// 選局でセットする（でないと前の局のデータ放送が次の局で開く）。
    stop: Arc<AtomicBool>,
    hidden: Arc<AtomicBool>,
    unseen: u64,
    left_at: Option<Instant>,
    left: u64,
    away: away::Away,
    video_latency: i64,
    video_held: i64,
    /// The frame pool refuses a picture when full; it waits here rather than be lost.
    ready: VecDeque<(i64, Decoded)>,
    due: VecDeque<Due>,
    done: bool,
    tuned: Instant,
    first: First,
    told: u64,
    /// 要求した選局のストリームがまだ来ていない。真の間は、ソースが無くても終わりではない。
    /// 偽に戻すのは [`Player::retune`] だけ（ほかで戻すと deck が終了する）。
    holding: bool,
    /// The GPU holds one frame back waiting for the next.
    flushed: bool,
    drawn: u64,
    tuned_counts: (u64, u64),
    late: u64,
    numbered: Option<(i64, u64)>,
    redrawn: u64,
    #[cfg(all(feature = "wgpu", feature = "plugins"))]
    filter_host: Option<Vec<String>>,
    since: i64,
    stalled: bool,
    drew: bool,
    drew_at: Option<Instant>,
    refused: bool,
    took: u64,
    meter: Meter,
    read_bytes: Arc<AtomicU64>,
    /// 放送のリングバッファに持つ時間（ミリ秒）。開いているバッファにもすぐ効く。
    hold: Arc<AtomicU32>,
    /// 表示に間に合わないフレームがこれより多くたまっていたら、一回でまとめて飛ばす。
    pile_most: usize,
    read_stamp: Arc<AtomicI64>,
    read_stamp_first: Arc<AtomicI64>,
    dropped_access: Arc<AtomicU64>,
    newest_access: Arc<AtomicU64>,
    oldest_access: Arc<AtomicU64>,
    /// 0 なら `oldest_access` は意味を持たない（空のまま古くなり、全部捨ててしまう）。
    queued_access: Arc<AtomicI64>,
    newest_sound: Arc<AtomicU64>,
    spent_reading: Arc<Reading>,
    spent_filling: Arc<Filling>,
    ring: Option<Arc<crate::input::buffer::Buffer>>,
    spent_decoding: Arc<Decodes>,
    /// `Screen::spent` は読むたびに 0 に戻るので、読むのは一か所だけ。
    spent_meter: Spent,
    spent_total: Spent,
    rate_sample: (Instant, u64),
    bitrate: f64,
    data_to: Option<SyncSender<ToBroadcast>>,
    inset: Option<[f32; 5]>,
    pan_scan: crate::output::video::pan_scan::PanScan,
}

impl Drop for Player {
    /// 停止フラグを立てずに drop すると、来ないバイト列を待つスレッドが残る。
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for Player {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Player")
            .field("options", &self.options)
            .field("holding", &self.holding)
            .finish_non_exhaustive()
    }
}

impl Player {
    pub fn broadcast_to(&mut self, to: SyncSender<ToBroadcast>) {
        self.data_to = Some(to);
    }

    pub fn sound_sender(&self) -> Option<Sender<Say>> {
        self.say.clone()
    }

    pub fn set_pan_scan(&mut self, pan_scan: crate::output::video::pan_scan::PanScan) {
        if self.pan_scan == pan_scan {
            return;
        }
        self.pan_scan = pan_scan;
        if let Some(video) = &mut self.video {
            video.set_pan_scan(pan_scan);
        }
    }

    pub fn pan_scan(&self) -> crate::output::video::pan_scan::PanScan {
        self.pan_scan
    }

    pub fn set_inset(&mut self, inset: Option<[f32; 5]>) {
        if self.inset == inset {
            return;
        }
        self.inset = inset;
        if let Some(video) = &mut self.video {
            video.set_inset(inset);
        }
    }

    fn tell_sound(&self, say: Say) {
        if let Some(to) = self.say.as_ref() {
            let _ = to.send(say);
        }
    }

    pub fn ask_guide(&self) {
        self.ask(Ask::Guide);
    }

    pub fn taken_guide(&mut self) -> Option<Guide> {
        self.guides.as_ref()?.try_iter().last()
    }

    pub fn ask_tables(&self) {
        self.ask(Ask::Tables);
    }

    pub fn tables(&self) -> &Tables {
        &self.last_tables
    }

    /// 視聴中の番組の映像か音声が、デコードできない形式のまま届いているか。
    pub fn unreadable(&self) -> bool {
        self.shared.unreadable.load(Ordering::Relaxed)
    }

    pub fn picture_size(&self) -> Option<(u32, u32)> {
        self.picture_format()
            .map(|format| (format.width, format.height))
    }

    /// レンダリング前のフレームとレンダラーが保持している物を捨てる。映像出力もデコーダーも作り直さない。
    pub fn reset_viewer(&mut self) {
        if let Some(video) = &mut self.video {
            video.reset();
        }
        self.ready.clear();
        self.due.clear();
    }

    /// 映像出力を捨てて次のフレームで作り直させる。映像の形式は保持する。
    pub fn rebuild_screen(&mut self) {
        self.video = None;
        self.ready.clear();
        self.due.clear();
    }

    /// 再生オフの間は映像と音声をデコードせず（受信と録画は続く）、レンダラーも解放する。
    pub fn set_playback(&mut self, on: bool) {
        self.shared.viewer_off.store(!on, Ordering::Relaxed);
        if !on {
            self.video = None;
            self.ready.clear();
            self.due.clear();
        }
    }

    pub fn playback(&self) -> bool {
        !self.shared.viewer_off.load(Ordering::Relaxed)
    }

    /// 最後に表示したフレームと重ねる物を一枚にレンダリングする。映像出力が無ければ `None`。
    #[cfg(unix)]
    pub fn capture_gles(
        &mut self,
        size: (u32, u32),
        video: Option<(u32, u32, u32, u32)>,
        layer: Option<&dyn crate::output::present::Layer>,
    ) -> Option<Result<crate::output::present::Readback, crate::output::video::Error>> {
        self.video
            .as_mut()
            .map(|screen| screen.capture_gles(size, video, layer))
    }

    pub fn has_screen(&self) -> bool {
        self.video.is_some()
    }

    /// 最後にレンダリングしたフレームをもう一度レンダリングする。時計もキューも動かさない。
    pub fn redraw(&mut self, canvas: &crate::screen::Canvas<'_>) -> bool {
        match self.video.as_mut() {
            Some(video) => {
                video.redraw(canvas);
                true
            }
            None => false,
        }
    }

    pub fn redraw_unfiltered(&mut self, canvas: &crate::screen::Canvas<'_>) -> bool {
        match self.video.as_mut() {
            Some(video) => {
                video.redraw_unfiltered(canvas);
                true
            }
            None => false,
        }
    }

    #[cfg(all(feature = "wgpu", feature = "plugins"))]
    pub fn filter_report(&self) -> Option<crate::output::video::hook::Report> {
        let host = self.filter_host.as_ref();
        let mut report = self
            .video
            .as_ref()
            .and_then(Screen::filter_report)
            .unwrap_or_default();
        if report.host.is_empty() {
            report.host = host?.clone();
        }
        Some(report)
    }

    #[cfg(all(feature = "wgpu", feature = "plugins"))]
    fn remember_host(&mut self, painter: &Painter) {
        if self.filter_host.is_none() {
            self.filter_host = Some(painter.host_jobs());
        }
    }

    #[cfg(feature = "plugins")]
    pub fn set_filter_program(&mut self, program: Option<zwsdk::video::Program>) {
        if let Some(video) = self.video.as_mut() {
            video.set_filter_program(program);
        }
    }

    pub fn picture_format(&self) -> Option<Format> {
        self.format
            .filter(|format| format.width > 0 && format.height > 0)
    }

    pub fn set_video_latency(&mut self, ms: f64) {
        let samples = (ms * f64::from(self.rate) / 1000.0).round() as i64;
        if samples == self.video_latency {
            return;
        }
        self.video_latency = samples;
        self.shared
            .video_latency
            .store(samples + self.video_held, Ordering::Relaxed);
        tracing::debug!("表示の遅延 {ms:.1} ms、音声を同じだけ遅らせる");
    }

    /// 映像フィルターが映像を遅らせる時間（ミリ秒）。表示の遅延とは別に加える。
    pub(crate) fn set_video_held(&mut self, ms: f64) {
        let samples = (ms * f64::from(self.rate) / 1000.0).round() as i64;
        if samples == self.video_held {
            return;
        }
        self.video_held = samples;
        self.shared
            .video_latency
            .store(self.video_latency + samples, Ordering::Relaxed);
        tracing::debug!("映像フィルターの遅延 {ms:.1} ms、音声を同じだけ遅らせる");
    }

    pub fn heard_rate(&self) -> Option<u32> {
        match self.shared.heard_rate.load(Ordering::Relaxed) {
            0 => None,
            rate => Some(rate),
        }
    }

    /// ウィンドウが隠れている間もデコードは続け、表示時刻を過ぎたフレームを捨てる。デコードせずに読み飛ばすと、
    /// 戻ったとき次のランダムアクセスポイントまで待ち、古いフレームが一瞬出る。
    pub fn set_hidden(&mut self, hidden: bool) {
        let was = self.hidden.swap(hidden, Ordering::Relaxed);
        if hidden && !was {
            self.go_away();
        } else if !hidden && was {
            self.come_back();
        }
    }

    pub fn surface_timed_out(&mut self) {
        self.away.timed_out();
    }

    pub fn told(&self) -> u64 {
        self.told
    }

    pub fn hold(&mut self) {
        self.holding = true;
    }

    /// 要求した選局を取りやめて前のソースのまま続ける。
    pub fn unhold(&mut self) {
        self.holding = false;
    }

    pub fn screen_ready(&self) {
        self.video_ready.store(true, Ordering::Relaxed);
    }

    pub fn has_picture_on_gpu(&self) -> bool {
        self.video.as_ref().is_some_and(|video| video.pending() > 0)
    }

    pub fn now(&self) -> Option<i64> {
        self.shared.with_clock(|clock| clock.now())
    }

    pub fn between(&self) -> bool {
        self.holding
    }

    /// まだ一枚もレンダリングしていない、選局待ち、時計が動いていないのどれか。
    pub fn blank(&self) -> bool {
        !self.ever_drawn() || self.holding || self.now().is_none()
    }

    pub fn fault(&self) -> Option<String> {
        self.shared.fault()
    }

    pub fn data_only(&self) -> bool {
        let tables = &self.last_tables;
        tables.watching.is_some() && tables.video.is_none() && tables.sound.is_none()
    }

    pub fn audio_only(&self) -> bool {
        let tables = &self.last_tables;
        tables.watching.is_some() && tables.video.is_none() && tables.sound.is_some()
    }

    /// 起動してから一度でも映像を表示したか（選局してもリセットしない）。
    pub fn ever_drawn(&self) -> bool {
        self.drawn > 0
    }

    pub fn ask(&self, ask: Ask) {
        if let Some(asking) = &self.asking {
            let _ = asking.send(ask);
        }
    }

    pub fn dual_mono(&self) -> DualMono {
        self.options.dual_mono
    }

    pub fn set_dual_mono(&mut self, mode: DualMono) {
        self.options.dual_mono = mode;
    }

    /// 選局後に安定したか（[`SETTLING`]）。
    pub fn settled(&self) -> bool {
        self.tuned.elapsed() >= SETTLING
    }

    pub fn scaling(&self) -> crate::output::video::Scaling {
        self.options.scaling
    }

    /// デコーダー・デインターレース・映像の本数は次の選局から反映する。拡大縮小・逆テレシネ・出力の遅延はすぐ反映する。
    pub fn set_decoding(&mut self, options: &Options) {
        self.options.decoder = options.decoder;
        self.options.api = options.api;
        crate::decode::set_api(options.api);
        self.options.deinterlace = options.deinterlace;
        self.options.deinterlace_by = options.deinterlace_by;
        self.options.video = options.video;
        if self.options.scaling != options.scaling {
            self.options.scaling = options.scaling;
            if let Some(video) = self.video.as_mut() {
                video.set_scaling(options.scaling);
            }
        }
        if self.options.telecine != options.telecine {
            self.options.telecine = options.telecine;
            let mut settings = self.settings;
            settings.telecine = options.telecine.unwrap_or(true)
                && settings.deinterlace != crate::output::video::Deinterlace::Weave;
            self.settings = settings;
            if let Some(video) = self.video.as_mut() {
                video.set_settings(settings);
            }
            tracing::info!(
                "逆テレシネ: {}",
                if settings.telecine {
                    "オン"
                } else {
                    "オフ"
                }
            );
        }
        if self.options.audio_latency != options.audio_latency {
            self.options.audio_latency = options.audio_latency;
            let extra = i64::from(options.audio_latency) * i64::from(self.rate) / 1000;
            self.tell_sound(Say::Latency(extra));
        }
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    /// 放送のリングバッファに持つ時間。
    pub fn set_buffer(&mut self, hold: std::time::Duration) {
        let millis = u32::try_from(hold.as_millis()).unwrap_or(u32::MAX).max(1);
        self.hold.store(millis, Ordering::Relaxed);
    }

    /// 表示に間に合わないフレームを、何枚たまったらまとめて飛ばすか。
    pub fn set_queue(&mut self, frames: usize) {
        self.pile_most = frames.max(1);
    }

    pub fn set_mix(&mut self, mix: Mix) {
        if self.options.mix == mix {
            return;
        }
        self.options.mix = mix;
        self.tell_sound(Say::Mix(Box::new(mix)));
    }

    pub fn mix(&self) -> Mix {
        self.options.mix
    }

    pub fn set_volume(&mut self, volume: u32) {
        self.options.volume = volume.min(crate::sound::volume::MAX);
        self.tell_gain();
    }

    pub fn volume(&self) -> u32 {
        self.options.volume
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.options.muted = muted;
        self.tell_gain();
    }

    pub fn muted(&self) -> bool {
        self.options.muted
    }

    /// 音声を常にビットストリームのまま出力しているか（そのとき音量は掛からない）。
    pub fn passes_sound_through(&self) -> bool {
        self.passthrough == crate::sound::Passthrough::On
    }

    fn tell_gain(&self) {
        let gain = crate::sound::volume::gain(self.options.volume, self.options.muted);
        self.tell_sound(Say::Gain(gain));
    }
}

/// ユーザーが選べる音声の一つ。二重音声は一本のストリームから三つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundChoice {
    pub label: String,
    pub stream: usize,
    pub mode: Option<DualMono>,
}

#[derive(Debug, Clone)]
pub enum Ask {
    /// The next picture, going round at the end (rain-fade pairs).
    NextPicture,
    Sound(usize),
    Picture(usize),
    DualMono(DualMono),
    Tables,
    Guide,
}

#[derive(Debug, Clone, Default)]
pub struct Tables {
    pub identity: Option<Identity>,
    pub services: Vec<Service>,
    pub watching: Option<crate::guide::Key>,
    pub sound: Option<Stream>,
    pub sound_at: Option<usize>,
    pub dual_mono: bool,
    pub he_aac: bool,
    pub sound_rate: Option<u32>,
    /// 二重音声の一本は三つ（主・副・主＋副）に広げてある。
    pub choices: Vec<SoundChoice>,
    pub choice_at: Option<usize>,
    pub picture_at: Option<usize>,
    pub pictures: Vec<String>,
    pub programs: Vec<crate::filters::Program>,
    /// 現在のストリームの SDT が示したサービス。番組表由来ではない。
    pub neighbours: Vec<ServiceInfo>,
    pub frame_header: Option<isdb::tsmf::Info>,
    pub video: Option<Kind>,
    pub now: Option<Programme>,
    pub next: Option<Programme>,
    pub networks: Vec<NetworkInfo>,
    pub broadcasters: Vec<BroadcasterInfo>,
    /// 緊急情報記述子が示しているサービス。終了信号も含む。
    pub emergency: Vec<isdb::nitdescriptors::EmergencyService>,
    /// 番組一覧のテーブル（PAT / PLT）が示したサービス。
    pub listed: Option<Vec<u16>>,
    pub emm: Vec<(u16, u16)>,
    pub downloads: Vec<isdb::tstables::SoftwareDownload>,
    pub logos: Vec<crate::si::LogoImage>,
    pub partial: Option<crate::si::PartialInfo>,
    pub packages: Vec<isdb::mmttables::ListedPackage>,
    pub layouts: Vec<isdb::mmttables::Layout>,
    /// MH-AIT の識別子と、載っているアプリケーションの数。
    pub applications: Vec<(u16, usize)>,
    pub counts: Counts,
    pub container: Container,
    pub pids: Vec<(u16, Counted)>,
    pub carried: Vec<(u16, u16, String)>,
    pub clock: Option<crate::guide::Moment>,
}

fn frame_ticks(format: Format) -> i64 {
    if format.rate.0 == 0 {
        return 3_003;
    }
    i64::from(format.rate.1) * HZ / i64::from(format.rate.0)
}
