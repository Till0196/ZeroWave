// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィールドマッチの CPU 側：数えたコーミングとフィールドの差から film か 60i か、24p の周期、
//! 元からある縞を決める（[`Judge`]）。数えるのは GPU（`gpu::matcher`）か [`count`]。

mod cadence;
mod count;
mod floor;
mod verdict;

pub use cadence::{Phase, Repeat};
pub use count::count;

use cadence::Cadence;
use floor::Floor;
use verdict::{History, Said};

/// 縞のしきい値（隣の行との差、0..1）。
pub const STRIPE_SHIFT: f32 = 8.0 / 255.0;
/// 動きのしきい値（前のフレームの同じ画素との差）。
pub const MOTION: f32 = 7.0 / 255.0;

/// 1 フレームについて数えた値（`comb.wgsl` の `combed` の [0]..[4]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// 自分の二つのフィールドでウィーブした（c）、表示するフィールドと前のフレームの反対のフィールドでウィーブした（p）ときの、動いている縞の画素。
    pub c: u32,
    pub p: u32,
    pub moving: u32,
    /// 表示するフィールドと反対のフィールドの、前のフレームの同じフィールドとの差の和（8 ビットの単位）。
    pub first: u32,
    pub other: u32,
}

/// 「動いたフレーム」のしきい値：全画素のこれ分の一。止まった 60i はコーミングが出ないので film の根拠にしない。
const MOVING_PART: u32 = 400;

/// 「動くオーバーレイ」のしきい値：直近 [`CYCLE`] フレームの動いた画素の最小が全画素のこれ分の一より多い。
/// 繰り返しのフレームでも動く物は 60i のオーバーレイで、ウィーブすると崩れる。
const OVERLAY_PART: u32 = 400;
/// 「ウィーブしても残る縞」のしきい値：[`Floor`] の水準が全画素のこれ分の一より多い。
const OVERLAY_STRIPES: u32 = 1000;
const CYCLE: usize = 5;
/// オーバーレイと判定するまで・判定しなくなるまでの数。消えるほうを遅くしてテロップの切れ目で行き来しない。
const OVERLAY_ON: u8 = 3;
const OVERLAY_MOST: u8 = 15;

#[derive(Debug)]
pub struct Judge {
    history: History,
    film: bool,
    matching: bool,
    recent: std::collections::VecDeque<u32>,
    overlay_score: u8,
    overlay: bool,
    avoid_overlay: bool,
    cadence: Cadence,
    floor: Floor,
    combed_frames: u64,
    pixels: u32,
}

