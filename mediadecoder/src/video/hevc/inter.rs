// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Prediction from other pictures, clause 8.5.3.3.
//! Everything is at fourteen bits so that two predictions are averaged before rounding.

use super::params::Weight;
use super::tables::{CHROMA_FILTER, LUMA_FILTER};

#[derive(Debug, Clone, Copy)]
pub struct Reference<'a> {
    pub samples: &'a [u16],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Reference<'_> {
    /// Copies the samples a filter reads, repeating the edge, so the filter needs no clamping.
    fn gather(&self, x: i32, y: i32, width: usize, height: usize, into: &mut [i32], stride: usize) {
        let inside = x >= 0
            && y >= 0
            && x as usize + width <= self.width
            && y as usize + height <= self.height;
        for row in 0..height {
            let into = &mut into[row * stride..row * stride + width];
            if inside {
                let at = (y as usize + row) * self.stride + x as usize;
                for (slot, &value) in into.iter_mut().zip(&self.samples[at..at + width]) {
                    *slot = i32::from(value);
                }
                continue;
            }
            let sy = (y + row as i32).clamp(0, self.height as i32 - 1) as usize;
            let at = sy * self.stride;
            for (column, slot) in into.iter_mut().enumerate() {
                let sx = (x + column as i32).clamp(0, self.width as i32 - 1) as usize;
                *slot = i32::from(self.samples[at + sx]);
            }
        }
    }
}

pub const MOST: usize = 64;
const GATHER: usize = MOST + 7;
pub const SCRATCH: usize = GATHER * GATHER;

/// Clause 8.5.3.3.3.2: the luma prediction of one block, at fourteen bits.
#[allow(clippy::too_many_arguments)]
pub fn luma(
    reference: &Reference,
    x: i32,
    y: i32,
    mv: [i16; 2],
    width: usize,
    height: usize,
    depth: u8,
    into: &mut [i32],
    stride: usize,
    scratch: &mut [i32],
    gathered: &mut [i32],
) {
    let shift1 = (i32::from(depth) - 8).min(4);
    let shift3 = (14 - i32::from(depth)).max(2);
    let ix = x + (i32::from(mv[0]) >> 2);
    let iy = y + (i32::from(mv[1]) >> 2);
    let fx = (i32::from(mv[0]) & 3) as usize;
    let fy = (i32::from(mv[1]) & 3) as usize;

    if fx == 0 && fy == 0 {
        reference.gather(ix, iy, width, height, gathered, GATHER);
        for row in 0..height {
            let from = &gathered[row * GATHER..row * GATHER + width];
            let out = &mut into[row * stride..row * stride + width];
            for (slot, &value) in out.iter_mut().zip(from) {
                *slot = value << shift3;
            }
        }
        return;
    }
    if fy == 0 {
        reference.gather(ix - 3, iy, width + 7, height, gathered, GATHER);
        let taps = &LUMA_FILTER[fx];
        for row in 0..height {
            let source = &gathered[row * GATHER..];
            for column in 0..width {
                let mut sum = 0i32;
                for (tap, &weight) in taps.iter().enumerate() {
                    sum += weight * source[column + tap];
                }
                into[column + row * stride] = sum >> shift1;
            }
        }
        return;
    }
    if fx == 0 {
        reference.gather(ix, iy - 3, width, height + 7, gathered, GATHER);
        let taps = &LUMA_FILTER[fy];
        for row in 0..height {
            for column in 0..width {
                let mut sum = 0i32;
                for (tap, &weight) in taps.iter().enumerate() {
                    sum += weight * gathered[column + (row + tap) * GATHER];
                }
                into[column + row * stride] = sum >> shift1;
            }
        }
        return;
    }
    // Horizontal first (with 3 rows above, 4 below), then vertical: the standard fixes the order.
    reference.gather(ix - 3, iy - 3, width + 7, height + 7, gathered, GATHER);
    let taps = &LUMA_FILTER[fx];
    for row in 0..height + 7 {
        let source = &gathered[row * GATHER..];
        for column in 0..width {
            let mut sum = 0i32;
            for (tap, &weight) in taps.iter().enumerate() {
                sum += weight * source[column + tap];
            }
            scratch[column + row * GATHER] = sum >> shift1;
        }
    }
    let taps = &LUMA_FILTER[fy];
    for row in 0..height {
        for column in 0..width {
            let mut sum = 0i32;
            for (tap, &weight) in taps.iter().enumerate() {
                sum += weight * scratch[column + (row + tap) * GATHER];
            }
            into[column + row * stride] = sum >> 6;
        }
    }
}

