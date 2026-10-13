// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 音声を出力するスレッド。音声がマスタークロックなので、サーフェスを取得するスレッドから分けてある。

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::clock::{Clock, Drift};
use crate::engine::{Out, QUEUE};
use crate::sound::iec61937;
use crate::sound::mix::{self, Mix};
use crate::sound::resample::Resampler;
use crate::sound::stretch::Stretcher;
use mediadecoder::audio::{Sound, aiff};

use super::Device;

/// Depth the queue actually reaches, or taking never stops.
pub const AHEAD: u64 = QUEUE * 2;

const OPEN_WITH: u64 = QUEUE;
const OPEN_MOST: Duration = Duration::from_millis(100);
/// 音声がそろっても、最初のフレームがデコードできるまではこれだけ待つ。
const OPEN_FOR_VIDEO: Duration = Duration::from_millis(1500);
/// 映像のデータが届いているときの上限（デコーダーが一枚も出力しないときのため）。
const OPEN_FOR_VIDEO_EXPECTED: Duration = Duration::from_secs(10);
const VIDEO_MARGIN: i64 = 90 * 60;
const PREROLL_MOST: Duration = Duration::from_millis(1500);

/// How far the queue may wander from [`AHEAD`] before a coded frame is dropped or repeated.
const SLACK: u64 = 4 * iec61937::BLOCK as u64;

/// 何も来ていないときのスリープ時間。ループの間隔が、デバイスのバッファ量を見る精度になる。
const REST_FOR: Duration = Duration::from_millis(1);
const IDLE_REST_FOR: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub struct Shared {
    /// 書くのは音声のスレッド、読むのはレンダリングスレッド。ロックの中では足し算だけ。
    pub clock: Mutex<Clock>,
    pub picture: AtomicBool,
    pub ended: AtomicBool,
    pub queued: AtomicU64,
    pub heard: AtomicU64,
    pub missed: AtomicU64,
    pub dropped: AtomicU64,
    pub repeated: AtomicU64,
    pub heard_rate: AtomicU32,
    pub video_latency: AtomicI64,
    /// 現在再生する音声。エンジンは全部の音声を流し、どれを再生するかはデバイスの手前で決める。
    pub sound: AtomicU32,
    /// 音声出力が続けられなくなった理由。クロックはもう進まない。
    pub fault: Mutex<Option<String>>,
    /// 視聴中の番組が読めない。
    pub unreadable: AtomicBool,
    pub viewer_off: AtomicBool,
    pub shape: Mutex<Option<Shape>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub rate: u32,
    pub speakers: Vec<mediadecoder::audio::Speaker>,
    pub mixdown: mediadecoder::audio::Mixdown,
    pub out: usize,
    pub device: usize,
}

pub const NO_SOUND: u32 = u32::MAX;

impl Shared {
    pub fn new(rate: u32) -> Self {
        Self {
            clock: Mutex::new(Clock::new(rate)),
            picture: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            queued: AtomicU64::new(0),
            heard: AtomicU64::new(0),
            missed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            repeated: AtomicU64::new(0),
            heard_rate: AtomicU32::new(0),
            video_latency: AtomicI64::new(0),
            sound: AtomicU32::new(NO_SOUND),
            fault: Mutex::new(None),
            unreadable: AtomicBool::new(false),
            viewer_off: AtomicBool::new(false),
            shape: Mutex::new(None),
        }
    }

    pub fn fault(&self) -> Option<String> {
        self.fault
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn with_clock<T>(&self, look: impl FnOnce(&mut Clock) -> T) -> T {
        let mut clock = self
            .clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        look(&mut clock)
    }
}

pub enum Say {
    Play {
        rest: Receiver<Out>,
        /// 読み取りスレッドに、この時刻より前のデータを捨てるよう伝える値。
        skip_to: Arc<AtomicI64>,
        video_first: Arc<AtomicI64>,
        video_decoded: Arc<AtomicU64>,
        video_expected: Arc<AtomicBool>,
        video_ready: Arc<AtomicBool>,
        live: bool,
    },
    /// クロックとデバイスを 0 に戻す。選局では [`Say::Play`] が続く。
    Stop {
        rate: u32,
    },
    Pause(bool),
    Speed(f64),
    /// 受信機の内蔵音（TR-B14 第二分冊 第三編 第 2 部 3.3.5）。
    Chime(u32),
    Aiff(Vec<u8>),
    Mix(Box<Mix>),
    /// デバイスのコールバックで掛かるのでキューを待たずに効く。
    Gain(f32),
    Latency(i64),
}

impl std::fmt::Debug for Say {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Play { live, .. } => write!(out, "Play {{ live: {live} }}"),
            Self::Stop { rate } => write!(out, "Stop {{ rate: {rate} }}"),
            Self::Pause(paused) => write!(out, "Pause({paused})"),
            Self::Speed(pace) => write!(out, "Speed({pace})"),
            Self::Chime(number) => write!(out, "Chime({number})"),
            Self::Aiff(bytes) => write!(out, "Aiff({} bytes)", bytes.len()),
            Self::Mix(mix) => write!(out, "Mix({mix:?})"),
            Self::Gain(gain) => write!(out, "Gain({gain})"),
            Self::Latency(extra) => write!(out, "Latency({extra})"),
        }
    }
}