impl Judge {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            history: History::default(),
            film: false,
            matching: false,
            recent: std::collections::VecDeque::with_capacity(CYCLE),
            overlay_score: 0,
            overlay: false,
            avoid_overlay: true,
            cadence: Cadence::new(width * height / 2),
            floor: Floor::default(),
            combed_frames: 0,
            pixels: width * height,
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.film = false;
        self.matching = false;
        self.recent.clear();
        self.overlay_score = 0;
        self.overlay = false;
        self.cadence.clear();
        self.floor.clear();
    }

    /// 1 フレームの数を加えて決め直す。`index` はフレームの番号。返すのは film ↔ 60i が替わったか。
    pub fn push(&mut self, index: u64, counts: Counts) -> bool {
        let was = self.matching;
        let moved = counts.moving.saturating_mul(MOVING_PART) > self.pixels;
        self.cadence.push(index, &counts, moved, self.pixels);
        if moved {
            self.floor.push(counts.c, counts.p);
        }
        let said = verdict::said(&counts, moved, self.pixels, self.floor.level());
        if said == Said::Combed {
            self.combed_frames += 1;
        }
        self.history.push(said);
        let was_film = self.film;
        self.film = self.history.film(self.film)
            // 周期にロックできたならそれが film の根拠（静止画の多いアニメのため）。
            || (self.cadence.phase().is_some() && !self.history.leaving());
        // 60i へ移ったら位相は捨てる（オーバーレイで 60i にしている間は film のままなので捨てない）。
        if was_film && !self.film {
            self.cadence.forget();
        }
        self.watch_overlay(counts.moving);
        self.matching = self.film && !(self.avoid_overlay && self.overlay);
        was != self.matching
    }

    fn watch_overlay(&mut self, moving: u32) {
        if self.recent.len() == CYCLE {
            self.recent.pop_front();
        }
        self.recent.push_back(moving);
        if self.recent.len() < CYCLE {
            return;
        }
        let least = self.recent.iter().copied().min().unwrap_or(0);
        let striped = self.floor.level().saturating_mul(OVERLAY_STRIPES) > self.pixels;
        if striped && least.saturating_mul(OVERLAY_PART) > self.pixels {
            self.overlay_score = (self.overlay_score + 1).min(OVERLAY_MOST);
        } else {
            self.overlay_score = self.overlay_score.saturating_sub(1);
        }
        if self.overlay_score >= OVERLAY_ON {
            self.overlay = true;
        } else if self.overlay_score == 0 {
            self.overlay = false;
        }
    }

    pub fn overlay(&self) -> bool {
        self.film && self.overlay
    }

    /// 動くオーバーレイがあるとき、ウィーブせずに 60i でレンダリングするか（既定はする）。
    pub fn set_avoid_overlay(&mut self, avoid: bool) {
        self.avoid_overlay = avoid;
    }

    pub fn matching(&self) -> bool {
        self.matching
    }

    pub fn phase(&self) -> Option<Phase> {
        self.cadence.phase()
    }

    pub fn locked(&self) -> bool {
        self.phase().is_some()
    }

    pub fn whole_frames(&self) -> bool {
        self.phase()
            .is_some_and(|phase| phase.repeat == Repeat::Frame)
    }

    pub fn drop(&self, index: u64) -> bool {
        self.phase().is_some_and(|phase| phase.drops(index))
    }

    pub fn stripes(&self) -> u32 {
        self.floor.level()
    }

    pub fn combed(&mut self) -> u64 {
        std::mem::take(&mut self.combed_frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_60i_scene_is_never_film() {
        let mut judge = Judge::new(1440, 1080);
        for index in 0..300u64 {
            let wobble = (index % 7) as u32 * 150;
            judge.push(
                index,
                Counts {
                    c: 1800 + wobble,
                    p: 2500 + wobble,
                    moving: 15_000,
                    first: 900_000,
                    other: 950_000,
                },
            );
            assert!(!judge.matching(), "{index} フレーム目で film と見た");
        }
    }

    #[test]
    fn a_tennis_match_is_60i_throughout() {
        let mut judge = Judge::new(1920, 1080);
        let mut seed = 7u32;
        for index in 0..600u64 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let wobble = (seed >> 16) % 1000;
            let diff = 900_000 + wobble * 1500;
            judge.push(
                index,
                Counts {
                    c: 3800 + wobble * 4,
                    p: 4100 + wobble * 4,
                    moving: 16_000 + wobble * 14,
                    first: diff,
                    other: diff + wobble * 30,
                },
            );
            assert!(!judge.matching(), "{index} フレーム目で film と見た");
            assert_eq!(judge.phase(), None, "{index} フレーム目で周期にロックした");
        }
    }

    #[test]
    fn a_slow_cartoon_is_locked_and_film_by_where_its_comb_falls() {
        let field = 1440 * 540;
        let quiet = |c, p| Counts {
            c,
            p,
            moving: c.max(p) + 100,
            first: field / 5,
            other: field / 5,
        };
        let mut judge = Judge::new(1440, 1080);
        for index in 0..90u64 {
            let counts = match ((index + 5 - 3) % 5, index % 4) {
                (0 | 1, 0) => quiet(0, 300),
                (2 | 3, 2) => quiet(300, 0),
                _ => quiet(0, 0),
            };
            judge.push(index, counts);
        }
        assert_eq!(judge.phase(), Some(Phase::fields(3)));
        assert!(judge.matching(), "周期にロックすれば film");
    }

    #[test]
    fn a_30p_picture_locks_nothing() {
        let field = 1440 * 540;
        let mut judge = Judge::new(1440, 1080);
        for index in 0..90u64 {
            judge.push(
                index,
                Counts {
                    c: 50,
                    p: 62_000,
                    moving: 90_000,
                    first: field * 4,
                    other: field * 4,
                },
            );
        }
        assert_eq!(judge.phase(), None);
    }

    #[test]
    fn a_repeated_whole_frame_is_a_cadence_too() {
        let field = 1440 * 540;
        let mut judge = Judge::new(1440, 1080);
        for index in 0..90u64 {
            let counts = if index % 5 == 3 {
                Counts {
                    first: field / 16,
                    other: field / 16,
                    ..Counts::default()
                }
            } else {
                Counts {
                    c: 10,
                    p: 1500,
                    moving: 10_000,
                    first: field,
                    other: field,
                }
            };
            judge.push(index, counts);
        }
        assert!(judge.whole_frames());
        assert!(judge.drop(93) && !judge.drop(94), "足したフレームを抜く");
    }

    /// 3:2 の film を一周分。`residual` はウィーブしても残る縞、`still` は繰り返しのフレームの動き。
    fn pulldown(cycle: u64, residual: u32, still: u32) -> [Counts; 5] {
        let wobble = (cycle % 3) as u32 * 3000;
        let moving = |c: u32, p: u32, m: u32, first: u32, other: u32| Counts {
            c: c + residual,
            p: p + residual,
            moving: m + wobble,
            first,
            other,
        };
        [
            moving(0, 80_000, 120_000, 5_900_000, 5_900_000),
            moving(80_000, 0, 118_000, 2_500_000, 5_800_000),
            moving(0, 0, still, 5_600_000, 2_400_000),
            moving(0, 70_000, 110_000, 5_500_000, 5_500_000),
            moving(60_000, 0, 100_000, 2_400_000, 5_200_000),
        ]
    }

    #[test]
    fn film_under_a_scrolling_caption_is_shown_at_60i() {
        let mut judge = Judge::new(1920, 1080);
        let mut index = 0;
        for cycle in 0..40 {
            for counts in pulldown(cycle, 9_000, 15_000) {
                judge.push(index, counts);
                index += 1;
            }
        }
        assert!(judge.overlay(), "流れるテロップを見つける");
        assert!(!judge.matching(), "ウィーブせずに 60i でレンダリングする");
        for cycle in 40..120 {
            for counts in pulldown(cycle, 50, 300) {
                judge.push(index, counts);
                index += 1;
            }
        }
        assert!(!judge.overlay());
        assert!(judge.matching(), "film に戻る");
    }

    #[test]
    fn plain_film_is_still_matched() {
        let mut judge = Judge::new(1920, 1080);
        let mut index = 0;
        for cycle in 0..40 {
            for counts in pulldown(cycle, 50, 300) {
                judge.push(index, counts);
                index += 1;
            }
        }
        assert!(!judge.overlay());
        assert!(judge.matching());
    }

    #[test]
    fn the_overlay_check_can_be_turned_off() {
        let mut judge = Judge::new(1920, 1080);
        judge.set_avoid_overlay(false);
        let mut index = 0;
        for cycle in 0..40 {
            for counts in pulldown(cycle, 9_000, 15_000) {
                judge.push(index, counts);
                index += 1;
            }
        }
        assert!(judge.overlay());
        assert!(judge.matching(), "切ってあればウィーブする");
    }
}
