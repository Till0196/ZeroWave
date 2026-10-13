// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Prediction from the samples around a block, clause 8.4.4.2.
//! The reference run is one array in substitution order (up the left column, round the corner,
//! along the top), so the patching of 8.4.4.2.2 is one forward pass.

use super::tables::{INTRA_ANGLE, INTRA_HOR_VER_DIST_THRES, INTRA_INV_ANGLE};

/// The longest run: 64 above a 32x32 block, 64 to its left, and the corner.
const MOST: usize = 129;

#[derive(Debug, Clone)]
pub struct Neighbours {
    /// The run, from `p[-1][2n-1]` up to `p[-1][-1]` and on to `p[2n-1][-1]`.
    samples: [u16; MOST],
    available: [bool; MOST],
    filtered: [u16; MOST],
    side: usize,
    any: bool,
}

impl Default for Neighbours {
    fn default() -> Self {
        Self {
            samples: [0; MOST],
            available: [false; MOST],
            filtered: [0; MOST],
            side: 4,
            any: false,
        }
    }
}

impl Neighbours {
    pub fn begin(&mut self, side: usize) {
        self.side = side;
        self.any = false;
        self.available[..4 * side + 1].fill(false);
    }

    #[inline]
    fn at_left(&self, y: isize) -> usize {
        (2 * self.side as isize - 1 - y) as usize
    }

    #[inline]
    fn at_top(&self, x: isize) -> usize {
        (2 * self.side as isize + 1 + x) as usize
    }

    pub fn set_left(&mut self, y: usize, value: u16) {
        let at = self.at_left(y as isize);
        self.samples[at] = value;
        self.available[at] = true;
        self.any = true;
    }

    pub fn set_top(&mut self, x: usize, value: u16) {
        let at = self.at_top(x as isize);
        self.samples[at] = value;
        self.available[at] = true;
        self.any = true;
    }

    pub fn set_corner(&mut self, value: u16) {
        let at = 2 * self.side;
        self.samples[at] = value;
        self.available[at] = true;
        self.any = true;
    }

    fn left(&self, y: isize) -> i32 {
        i32::from(self.samples[self.at_left(y)])
    }

    fn top(&self, x: isize) -> i32 {
        i32::from(self.samples[self.at_top(x)])
    }

    /// Clause 8.4.4.2.2: substitutes unavailable positions (none available: mid range).
    pub fn substitute(&mut self, depth: u8) {
        let count = 4 * self.side + 1;
        if !self.any {
            let middle = 1u16 << (depth - 1);
            self.samples[..count].fill(middle);
            self.available[..count].fill(true);
            return;
        }
        if self.available[..count].iter().all(|&flag| flag) {
            return;
        }
        // One forward walk, equal to the standard's three searches.
        if !self.available[0]
            && let Some(found) = (0..count).find(|&at| self.available[at])
        {
            self.samples[0] = self.samples[found];
            self.available[0] = true;
        }
        for at in 1..count {
            if !self.available[at] {
                self.samples[at] = self.samples[at - 1];
                self.available[at] = true;
            }
        }
    }

    /// Clause 8.4.4.2.3: smooths the references (strong filter for flat 32x32 luma).
    pub fn filter(&mut self, mode: usize, chroma: bool, depth: u8, strong: bool) -> bool {
        let side = self.side;
        // Clause 8.4.4.2.1: chroma is filtered only at 4:4:4, never at 4:2:0.
        if chroma || mode == 1 || side == 4 {
            return false;
        }
        let distance = (mode as i32 - 26).abs().min((mode as i32 - 10).abs());
        let threshold = INTRA_HOR_VER_DIST_THRES[side.trailing_zeros() as usize - 3];
        if distance <= threshold {
            return false;
        }
        let count = 4 * side + 1;
        let corner = 2 * side;
        let bilinear = strong
            && !chroma
            && side == 32
            && (self.left(-1) + self.top(63) - 2 * self.top(31)).abs() < 1 << (depth - 5)
            && (self.left(-1) + self.left(63) - 2 * self.left(31)).abs() < 1 << (depth - 5);
        if bilinear {
            let corner_value = self.left(-1);
            let bottom = self.left(63);
            let right = self.top(63);
            self.filtered[corner] = corner_value as u16;
            for y in 0..63 {
                let value = ((63 - y) * corner_value + (y + 1) * bottom + 32) >> 6;
                self.filtered[self.at_left(y as isize)] = value as u16;
            }
            self.filtered[self.at_left(63)] = bottom as u16;
            for x in 0..63 {
                let value = ((63 - x) * corner_value + (x + 1) * right + 32) >> 6;
                self.filtered[self.at_top(x as isize)] = value as u16;
            }
            self.filtered[self.at_top(63)] = right as u16;
            return true;
        }
        self.filtered[0] = self.samples[0];
        self.filtered[count - 1] = self.samples[count - 1];
        for at in 1..count - 1 {
            let value = i32::from(self.samples[at - 1])
                + 2 * i32::from(self.samples[at])
                + i32::from(self.samples[at + 1]);
            self.filtered[at] = ((value + 2) >> 2) as u16;
        }
        true
    }

