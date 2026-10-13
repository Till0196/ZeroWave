// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 入力を N 個（1 か 2）遅らせて出す補間（AMF・外部プロセス）。入力 k が来たら k−1 と k の
//! 中間フレームを待たずに要求し、画面には N 個前の区間を位相で出す。中間フレームが完了していなければ近い方の
//! フレームを出す（止まらない）。遅れる N 入力分はチェーンの遅延に足す（[`super::Host::held_inputs`]）。

use std::time::Instant;

use super::{Convert, Gpu, Run};

pub(super) const MOST_HELD: u32 = 2;

/// プラグインが指定しないときの数。バックエンドは 60p の一入力（16.7 ms）に収まらない回があるので 2。
pub(super) const DEFAULT_HELD: u32 = 2;

/// 出している区間の 2 フレーム、その後ろに要求した物、今の入力。
pub(super) const SLOTS: usize = MOST_HELD as usize + 2;

pub(super) fn slot(index: u64) -> usize {
    (index % SLOTS as u64) as usize
}

pub(super) trait Frc {
    fn size(&self) -> [u32; 2];
    /// 前の入力をバックエンドが自分で持つか（AMF）。持つなら、要求し損ねた次の入力の中間フレームは使えない。
    fn chained(&self) -> bool;
    /// 毎回呼ぶ。
    fn poll(&mut self, gpu: &mut Gpu<'_>) -> Result<(), String>;
    /// ビジーで要求できなければ false（次の回にまた呼ぶ）。
    fn submit(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        prev: &wgpu::Texture,
        cur: &wgpu::Texture,
    ) -> Result<bool, String>;
    /// CPU でフェンスを見るだけ。
    fn ready(&mut self, index: u64) -> Result<bool, String>;
    /// 初めてならレンダリング側へインポートしてから。
    fn put(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        target: &wgpu::Texture,
    ) -> Result<(), String>;
    fn work_ms(&self) -> f64 {
        0.0
    }
    /// 毎回呼ぶ。
    fn news(&mut self, gpu: &mut Gpu<'_>) -> Option<zwsdk::video::Event> {
        let _ = gpu;
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pick {
    Prev,
    Mid,
    Cur,
}

/// 中間フレームがあれば位相 0.25〜0.75 に、無ければ近い方のフレーム。
pub(super) fn pick(phase: f32, mid: bool) -> Pick {
    if mid_phase(phase) && mid {
        Pick::Mid
    } else if phase < 0.5 {
        Pick::Prev
    } else {
        Pick::Cur
    }
}

pub(super) fn mid_phase(phase: f32) -> bool {
    (0.25..=0.75).contains(&phase)
}

/// 0 は [`DEFAULT_HELD`]。
pub(super) fn held(wanted: u32) -> u32 {
    let held = if wanted == 0 { DEFAULT_HELD } else { wanted };
    held.clamp(1, MOST_HELD)
}

/// GPU を持たない。
#[derive(Debug, Default, Clone)]
pub(super) struct Ledger {
    held: u32,
    /// 今の入力の番号はこれ − 1。
    inputs: u64,
    asked: [Option<(u64, bool)>; SLOTS],
    first: Option<u64>,
    last: Option<u64>,
    mid_phase: bool,
    mid_shown: bool,
    /// バックエンドの準備が済んで、初めに要求できた入力の区間から数える。
    pub made: u64,
    pub missed: u64,
    pub requests: u64,
}

impl Ledger {
    pub fn hold(&mut self, held: u32) {
        self.held = held.clamp(1, MOST_HELD);
    }

    pub fn held(&self) -> u32 {
        self.held.max(1)
    }

    pub fn arrive(&mut self) -> u64 {
        let counted = self
            .current()
            .and_then(|k| k.checked_sub(u64::from(self.held())))
            .is_some_and(|want| self.first.is_some_and(|first| want >= first));
        if self.mid_phase && counted {
            if self.mid_shown {
                self.made += 1;
            } else {
                self.missed += 1;
            }
        }
        self.mid_phase = false;
        self.mid_shown = false;
        let index = self.inputs;
        self.inputs += 1;
        index
    }

    pub fn current(&self) -> Option<u64> {
        self.inputs.checked_sub(1)
    }

    /// 2 入力遅らせるなら、ビジーで要求できなかった今の入力を次の入力まで要求し直す。1 入力では
    /// 遅れて要求した物はまず間に合わず、次を要求する邪魔になるので要求し直さない。
    pub fn to_ask(&self, new_input: bool) -> Option<u64> {
        self.current()
            .filter(|&k| self.last != Some(k))
            .filter(|_| new_input || self.held() >= 2)
    }

    /// `chained` なら一つ前も要求できていなければ中間フレームを使わない。
    pub fn asked(&mut self, index: u64, chained: bool) {
        let usable = index > 0 && (!chained || self.last == Some(index - 1));
        self.asked[slot(index)] = Some((index, usable));
        self.first.get_or_insert(index);
        self.last = Some(index);
        self.requests += 1;
    }

    /// k−N−1 と k−N、初めは 0 で止める。
    pub fn shown(&self) -> Option<(u64, u64)> {
        let cur = self.current()?.saturating_sub(u64::from(self.held()));
        Some((cur.saturating_sub(1), cur))
    }

    /// k−N。要求できていないか使えなければ無し。
    pub fn wanted(&self) -> Option<u64> {
        let want = self.current()?.checked_sub(u64::from(self.held()))?;
        match self.asked[slot(want)] {
            Some((at, true)) if at == want => Some(want),
            _ => None,
        }
    }

    pub fn saw(&mut self, mid: bool) {
        self.mid_phase = true;
        self.mid_shown |= mid;
    }
}

/// 要求してから完了を CPU で確認するまでの時間。出すかどうかとは別に毎回見る。
#[derive(Debug, Default, Clone)]
pub(super) struct Turnaround {
    asked: [Option<(u64, Instant)>; SLOTS],
    sum: f64,
    count: u64,
}

impl Turnaround {
    pub fn ask(&mut self, index: u64, at: Instant) {
        // 同じスロットの前の物は諦めた物なので数えない。
        self.asked[slot(index)] = Some((index, at));
    }

    pub fn waiting(&self) -> [Option<u64>; SLOTS] {
        self.asked.map(|asked| asked.map(|(index, _)| index))
    }

    pub fn done(&mut self, index: u64, at: Instant) {
        if let Some((_, since)) = self.asked[slot(index)].take_if(|(i, _)| *i == index) {
            self.sum += at.saturating_duration_since(since).as_secs_f64() * 1000.0;
            self.count += 1;
        }
    }

    /// まだ無ければ 0。
    pub fn mean_ms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}

pub(super) struct Lagged<F> {
    pub frc: F,
    ledger: Ledger,
    /// 番号 % [`SLOTS`]。
    pictures: Vec<wgpu::Texture>,
    turnaround: Turnaround,
    spent: f64,
}

impl<F: Frc> Lagged<F> {
    pub fn new(frc: F) -> Self {
        let mut ledger = Ledger::default();
        ledger.hold(DEFAULT_HELD);
        Self {
            frc,
            ledger,
            pictures: Vec::new(),
            turnaround: Turnaround::default(),
            spent: 0.0,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.frc.size()
    }

    pub fn hold(&mut self, wanted: u32) {
        self.ledger.hold(held(wanted));
    }

    pub fn held(&self) -> u32 {
        self.ledger.held()
    }

    pub fn outside_ms(&self) -> f64 {
        self.spent
    }

    pub fn news(&mut self, gpu: &mut Gpu<'_>) -> Option<zwsdk::video::Event> {
        self.frc.news(gpu)
    }

    pub fn report(&self) -> super::Lag {
        super::Lag {
            made: self.ledger.made,
            missed: self.ledger.missed,
            requests: self.ledger.requests,
            turnaround_ms: self.turnaround.mean_ms(),
            work_ms: self.frc.work_ms(),
        }
    }

    /// フォーマットは出力先の物、大きさは入力の物。フォーマットか大きさが変われば作り直す。
    fn keep(&mut self, gpu: &mut Gpu<'_>, run: &Run<'_>) {
        let format = run.target.format();
        let size = run.cur.size();
        if self
            .pictures
            .first()
            .is_none_or(|t| t.format() != format || t.size() != size)
        {
            self.pictures = (0..SLOTS)
                .map(|_| {
                    gpu.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("zw lag picture"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    })
                })
                .collect();
        }
    }

    pub fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        let began = Instant::now();
        let result = self.step(gpu, convert, run);
        let ms = began.elapsed().as_secs_f64() * 1000.0;
        self.spent = if self.spent > 0.0 {
            self.spent * 0.9 + ms * 0.1
        } else {
            ms
        };
        result
    }

    fn step(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        self.hold(run.held);
        self.frc.poll(gpu)?;
        if run.new_input {
            self.keep(gpu, run);
            let index = self.ledger.arrive();
            convert.put(gpu, run.cur, &self.pictures[slot(index)]);
        }
        if let Some(index) = self.ledger.to_ask(run.new_input) {
            let prev = &self.pictures[slot(index.saturating_sub(1))];
            let cur = &self.pictures[slot(index)];
            let (prev, cur) = (prev.clone(), cur.clone());
            if self.frc.submit(gpu, convert, index, &prev, &cur)? {
                self.ledger.asked(index, self.frc.chained());
                self.turnaround.ask(index, Instant::now());
            }
        }
        for index in self.turnaround.waiting().into_iter().flatten() {
            if self.frc.ready(index)? {
                self.turnaround.done(index, Instant::now());
            }
        }
        let Some((before, after)) = self.ledger.shown().filter(|_| !self.pictures.is_empty())
        else {
            convert.put(gpu, run.cur, run.target);
            return Ok(());
        };
        let wanted = self.ledger.wanted();
        let mid = match wanted {
            Some(index) if mid_phase(run.phase) => self.frc.ready(index)?,
            _ => false,
        };
        if mid_phase(run.phase) {
            self.ledger.saw(mid);
        }
        match (pick(run.phase, mid), wanted) {
            (Pick::Mid, Some(index)) => self.frc.put(gpu, convert, index, run.target)?,
            (Pick::Cur, _) | (Pick::Mid, None) => {
                convert.put(gpu, &self.pictures[slot(after)], run.target)
            }
            (Pick::Prev, _) => convert.put(gpu, &self.pictures[slot(before)], run.target),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ledger(held: u32) -> Ledger {
        let mut ledger = Ledger::default();
        ledger.hold(held);
        ledger
    }

    #[test]
    fn the_shown_interval_is_one_input_behind() {
        let mut ledger = ledger(1);
        assert_eq!(ledger.shown(), None);
        ledger.arrive();
        assert_eq!(ledger.shown(), Some((0, 0)), "初めは今のフレームで埋める");
        ledger.arrive();
        assert_eq!(ledger.shown(), Some((0, 0)));
        ledger.arrive();
        assert_eq!(ledger.shown(), Some((0, 1)));
        ledger.arrive();
        assert_eq!(ledger.shown(), Some((1, 2)), "入力 3 の間は 1 と 2 の間");
    }

    #[test]
    fn the_shown_interval_is_two_inputs_behind() {
        let mut ledger = ledger(2);
        let shown: Vec<_> = (0..5)
            .map(|_| {
                ledger.arrive();
                ledger.shown()
            })
            .collect();
        assert_eq!(
            shown,
            [
                Some((0, 0)),
                Some((0, 0)),
                Some((0, 0)),
                Some((0, 1)),
                Some((1, 2))
            ],
            "入力 4 の間は 1 と 2 の間"
        );
    }

    #[test]
    fn the_middle_comes_from_the_input_held_back() {
        for (held, want) in [(1, 2), (2, 1)] {
            let mut ledger = ledger(held);
            for _ in 0..4 {
                let k = ledger.arrive();
                ledger.asked(k, true);
            }
            // 入力 3 の間に出すのは N 個前の区間で、入力 3 − N のときに要求した物。
            assert_eq!(ledger.wanted(), Some(want), "N = {held}");
            assert_eq!(ledger.shown(), Some((want - 1, want)));
        }
    }

    #[test]
    fn the_first_input_has_no_middle() {
        for held in [1, 2] {
            let mut ledger = ledger(held);
            for k in 0..=held {
                assert_eq!(ledger.arrive(), u64::from(k));
                ledger.asked(u64::from(k), false);
            }
            assert_eq!(ledger.wanted(), None, "入力 0 は前が無い（N = {held}）");
            let k = ledger.arrive();
            ledger.asked(k, false);
            assert_eq!(ledger.wanted(), Some(1));
        }
    }

    #[test]
    fn a_skipped_input_spoils_the_next_middle_of_a_chained_worker() {
        for held in [1u64, 2] {
            for chained in [true, false] {
                let mut ledger = ledger(held as u32);
                for k in 0..8 {
                    assert_eq!(ledger.arrive(), k);
                    if k != 3 {
                        ledger.asked(k, chained);
                    }
                    let wanted = ledger.wanted();
                    match k - held.min(k) {
                        3 => assert_eq!(wanted, None, "要求できなかった入力 3"),
                        4 if chained => {
                            assert_eq!(wanted, None, "AMF は 2 と 4 の間を作ってしまう")
                        }
                        4 => assert_eq!(wanted, Some(4), "2 フレームとも渡すなら使える"),
                        _ => {}
                    }
                }
            }
        }
    }

    #[test]
    fn a_busy_worker_is_asked_again_until_the_next_input() {
        let mut ledger = ledger(2);
        assert_eq!(ledger.to_ask(false), None);
        let k = ledger.arrive();
        assert_eq!(ledger.to_ask(true), Some(k));
        ledger.asked(k, false);
        assert_eq!(ledger.to_ask(false), None, "要求できたらもう要求しない");
        ledger.arrive();
        assert_eq!(ledger.to_ask(true), Some(1));
        assert_eq!(ledger.to_ask(false), Some(1), "空くまで要求し直す");
        let k = ledger.arrive();
        assert_eq!(ledger.to_ask(true), Some(k), "要求するのは今の入力だけ");
        ledger.asked(k, false);
        assert_eq!(ledger.requests, 2);
    }

    #[test]
    fn one_input_behind_asks_only_when_the_input_comes() {
        let mut ledger = ledger(1);
        let k = ledger.arrive();
        assert_eq!(ledger.to_ask(true), Some(k));
        assert_eq!(ledger.to_ask(false), None, "遅れて要求しても間に合わない");
    }

    #[test]
    fn an_old_job_in_the_same_slot_is_not_taken() {
        for held in [1u64, 2] {
            let mut ledger = ledger(held as u32);
            let k = ledger.arrive();
            ledger.asked(k, false);
            let k = ledger.arrive();
            ledger.asked(k, false);
            while ledger.current() != Some(1 + SLOTS as u64 + held) {
                ledger.arrive();
            }
            let want = ledger.current().unwrap() - held;
            assert_eq!(slot(want), 1);
            assert_eq!(
                ledger.wanted(),
                None,
                "スロットに残る入力 1 の物は {want} の中間フレームではない"
            );
        }
    }

    /// 二回に一つ新しい入力で回す。バックエンドは `latency` 回後に完了し、同時に `jobs` 件まで処理できる。
    fn simulate(held: u32, jobs: usize, latency: u64, ticks: u64) -> (u64, u64, u64) {
        let mut ledger = ledger(held);
        let mut done_at = std::collections::BTreeMap::new();
        for tick in 0..ticks {
            let new_input = tick % 2 == 0;
            if new_input {
                ledger.arrive();
            }
            if let Some(index) = ledger.to_ask(new_input) {
                let busy = done_at.values().filter(|&&at| at > tick).count();
                if busy < jobs {
                    ledger.asked(index, false);
                    done_at.insert(index, tick + latency);
                }
            }
            let phase = if new_input { 0.0 } else { 0.5 };
            let wanted = ledger.wanted();
            let mid = wanted.is_some_and(|index| done_at[&index] <= tick);
            if mid_phase(phase) {
                ledger.saw(mid);
            }
            let shown = pick(phase, mid);
            assert!(
                mid || shown != Pick::Mid,
                "完了していない中間フレームは出さない"
            );
        }
        (ledger.made, ledger.missed, ledger.requests)
    }

    #[test]
    fn two_jobs_at_once_keep_up_with_a_slow_worker() {
        // 一つの処理に入力二つ分かかるバックエンド。入力 0 は前が無いので、その区間はいつも埋める。
        assert_eq!(
            simulate(2, 1, 4, 200),
            (48, 49, 50),
            "一つずつなら入力の半分しか要求できない"
        );
        assert_eq!(
            simulate(2, 2, 4, 200),
            (96, 1, 100),
            "二つ同時に処理できれば全部の入力を要求できて、二入力遅らせれば間に合う"
        );
        let (made, _, asked) = simulate(1, 2, 4, 200);
        assert_eq!((made, asked), (0, 100));
    }

    #[test]
    fn a_quick_worker_needs_only_one_job() {
        for jobs in [1, 2] {
            assert_eq!(simulate(1, jobs, 2, 200), (97, 1, 100), "同時に {jobs}");
        }
    }

    #[test]
    fn without_a_middle_the_nearer_picture_is_shown() {
        assert_eq!(pick(0.0, true), Pick::Prev);
        assert_eq!(pick(0.2, true), Pick::Prev);
        assert_eq!(pick(0.25, true), Pick::Mid);
        assert_eq!(pick(0.75, true), Pick::Mid);
        assert_eq!(pick(0.8, true), Pick::Cur);
        assert_eq!(pick(0.3, false), Pick::Prev);
        assert_eq!(pick(0.5, false), Pick::Cur);
        assert_eq!(pick(0.7, false), Pick::Cur);
    }

    #[test]
    fn the_middle_is_shown_only_around_the_half() {
        for step in 0..=20 {
            let phase = step as f32 / 20.0;
            let with = pick(phase, true);
            let without = pick(phase, false);
            assert_ne!(without, Pick::Mid, "位相 {phase}");
            if mid_phase(phase) {
                assert_eq!(with, Pick::Mid, "位相 {phase}");
            } else {
                assert_eq!(
                    with, without,
                    "位相 0.25〜0.75 の外は近い方（位相 {phase}）"
                );
            }
        }
    }

    #[test]
    fn intervals_are_counted_once() {
        let mut ledger = Ledger::default();
        let k = ledger.arrive();
        ledger.asked(k, false);
        ledger.arrive();
        ledger.saw(false);
        ledger.saw(true);
        ledger.arrive();
        ledger.saw(false);
        ledger.arrive();
        ledger.arrive();
        assert_eq!((ledger.made, ledger.missed), (1, 1));
    }

    #[test]
    fn intervals_before_the_worker_is_ready_are_not_counted() {
        for held in [1, 2] {
            let mut ledger = ledger(held);
            for _ in 0..3 {
                ledger.arrive();
                ledger.saw(false);
            }
            let k = ledger.arrive();
            ledger.asked(k, false);
            ledger.saw(false);
            // 入力 3 の中間フレームを出す区間から数える。
            for _ in 0..held {
                ledger.arrive();
                assert_eq!(ledger.missed, 0, "N = {held}");
                ledger.saw(false);
            }
            ledger.arrive();
            assert_eq!((ledger.made, ledger.missed), (0, 1), "N = {held}");
        }
    }

    #[test]
    fn the_held_inputs_follow_the_plugin() {
        assert_eq!(held(0), DEFAULT_HELD, "指定しなければ既定");
        assert_eq!(held(1), 1);
        assert_eq!(held(2), 2);
        assert_eq!(held(9), MOST_HELD);
        assert_eq!(Ledger::default().held(), 1);
    }

    #[test]
    fn the_turnaround_is_from_asking_to_seeing_it_done() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut turnaround = Turnaround::default();
        assert_eq!(turnaround.mean_ms(), 0.0);
        turnaround.ask(0, ms(0));
        turnaround.ask(1, ms(17));
        assert_eq!(turnaround.waiting(), [Some(0), Some(1), None, None]);
        turnaround.done(0, ms(12));
        // 出すのが二入力あとでも、数えるのは見た時刻まで。
        turnaround.done(1, ms(17 + 20));
        turnaround.done(1, ms(500));
        assert!((turnaround.mean_ms() - 16.0).abs() < 1e-9);
        assert_eq!(turnaround.waiting(), [None; SLOTS]);
        turnaround.ask(2, ms(30));
        turnaround.ask(2 + SLOTS as u64, ms(100));
        turnaround.done(2, ms(110));
        turnaround.done(2 + SLOTS as u64, ms(110));
        assert!((turnaround.mean_ms() - 14.0).abs() < 1e-9);
    }

    #[test]
    fn slots_turn_round() {
        assert_eq!(SLOTS, 4);
        assert_eq!(
            (0..9).map(slot).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 0, 1, 2, 3, 0]
        );
    }
}
