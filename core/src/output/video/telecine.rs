// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 60i の中の 24p（2:3 プルダウン）をサンプルから見つける（放送はハードテレシネでフラグが無い）。
//! フィールドごとに二つ前の同じパリティのフィールドとブロック単位で比べ、5 フレームのパターンと照合する。
//! 判定できないフレームは 60i と見なす（ウィーブを誤ると縞になる）。

use std::collections::VecDeque;

use mediadecoder::video::Frame;

use super::field::{Cadence, Parity, first_field};

/// 再エンコードされた繰り返しのフィールドは一、二ずれる。
const MOVED: u8 = 8;

/// フィールドのサンプルで 16 列 × 8 行。全体の数では再エンコードのノイズと小さなオーバーレイ（字幕・時計）を
/// 見分けられないが、ブロックなら二桁違う。
const BLOCK_WIDTH: usize = 16;
const BLOCK_HEIGHT: usize = 8;

/// 4 なら四分の一。
const BUSY: u32 = 4;

/// 繰り返しと呼べる動きのあるブロックの割合。再エンコードで 0 にはならず、いちばん小さいオーバーレイより
/// 十分少ない所に置く。
const QUIET_PART: usize = 1024;
const QUIET_MOST: usize = 1;

/// 本当に動いたと言える割合。間のフィールドは判定に使わない。
const MOVING_PART: usize = 128;
const MOVING_LEAST: usize = 4;

/// パターンの 3 周期。判定に 2 周期、3 つ目は判定の根拠がすぐ古くならないため。
const WINDOW: usize = 15;

/// 判定に使えないフレームがこれだけ続いたらロックを外す（静止の間に編集点が来うる）。
const MOST_STILL: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Still,
    Moved,
    /// 先に見せるフィールドが、二つ前のフィールドの繰り返し。
    RepeatFirst,
    RepeatSecond,
}

/// 繰り返しは 3 フレーム目と 5 フレーム目で、繰り返すフィールドが違うことで位相が決まる。
const PATTERN: [Symbol; 5] = [
    Symbol::Moved,
    Symbol::Moved,
    Symbol::RepeatFirst,
    Symbol::Moved,
    Symbol::RepeatSecond,
];

const BECOMES: [Cadence; 5] = [
    Cadence::Whole,
    Cadence::Whole,
    Cadence::Drop,
    Cadence::Spanning,
    Cadence::Spanning,
];

#[derive(Debug, Clone)]
struct Snapshot {
    first: Parity,
    /// [`Parity::first_row`] で求める。
    rows: [Vec<u8>; 2],
    width: usize,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            first: Parity::Top,
            rows: [Vec::new(), Vec::new()],
            width: 0,
        }
    }
}

impl Snapshot {
    /// [`super::filter::Samples`] を通すと毎フレーム CPU で重いので、行ごとに切り出して読む。
    fn take(&mut self, frame: &Frame) {
        let plane = &frame.planes[0];
        let wide = frame.format.depth > 8;
        let shift = frame.format.depth.saturating_sub(8);
        self.first = first_field(&crate::decode::About::of_frame(frame));
        self.width = plane.width;
        for (parity, out) in self.rows.iter_mut().enumerate() {
            out.clear();
            out.reserve(plane.width * plane.height / 2);
            for y in (parity..plane.height).step_by(2) {
                let row = plane.row(y);
                if !wide {
                    match row.get(..plane.width) {
                        Some(row) => out.extend_from_slice(row),
                        None => out.resize(out.len() + plane.width, 0),
                    }
                    continue;
                }
                for x in 0..plane.width {
                    let at = x * 2;
                    let low = u32::from(row.get(at).copied().unwrap_or(0));
                    let high = u32::from(row.get(at + 1).copied().unwrap_or(0));
                    out.push(((low | (high << 8)) >> shift) as u8);
                }
            }
        }
    }

    /// 比べられなければ `None`。
    fn busy(&self, earlier: &Self, parity: Parity) -> Option<(usize, usize)> {
        let at = parity.first_row() as usize;
        let (now, then) = (&self.rows[at], &earlier.rows[at]);
        let width = self.width;
        if width == 0 || earlier.width != width || now.len() != then.len() || now.is_empty() {
            return None;
        }
        let rows = now.len() / width;
        let columns = width.div_ceil(BLOCK_WIDTH);
        let mut counts = vec![0u32; columns];
        let mut busy = 0;
        let mut y = 0;
        while y < rows {
            let deep = BLOCK_HEIGHT.min(rows - y);
            counts.fill(0);
            for row in y..y + deep {
                let (a, b) = (
                    &now[row * width..(row + 1) * width],
                    &then[row * width..(row + 1) * width],
                );
                for (block, (a, b)) in a.chunks(BLOCK_WIDTH).zip(b.chunks(BLOCK_WIDTH)).enumerate()
                {
                    // 数えずに足す：ベクトル命令になる。
                    counts[block] += a
                        .iter()
                        .zip(b)
                        .map(|(a, b)| u32::from(a.abs_diff(*b) > MOVED))
                        .sum::<u32>();
                }
            }
            for (block, count) in counts.iter().enumerate() {
                let wide = BLOCK_WIDTH.min(width - block * BLOCK_WIDTH);
                busy += usize::from(count * BUSY > (wide * deep) as u32);
            }
            y += deep;
        }
        Some((busy, columns * rows.div_ceil(BLOCK_HEIGHT)))
    }
}

