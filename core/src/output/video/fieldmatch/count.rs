// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `comb.wgsl` と同じ数え方を CPU で。丸めの向きを GPU と揃えるため 0..1 の浮動小数で比べる。

use super::{Counts, MOTION, STRIPE_SHIFT};

static UNIT: [f32; 256] = {
    let mut unit = [0.0; 256];
    let mut value = 0;
    while value < 256 {
        unit[value] = value as f32 / 255.0;
        value += 1;
    }
    unit
};

/// 行 a・b・c・d が交互に振れているか（差の符号が二度反転し、どれもしきい値より大きい）。
fn striped(a: f32, b: f32, c: f32, d: f32) -> bool {
    let t2 = STRIPE_SHIFT * STRIPE_SHIFT;
    (a - b) * (b - c) < -t2 && (b - c) * (c - d) < -t2
}

/// 1 フレームを数える。`first_row` は表示するフィールドの行（0 か 1）。見るのは反対のフィールドの行だけ。
pub fn count(
    cur: &[u8],
    before: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    first_row: usize,
) -> Counts {
    // 上下の端は端の行を繰り返す（`comb.wgsl` の clamp と同じ）。
    let rows = |y: usize, offset: isize| {
        let y = (y as isize + offset).clamp(0, height as isize - 1) as usize;
        y * stride..y * stride + width
    };
    let unit = |value: u8| UNIT[usize::from(value)];
    let mut counts = Counts::default();
    for y in ((1 - first_row)..height).step_by(2) {
        let above = &cur[rows(y, -1)];
        let below = &cur[rows(y, 1)];
        let own = &cur[rows(y, 0)];
        let own_next = &cur[rows(y, 2)];
        let was_above = &before[rows(y, -1)];
        let was = &before[rows(y, 0)];
        let was_next = &before[rows(y, 2)];
        for x in 0..width {
            counts.first += u32::from(above[x].abs_diff(was_above[x]));
            counts.other += u32::from(own[x].abs_diff(was[x]));
            let (bc, bp) = (unit(own[x]), unit(was[x]));
            if (bc - bp).abs() > MOTION {
                let (a, c) = (unit(above[x]), unit(below[x]));
                counts.moving += 1;
                counts.c += u32::from(striped(a, bc, c, unit(own_next[x])));
                counts.p += u32::from(striped(a, bp, c, unit(was_next[x])));
            }
        }
    }
    counts
}