struct Sounding {
    shared: Arc<Shared>,
    /// 無いときも受け取って捨てないとチャネルが詰まって読み取りが止まる。
    device: Option<Device>,
    says: Receiver<Say>,
    rest: Option<Receiver<Out>>,
    skip_to: Option<Arc<AtomicI64>>,
    video_first: Option<Arc<AtomicI64>>,
    video_decoded: Option<Arc<AtomicU64>>,
    video_expected: Option<Arc<AtomicBool>>,
    video_ready: Option<Arc<AtomicBool>>,
    live: bool,
    ahead: u64,
    drift: Drift,
    stretch: Vec<Resampler>,
    pacer: Option<Stretcher>,
    pace: f64,
    pacer_fed: u64,
    pacer_times: std::collections::VecDeque<(u64, i64)>,
    paused: bool,
    heard_rate: Option<u32>,
    stretch_to: u32,
    opening: std::collections::VecDeque<(i64, u64, Sound)>,
    /// まだ渡していない音声。一度に書くとリングバッファからあふれてクロックがずれる。
    pending: std::collections::VecDeque<(i64, u64, Sound)>,
    opening_samples: u64,
    opening_since: Option<Instant>,
    started: bool,
    filled: bool,
    dropped: u64,
    repeated: u64,
    heard: u64,
    missed: u64,
    silent: u64,
    extra_latency: i64,
    mix: Mix,
    shaped: Option<Shape>,
}

pub fn spawn(
    device: Option<Device>,
    shared: Arc<Shared>,
    extra_latency: i64,
    mix: Mix,
) -> Option<Sender<Say>> {
    let has_device = device.is_some();
    let (say, says) = channel();
    let sounding = Sounding {
        shared,
        device,
        says,
        rest: None,
        skip_to: None,
        video_first: None,
        video_decoded: None,
        video_expected: None,
        video_ready: None,
        live: false,
        ahead: AHEAD,
        drift: Drift::new(QUEUE),
        stretch: Vec::new(),
        pacer: None,
        pace: 1.0,
        pacer_fed: 0,
        pacer_times: std::collections::VecDeque::new(),
        paused: false,
        heard_rate: None,
        stretch_to: 0,
        opening: std::collections::VecDeque::new(),
        pending: std::collections::VecDeque::new(),
        opening_samples: 0,
        opening_since: None,
        started: !has_device,
        filled: false,
        dropped: 0,
        repeated: 0,
        heard: 0,
        missed: 0,
        silent: 0,
        extra_latency,
        mix,
        shaped: None,
    };
    match std::thread::Builder::new()
        .name(String::from("sound"))
        .spawn(move || sounding.run())
    {
        Ok(_) => Some(say),
        Err(error) => {
            tracing::warn!("音声のスレッドを起動できない: {error}");
            None
        }
    }
}

impl Sounding {
    fn run(mut self) {
        loop {
            if self.listen().is_err() {
                return;
            }
            let took = self.take_all();
            self.move_clock();
            self.publish();
            if !took {
                let idle = self.rest.is_none() || self.shared.viewer_off.load(Ordering::Relaxed);
                std::thread::sleep(if idle { IDLE_REST_FOR } else { REST_FOR });
            }
        }
    }