/// プルダウンの検出器。見逃すもの：60i のオーバーレイが載ったフィルム、長い静止、2:2 プルダウンと
/// 60i の中の 30p、混在したプルダウン、編集点のフレーム。
#[derive(Debug, Default)]
pub struct Detector {
    /// 毎フレーム走るので確保し直さず入れ替える。
    previous: Snapshot,
    scratch: Snapshot,
    held: bool,
    window: VecDeque<Symbol>,
    count: u64,
    phase: Option<u64>,
    /// 先読みはロックした周期の残りには手を出さない。
    locked_at: u64,
    still: usize,
    /// 前のフレームの先のフィールド。違えば比べない。
    last_first: Option<Parity>,
}

impl Detector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.held = false;
        self.window.clear();
        self.count = 0;
        self.phase = None;
        self.still = 0;
    }

    /// 結果がまだ届いていないフレームを、ロックしている周期から先読みして決める（GPU の結果は遅れて届く）。
    /// ロックした周期の残りは先読みしない（前のフレームのフィールドを当てにするため）。
    pub fn predict(&self, ahead: u64) -> Cadence {
        let Some(phase) = self.phase else {
            return Cadence::None;
        };
        let at = self.count + ahead;
        let step = (at + 5 - phase % 5) % 5;
        if at - step <= self.locked_at {
            return Cadence::None;
        }
        BECOMES[step as usize]
    }

    pub fn locked(&self) -> bool {
        self.phase.is_some()
    }

    /// サンプルが要るのでソフトウェアのパスだけ。
    pub fn push(&mut self, frame: &Frame) -> Cadence {
        let mut now = std::mem::take(&mut self.scratch);
        now.take(frame);
        // フィールドの順が前と違うフレームは、パターンへの反証ではなく連続の終わり。
        let symbol = if self.held && self.previous.first == now.first {
            classify(&self.previous, &now)
        } else {
            None
        };
        self.last_first = Some(now.first);
        self.scratch = std::mem::replace(&mut self.previous, now);
        self.held = true;
        self.advance(symbol)
    }

    /// `measured` は先と後のフィールドの（動きのあるブロック数、総数）。無ければ比べられなかったフレーム。
    pub fn push_measured(
        &mut self,
        first: Parity,
        measured: Option<((usize, usize), (usize, usize))>,
    ) -> Cadence {
        let symbol = match measured {
            Some((leading, trailing)) if self.last_first == Some(first) => {
                classify_counts(leading, trailing)
            }
            _ => None,
        };
        self.last_first = Some(first);
        self.advance(symbol)
    }

    fn advance(&mut self, symbol: Option<Symbol>) -> Cadence {
        let at = self.count;
        self.count += 1;

        // 比べられなかったフレームは反証ではなく、連続の終わり。
        let Some(symbol) = symbol else {
            self.window.clear();
            self.phase = None;
            self.still = 0;
            return Cadence::None;
        };
        self.window.push_back(symbol);
        if self.window.len() > WINDOW {
            self.window.pop_front();
        }
        self.still = if symbol == Symbol::Still {
            self.still + 1
        } else {
            0
        };
        if self.still >= MOST_STILL {
            self.phase = None;
        }

        if let Some(phase) = self.phase {
            let step = ((at + 5 - phase % 5) % 5) as usize;
            if symbol != Symbol::Still && symbol != PATTERN[step] {
                self.phase = None;
                self.window.clear();
                return Cadence::None;
            }
            return BECOMES[step];
        }

        // 位相は周期の頭でだけロックする（最初の出力が前のフレームを要らないように）。
        if let Some(phase) = self.candidate() {
            let step = ((at + 5 - phase % 5) % 5) as usize;
            if step < 2 {
                self.phase = Some(phase);
                self.locked_at = at;
                return BECOMES[step];
            }
        }
        Cadence::None
    }

    /// ウィンドウがちょうど一つの位相に二周期分合っていれば、その位相。
    fn candidate(&self) -> Option<u64> {
        let first = self.count - self.window.len() as u64;
        let mut found = None;
        for phase in 0..5u64 {
            let mut firsts = 0;
            let mut seconds = 0;
            let mut moved = 0;
            let mut agrees = true;
            for (index, symbol) in self.window.iter().enumerate() {
                let at = first + index as u64;
                let want = PATTERN[((at + 5 - phase) % 5) as usize];
                if *symbol == Symbol::Still {
                    continue;
                }
                if *symbol != want {
                    agrees = false;
                    break;
                }
                match want {
                    Symbol::RepeatFirst => firsts += 1,
                    Symbol::RepeatSecond => seconds += 1,
                    _ => moved += 1,
                }
            }
            // 動いたフレームも求めて、静止ばかりを数えない。
            if agrees && firsts >= 2 && seconds >= 2 && moved >= 4 {
                if found.is_some() {
                    return None;
                }
                found = Some(phase);
            }
        }
        found
    }
}

