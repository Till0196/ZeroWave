// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 フレームが film か 60i かの判定と、その履歴から決める切り替え。

use std::collections::VecDeque;

use super::Counts;

/// 「どうウィーブしてもコーミング」の広さ：全画素のこれ分の一。
const COMBED_PART: u32 = 400;
/// 狭くても動いた画素のこれ分の一以上がコーミングなら 60i（全画素の [`SMALLEST_PART`] 分の一より多いとき）。
const MOVING_SHARE: u32 = 8;
const SMALLEST_PART: u32 = 4000;
/// 少ないほうが多いほうのこれ分の一以上なら「どうウィーブしても」。
const COMPARABLE: u32 = 4;

/// film へ入るウィンドウ（フレーム数）、許すコーミングのフレーム数、要る「動いてコーミングの出ないフレーム」の数。静止画の多いアニメに合わせる。
const SETTLE: usize = 60;
const SETTLE_MOST: usize = 1;
const SETTLE_LEAST: usize = 8;
/// 60i へ戻るウィンドウ（フレーム数）と、その中のコーミングのフレーム数。
const LEAVE: usize = 15;
const LEAVE_LEAST: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Said {
    /// 動いていて、どうウィーブしてもコーミングが出る（60i）。
    Combed,
    /// 動いていて、正しい組み合わせがはっきり勝つ（film）。
    Clean,
    Still,
}

/// 1 フレームの判定。コーミングの数から元からある縞（`floor`）を引いて見る。
pub(super) fn said(counts: &Counts, moved: bool, pixels: u32, floor: u32) -> Said {
    let (low, high) = (counts.c.min(counts.p), counts.c.max(counts.p));
    let comparable = low.saturating_mul(COMPARABLE) >= high;
    let left = low.saturating_sub(floor);
    let wide = left.saturating_mul(COMBED_PART) > pixels;
    let dense = left.saturating_mul(MOVING_SHARE) >= counts.moving
        && left.saturating_mul(SMALLEST_PART) > pixels;
    if comparable && (wide || dense) {
        return Said::Combed;
    }
    let high = high.saturating_sub(floor);
    if moved && left.saturating_mul(COMPARABLE) < high && high.saturating_mul(COMBED_PART) > pixels
    {
        Said::Clean
    } else {
        Said::Still
    }
}

#[derive(Debug, Default)]
pub(super) struct History {
    said: VecDeque<Said>,
    combed: usize,
    clean: usize,
}

impl History {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn push(&mut self, said: Said) {
        self.said.push_back(said);
        self.tally(said, true);
        if self.said.len() > SETTLE
            && let Some(old) = self.said.pop_front()
        {
            self.tally(old, false);
        }
    }

    fn tally(&mut self, said: Said, add: bool) {
        let count = match said {
            Said::Combed => &mut self.combed,
            Said::Clean => &mut self.clean,
            Said::Still => return,
        };
        if add {
            *count += 1;
        } else {
            *count -= 1;
        }
    }

    pub fn leaving(&self) -> bool {
        self.said
            .iter()
            .rev()
            .take(LEAVE)
            .filter(|said| **said == Said::Combed)
            .count()
            >= LEAVE_LEAST
    }

    /// film と見るか。いま film なら、戻るまで film。
    pub fn film(&self, matching: bool) -> bool {
        if matching {
            !self.leaving()
        } else {
            self.said.len() >= SETTLE && self.combed <= SETTLE_MOST && self.clean >= SETTLE_LEAST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(said: &[Said]) -> History {
        let mut history = History::default();
        for said in said {
            history.push(*said);
        }
        history
    }

    fn counts(c: u32, p: u32, moving: u32) -> Counts {
        Counts {
            c,
            p,
            moving,
            ..Counts::default()
        }
    }

    #[test]
    fn film_takes_a_second_of_clean_frames_and_60i_takes_a_few_combed_ones() {
        assert!(!history(&[Said::Clean; 59]).film(false), "一秒に足りない");
        assert!(history(&[Said::Clean; 60]).film(false));
        let mut mostly = vec![Said::Clean; 60];
        mostly[10] = Said::Combed;
        assert!(history(&mostly).film(false), "カットの 1 フレームは許す");
        mostly[40] = Said::Combed;
        assert!(!history(&mostly).film(false));
        let mut leaving = vec![Said::Clean; 60];
        leaving[57..].fill(Said::Combed);
        assert!(!history(&leaving).film(true), "3 フレームで戻る");
        leaving[57] = Said::Clean;
        assert!(history(&leaving).film(true));
    }

    #[test]
    fn a_still_stretch_is_no_evidence_either_way() {
        let mut still = vec![Said::Still; 60];
        assert!(!history(&still).film(false));
        still[..SETTLE_LEAST].fill(Said::Clean);
        assert!(history(&still).film(false));
        let mut stopping = vec![Said::Still; 60];
        stopping[59] = Said::Combed;
        assert!(
            history(&stopping).film(true),
            "止まった中の 1 フレームでは戻らない"
        );
    }

    #[test]
    fn the_counts_follow_the_window() {
        let mut history = history(&[Said::Combed; 3]);
        for _ in 0..SETTLE {
            history.push(Said::Clean);
        }
        assert_eq!((history.combed, history.clean), (0, SETTLE));
    }

    #[test]
    fn what_one_frame_says() {
        let (sd, hd) = (1440 * 1080, 1920 * 1080);
        assert_eq!(said(&counts(1745, 2498, sd), true, sd, 0), Said::Still);
        assert_eq!(said(&counts(4146, 5109, 22_559), true, hd, 0), Said::Combed);
        assert_eq!(said(&counts(12_759, 725_425, hd), true, hd, 0), Said::Clean);
        assert_eq!(
            said(&counts(200_000, 240_000, hd), true, hd, 0),
            Said::Combed
        );
        assert_eq!(said(&counts(5000, 12_000, sd), true, sd, 0), Said::Combed);
        assert_eq!(said(&counts(5000, 12_000, sd), true, sd, 5000), Said::Clean);
        assert_eq!(
            said(&counts(25_000, 12_000, sd), true, sd, 5000),
            Said::Combed
        );
    }
}
