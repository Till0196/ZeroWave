// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The timebase, and keeping the sound from drifting away from it.
//! The time is the stream's, the rate is the device's; the difference is taken out by resampling.

use std::collections::VecDeque;
use std::time::Instant;

pub const HZ: i64 = 90_000;

const WRAP: i64 = 1 << 33;

const HALF: i64 = WRAP / 2;

/// Unwraps the 33-bit count of a transport stream. A channel change must call [`Wrap::reset`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Wrap {
    last: Option<i64>,
    added: i64,
}

impl Wrap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, ticks: i64) -> i64 {
        let Some(last) = self.last else {
            self.last = Some(ticks);
            return ticks;
        };
        // Times do not arrive in order (B pictures): only a big step back is a wrap.
        if ticks < last - HALF {
            self.added += WRAP;
        } else if ticks > last + HALF {
            self.added -= WRAP;
        }
        self.last = Some(ticks);
        ticks + self.added
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub(crate) fn sane_pace(pace: f64) -> f64 {
    if pace.is_finite() && pace > 0.0 {
        pace
    } else {
        1.0
    }
}

const FIT_OVER: f64 = 2.0;
const FIT_MOST: usize = 256;
const FIT_LEAST: f64 = 0.05;
const FIT_SLOPE: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Mark {
    at: u64,
    ticks: i64,
    pace: f64,
}

/// What time it is, looked up from what the device has played. With no sound, the caller
/// drives [`Clock::advance`].
#[derive(Debug)]
pub struct Clock {
    rate: u32,
    marks: VecDeque<Mark>,
    queued: u64,
    played: u64,
    base: u64,
    free: Option<i64>,
    step: u64,
    epoch: Instant,
    /// (wall seconds, samples) each time the count rose; a line through them gives the real rate.
    treads: VecDeque<(f64, f64)>,
    latency: i64,
    frozen: Option<i64>,
}

impl Clock {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1),
            marks: VecDeque::new(),
            queued: 0,
            played: 0,
            base: 0,
            free: None,
            step: 0,
            epoch: Instant::now(),
            treads: VecDeque::new(),
            latency: 0,
            frozen: None,
        }
    }

    pub fn pause(&mut self, paused: bool) {
        if paused {
            if self.frozen.is_none() {
                self.frozen = self.now();
            }
        } else if self.frozen.take().is_some() {
            self.treads.clear();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.frozen.is_some()
    }

    fn wall(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// A device reports what it has taken, not what is audible. Negative where the picture lags.
    pub fn set_latency(&mut self, samples: i64) {
        self.latency = samples;
    }

    /// A device counts from when it was opened; without this every picture is judged late.
    pub fn rebase(&mut self, total: u64) {
        self.base = total;
    }

    pub fn wrote(&mut self, ticks: i64, samples: usize) {
        self.wrote_paced(ticks, samples, 1.0);
    }

    pub fn wrote_paced(&mut self, ticks: i64, samples: usize, pace: f64) {
        self.marks.push_back(Mark {
            at: self.queued,
            ticks,
            pace: sane_pace(pace),
        });
        self.queued += samples as u64;
        self.free = None;
    }

    fn ticks_into(&self, mark: &Mark, samples: u64) -> i64 {
        (self.ticks_of(samples) as f64 * mark.pace) as i64
    }

    fn ends_at(&self) -> Option<i64> {
        let mark = self.marks.back()?;
        Some(mark.ticks + self.ticks_into(mark, self.queued.saturating_sub(mark.at)))
    }

    /// 時刻を進めない無音を足す（ライブの端までの余裕を戻すため）。この間は映像も止まる。
    pub fn held(&mut self, samples: usize) {
        let Some(at) = self.ends_at() else {
            return;
        };
        self.marks.push_back(Mark {
            at: self.queued,
            ticks: at,
            pace: 0.0,
        });
        self.queued += samples as u64;
        self.free = None;
    }

    /// Without this the player runs at the rate the sound arrives at.
    pub fn missed(&mut self, samples: u64) {
        if samples == 0 {
            return;
        }
        let Some(at) = self.ends_at() else {
            return;
        };
        self.wrote(at, samples as usize);
    }

    pub fn played(&mut self, total: u64) {
        let wall = self.wall();
        self.played_at(total, wall);
    }

    pub fn played_at(&mut self, total: u64, wall: f64) {
        let total = total.saturating_sub(self.base);
        let was = self.played;
        self.played = self.played.max(total).min(self.queued);
        if self.played > was {
            let gained = self.played - was;
            self.step = gained;
            self.treads.push_back((wall, self.played as f64));
            while self
                .treads
                .front()
                .is_some_and(|(at, _)| wall - at > FIT_OVER)
                || self.treads.len() > FIT_MOST
            {
                self.treads.pop_front();
            }
        }
        while self.marks.len() > 1 && self.marks[1].at <= self.played {
            self.marks.pop_front();
        }
    }

    pub fn now(&self) -> Option<i64> {
        self.now_at(self.wall())
    }

    pub fn now_at(&self, wall: f64) -> Option<i64> {
        if let Some(frozen) = self.frozen {
            return Some(frozen);
        }
        if self.marks.is_empty() {
            return self.free;
        }
        let estimate = self.fit(wall).clamp(self.played as f64, self.queued as f64) as u64;
        let mark = self
            .marks
            .iter()
            .rev()
            .find(|mark| mark.at <= estimate)
            .or_else(|| self.marks.front())?;
        let into = estimate.saturating_sub(mark.at);
        let behind = self.ticks_of_signed(self.latency);
        Some(mark.ticks + self.ticks_into(mark, into) - behind)
    }

    fn fit(&self, wall: f64) -> f64 {
        let rate = f64::from(self.rate);
        let Some(&(last_wall, last_played)) = self.treads.back() else {
            return self.played as f64;
        };
        let n = self.treads.len() as f64;
        let span = last_wall - self.treads.front().map_or(last_wall, |(at, _)| *at);
        if self.treads.len() < 2 || span < FIT_LEAST {
            return last_played + (wall - last_wall).max(0.0) * rate;
        }
        let mean_wall = self.treads.iter().map(|(at, _)| at).sum::<f64>() / n;
        let mean_played = self.treads.iter().map(|(_, p)| p).sum::<f64>() / n;
        let (mut cov, mut var) = (0.0, 0.0);
        for (at, played) in &self.treads {
            cov += (at - mean_wall) * (played - mean_played);
            var += (at - mean_wall) * (at - mean_wall);
        }
        let slope = if var > 0.0 { cov / var } else { rate };
        let slope = slope.clamp(rate * (1.0 - FIT_SLOPE), rate * (1.0 + FIT_SLOPE));
        mean_played + slope * (wall - mean_wall)
    }

    pub fn step(&self) -> u64 {
        self.step
    }

    /// For a stream that has no sound.
    pub fn advance(&mut self, ticks: i64) {
        if !self.marks.is_empty() || self.frozen.is_some() {
            return;
        }
        if let Some(free) = self.free.as_mut() {
            *free += ticks;
        }
    }

    pub fn start(&mut self, ticks: i64) {
        if self.marks.is_empty() {
            self.free = Some(ticks);
        }
    }

    pub fn queued(&self) -> u64 {
        self.queued - self.played
    }

    pub fn buffered(&self) -> i64 {
        self.ticks_of(self.queued())
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn reset(&mut self) {
        self.marks.clear();
        self.queued = 0;
        self.played = 0;
        self.free = None;
        self.step = 0;
        self.treads.clear();
        self.frozen = None;
    }

    pub fn latency(&self) -> i64 {
        self.latency
    }

    fn ticks_of_signed(&self, samples: i64) -> i64 {
        samples.saturating_mul(HZ) / i64::from(self.rate)
    }

    fn ticks_of(&self, samples: u64) -> i64 {
        (samples as i64).saturating_mul(HZ) / i64::from(self.rate)
    }
}

/// 音声デバイスに溜めた量を目標に保つよう、1 に近い比で音声を伸び縮みする。ライブでは溜めた量がそのまま
/// ライブの端までの余裕なので、放送と PC の時計のずれ（チューナーによっては 0.4%）をここで吸収する。
/// 量は数秒でならす：まとめて届く分で音の高さが揺れないように。
#[derive(Debug, Clone, Copy)]
pub struct Drift {
    target: f64,
    /// ならした誤差（目標に対する比）。途切れた後は測り直す。
    smoothed: Option<f64>,
    /// 積分した比。時計のずれそのものに落ち着き、選局をまたいで引き継ぐ。
    lean: f64,
    ratio: f64,
    /// 途切れた後、調整を止めておく残り時間（秒）。
    held: f64,
}

/// ならす時定数（秒）。
const DRIFT_SMOOTH: f64 = 4.0;
/// 溜めた量は速さの差を積み上げた物なので、比例の項で揺れを抑え、積分の項がずれそのものに落ち着く。
/// 0.4% のずれを数十秒で吸収し、1〜250 ms のばらばらな間隔で届いても揺れは 0.3% 以内（試験の値）。
const DRIFT_PROPORTIONAL: f64 = 0.02;
/// 一秒あたりの積分の利き。
const DRIFT_INTEGRAL: f64 = 5e-4;
/// 伸び縮みの上限。0.5% で音の高さは 9 セントほど。
pub const DRIFT_MOST: f64 = 5e-3;
/// 途切れた（音声が届かなかった）後に調整を止める時間（秒）。
const DRIFT_HOLD: f64 = 1.0;

impl Drift {
    pub fn new(target: u64) -> Self {
        Self {
            target: (target as f64).max(1.0),
            smoothed: None,
            lean: 0.0,
            ratio: 1.0,
            held: 0.0,
        }
    }

    /// 目標を替える。積分した比（時計のずれ）は引き継ぐ。
    pub fn retarget(self, target: u64) -> Self {
        Self {
            lean: self.lean,
            ratio: 1.0 + self.lean,
            ..Self::new(target)
        }
    }

    pub fn target(&self) -> u64 {
        self.target as u64
    }

    /// `seconds` 前からの溜めた量 `queued` で比を決める。
    pub fn update(&mut self, queued: u64, seconds: f64) -> f64 {
        let seconds = seconds.clamp(0.0, 0.5);
        let error = (queued as f64 - self.target) / self.target;
        let smoothed = match self.smoothed {
            Some(was) => was + (error - was) * seconds / (DRIFT_SMOOTH + seconds),
            None => error,
        };
        self.smoothed = Some(smoothed);
        if self.held > 0.0 {
            self.held -= seconds;
            self.ratio = 1.0 + self.lean;
            return self.ratio;
        }
        self.lean =
            (self.lean + smoothed * DRIFT_INTEGRAL * seconds).clamp(-DRIFT_MOST, DRIFT_MOST);
        self.ratio = (1.0 + self.lean + smoothed * DRIFT_PROPORTIONAL)
            .clamp(1.0 - DRIFT_MOST, 1.0 + DRIFT_MOST);
        self.ratio
    }

    /// 音声が途切れた（デバイスが空になった）。溜めた量が減っても時計のずれではないので、しばらく調整しない。
    pub fn interrupted(&mut self) {
        self.held = DRIFT_HOLD;
        self.smoothed = None;
    }

    /// 調整せず、学んだ比のまま進める（溜めた量が大きく減ったのを別の手段で戻している間）。
    pub fn coast(&mut self) -> f64 {
        self.smoothed = None;
        self.ratio = 1.0 + self.lean;
        self.ratio
    }

    pub fn holding(&self) -> bool {
        self.held > 0.0
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn reset(&mut self) {
        self.lean = 0.0;
        self.ratio = 1.0;
        self.smoothed = None;
        self.held = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, DRIFT_MOST, Drift, HZ, Wrap};

    #[test]
    fn a_count_that_starts_again_keeps_going_up() {
        let mut wrap = Wrap::new();
        let last = (1 << 33) - 90_000;
        assert_eq!(wrap.feed(last), last);
        assert_eq!(wrap.feed(0), 1 << 33);
        assert_eq!(wrap.feed(90_000), (1 << 33) + 90_000);
    }

    #[test]
    fn a_step_backwards_is_not_a_wrap() {
        let mut wrap = Wrap::new();
        assert_eq!(wrap.feed(1_000_000), 1_000_000);
        assert_eq!(wrap.feed(996_997), 996_997);
        assert_eq!(wrap.feed(1_003_003), 1_003_003);
    }

    #[test]
    fn the_time_is_the_time_of_the_sample_being_played() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 1_000);
        clock.wrote(900_000 + 1_875, 1_000);
        clock.played_at(0, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000));
        clock.played_at(500, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 500 * HZ / 48_000));
        clock.played_at(1_000, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 1_875));
    }

    #[test]
    fn a_paced_run_moves_the_clock_faster() {
        let mut clock = Clock::new(48_000);
        clock.wrote_paced(900_000, 1_000, 2.0);
        clock.played_at(500, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 2 * (500 * HZ / 48_000)));
        clock.played_at(1_000, 0.0);
        clock.missed(1_000);
        clock.played_at(1_500, 0.0);
        let end_of_run = 900_000 + 2 * (1_000 * HZ / 48_000);
        assert_eq!(clock.now_at(0.0), Some(end_of_run + 500 * HZ / 48_000));
    }

    #[test]
    fn a_paused_clock_stands_still() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 48_000);
        clock.played_at(0, 0.0);
        clock.played_at(4_800, 0.1);
        let before = clock.now_at(0.1).expect("running");
        clock.pause(true);
        assert!(clock.is_paused());
        assert_eq!(clock.now_at(1.1), Some(before));
        clock.pause(false);
        clock.played_at(4_800, 1.1);
        let after = clock.now_at(1.1).expect("running");
        assert!((after - before).abs() <= HZ / 100, "{after} vs {before}");
    }

    #[test]
    fn what_is_heard_is_what_the_device_took_a_latency_ago() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 48_000);
        clock.played_at(24_000, 0.0);
        let without = clock.now_at(0.0).expect("sound has been written");
        clock.set_latency(4_800);
        assert_eq!(clock.now_at(0.0), Some(without - HZ / 10));
    }

    #[test]
    fn a_picture_slower_than_the_sound_moves_the_clock_the_other_way() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 48_000);
        clock.played_at(24_000, 0.0);
        let without = clock.now_at(0.0).expect("sound has been written");
        clock.set_latency(-4_800);
        assert_eq!(clock.now_at(0.0), Some(without + HZ / 10));
        assert_eq!(clock.latency(), -4_800);
    }

    #[test]
    fn the_latency_counts_from_the_first_sample_too() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 48_000);
        clock.played_at(0, 0.0);
        clock.set_latency(4_800);
        assert_eq!(clock.now_at(0.0), Some(900_000 - HZ / 10));
    }

    #[test]
    fn between_two_callbacks_the_time_is_read_off_the_line_through_the_treads() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 48_000);
        for tread in 1..=20u64 {
            clock.played_at(tread * 480, tread as f64 * 0.010);
        }
        assert_eq!(clock.step(), 480);
        assert_eq!(clock.now_at(0.200), Some(20 * 900));
        let third = clock
            .now_at(0.200 + 0.010 / 3.0)
            .expect("sound has been written");
        assert!((third - (20 * 900 + 300)).abs() <= 2, "{third}");
        clock.played_at(21 * 480, 0.210);
        let landed = clock.now_at(0.210).expect("sound has been written");
        assert!((landed - 21 * 900).abs() <= 2, "{landed}");
    }

    #[test]
    fn the_line_never_runs_past_what_was_handed_over() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 500);
        clock.played_at(480, 0.010);
        assert_eq!(clock.now_at(0.030), Some(500 * HZ / 48_000));
    }

    #[test]
    fn the_line_has_the_devices_own_rate() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 480_000);
        for tread in 1..=100u64 {
            clock.played_at(tread * 480, tread as f64 * 0.01005);
        }
        let at_end = clock.now_at(1.005).expect("sound has been written");
        assert!((at_end - 100 * 900).abs() <= 2, "{at_end}");
        let half = clock
            .now_at(1.005 + 0.005025)
            .expect("sound has been written");
        assert!((half - (100 * 900 + 450)).abs() <= 3, "{half}");
    }

    #[test]
    fn a_turn_that_sees_two_callbacks_does_not_make_the_time_jump() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 48_000);
        let mut seen = 0u64;
        let mut last: Option<i64> = None;
        for turn in 1..=40u64 {
            let wall = turn as f64 * 0.015;
            while (seen + 1) as f64 * 0.010 <= wall {
                seen += 1;
                clock.played_at(seen * 480, seen as f64 * 0.010);
            }
            let now = clock.now_at(wall).expect("sound has been written");
            if let Some(last) = last
                && turn > 4
            {
                let moved = now - last;
                assert!(
                    (moved - 1_350).abs() <= 120,
                    "turn {turn} moved the clock by {moved}, not about 1350"
                );
            }
            last = Some(now);
        }
    }

    #[test]
    fn silence_the_device_played_still_moves_the_time() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 480);
        clock.played_at(480, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 900));
        clock.missed(480);
        clock.played_at(960, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 1_800));
        clock.wrote(900_000 + 1_800, 480);
        clock.played_at(1_440, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 2_700));
    }

    #[test]
    fn silence_before_the_first_sound_is_not_written_down() {
        let mut clock = Clock::new(48_000);
        clock.missed(4_800);
        assert_eq!(clock.queued(), 0);
        assert_eq!(clock.now_at(0.0), None);
    }

    #[test]
    fn a_gap_in_the_sound_does_not_move_the_time() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 1_000);
        clock.wrote(900_000 + HZ, 1_000);
        clock.played_at(1_000, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + HZ));
    }

    #[test]
    fn the_queue_is_what_was_written_less_what_was_played() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 4_800);
        assert_eq!(clock.queued(), 4_800);
        assert_eq!(clock.buffered(), HZ / 10);
        clock.played_at(2_400, 0.0);
        assert_eq!(clock.queued(), 2_400);
    }

    #[test]
    fn a_device_that_says_it_played_more_than_it_was_given_is_not_believed() {
        let mut clock = Clock::new(48_000);
        clock.wrote(0, 100);
        clock.played_at(1_000_000, 0.0);
        assert_eq!(clock.queued(), 0);
        assert_eq!(clock.now_at(0.0), Some(100 * HZ / 48_000));
    }

    #[test]
    fn a_stream_with_no_sound_is_driven_by_the_caller() {
        let mut clock = Clock::new(48_000);
        clock.advance(3_003);
        assert_eq!(clock.now_at(0.0), None);
        clock.start(900_000);
        assert_eq!(clock.now_at(0.0), Some(900_000));
        clock.advance(3_003);
        assert_eq!(clock.now_at(0.0), Some(903_003));
        clock.wrote(1_000_000, 48);
        assert_eq!(clock.now_at(0.0), Some(1_000_000));
        clock.advance(3_003);
        assert_eq!(clock.now_at(0.0), Some(1_000_000));
    }

    #[test]
    fn a_queue_filling_up_is_answered_by_reading_the_input_faster() {
        let mut drift = Drift::new(4_800);
        let ratio = drift.update(7_200, 0.01);
        assert!(ratio > 1.0, "{ratio}");
        assert!(ratio <= 1.0 + DRIFT_MOST, "{ratio}");
    }

    #[test]
    fn a_queue_running_dry_is_answered_by_reading_the_input_slower() {
        let mut drift = Drift::new(4_800);
        let ratio = drift.update(2_400, 0.01);
        assert!(ratio < 1.0, "{ratio}");
        assert!(ratio >= 1.0 - DRIFT_MOST, "{ratio}");
    }

    #[test]
    fn a_queue_at_its_depth_is_left_alone() {
        let mut drift = Drift::new(4_800);
        for _ in 0..10_000 {
            assert_eq!(drift.update(4_800, 0.001), 1.0);
        }
    }

    #[test]
    fn the_ratio_never_goes_far_from_one() {
        let mut drift = Drift::new(4_800);
        for _ in 0..100_000 {
            let ratio = drift.update(0, 0.01);
            assert!(
                (1.0 - DRIFT_MOST..=1.0 + DRIFT_MOST).contains(&ratio),
                "{ratio}"
            );
        }
    }

    #[test]
    fn what_was_learned_outlives_a_new_depth() {
        let mut drift = Drift::new(4_800);
        for _ in 0..20_000 {
            drift.update(1_000, 0.01);
        }
        let learned = drift.ratio();
        assert!(learned < 1.0);
        let moved = drift.retarget(19_200);
        assert!((moved.ratio() - (1.0 + drift.lean)).abs() < 1e-12);
        assert_eq!(moved.target(), 19_200);
    }

    /// 音声デバイスまでの一本の道。ソースは PC の音声の `source` 倍の速さで、`burst` 秒ごとにまとめて届く
    /// （`gaps` の間は何も届かない）。デバイスは 1 ms に 48 サンプル再生し、溜められるのは目標の 2 倍まで。
    struct Line {
        drift: Drift,
        queued: f64,
        waiting: f64,
        least_after: f64,
        underruns: u32,
        ratios: Vec<(f64, f64)>,
    }

    const DEPTH: u64 = 19_200;

    fn run(source: f64, seconds: f64, burst: impl Fn(u64) -> f64, gaps: &[(f64, f64)]) -> Line {
        let mut line = Line {
            drift: Drift::new(DEPTH),
            queued: DEPTH as f64,
            waiting: 0.0,
            least_after: f64::MAX,
            underruns: 0,
            ratios: Vec::new(),
        };
        let step = 0.001;
        let mut next_burst = burst(0);
        let mut bursts = 1;
        let mut arrived = 0.0;
        for tick in 0..(seconds / step) as u64 {
            let now = tick as f64 * step;
            if !gaps.iter().any(|(from, to)| (*from..*to).contains(&now)) {
                arrived += source * 48.0;
            }
            if now >= next_burst {
                line.waiting += arrived;
                arrived = 0.0;
                next_burst += burst(bursts);
                bursts += 1;
            }
            let ratio = line.drift.ratio();
            let room = (DEPTH * 2) as f64 - line.queued;
            let written = (line.waiting / ratio).min(room.max(0.0));
            line.waiting -= written * ratio;
            line.queued += written;
            line.queued -= 48.0;
            if line.queued < 0.0 {
                line.queued = 0.0;
                line.underruns += 1;
                line.drift.interrupted();
            } else {
                line.drift.update(line.queued as u64, step);
            }
            if now > seconds / 2.0 {
                line.least_after = line.least_after.min(line.queued);
            }
            if tick % 1_000 == 0 {
                line.ratios.push((now, line.drift.ratio()));
            }
        }
        line
    }

    #[test]
    fn a_tuner_four_thousandths_slow_is_taken_out_without_running_dry() {
        // us3pout：PC の音声に対して 0.4% 遅い。
        let line = run(0.996, 400.0, |_| 0.03, &[]);
        assert_eq!(line.underruns, 0);
        assert!(
            line.least_after > DEPTH as f64 * 0.75,
            "後半の最少 {:.0}",
            line.least_after
        );
        let last = line.drift.ratio();
        assert!((last - 0.996).abs() < 5e-4, "{last}");
    }

    #[test]
    fn a_source_a_little_fast_is_read_faster_and_does_not_pile_up() {
        let line = run(1.003, 400.0, |_| 0.03, &[]);
        assert_eq!(line.underruns, 0);
        assert!(
            (line.drift.ratio() - 1.003).abs() < 5e-4,
            "{}",
            line.drift.ratio()
        );
        assert!(
            line.waiting < 48_000.0,
            "溜まり続けない: {:.0}",
            line.waiting
        );
    }

    #[test]
    fn bursty_arrival_on_time_does_not_bend_the_pitch() {
        // 0〜250 ms のばらばらな間隔でまとめて届く。速さそのものは同じ。
        let gap = |n: u64| {
            let x = n
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((x >> 33) % 250) as f64 / 1000.0 + 0.001
        };
        let line = run(1.0, 300.0, gap, &[]);
        assert_eq!(line.underruns, 0);
        // 平均 125 ms ずつまとめて届くのは極端な場合。それでも 0.3%（音の高さで 5 セント）に収まる。
        for (at, ratio) in line.ratios.iter().filter(|(at, _)| *at > 30.0) {
            assert!((ratio - 1.0).abs() < 3e-3, "{at:.0} 秒で {ratio}");
        }
    }

    #[test]
    fn a_dropout_is_not_taken_for_a_slow_clock() {
        // 電波が 3 秒途切れる。届かない間に溜めた分は無くなるが、ソースの速さは変わっていない。
        let line = run(1.0, 60.0, |_| 0.03, &[(20.0, 23.0)]);
        assert!(line.underruns > 0, "途切れたのでデバイスは空になる");
        let around: Vec<f64> = line
            .ratios
            .iter()
            .filter(|(at, _)| (19.0..25.0).contains(at))
            .map(|(_, ratio)| *ratio)
            .collect();
        for ratio in around {
            assert!(
                (ratio - 1.0).abs() < 2e-3,
                "途切れの前後で比を大きく動かさない: {ratio}"
            );
        }
    }

    #[test]
    fn a_held_stretch_of_silence_stops_the_time() {
        let mut clock = Clock::new(48_000);
        clock.wrote(900_000, 4_800);
        clock.held(4_800);
        clock.wrote(900_000 + 9_000, 4_800);
        clock.played_at(4_800, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 9_000));
        clock.played_at(7_200, 0.0);
        assert_eq!(
            clock.now_at(0.0),
            Some(900_000 + 9_000),
            "無音の間は時刻が進まない"
        );
        clock.played_at(9_600, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 9_000));
        clock.played_at(12_000, 0.0);
        assert_eq!(clock.now_at(0.0), Some(900_000 + 9_000 + 4_500));
    }
}