fn classify(before: &Snapshot, now: &Snapshot) -> Option<Symbol> {
    let leading = now.busy(before, now.first)?;
    let trailing = now.busy(before, now.first.other())?;
    classify_counts(leading, trailing)
}

fn classify_counts(leading: (usize, usize), trailing: (usize, usize)) -> Option<Symbol> {
    let (first, blocks) = leading;
    let (second, _) = trailing;
    let moving = (blocks / MOVING_PART).max(MOVING_LEAST);
    let quiet = (blocks / QUIET_PART).max(QUIET_MOST);
    if first <= quiet && second >= moving {
        return Some(Symbol::RepeatFirst);
    }
    if second <= quiet && first >= moving {
        return Some(Symbol::RepeatSecond);
    }
    if first < moving && second < moving {
        return Some(Symbol::Still);
    }
    Some(Symbol::Moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_measured_elsewhere_lock_the_same_cadence() {
        let blocks = 1000;
        let moved = 200;
        let quiet = 0;
        let cycle = [
            (moved, moved),
            (moved, moved),
            (quiet, moved),
            (moved, moved),
            (moved, quiet),
        ];
        let mut detector = Detector::new();
        let mut answers = Vec::new();
        for at in 0..30 {
            let (first, second) = cycle[at % 5];
            let cadence =
                detector.push_measured(Parity::Top, Some(((first, blocks), (second, blocks))));
            answers.push(cadence);
        }
        assert!(detector.locked(), "{answers:?}");
        // ロックしてからは一周期に Whole が 2 フレーム、Spanning が 2 フレーム、Drop が 1 フレーム。
        let last: Vec<Cadence> = answers[25..30].to_vec();
        assert_eq!(
            last.iter().filter(|c| **c == Cadence::Whole).count(),
            2,
            "{last:?}"
        );
        assert_eq!(
            last.iter().filter(|c| **c == Cadence::Drop).count(),
            1,
            "{last:?}"
        );
        let cadence =
            detector.push_measured(Parity::Bottom, Some(((moved, blocks), (moved, blocks))));
        assert_eq!(cadence, Cadence::None);
        assert!(!detector.locked());
    }

    #[test]
    fn a_lock_is_carried_ahead_of_the_answers() {
        let blocks = 1000;
        let moved = 200;
        let cycle = [
            (moved, moved),
            (moved, moved),
            (0, moved),
            (moved, moved),
            (moved, 0),
        ];
        let mut detector = Detector::new();
        assert_eq!(detector.predict(0), Cadence::None);
        let mut answers = Vec::new();
        for at in 0..30 {
            let (first, second) = cycle[at % 5];
            answers.push(
                detector.push_measured(Parity::Top, Some(((first, blocks), (second, blocks)))),
            );
        }
        assert!(detector.locked());
        let predicted: Vec<Cadence> = (0..10).map(|ahead| detector.predict(ahead)).collect();
        for (ahead, predicted) in predicted.into_iter().enumerate() {
            let (first, second) = cycle[(30 + ahead) % 5];
            let said =
                detector.push_measured(Parity::Top, Some(((first, blocks), (second, blocks))));
            assert_eq!(predicted, said, "{ahead}");
        }
        let mut fresh = Detector::new();
        let mut locked_after = None;
        for at in 0..30 {
            let (first, second) = cycle[at % 5];
            fresh.push_measured(Parity::Top, Some(((first, blocks), (second, blocks))));
            if fresh.locked() && locked_after.is_none() {
                locked_after = Some(at);
                break;
            }
        }
        assert!(locked_after.is_some());
        let step = ((fresh.count - 1 + 5 - fresh.phase.unwrap() % 5) % 5) as usize;
        let rest = 4 - step;
        for ahead in 0..rest as u64 {
            assert_eq!(fresh.predict(ahead), Cadence::None, "{ahead}");
        }
        assert_eq!(fresh.predict(rest as u64), Cadence::Whole);
    }
    use mediadecoder::es::{Format, Structure};
    use mediadecoder::video::Plane;

    const W: usize = 128;
    const H: usize = 128;

    fn film(at: usize) -> Vec<u8> {
        let mut data = vec![0u8; W * H];
        for y in 0..H {
            for x in 0..W {
                let value = ((x + 3 * at) % 16) * 14 + (y % 5) * 3;
                data[y * W + x] = value.min(235) as u8;
            }
        }
        data
    }

    fn coded(top: &[u8], bottom: &[u8], first: Parity) -> Frame {
        let mut data = vec![0u8; W * H];
        for y in 0..H {
            let from = if y % 2 == 0 { top } else { bottom };
            data[y * W..y * W + W].copy_from_slice(&from[y * W..y * W + W]);
        }
        let plane = |width: usize, height: usize| Plane {
            data: vec![128u8; width * height],
            stride: width,
            width,
            height,
        };
        Frame {
            tag: 0,
            format: Format {
                width: W as u32,
                height: H as u32,
                depth: 8,
                chroma: 1,
                rate: (30000, 1001),
                progressive: false,
                aspect: (16, 9),
                field_pictures: false,
                woven: false,
                colour: Default::default(),
            },
            planes: [
                Plane {
                    data,
                    stride: W,
                    width: W,
                    height: H,
                },
                plane(W / 2, H / 2),
                plane(W / 2, H / 2),
            ],
            structure: Structure::Frame,
            top_field_first: first == Parity::Top,
            repeat_first_field: false,
            progressive: false,
        }
    }

    fn telecined(cycle: usize) -> Vec<Frame> {
        let f = |n: usize| film(4 * cycle + n);
        let (a, b, c, d) = (f(0), f(1), f(2), f(3));
        vec![
            coded(&a, &a, Parity::Top),
            coded(&b, &b, Parity::Top),
            coded(&b, &c, Parity::Top),
            coded(&c, &d, Parity::Top),
            coded(&d, &d, Parity::Top),
        ]
    }

    #[test]
    fn a_pulldown_is_found_and_its_phase_is_right() {
        let mut detector = Detector::new();
        let mut got = Vec::new();
        for cycle in 0..6 {
            for frame in telecined(cycle) {
                got.push(detector.push(&frame));
            }
        }
        assert!(detector.locked(), "the cadence was never believed");
        let tail = &got[got.len() - 10..];
        assert_eq!(
            tail,
            [
                Cadence::Whole,
                Cadence::Whole,
                Cadence::Drop,
                Cadence::Spanning,
                Cadence::Spanning,
                Cadence::Whole,
                Cadence::Whole,
                Cadence::Drop,
                Cadence::Spanning,
                Cadence::Spanning,
            ],
            "{got:?}"
        );
        let shown = got.iter().filter(|c| **c != Cadence::Drop).count();
        assert!(shown >= 24, "{shown} of 30 shown");
    }

    #[test]
    fn genuine_sixty_fields_a_second_never_fires() {
        let mut detector = Detector::new();
        for at in 0..60 {
            let frame = coded(&film(2 * at), &film(2 * at + 1), Parity::Top);
            assert_eq!(detector.push(&frame), Cadence::None, "{at} フレーム目");
        }
        assert!(!detector.locked());
    }

    #[test]
    fn film_under_a_moving_overlay_is_not_woven() {
        let mut detector = Detector::new();
        let mut fired = 0;
        let mut at = 0usize;
        for cycle in 0..6 {
            for mut frame in telecined(cycle) {
                for y in 40..48 {
                    for x in 16..48 {
                        frame.planes[0].data[y * W + x] = ((x + 5 * at) % 32 * 7) as u8;
                    }
                }
                at += 1;
                if detector.push(&frame) != Cadence::None {
                    fired += 1;
                }
            }
        }
        assert_eq!(fired, 0, "{fired} フレームで誤検出した");
    }

    #[test]
    fn a_break_gives_the_lock_up_at_once() {
        let mut detector = Detector::new();
        for cycle in 0..4 {
            for frame in telecined(cycle) {
                detector.push(&frame);
            }
        }
        assert!(detector.locked());
        for at in 0..3 {
            let frame = coded(&film(200 + 2 * at), &film(201 + 2 * at), Parity::Top);
            detector.push(&frame);
        }
        assert!(!detector.locked(), "the lock survived a break");
    }

    #[test]
    fn a_long_still_stretch_gives_the_lock_up() {
        let mut detector = Detector::new();
        for cycle in 0..4 {
            for frame in telecined(cycle) {
                detector.push(&frame);
            }
        }
        assert!(detector.locked());
        let held = coded(&film(0), &film(0), Parity::Top);
        detector.push(&held);
        for _ in 1..MOST_STILL {
            detector.push(&held);
            assert!(detector.locked());
        }
        detector.push(&held);
        assert!(!detector.locked());
    }
}
