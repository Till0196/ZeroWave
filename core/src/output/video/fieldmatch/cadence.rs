// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 24p の周期（3:2 か ABCDD のコマの繰り返し）にロックする。抜くのはどちらも周期の 2 フレーム目。
//! 位相は矛盾の無さで選び、フィールドの差はノイズフロア（[`NOISE_PERCENTILE`]）を引いて見る。

use std::collections::VecDeque;

use super::Counts;

/// 見る長さ（フレーム数、2 秒）と、探し始めるのに要るフレーム数（0.5 秒）。
const LOOK: usize = 60;
const FIND_LEAST: usize = 15;
const LOCK_LEAST: usize = 3;
const LOCK_STRAY: usize = 1;
/// ノイズフロア：直近の「少ないほうのフィールドの差」の、下からこの割合（%）の値。
const NOISE_PERCENTILE: usize = 20;
/// 「動いた」はノイズフロアから上の差がフィールドの画素のこれ分の一以上。「止まった」はその半分未満。
const MOVED: u32 = 2;
/// 繰り返しのはずのフィールドが反対のフィールドのこれ分の一未満なら繰り返し（カット直後はエンコードの差が膨らむ）。
const REPEAT: u32 = 3;
const AGAINST: u32 = 2;
/// コマの繰り返しで足したフレームは、二つのフィールドとも前のフレームとの差のこれ分の一未満。
const WHOLE_REPEAT: u32 = 4;
/// 片側のコーミング：多いほうが全画素のこれ分の一以上で、少ないほうがその [`SPLIT_SIDED`] 分の一以下。
const SPLIT_PART: u32 = 8000;
const SPLIT_SIDED: u32 = 8;
/// フィールドの差がフィールドの画素のこれ倍を越えるフレームはカット。周期に従わないのでコーミングの位置を矛盾にしない。
const CUT: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Field,
    Frame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase {
    pub repeat: Repeat,
    pub start: u8,
}

impl Phase {
    pub fn fields(start: u8) -> Self {
        Self {
            repeat: Repeat::Field,
            start,
        }
    }

    pub fn frames(start: u8) -> Self {
        Self {
            repeat: Repeat::Frame,
            start,
        }
    }

    fn step(self, index: u64) -> u8 {
        ((index + 5 - u64::from(self.start)) % 5) as u8
    }

    pub fn drops(self, index: u64) -> bool {
        self.step(index) == 2
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.repeat {
            Repeat::Field => "フィールド",
            Repeat::Frame => "コマ",
        };
        write!(out, "{}{what}", self.start)
    }
}

/// 片側にだけ出たコーミング。3:2 では c が違うコマなのは周期の 2・3 フレーム目、p は 0・1 フレーム目だけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Split {
    Own,
    Previous,
}

impl Split {
    fn of(c: u32, p: u32, pixels: u32) -> Option<Self> {
        let (low, high) = (c.min(p), c.max(p));
        if high.saturating_mul(SPLIT_PART) < pixels || low.saturating_mul(SPLIT_SIDED) > high {
            return None;
        }
        Some(if c > p { Split::Own } else { Split::Previous })
    }