    fn run(&self, filtered: bool) -> &[u16] {
        if filtered {
            &self.filtered
        } else {
            &self.samples
        }
    }
}

/// One block's prediction into `into`, `stride` apart; `filtered` from [`Neighbours`::filter].
pub fn predict(
    neighbours: &Neighbours,
    mode: usize,
    chroma: bool,
    depth: u8,
    filtered: bool,
    into: &mut [u16],
    stride: usize,
) {
    let side = neighbours.side;
    let run = neighbours.run(filtered);
    let left = |y: isize| i32::from(run[neighbours.at_left(y)]);
    let top = |x: isize| i32::from(run[neighbours.at_top(x)]);
    let corner = i32::from(run[2 * side]);
    let most = (1i32 << depth) - 1;

    match mode {
        // Equation 8-46: planar.
        0 => {
            let shift = side.trailing_zeros() as i32 + 1;
            let right = top(side as isize);
            let bottom = left(side as isize);
            for y in 0..side {
                for x in 0..side {
                    let value = (side as i32 - 1 - x as i32) * left(y as isize)
                        + (x as i32 + 1) * right
                        + (side as i32 - 1 - y as i32) * top(x as isize)
                        + (y as i32 + 1) * bottom
                        + side as i32;
                    into[x + y * stride] = (value >> shift) as u16;
                }
            }
        }
        // Equation 8-47: DC, with the first row and column pulled towards their neighbours.
        1 => {
            let mut sum = side as i32;
            for i in 0..side as isize {
                sum += top(i) + left(i);
            }
            let dc = sum >> (side.trailing_zeros() + 1);
            for y in 0..side {
                for x in 0..side {
                    into[x + y * stride] = dc as u16;
                }
            }
            if !chroma && side < 32 {
                into[0] = ((left(0) + 2 * dc + top(0) + 2) >> 2) as u16;
                for (x, slot) in into[1..side].iter_mut().enumerate() {
                    *slot = ((top(x as isize + 1) + 3 * dc + 2) >> 2) as u16;
                }
                for y in 1..side {
                    into[y * stride] = ((left(y as isize) + 3 * dc + 2) >> 2) as u16;
                }
            }
        }
        _ => {
            let angle = INTRA_ANGLE[mode];
            // The main reference, with the other side projected onto its far end when the angle
            // points back past the corner. Index nought is `ref[-(2 * side)]`.
            let base = 2 * side;
            let mut reference = [0i32; 4 * 32 + 2];
            let vertical = mode >= 18;
            let along = |i: isize| if vertical { top(i) } else { left(i) };
            let across = |i: isize| if vertical { left(i) } else { top(i) };
            for x in 0..=side as isize {
                reference[(base as isize + x) as usize] = along(x - 1);
            }
            if angle < 0 {
                let end = (side as i32 * angle) >> 5;
                if end < -1 {
                    let inverse = INTRA_INV_ANGLE[mode];
                    for x in end..=-1 {
                        let source = -1 + ((x * inverse + 128) >> 8);
                        reference[(base as isize + x as isize) as usize] = across(source as isize);
                    }
                }
            } else {
                for x in side as isize + 1..=2 * side as isize {
                    reference[(base as isize + x) as usize] = along(x - 1);
                }
            }
            for y in 0..side as i32 {
                let index = ((y + 1) * angle) >> 5;
                let fraction = ((y + 1) * angle) & 31;
                for x in 0..side as i32 {
                    let at = (base as i32 + x + index + 1) as usize;
                    let value = if fraction != 0 {
                        ((32 - fraction) * reference[at] + fraction * reference[at + 1] + 16) >> 5
                    } else {
                        reference[at]
                    };
                    let (px, py) = if vertical {
                        (x as usize, y as usize)
                    } else {
                        (y as usize, x as usize)
                    };
                    into[px + py * stride] = value as u16;
                }
            }
            // Equations 8-60 and 8-68: pure horizontal or vertical; the first line is pulled back
            // by half the step against the other side.
            if !chroma && side < 32 && (mode == 26 || mode == 10) {
                for i in 0..side as isize {
                    let (px, py) = if mode == 26 {
                        (0, i as usize)
                    } else {
                        (i as usize, 0)
                    };
                    let edge = if mode == 26 { left(i) } else { top(i) };
                    let line = if mode == 26 { top(0) } else { left(0) };
                    let value = line + ((edge - corner) >> 1);
                    into[px + py * stride] = value.clamp(0, most) as u16;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Neighbours, predict};

    fn gradient(side: usize) -> Neighbours {
        let mut neighbours = Neighbours::default();
        neighbours.begin(side);
        neighbours.set_corner(100);
        for i in 0..2 * side {
            neighbours.set_left(i, (100 + i) as u16);
            neighbours.set_top(i, (100 + i) as u16);
        }
        neighbours.substitute(8);
        neighbours
    }

    #[test]
    fn a_block_with_nothing_around_it_is_the_middle_of_the_range() {
        for depth in [8u8, 10] {
            let mut neighbours = Neighbours::default();
            neighbours.begin(4);
            neighbours.substitute(depth);
            let mut out = [0u16; 16];
            predict(&neighbours, 0, false, depth, false, &mut out, 4);
            assert!(out.iter().all(|&value| value == 1 << (depth - 1)));
        }
    }

    #[test]
    fn what_is_missing_takes_the_nearest_sample_that_is_not() {
        let mut neighbours = Neighbours::default();
        neighbours.begin(4);
        neighbours.set_left(7, 40);
        neighbours.substitute(8);
        let mut out = [0u16; 16];
        predict(&neighbours, 1, false, 8, false, &mut out, 4);
        assert!(out.iter().all(|&value| value == 40));

        let mut neighbours = Neighbours::default();
        neighbours.begin(4);
        for i in 0..8 {
            neighbours.set_top(i, 60);
        }
        neighbours.substitute(8);
        let mut out = [0u16; 16];
        predict(&neighbours, 1, false, 8, false, &mut out, 4);
        assert!(out.iter().all(|&value| value == 60));
    }

    #[test]
    fn the_plane_is_equation_8_46() {
        let neighbours = gradient(4);
        let mut out = [0u16; 16];
        predict(&neighbours, 0, false, 8, false, &mut out, 4);
        for y in 0..4usize {
            for x in 0..4usize {
                let left = 100 + y as i32;
                let top = 100 + x as i32;
                let right = 100 + 4;
                let bottom = 100 + 4;
                let wanted = ((3 - x as i32) * left
                    + (x as i32 + 1) * right
                    + (3 - y as i32) * top
                    + (y as i32 + 1) * bottom
                    + 4)
                    >> 3;
                assert_eq!(i32::from(out[x + y * 4]), wanted, "at {x},{y}");
            }
        }
    }

    #[test]
    fn the_flat_average_is_equations_8_47_to_8_51() {
        let neighbours = gradient(4);
        let mut out = [0u16; 16];
        predict(&neighbours, 1, false, 8, false, &mut out, 4);
        let sum: i32 = (0..4).map(|i| 100 + i).sum::<i32>() * 2 + 4;
        let dc = sum >> 3;
        assert_eq!(i32::from(out[0]), (100 + 2 * dc + 100 + 2) >> 2);
        for (x, &got) in out[1..4].iter().enumerate() {
            let x = x as i32 + 1;
            assert_eq!(i32::from(got), (100 + x + 3 * dc + 2) >> 2);
        }
        for y in 1..4 {
            assert_eq!(i32::from(out[y * 4]), (100 + y as i32 + 3 * dc + 2) >> 2);
        }
        assert_eq!(i32::from(out[5]), dc);

        let mut out = [0u16; 16];
        predict(&neighbours, 1, true, 8, false, &mut out, 4);
        assert!(out.iter().all(|&value| i32::from(value) == dc));
    }

    #[test]
    fn vertical_prediction_copies_the_row_above_and_corrects_its_first_column() {
        let neighbours = gradient(8);
        let mut out = [0u16; 64];
        predict(&neighbours, 26, false, 8, false, &mut out, 8);
        for y in 0..8usize {
            for x in 1..8usize {
                assert_eq!(i32::from(out[x + y * 8]), 100 + x as i32, "at {x},{y}");
            }
            let wanted = 100 + ((100 + y as i32 - 100) >> 1);
            assert_eq!(i32::from(out[y * 8]), wanted, "first column at {y}");
        }
        let mut out = [0u16; 64];
        predict(&neighbours, 26, true, 8, false, &mut out, 8);
        for y in 0..8usize {
            assert_eq!(i32::from(out[y * 8]), 100);
        }
    }

    #[test]
    fn horizontal_prediction_copies_the_column_beside_it() {
        let neighbours = gradient(8);
        let mut out = [0u16; 64];
        predict(&neighbours, 10, false, 8, false, &mut out, 8);
        for y in 1..8usize {
            for x in 0..8usize {
                assert_eq!(i32::from(out[x + y * 8]), 100 + y as i32, "at {x},{y}");
            }
        }
        for (x, &got) in out[..8].iter().enumerate() {
            assert_eq!(i32::from(got), 100 + ((x as i32) >> 1));
        }
    }

    #[test]
    fn an_angled_prediction_weighs_the_two_references_it_falls_between() {
        let neighbours = gradient(4);
        let mut out = [0u16; 16];
        predict(&neighbours, 27, false, 8, false, &mut out, 4);
        for y in 0..4i32 {
            let index = ((y + 1) * 2) >> 5;
            let fraction = ((y + 1) * 2) & 31;
            for x in 0..4i32 {
                let first = 100 + x + index;
                let second = 100 + x + index + 1;
                let wanted = ((32 - fraction) * first + fraction * second + 16) >> 5;
                assert_eq!(i32::from(out[(x + y * 4) as usize]), wanted, "at {x},{y}");
            }
        }
    }

    #[test]
    fn an_angle_past_the_corner_reaches_round_it() {
        let mut neighbours = Neighbours::default();
        neighbours.begin(4);
        neighbours.set_corner(0);
        for i in 0..8 {
            neighbours.set_left(i, (10 + i) as u16);
            neighbours.set_top(i, (200 - i) as u16);
        }
        neighbours.substitute(8);
        let mut out = [0u16; 16];
        predict(&neighbours, 22, false, 8, false, &mut out, 4);
        assert!(
            i32::from(out[12]) < 100,
            "the corner was not reached round: {}",
            out[12]
        );
    }

    #[test]
    fn the_reference_smoothing_is_clause_8_4_4_2_3() {
        let mut neighbours = gradient(4);
        assert!(!neighbours.filter(18, false, 8, true));
        let mut neighbours = gradient(8);
        assert!(!neighbours.filter(1, false, 8, true));
        assert!(!neighbours.filter(18, true, 8, true));
        assert!(!neighbours.filter(19, false, 8, true));
        assert!(neighbours.filter(18, false, 8, true));

        let mut neighbours = gradient(32);
        assert!(neighbours.filter(18, false, 8, true));
        let mut out = [0u16; 32 * 32];
        predict(&neighbours, 26, false, 8, true, &mut out, 32);
        for (x, &got) in out[1..32].iter().enumerate() {
            let x = x as i32 + 1;
            let wanted = ((63 - x) * 100 + (x + 1) * 163 + 32) >> 6;
            assert_eq!(i32::from(got), wanted, "at {x}");
        }
    }
}