    fn listen(&mut self) -> Result<(), ()> {
        loop {
            match self.says.try_recv() {
                Ok(Say::Play {
                    rest,
                    skip_to,
                    video_first,
                    video_decoded,
                    video_expected,
                    video_ready,
                    live,
                }) => {
                    self.rest = Some(rest);
                    self.skip_to = Some(skip_to);
                    self.video_first = Some(video_first);
                    self.video_decoded = Some(video_decoded);
                    self.video_expected = Some(video_expected);
                    self.video_ready = Some(video_ready);
                    self.live = live;
                    self.ahead = AHEAD;
                    self.shared.ended.store(false, Ordering::Relaxed);
                }
                Ok(Say::Stop { rate }) => self.stop(rate),
                Ok(Say::Pause(paused)) => {
                    self.paused = paused;
                    if let Some(device) = &self.device {
                        device.set_paused(paused);
                    }
                    self.shared.with_clock(|clock| clock.pause(paused));
                }
                Ok(Say::Speed(pace)) => {
                    let pace = crate::engine::clock::sane_pace(pace);
                    if (pace - self.pace).abs() > 1e-6 {
                        self.pace = pace;
                        self.drop_pacer();
                    }
                }
                Ok(Say::Mix(mix)) => self.mix = *mix,
                Ok(Say::Latency(extra)) => self.extra_latency = extra,
                Ok(Say::Gain(gain)) => {
                    if let Some(device) = &self.device {
                        device.set_gain(gain);
                    }
                }
                Ok(Say::Chime(number)) => {
                    if let Some(device) = &self.device {
                        match super::chime::synthesize(number, device.rate()) {
                            Some(samples) => device.chime(&samples),
                            None => tracing::info!("内蔵音 {number} はない"),
                        }
                    }
                }
                Ok(Say::Aiff(bytes)) => {
                    if let Some(device) = &self.device {
                        if bytes.is_empty() {
                            device.chime(&[]);
                        } else {
                            match aiff::decode(&bytes) {
                                Ok(sound) => device.chime(&mono_at(&sound, device.rate())),
                                Err(why) => {
                                    tracing::warn!("データ放送の音声ファイルを読み込めない: {why}")
                                }
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err(()),
            }
        }
    }

    fn stop(&mut self, rate: u32) {
        self.rest = None;
        self.skip_to = None;
        self.heard = 0;
        self.dropped = 0;
        self.repeated = 0;
        self.silent = self.missed;
        self.shaped = None;
        if let Ok(mut shape) = self.shared.shape.lock() {
            *shape = None;
        }
        self.publish();
        // 停止したまま開くと映像も音声も出ない。
        if self.paused {
            self.paused = false;
            if let Some(device) = &self.device {
                device.set_paused(false);
            }
        }
        self.restart(rate);
    }

    fn restart(&mut self, rate: u32) {
        self.stretch.clear();
        self.drop_pacer();
        self.heard_rate = None;
        self.opening.clear();
        self.pending.clear();
        self.opening_samples = 0;
        self.opening_since = None;
        self.started = self.device.is_none();
        self.filled = false;
        self.drift = Drift::new(QUEUE);
        let mut clock = Clock::new(rate);
        if let Some(device) = self.device.as_ref() {
            clock.rebase(device.flush());
        }
        self.shared.with_clock(|held| *held = clock);
        self.shared.heard_rate.store(0, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);
    }

    fn drop_pacer(&mut self) {
        self.pacer = None;
        self.pacer_fed = 0;
        self.pacer_times.clear();
    }

    fn device_rate(&self) -> u32 {
        self.device.as_ref().map_or(48_000, Device::rate)
    }

    fn take_all(&mut self) -> bool {
        let mut took = false;
        loop {
            // 再生が始まるまでは全部受け取って古いほうを捨てる。
            if self.started && self.queued() >= self.ahead {
                break;
            }
            if let Some((at, _, sound)) = self.pending.pop_front() {
                took = true;
                self.play(at, &sound);
                continue;
            }
            let Some(rest) = self.rest.as_ref() else {
                break;
            };
            match rest.try_recv() {
                Ok(out) => {
                    took = true;
                    if let Out::Sound { stream, .. } = &out
                        && u32::from(*stream) != self.shared.sound.load(Ordering::Relaxed)
                    {
                        continue;
                    }
                    if matches!(out, Out::Sound { .. } | Out::Coded { .. })
                        && self.shared.viewer_off.load(Ordering::Relaxed)
                    {
                        continue;
                    }
                    self.take(out);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rest = None;
                    self.shared.ended.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        self.open();
        took
    }

    /// 最初のフレームを待つ間に溜まる音声。クロックを映像に合わせるにはその音声が要る。
    fn held_while_opening(&self) -> u64 {
        let rate = u64::from(self.device_rate());
        let expected = self
            .video_expected
            .as_ref()
            .is_some_and(|expected| expected.load(Ordering::Relaxed));
        let wait = if expected {
            OPEN_FOR_VIDEO_EXPECTED
        } else {
            OPEN_FOR_VIDEO
        };
        (wait.as_millis() as u64 * rate / 1000).max(AHEAD)
    }

    fn take(&mut self, out: Out) {
        match out {
            Out::Sound { at, sound, .. } => {
                let Some(device) = self.device.as_ref() else {
                    return;
                };
                if !self.started {
                    let rate = u64::from(device.rate());
                    let samples = sound.len() as u64 * rate / u64::from(sound.rate.max(1));
                    self.opening_samples += samples;
                    self.opening_since.get_or_insert_with(Instant::now);
                    self.opening.push_back((at, samples, sound));
                    while let Some((_, oldest, _)) = self.opening.front()
                        && self.opening_samples - oldest >= self.held_while_opening()
                    {
                        self.opening_samples -= oldest;
                        self.opening.pop_front();
                    }
                    return;
                }
                self.play(at, &sound);
            }
            Out::Coded { at, frame } => {
                if self.device.is_some() {
                    self.pass(at, &frame);
                }
            }
            Out::Trouble { stream, what } => tracing::warn!("ストリーム 0x{stream:04x}: {what}"),
            Out::Switched => {
                let rate = self.shared.with_clock(|clock| clock.rate());
                self.restart(rate);
            }
            Out::Picture { .. }
            | Out::Access(_)
            | Out::DataStreams(_)
            | Out::ApplicationService(_)
            | Out::Data { .. }
            | Out::Item { .. }
            | Out::CaptionStreams(_)
            | Out::Caption { .. } => {}
        }
    }

    /// The device counts silence too; otherwise every picture shows early.
    fn start_count(&mut self) {
        if self.started {
            return;
        }
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let played = device.played();
        self.shared.with_clock(|clock| clock.rebase(played));
        self.silent = device.missed();
        self.missed = self.silent;
        self.started = true;
    }

    fn side(&mut self, burst: bool) {
        let switched = self
            .device
            .as_mut()
            .is_some_and(|device| device.use_burst(burst));
        if switched {
            let rate = self.shared.with_clock(|clock| clock.rate());
            self.restart(rate);
        }
    }

    fn play(&mut self, at: i64, sound: &Sound) {
        self.side(false);
        self.start_count();
        let stretched = self.stretch(sound);
        let (played, at, pace) = if (self.pace - 1.0).abs() > 1e-6 {
            let pacer = self.pacer.get_or_insert_with(|| {
                Stretcher::new(stretched.rate, stretched.channels.len(), self.pace)
            });
            if pacer.pace() != self.pace {
                *pacer = Stretcher::new(stretched.rate, stretched.channels.len(), self.pace);
            }
            let begins = pacer.position();
            let total_before = self.pacer_fed;
            self.pacer_fed += stretched.len() as u64;
            let out = pacer.push(&stretched);
            self.pacer_times.push_back((total_before, at));
            while self.pacer_times.len() > 64 {
                self.pacer_times.pop_front();
            }
            let at_out = self
                .pacer_times
                .iter()
                .rev()
                .find(|(from, _)| *from <= begins)
                .map(|(from, ticks)| {
                    ticks
                        + (begins - from) as i64 * crate::engine::clock::HZ
                            / i64::from(stretched.rate.max(1))
                })
                .unwrap_or(at);
            (out, at_out, self.pace)
        } else {
            (stretched, at, 1.0)
        };
        let Some(device) = self.device.as_ref() else {
            return;
        };
        if played.is_empty() {
            return;
        }
        let mixed = mix::mix(&played, device.channels(), &self.mix);
        let out = mixed.channels.len();
        if self.shaped.as_ref().is_none_or(|shape| {
            shape.rate != played.rate
                || shape.speakers != played.speakers
                || shape.mixdown != played.mixdown
                || shape.out != out
                || shape.device != device.channels()
        }) {
            let shape = Shape {
                rate: played.rate,
                speakers: played.speakers.clone(),
                mixdown: played.mixdown,
                out,
                device: device.channels(),
            };
            if let Ok(mut shared) = self.shared.shape.lock() {
                *shared = Some(shape.clone());
            }
            self.shaped = Some(shape);
        }
        let played = mixed;
        let written = device.write(&played);
        // The stream's time, not one counted from samples: stretching must not change it.
        self.shared
            .with_clock(|clock| clock.wrote_paced(at, written, pace));
        self.heard += written as u64;
    }

    /// Starts the device once sound, pictures and screen are ready. The clock is placed just
    /// before the first decoded picture; older sound is thrown away.
    fn open(&mut self) {
        if self.started {
            return;
        }
        let Some(since) = self.opening_since else {
            return;
        };
        let ended = self.rest.is_none();
        if !(self.opening_samples >= OPEN_WITH || since.elapsed() >= OPEN_MOST || ended) {
            return;
        }
        let video_first = self
            .video_first
            .as_ref()
            .map(|first| first.load(Ordering::Relaxed))
            .filter(|first| *first != i64::MIN);
        if !ended && !self.opening_done(since, video_first) {
            return;
        }
        if let Some(video) = video_first
            && let Some(rate) = self.device.as_ref().map(|device| i64::from(device.rate()))
            && let Some((newest_at, newest_len, _)) = self.opening.back()
        {
            let ticks = |samples: u64| (samples as i64) * 90_000 / rate.max(1);
            let from =
                (video - VIDEO_MARGIN).min(newest_at + ticks(*newest_len) - ticks(OPEN_WITH));
            while let Some((at, samples, _)) = self.opening.front()
                && self.opening.len() > 1
                && *at + ticks(*samples) <= from
            {
                self.opening_samples -= samples;
                self.opening.pop_front();
            }
        }
        let Some((first, _, _)) = self.opening.front() else {
            return;
        };
        tracing::debug!(
            "音声の再生開始: 映像 {}  音声 {:.0} ms  デコード済み {} 枚  待ち {:.0} ms",
            video_first.map_or("無し".to_owned(), |v| format!(
                "{:.0} ms",
                v as f64 / 90.0
            )),
            *first as f64 / 90.0,
            self.decoded_pictures(),
            since.elapsed().as_secs_f64() * 1000.0,
        );
        if let Some(skip_to) = self.skip_to.as_ref() {
            skip_to.fetch_max(*first, Ordering::Relaxed);
        }
        let mut opening = std::mem::take(&mut self.opening);
        let mut handed = 0u64;
        while handed < self.ahead
            && let Some((at, samples, sound)) = opening.pop_front()
        {
            self.play(at, &sound);
            handed += samples;
        }
        self.pending = opening;
        self.opening_samples = 0;
        self.opening_since = None;
        // ライブでは溜まったバッファ量を目標にする（減らすと映像が遅れる）。
        if self.live {
            let depth = self.queued().max(QUEUE);
            self.ahead = (depth * 2).max(AHEAD);
            self.drift = Drift::new(depth);
            tracing::debug!(
                "音声のバッファ {:.0} ms（映像に合わせる）",
                depth as f64 * 1000.0 / f64::from(self.device_rate())
            );
        }
    }

    fn opening_done(&self, since: Instant, video_first: Option<i64>) -> bool {
        let flag = |flag: &Option<Arc<AtomicBool>>| {
            flag.as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
        };
        let Some(_) = video_first else {
            let wait = if flag(&self.video_expected) {
                OPEN_FOR_VIDEO_EXPECTED
            } else {
                OPEN_FOR_VIDEO
            };
            return since.elapsed() >= wait;
        };
        let warmed = self.decoded_pictures() >= crate::decode::PREROLL_PICTURES;
        warmed && flag(&self.video_ready) || since.elapsed() >= OPEN_FOR_VIDEO + PREROLL_MOST
    }

    fn decoded_pictures(&self) -> u64 {
        self.video_decoded
            .as_ref()
            .map_or(0, |decoded| decoded.load(Ordering::Relaxed))
    }

    /// A frame that cannot be passed through (22.2ch) drops the device ([`Shared::fault`]).
    fn pass(&mut self, at: i64, frame: &[u8]) {
        self.side(true);
        self.start_count();
        let queued = self.shared.with_clock(|clock| clock.queued());
        let repeats = Self::pace(
            queued,
            self.ahead,
            self.filled,
            &mut self.dropped,
            &mut self.repeated,
        );
        let mut written = 0;
        for _ in 0..repeats {
            let Some(device) = self.device.as_mut() else {
                return;
            };
            match device.write_burst(frame) {
                Ok(samples) => written += samples,
                Err(error) => {
                    let why = format!("この音声はパススルーできません: {error}");
                    tracing::warn!("{why}");
                    if let Ok(mut fault) = self.shared.fault.lock() {
                        *fault = Some(why);
                    }
                    self.device = None;
                    return;
                }
            }
        }
        if written == 0 {
            return;
        }
        let queued = self.shared.with_clock(|clock| {
            clock.wrote(at, written);
            clock.queued()
        });
        self.heard += written as u64;
        if queued >= self.ahead {
            self.filled = true;
        }
    }

    /// A burst cannot be stretched, so the step is a whole frame.
    fn pace(queued: u64, ahead: u64, filled: bool, dropped: &mut u64, repeated: &mut u64) -> usize {
        if !filled {
            return 1;
        }
        if queued > ahead + SLACK {
            *dropped += 1;
            return 0;
        }
        if queued + SLACK < ahead {
            *repeated += 1;
            return 2;
        }
        1
    }

    fn stretch(&mut self, sound: &Sound) -> Sound {
        let device = self.device_rate();
        let ratio = self.drift.ratio();
        // The rate can change (MMT assets, ワンセグ's 24 kHz core).
        if self.heard_rate != Some(sound.rate) || self.stretch_to != device {
            if sound.rate != device {
                tracing::debug!("音声を {} Hz から {device} Hz に変換する", sound.rate);
            }
            self.heard_rate = Some(sound.rate);
            self.stretch_to = device;
            self.shared.heard_rate.store(sound.rate, Ordering::Relaxed);
            self.stretch.clear();
        }
        self.stretch.resize_with(sound.channels.len(), || {
            Resampler::for_rates(sound.rate, device)
        });
        let mut out = Sound {
            rate: device,
            channels: Vec::with_capacity(sound.channels.len()),
            speakers: sound.speakers.clone(),
            mixdown: sound.mixdown,
        };
        for (channel, resampler) in sound.channels.iter().zip(self.stretch.iter_mut()) {
            resampler.set_ratio(ratio);
            let mut taken = Vec::with_capacity(channel.len() + 8);
            resampler.push(channel, &mut taken);
            out.channels.push(taken);
        }
        let shortest = out.channels.iter().map(Vec::len).min().unwrap_or(0);
        for channel in &mut out.channels {
            channel.truncate(shortest);
        }
        out
    }

    fn move_clock(&mut self) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let missed = device.missed();
        let played = device.played();
        let behind = device.latency() as i64;
        let video = self.shared.video_latency.load(Ordering::Relaxed);
        let gained = missed.saturating_sub(self.missed);
        if gained > 0 {
            self.missed = missed;
        }
        let queued = self.shared.with_clock(|clock| {
            if gained > 0 {
                clock.missed(gained);
            }
            clock.played(played);
            clock.set_latency(behind - video + self.extra_latency);
            clock.queued()
        });
        self.drift.update(queued);
    }

    fn queued(&self) -> u64 {
        self.device
            .as_ref()
            .map_or(0, |device| device.queued() as u64)
    }

    fn publish(&self) {
        let shared = &self.shared;
        shared.queued.store(self.queued(), Ordering::Relaxed);
        shared.heard.store(self.heard, Ordering::Relaxed);
        shared
            .missed
            .store(self.missed.saturating_sub(self.silent), Ordering::Relaxed);
        shared.dropped.store(self.dropped, Ordering::Relaxed);
        shared.repeated.store(self.repeated, Ordering::Relaxed);
    }
}

const SOUND_MOST_SECONDS: u64 = 600;

fn mono_at(sound: &Sound, rate: u32) -> Vec<f32> {
    let count = sound.len();
    let channels = sound.channels.len().max(1) as f32;
    let mono: Vec<f32> = (0..count)
        .map(|i| sound.channels.iter().map(|c| c[i]).sum::<f32>() / channels)
        .collect();
    if sound.rate == rate || mono.is_empty() {
        return mono;
    }
    let converted = count as u64 * u64::from(rate) / u64::from(sound.rate.max(1));
    if converted > u64::from(rate) * SOUND_MOST_SECONDS {
        return Vec::new();
    }
    let mut resampler = Resampler::for_rates(sound.rate, rate);
    let mut out = Vec::with_capacity(converted as usize + 64);
    resampler.push(&mono, &mut out);
    // フィルター長の半分だけ後ろに残る。無音を流して出し切る。
    resampler.push(&[0.0; 64], &mut out);
    out
}