    fn fits(self, step: u8) -> bool {
        match self {
            Split::Own => matches!(step, 2 | 3),
            Split::Previous => matches!(step, 0 | 1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Seen {
    index: u64,
    first: u32,
    other: u32,
    moved: bool,
    split: Option<Split>,
}

struct Scale {
    noise: u32,
    moved: u32,
}

impl Scale {
    fn moving(&self, diff: u32) -> bool {
        diff.saturating_sub(self.noise) >= self.moved
    }

    fn still(&self, diff: u32) -> bool {
        diff.saturating_sub(self.noise) < self.moved / 2
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    against: usize,
    support: usize,
}

impl Tally {
    fn fits(self) -> bool {
        self.against <= LOCK_STRAY && self.support >= LOCK_LEAST
    }

    /// ロックしている位相を保つか。裏付けが多ければ三分の一までの矛盾は許す（60i の字幕が流れる 24p）。
    fn holds(self) -> bool {
        self.against <= LOCK_STRAY.max(self.support / 3)
    }
}

enum Found {
    Phase(Phase),
    /// 根拠が足りない（止まった場面、見始め）。ロックした周期は保つ。
    Unclear,
    /// ロックしていた位相に矛盾が出て、他に合う位相も無い。
    Against,
}

#[derive(Debug)]
pub(super) struct Cadence {
    seen: VecDeque<Seen>,
    phase: Option<Phase>,
    field_pixels: u32,
}

impl Cadence {
    pub fn new(field_pixels: u32) -> Self {
        Self {
            seen: VecDeque::with_capacity(LOOK + 1),
            phase: None,
            field_pixels,
        }
    }

    pub fn clear(&mut self) {
        self.seen.clear();
        self.phase = None;
    }

    pub fn forget(&mut self) {
        self.phase = None;
    }

    pub fn phase(&self) -> Option<Phase> {
        self.phase
    }

    pub fn push(&mut self, index: u64, counts: &Counts, moved: bool, pixels: u32) {
        self.remember(Seen {
            index,
            first: counts.first,
            other: counts.other,
            moved,
            split: Split::of(counts.c, counts.p, pixels),
        });
    }

    fn remember(&mut self, seen: Seen) {
        self.seen.push_back(seen);
        if self.seen.len() > LOOK {
            self.seen.pop_front();
        }
        match self.find() {
            Found::Phase(phase) => self.phase = Some(phase),
            Found::Unclear => {}
            Found::Against => self.phase = None,
        }
    }

    fn find(&self) -> Found {
        if self.seen.len() < FIND_LEAST {
            return Found::Unclear;
        }
        let tallies = self.tallies(&self.scale());
        let tally = |phase: Phase| tallies[phase.repeat as usize][usize::from(phase.start)];
        if let Some(phase) = self.phase
            && tally(phase).holds()
        {
            return Found::Phase(phase);
        }
        let mut fits = (0..5)
            .flat_map(|start| [Phase::fields(start), Phase::frames(start)])
            .filter(|phase| tally(*phase).fits());
        match (fits.next(), fits.next(), self.phase) {
            (Some(phase), None, _) => Found::Phase(phase),
            (_, _, Some(_)) => Found::Against,
            (_, _, None) => Found::Unclear,
        }
    }

    fn scale(&self) -> Scale {
        let mut least = [0u32; LOOK];
        for (slot, seen) in least.iter_mut().zip(&self.seen) {
            *slot = seen.first.min(seen.other);
        }
        let least = &mut least[..self.seen.len()];
        let at = least.len() * NOISE_PERCENTILE / 100;
        Scale {
            noise: *least.select_nth_unstable(at).1,
            moved: self.field_pixels / MOVED,
        }
    }

    fn tallies(&self, scale: &Scale) -> [[Tally; 5]; 2] {
        let mut tallies = [[Tally::default(); 5]; 2];
        let mut before: Option<&Seen> = None;
        for seen in &self.seen {
            let adjacent = before.filter(|before| before.index + 1 == seen.index);
            for start in 0..5 {
                let step = Phase::fields(start).step(seen.index);
                let fields = self.fields(seen, step, scale);
                let frames = self.frames(seen, adjacent, step, scale);
                for (repeat, more) in [(Repeat::Field, fields), (Repeat::Frame, frames)] {
                    let tally = &mut tallies[repeat as usize][usize::from(start)];
                    tally.against += more.against;
                    tally.support += more.support;
                }
            }
            before = Some(seen);
        }
        tallies
    }

    fn cut(&self, seen: &Seen) -> bool {
        seen.first.max(seen.other) >= self.field_pixels.saturating_mul(CUT)
    }

    fn fields(&self, seen: &Seen, step: u8, scale: &Scale) -> Tally {
        let mut tally = Tally::default();
        if let Some(split) = seen.split {
            if split.fits(step) {
                tally.support += 1;
            } else if !self.cut(seen) {
                tally.against += 1;
            }
        }
        if !seen.moved {
            return tally;
        }
        let (repeated, fresh) = match step {
            2 => (seen.first, seen.other),
            4 => (seen.other, seen.first),
            _ => return tally,
        };
        if scale.moving(repeated) && repeated.saturating_mul(AGAINST) >= fresh {
            tally.against += 1;
        } else if scale.moving(fresh)
            && (scale.still(repeated) || repeated.saturating_mul(REPEAT) < fresh)
        {
            tally.support += 1;
        }
        tally
    }

    /// コマの繰り返しの周期の `step` フレーム目として、このフレームから分かる判定。`before` は一つ前のフレーム。
    fn frames(&self, seen: &Seen, before: Option<&Seen>, step: u8, scale: &Scale) -> Tally {
        let against = Tally {
            against: 1,
            support: 0,
        };
        let support = Tally {
            against: 0,
            support: 1,
        };
        // どのフレームもプログレッシブで c にコーミングは出ず、足したフレームは前のフレームそのもので p にも出ない。
        match seen.split {
            Some(Split::Own) if !self.cut(seen) => return against,
            Some(Split::Previous) if step == 2 && !self.cut(seen) => return against,
            _ if step != 2 => return Tally::default(),
            _ => {}
        }
        let changed_as_much = |before: &Seen| {
            seen.first.saturating_mul(AGAINST) >= before.first
                && seen.other.saturating_mul(AGAINST) >= before.other
        };
        let dwarfed = |before: &Seen| {
            seen.first.saturating_mul(WHOLE_REPEAT) < before.first
                && seen.other.saturating_mul(WHOLE_REPEAT) < before.other
        };
        if seen.moved
            && (scale.moving(seen.first)
                || scale.moving(seen.other)
                || before.is_some_and(|before| before.moved && changed_as_much(before)))
        {
            against
        } else if scale.still(seen.first)
            && scale.still(seen.other)
            && before.is_some_and(|before| {
                scale.moving(before.first) && scale.moving(before.other) && dwarfed(before)
            })
        {
            support
        } else {
            Tally::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Cadence {
        fn diffs(&mut self, index: u64, first: u32, other: u32) {
            self.remember(Seen {
                index,
                first,
                other,
                moved: true,
                split: None,
            });
        }
    }

    const FIELD: u32 = 1440 * 540;

    fn pulldown(start: u64, frames: u64, motion: u32, repeat: u32) -> Vec<(u64, u32, u32)> {
        (0..frames)
            .map(|index| match (index + 5 - start) % 5 {
                2 => (index, repeat, motion),
                4 => (index, motion, repeat),
                _ => (index, motion, motion),
            })
            .collect()
    }

    fn cartoon(start: u64, frames: u64, per: u64, noise: u32, motion: u32) -> Vec<(u64, u32, u32)> {
        let film = |index: u64| -> (u64, u64) {
            let from = index + 5 * 10 - start;
            let (cycle, at) = (from / 5, from % 5);
            let (top, bottom) = [(0, 0), (1, 1), (1, 2), (2, 3), (3, 3)][at as usize];
            (cycle * 4 + top, cycle * 4 + bottom)
        };
        let diff =
            |now: u64, before: u64| noise + if now / per != before / per { motion } else { 0 };
        (1..=frames)
            .map(|index| {
                let ((top, bottom), (top_was, bottom_was)) = (film(index), film(index - 1));
                (index, diff(top, top_was), diff(bottom, bottom_was))
            })
            .collect()
    }

    fn fed(diffs: impl IntoIterator<Item = (u64, u32, u32)>, field: u32) -> Cadence {
        let mut cadence = Cadence::new(field);
        for (index, first, other) in diffs {
            cadence.diffs(index, first, other);
        }
        cadence
    }

    #[test]
    fn a_pulldown_is_found_and_its_second_frame_is_dropped() {
        let cadence = fed(pulldown(3, 30, FIELD * 10, FIELD / 2), FIELD);
        let phase = cadence.phase().expect("ロックする");
        assert_eq!(phase, Phase::fields(3));
        let dropped: Vec<u64> = (30..40).filter(|index| phase.drops(*index)).collect();
        assert_eq!(dropped, vec![30, 35]);
    }

    #[test]
    fn a_cartoon_drawn_on_twos_is_found() {
        let diffs = pulldown(2, 30, FIELD * 6, 0)
            .into_iter()
            .map(|(index, first, other)| {
                let step = (index + 5 - 2) % 5;
                if index % 3 != 0 && step != 2 && step != 4 {
                    (index, FIELD / 8, FIELD / 8)
                } else {
                    (index, first, other)
                }
            });
        assert_eq!(fed(diffs, FIELD).phase(), Some(Phase::fields(2)));
    }

    #[test]
    fn a_cartoon_on_threes_with_noise_is_locked_by_what_never_moves() {
        for start in 0..5 {
            let cadence = fed(cartoon(start, 60, 3, FIELD / 2, FIELD), FIELD);
            assert_eq!(
                cadence.phase(),
                Some(Phase::fields(start as u8)),
                "位相 {start}"
            );
        }
    }

    #[test]
    fn a_still_stretch_keeps_the_phase_and_a_shift_takes_the_new_one() {
        let mut cadence = fed(pulldown(1, 30, FIELD * 10, 0), FIELD);
        assert_eq!(cadence.phase(), Some(Phase::fields(1)));
        for index in 30..90 {
            cadence.diffs(index, FIELD / 8, FIELD / 8);
        }
        assert_eq!(
            cadence.phase(),
            Some(Phase::fields(1)),
            "止まった 2 秒でロックを外さない"
        );
        for (index, first, other) in pulldown(4, 30, FIELD * 10, 0) {
            cadence.diffs(index + 90, first, other);
        }
        assert_eq!(
            cadence.phase(),
            Some(Phase::fields((4 + 90) % 5)),
            "ずれた周期にロックし直す"
        );
    }

    #[test]
    fn a_cut_does_not_shake_the_phase_loose() {
        let mut diffs = cartoon(2, 60, 3, FIELD / 2, FIELD);
        for at in [44usize, 47] {
            let (index, first, other) = diffs[at - 1];
            diffs[at - 1] = match (index + 5 - 2) % 5 {
                2 => (index, FIELD * 3 / 2, FIELD * 50),
                4 => (index, FIELD * 50, FIELD * 3 / 2),
                _ => (index, first, other),
            };
        }
        assert_eq!(fed(diffs, FIELD).phase(), Some(Phase::fields(2)));
    }

    #[test]
    fn a_swollen_repeat_while_the_picture_pans_is_not_against_the_phase() {
        let field = 1920 * 540;
        let mut diffs = cartoon(0, 60, 3, field / 2, field * 5);
        for (index, first, other) in diffs.iter_mut() {
            match *index % 5 {
                2 if *other > field * 2 => *first = field * 13 / 10,
                4 if *first > field * 2 => *other = field * 13 / 10,
                _ => {}
            }
        }
        assert_eq!(fed(diffs, field).phase(), Some(Phase::fields(0)));
    }

    #[test]
    fn video_and_stills_lock_nothing() {
        let video = fed((0..30).map(|index| (index, FIELD * 10, FIELD * 10)), FIELD);
        assert_eq!(video.phase(), None, "60i には繰り返しが無い");
        let still = (0..30).map(|index| {
            let wobble = if index % 5 == 2 { FIELD / 8 } else { FIELD / 4 };
            (index, wobble, FIELD / 4)
        });
        assert_eq!(
            fed(still, FIELD).phase(),
            None,
            "止まったフレームの揺らぎは根拠にならない"
        );
    }

    #[test]
    fn one_stray_repeat_is_tolerated_and_two_are_not() {
        let mut diffs = pulldown(1, 30, FIELD * 10, FIELD / 2);
        diffs[7] = (7, FIELD * 40, FIELD * 40);
        diffs[10] = (10, FIELD / 2, FIELD * 10);
        assert_eq!(fed(diffs.clone(), FIELD).phase(), Some(Phase::fields(1)));
        diffs[15] = (15, FIELD / 2, FIELD * 10);
        assert_eq!(fed(diffs, FIELD).phase(), None);
    }
}