/// Clause 8.5.3.3.3.3: the chroma prediction of one block, at fourteen bits.
#[allow(clippy::too_many_arguments)]
pub fn chroma(
    reference: &Reference,
    x: i32,
    y: i32,
    mv: [i16; 2],
    width: usize,
    height: usize,
    depth: u8,
    into: &mut [i32],
    stride: usize,
    scratch: &mut [i32],
    gathered: &mut [i32],
) {
    let shift1 = (i32::from(depth) - 8).min(4);
    let shift3 = (14 - i32::from(depth)).max(2);
    let ix = x + (i32::from(mv[0]) >> 3);
    let iy = y + (i32::from(mv[1]) >> 3);
    let fx = (i32::from(mv[0]) & 7) as usize;
    let fy = (i32::from(mv[1]) & 7) as usize;

    if fx == 0 && fy == 0 {
        reference.gather(ix, iy, width, height, gathered, GATHER);
        for row in 0..height {
            let from = &gathered[row * GATHER..row * GATHER + width];
            let out = &mut into[row * stride..row * stride + width];
            for (slot, &value) in out.iter_mut().zip(from) {
                *slot = value << shift3;
            }
        }
        return;
    }
    if fy == 0 {
        reference.gather(ix - 1, iy, width + 3, height, gathered, GATHER);
        let taps = &CHROMA_FILTER[fx];
        for row in 0..height {
            let source = &gathered[row * GATHER..];
            for column in 0..width {
                let mut sum = 0i32;
                for (tap, &weight) in taps.iter().enumerate() {
                    sum += weight * source[column + tap];
                }
                into[column + row * stride] = sum >> shift1;
            }
        }
        return;
    }
    if fx == 0 {
        reference.gather(ix, iy - 1, width, height + 3, gathered, GATHER);
        let taps = &CHROMA_FILTER[fy];
        for row in 0..height {
            for column in 0..width {
                let mut sum = 0i32;
                for (tap, &weight) in taps.iter().enumerate() {
                    sum += weight * gathered[column + (row + tap) * GATHER];
                }
                into[column + row * stride] = sum >> shift1;
            }
        }
        return;
    }
    reference.gather(ix - 1, iy - 1, width + 3, height + 3, gathered, GATHER);
    let taps = &CHROMA_FILTER[fx];
    for row in 0..height + 3 {
        let source = &gathered[row * GATHER..];
        for column in 0..width {
            let mut sum = 0i32;
            for (tap, &weight) in taps.iter().enumerate() {
                sum += weight * source[column + tap];
            }
            scratch[column + row * GATHER] = sum >> shift1;
        }
    }
    let taps = &CHROMA_FILTER[fy];
    for row in 0..height {
        for column in 0..width {
            let mut sum = 0i32;
            for (tap, &weight) in taps.iter().enumerate() {
                sum += weight * scratch[column + (row + tap) * GATHER];
            }
            into[column + row * stride] = sum >> 6;
        }
    }
}

/// Clause 8.5.3.3.4.2: one prediction, or the mean of two, rounded to the picture's depth.
#[allow(clippy::too_many_arguments)]
pub fn combine(
    first: &[i32],
    second: &[i32],
    used: [bool; 2],
    width: usize,
    height: usize,
    depth: u8,
    into: &mut [u16],
    in_stride: usize,
    out_stride: usize,
) {
    let most = (1i32 << depth) - 1;
    let shift = if used[0] && used[1] {
        (15 - i32::from(depth)).max(3)
    } else {
        (14 - i32::from(depth)).max(2)
    };
    let round = 1i32 << (shift - 1);
    for row in 0..height {
        for column in 0..width {
            let at = column + row * in_stride;
            let sum = match used {
                [true, true] => first[at] + second[at],
                [true, false] => first[at],
                _ => second[at],
            };
            into[column + row * out_stride] = ((sum + round) >> shift).clamp(0, most) as u16;
        }
    }
}

/// Clause 8.5.3.3.4.3: the same with an explicit weight and offset per reference.
#[allow(clippy::too_many_arguments)]
pub fn weighted(
    first: &[i32],
    second: &[i32],
    used: [bool; 2],
    weights: [Weight; 2],
    log2_denom: i32,
    width: usize,
    height: usize,
    depth: u8,
    into: &mut [u16],
    in_stride: usize,
    out_stride: usize,
) {
    let most = (1i32 << depth) - 1;
    let shift1 = (14 - i32::from(depth)).max(2);
    let log2_wd = log2_denom + shift1;
    let round = if log2_wd >= 1 { 1 << (log2_wd - 1) } else { 0 };
    for row in 0..height {
        for column in 0..width {
            let at = column + row * in_stride;
            let value = match used {
                [true, true] => {
                    (first[at] * weights[0].weight
                        + second[at] * weights[1].weight
                        + ((weights[0].offset + weights[1].offset + 1) << log2_wd))
                        >> (log2_wd + 1)
                }
                [true, false] => {
                    ((first[at] * weights[0].weight + round) >> log2_wd) + weights[0].offset
                }
                _ => ((second[at] * weights[1].weight + round) >> log2_wd) + weights[1].offset,
            };
            into[column + row * out_stride] = value.clamp(0, most) as u16;
        }
    }
}
