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
/// ライブで再生を始めるときに確保する、ライブの端までの余裕。届き方のむらと、映像が音声より遅れて届く分を吸う。
const LIVE_LEAD: Duration = Duration::from_millis(400);
/// 余裕が溜まるのを待つ上限。届くのが遅いソースでいつまでも始まらないように。
const LIVE_LEAD_WAIT: Duration = Duration::from_millis(1500);
/// 余裕が目標の半分を切ったまま、これだけ続いたら無音を挟んで戻す。
const LOW_FOR: f64 = 1.0;

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
    /// 余裕が目標の半分を切っている時間（秒）。
    low_for: f64,
    /// 次に書く音声の前に挟む、時刻を進めない無音（サンプル数）。
    pad: usize,
}

pub fn spawn(
    device: Option<Device>,
    shared: Arc<Shared>,
    extra_latency: i64,
    mix: Mix,
) -> Option<Sender<Say>> {
    let (say, says) = channel();
    let sounding = Sounding::new(device, shared, says, extra_latency, mix);
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
    fn new(
        device: Option<Device>,
        shared: Arc<Shared>,
        says: Receiver<Say>,
        extra_latency: i64,
        mix: Mix,
    ) -> Self {
        let has_device = device.is_some();
        Self {
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
            low_for: 0.0,
            pad: 0,
        }
    }

    fn run(mut self) {
        let mut last = Instant::now();
        loop {
            if self.listen().is_err() {
                return;
            }
            let now = Instant::now();
            let took = self.step((now - last).as_secs_f64());
            last = now;
            if !took {
                let idle = self.rest.is_none() || self.shared.viewer_off.load(Ordering::Relaxed);
                std::thread::sleep(if idle { IDLE_REST_FOR } else { REST_FOR });
            }
        }
    }

    /// 届いた物を書き、`seconds` 進んだ時計を合わせる。書いた物があれば真。
    fn step(&mut self, seconds: f64) -> bool {
        let took = self.take_all();
        self.move_clock(seconds);
        self.publish();
        took
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
        self.low_for = 0.0;
        self.pad = 0;
        // 同じソースなら時計のずれは同じなので、学んだ比は引き継ぐ。
        self.drift = self.drift.retarget(QUEUE);
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
        let pad = std::mem::take(&mut self.pad);
        let played = if pad > 0 {
            let mut padded = mixed.into_owned();
            for channel in &mut padded.channels {
                channel.splice(0..0, std::iter::repeat_n(0.0, pad));
            }
            std::borrow::Cow::Owned(padded)
        } else {
            mixed
        };
        let written = device.write(&played);
        // The stream's time, not one counted from samples: stretching must not change it.
        self.shared.with_clock(|clock| {
            clock.held(pad.min(written));
            clock.wrote_paced(at, written.saturating_sub(pad), pace);
        });
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
            let newest = newest_at + ticks(*newest_len);
            let from = (video - VIDEO_MARGIN).min(newest - ticks(OPEN_WITH));
            // ライブは余裕が溜まってから始める（溜まり方を成り行きにすると、選局ごとに端までの余裕が変わる）。
            if self.live
                && !ended
                && newest - from < LIVE_LEAD.as_millis() as i64 * 90
                && since.elapsed() < OPEN_FOR_VIDEO + PREROLL_MOST + LIVE_LEAD_WAIT
            {
                return;
            }
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
        // ライブでは溜めた量を目標にする（減らすと映像が遅れる）。少なくとも LIVE_LEAD。
        let lead = self.opening_samples.max(self.lead_samples());
        if self.live {
            self.ahead = (lead * 2).max(AHEAD);
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
        if self.live {
            self.drift = self.drift.retarget(lead);
            tracing::debug!(
                "音声のバッファ {:.0} ms（映像に合わせる）",
                lead as f64 * 1000.0 / f64::from(self.device_rate())
            );
        }
    }

    fn lead_samples(&self) -> u64 {
        LIVE_LEAD.as_millis() as u64 * u64::from(self.device_rate()) / 1000
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
        // 符号化したまま渡すので無音は挟めない（フレームを重ねて戻す：[`Self::pace`]）。
        self.pad = 0;
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

    fn move_clock(&mut self, seconds: f64) {
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
        // 始まるまでは溜めている途中で、溜めた量は目標と比べられない。
        if !self.started {
            return;
        }
        // 途切れて減った分は時計のずれではない。
        if gained > 0 {
            self.drift.interrupted();
            self.watch_lead(queued, seconds);
            return;
        }
        // 目標の半分を切るほど減ったのは途切れの名残で、無音を挟んで戻す（伸び縮みで追うと速さを読み違える）。
        if self.live && queued * 2 < self.drift.target() {
            self.drift.coast();
        } else {
            self.drift.update(queued, seconds);
        }
        self.watch_lead(queued, seconds);
    }

    /// ライブの端に追いつきそうなら、時刻を進めない無音を挟んで余裕を戻す（伸び縮みで戻るのを待つと、
    /// その間映像が間に合わずに捨てられる）。
    fn watch_lead(&mut self, queued: u64, seconds: f64) {
        let low = queued * 2 < self.drift.target();
        if !self.live || !self.started || self.paused || self.pad > 0 || !low {
            self.low_for = 0.0;
            return;
        }
        self.low_for += seconds;
        if self.low_for >= LOW_FOR {
            self.low_for = 0.0;
            // 足りない分を一度に挟む（小分けにすると映像がその回数だけ止まる）。
            let short = self.drift.target().saturating_sub(queued);
            self.pad = short as usize;
            tracing::debug!(
                "ライブの端に追いつきそうなので、無音を {:.0} ms 挟む",
                short as f64 * 1000.0 / f64::from(self.device_rate())
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::audio::{Mixdown, Speaker};
    use std::sync::Mutex;

    /// 試験の音声デバイス。呼ぶたびに [`Fake::play`] の分だけ再生し、無ければその分を無音で埋めて数える。
    #[derive(Debug, Default)]
    struct Tape {
        written: u64,
        played: u64,
        missed: u64,
    }

    #[derive(Debug, Clone, Default)]
    struct Fake(Arc<Mutex<Tape>>);

    impl Fake {
        fn tape(&self) -> std::sync::MutexGuard<'_, Tape> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        fn play(&self, samples: u64) {
            let mut tape = self.tape();
            let have = tape.written - tape.played;
            if have < samples {
                tape.missed += samples - have;
                tape.written += samples - have;
            }
            tape.played += samples;
        }

        fn queued(&self) -> u64 {
            let tape = self.tape();
            tape.written - tape.played
        }
    }

    impl crate::sound::Pcm for Fake {
        fn rate(&self) -> u32 {
            48_000
        }
        fn played(&self) -> u64 {
            self.tape().played
        }
        fn missed(&self) -> u64 {
            self.tape().missed
        }
        fn latency(&self) -> u64 {
            0
        }
        fn queued(&self) -> usize {
            Fake::queued(self) as usize
        }
        fn flush(&self) -> u64 {
            let mut tape = self.tape();
            tape.written = tape.played;
            tape.played
        }
        fn set_paused(&self, _: bool) {}
        fn chime(&self, _: &[f32]) {}
        fn write(&self, sound: &Sound) -> usize {
            self.tape().written += sound.len() as u64;
            sound.len()
        }
        fn channels(&self) -> usize {
            2
        }
        fn set_gain(&self, _: f32) {}
    }

    const STREAM: u16 = 0x0110;
    /// AAC の一フレーム。
    const FRAME: usize = 1024;
    const FRAME_TICKS: i64 = FRAME as i64 * 90_000 / 48_000;
    const STEP: f64 = 0.002;

    /// 放送の届き方。`source` は PC の音声に対する速さ、`burst(n)` は n 回目にまとめて届くまでの間隔、
    /// `gaps` は電波が途切れて何も届かない区間（秒）。
    struct Air {
        source: f64,
        burst: Box<dyn Fn(u64) -> f64>,
        gaps: Vec<(f64, f64)>,
    }

    impl Air {
        fn steady(source: f64) -> Self {
            Self {
                source,
                burst: Box::new(|_| 0.03),
                gaps: Vec::new(),
            }
        }
    }

    #[derive(Debug, Default)]
    struct Watched {
        /// （秒、デバイスに溜まった量、比）を 1 秒ごと。
        seconds: Vec<(f64, u64, f64)>,
        target: u64,
        started_at: Option<f64>,
        missed_after_start: u64,
        pads: Vec<f64>,
    }

    fn sound() -> Sound {
        Sound {
            rate: 48_000,
            channels: vec![vec![0.1; FRAME]; 2],
            speakers: vec![Speaker::FrontLeft, Speaker::FrontRight],
            mixdown: Mixdown::default(),
        }
    }

    /// `seconds` 秒の生放送を流し、音声のスレッドが何をしたかを見る。映像の最初のフレームは音声の
    /// 最初の 0.1 秒後。
    fn watch(air: &Air, seconds: f64) -> Watched {
        let fake = Fake::default();
        let shared = Arc::new(Shared::new(48_000));
        shared.sound.store(u32::from(STREAM), Ordering::Relaxed);
        let (_say, says) = channel();
        let mut sounding = Sounding::new(
            Some(Device::Pcm(Box::new(fake.clone()))),
            Arc::clone(&shared),
            says,
            0,
            Mix::default(),
        );
        let (send, rest) = channel();
        let first: i64 = 900_000;
        sounding.rest = Some(rest);
        sounding.live = true;
        sounding.skip_to = Some(Arc::new(AtomicI64::new(0)));
        sounding.video_first = Some(Arc::new(AtomicI64::new(first + 9_000)));
        sounding.video_decoded = Some(Arc::new(AtomicU64::new(crate::decode::PREROLL_PICTURES)));
        sounding.video_ready = Some(Arc::new(AtomicBool::new(true)));
        sounding.video_expected = Some(Arc::new(AtomicBool::new(true)));

        let mut watched = Watched::default();
        let mut made = 0u64;
        let mut ready = 0u64;
        let mut bursts = 0u64;
        let mut next_burst = (air.burst)(0);
        let mut missed_at_start = 0;
        let mut padding = false;
        let mut cut = 0.0;
        for tick in 0..(seconds / STEP) as u64 {
            let now = tick as f64 * STEP;
            if air
                .gaps
                .iter()
                .any(|(from, to)| (*from..*to).contains(&now))
            {
                cut += STEP;
            }
            // 届いた放送の分だけフレームができる。途切れた間の分は来ない。
            let aired = ((now - cut) * 48_000.0 * air.source / FRAME as f64) as u64;
            while made < aired {
                made += 1;
                if air
                    .gaps
                    .iter()
                    .any(|(from, to)| (*from..*to).contains(&now))
                {
                    ready = made;
                }
            }
            if now >= next_burst {
                while ready < made {
                    let at = first + (ready as i64) * FRAME_TICKS;
                    let _ = send.send(Out::Sound {
                        stream: STREAM,
                        at,
                        sound: sound(),
                    });
                    ready += 1;
                }
                bursts += 1;
                next_burst += (air.burst)(bursts);
            }
            if sounding.started {
                fake.play((48_000.0 * STEP) as u64);
            }
            sounding.step(STEP);
            if sounding.started && watched.started_at.is_none() {
                watched.started_at = Some(now);
                watched.target = sounding.drift.target();
                missed_at_start = fake.tape().missed;
            }
            if sounding.pad > 0 && !padding {
                watched.pads.push(now);
            }
            padding = sounding.pad > 0;
            if tick % (1.0 / STEP) as u64 == 0 {
                watched
                    .seconds
                    .push((now, fake.queued(), sounding.drift.ratio()));
            }
        }
        watched.missed_after_start = fake.tape().missed - missed_at_start;
        watched
    }

    fn lead_samples() -> u64 {
        LIVE_LEAD.as_millis() as u64 * 48
    }

    #[test]
    fn a_live_start_waits_for_its_lead() {
        let watched = watch(&Air::steady(1.0), 3.0);
        let started = watched.started_at.expect("始まる");
        assert!(
            started >= LIVE_LEAD.as_secs_f64() - 0.1,
            "{started:.2} 秒で始まった"
        );
        assert!(watched.target >= lead_samples(), "{}", watched.target);
    }

    #[test]
    fn a_tuner_that_runs_slow_keeps_its_lead_without_running_dry() {
        // us3pout：PC の音声に対して 0.4% 遅い。いままでは 0.1% しか吸収せず、溜めた量が尽きては補っていた。
        let watched = watch(&Air::steady(0.996), 240.0);
        assert_eq!(watched.missed_after_start, 0);
        assert!(
            watched.pads.is_empty(),
            "無音を挟まずに済む: {:?}",
            watched.pads
        );
        for (at, queued, _) in watched.seconds.iter().filter(|(at, _, _)| *at > 120.0) {
            assert!(
                *queued as f64 > watched.target as f64 * 0.75,
                "{at:.0} 秒で {queued}（目標 {}）",
                watched.target
            );
        }
        let (_, _, ratio) = watched.seconds.last().copied().expect("測った");
        assert!((ratio - 0.996).abs() < 1e-3, "{ratio}");
    }

    #[test]
    fn a_tuner_that_runs_fast_is_read_faster() {
        let watched = watch(&Air::steady(1.003), 240.0);
        assert_eq!(watched.missed_after_start, 0);
        let (_, queued, ratio) = watched.seconds.last().copied().expect("測った");
        assert!((ratio - 1.003).abs() < 1e-3, "{ratio}");
        assert!(
            (queued as f64) < watched.target as f64 * 1.5,
            "溜まり続けない: {queued}"
        );
    }

    #[test]
    fn a_dropout_is_bridged_with_silence_not_by_slowing_down() {
        // 電波が 3 秒途切れる。その間の放送は来ないので溜めた量は尽きる。速さは変わっていない。
        let mut air = Air::steady(1.0);
        air.gaps = vec![(40.0, 43.0)];
        let watched = watch(&air, 90.0);
        assert!(
            watched.missed_after_start > 0,
            "途切れたのでデバイスは空になる"
        );
        assert!(
            watched.pads.len() == 1 && (40.0..44.0).contains(&watched.pads[0]),
            "途切れたら一度だけ、足りない分の無音を挟んで余裕を戻す: {:?}",
            watched.pads
        );
        for (at, queued, ratio) in &watched.seconds {
            assert!(
                (ratio - 1.0).abs() < 3e-3,
                "{at:.0} 秒で比 {ratio}（溜めた量 {queued}）"
            );
        }
        let back: Vec<_> = watched
            .seconds
            .iter()
            .filter(|(at, _, _)| *at > 60.0)
            .map(|(_, queued, _)| *queued)
            .collect();
        assert!(
            back.iter().all(|queued| *queued * 2 >= watched.target),
            "戻った後は目標の半分より多い: {back:?}"
        );
    }

    #[test]
    fn bursty_arrival_does_not_bend_the_pitch_or_run_dry() {
        let gap = |n: u64| {
            let x = n
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((x >> 33) % 250) as f64 / 1000.0 + 0.001
        };
        let air = Air {
            source: 1.0,
            burst: Box::new(gap),
            gaps: Vec::new(),
        };
        let watched = watch(&air, 180.0);
        assert_eq!(watched.missed_after_start, 0);
        for (at, _, ratio) in watched.seconds.iter().filter(|(at, _, _)| *at > 30.0) {
            assert!((ratio - 1.0).abs() < 3e-3, "{at:.0} 秒で {ratio}");
        }
    }
}
