// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 正しくウィーブしても残る縞（止まったテロップ、アプコン）の下限。

use std::collections::VecDeque;

const LOOK: usize = 30;
const LEAST: usize = 8;

#[derive(Debug, Default)]
pub(super) struct Floor {
    samples: VecDeque<u32>,
}

impl Floor {
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn push(&mut self, c: u32, p: u32) {
        let (low, high) = (c.min(p), c.max(p));
        // 片方が半分未満のときだけ（どちらが正しいか分かるフレーム）。
        if low.saturating_mul(2) >= high {
            return;
        }
        self.samples.push_back(low);
        if self.samples.len() > LOOK {
            self.samples.pop_front();
        }
    }

    pub fn level(&self) -> u32 {
        let count = self.samples.len();
        if count < LEAST {
            return 0;
        }
        let mut all = [0u32; LOOK];
        for (slot, sample) in all.iter_mut().zip(&self.samples) {
            *slot = *sample;
        }
        *all[..count].select_nth_unstable(count / 2).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_films_own_stripes_are_learnt_and_60i_is_not() {
        let mut floor = Floor::default();
        for _ in 0..LEAST {
            floor.push(5000, 12_000);
        }
        assert_eq!(floor.level(), 5000);
        let mut video = Floor::default();
        for _ in 0..LOOK {
            video.push(30_000, 34_000);
        }
        assert_eq!(video.level(), 0, "60i では覚えない");
    }
}
