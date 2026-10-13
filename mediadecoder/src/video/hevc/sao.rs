// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The sample adaptive offset, clause 8.7.3: band or edge offsets per coding tree block.
//! Reads the deblocked picture and writes another: an offset sample must not be compared again.

#[derive(Debug, Clone, Copy, Default)]
pub struct Offsets {
    /// 0 none, 1 band, 2 edge.
    pub kind: u8,
    /// The five offsets; the first is always nought so "nothing to correct" needs no branch.
    pub value: [i32; 5],
    /// Which quarter of the range the four bands begin at.
    pub band: u8,
    pub class: u8,
}

/// Table 8-13: where the two neighbours of a sample are, by class.
const NEIGHBOURS: [[(i32, i32); 2]; 4] = [
    [(-1, 0), (1, 0)],
    [(0, -1), (0, 1)],
    [(-1, -1), (1, 1)],
    [(1, -1), (-1, 1)],
];

#[derive(Debug)]
pub struct Plane<'a> {
    pub source: &'a [u16],
    pub into: &'a mut [u16],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub depth: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Reach {
    /// Usable neighbours: left, right, up, down, then the four corners from the top left.
    pub usable: [bool; 8],
}

/// Runs the offset over one CTB of one plane; `at` and `size` are in this plane's samples.
pub fn apply<F: Fn(usize, usize) -> bool>(
    plane: &mut Plane,
    offsets: Offsets,
    at: (usize, usize),
    size: (usize, usize),
    untouched: F,
    reach: Reach,
) {
    if offsets.kind == 0 {
        return;
    }
    let most = (1i32 << plane.depth) - 1;
    let right = (at.0 + size.0).min(plane.width);
    let bottom = (at.1 + size.1).min(plane.height);
    if offsets.kind == 1 {
        let shift = plane.depth - 5;
        let mut table = [0usize; 32];
        for k in 0..4 {
            table[(k + usize::from(offsets.band)) & 31] = k + 1;
        }
        for y in at.1..bottom {
            for x in at.0..right {
                if untouched(x, y) {
                    continue;
                }
                let value = i32::from(plane.source[x + y * plane.stride]);
                let band = table[(value >> shift) as usize & 31];
                plane.into[x + y * plane.stride] =
                    (value + offsets.value[band]).clamp(0, most) as u16;
            }
        }
        return;
    }

    let pair = NEIGHBOURS[usize::from(offsets.class) & 3];
    // Categories: valley, step down, nothing, step up, peak; the middle is renumbered to
    // nought so it lands on the offset that is always zero.
    let mut category = [0usize; 5];
    for (index, slot) in category.iter_mut().enumerate() {
        *slot = match index {
            0 => 1,
            1 => 2,
            2 => 0,
            other => other,
        };
    }
    for y in at.1..bottom {
        // Whether both neighbours of every sample in this row may be read, so the row skips checks.
        let plain = y > at.1 && y + 1 < bottom;
        for x in at.0..right {
            if untouched(x, y) {
                continue;
            }
            let value = i32::from(plane.source[x + y * plane.stride]);
            let mut index = 2i32;
            let simple = plain && x > at.0 && x + 1 < right;
            if simple {
                for &(dx, dy) in &pair {
                    let other = i32::from(
                        plane.source
                            [(x as i32 + dx) as usize + (y as i32 + dy) as usize * plane.stride],
                    );
                    index += (value - other).signum();
                }
            } else {
                let mut usable = true;
                for &(dx, dy) in &pair {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0
                        || ny < 0
                        || nx as usize >= plane.width
                        || ny as usize >= plane.height
                        || !reach.allows(x, y, at, size, dx, dy)
                    {
                        usable = false;
                        break;
                    }
                    let other = i32::from(plane.source[nx as usize + ny as usize * plane.stride]);
                    index += (value - other).signum();
                }
                if !usable {
                    continue;
                }
            }
            plane.into[x + y * plane.stride] =
                (value + offsets.value[category[index as usize]]).clamp(0, most) as u16;
        }
    }
}

impl Reach {
    fn allows(
        &self,
        x: usize,
        y: usize,
        at: (usize, usize),
        size: (usize, usize),
        dx: i32,
        dy: i32,
    ) -> bool {
        let left = dx < 0 && x == at.0;
        let right = dx > 0 && x + 1 >= at.0 + size.0;
        let up = dy < 0 && y == at.1;
        let down = dy > 0 && y + 1 >= at.1 + size.1;
        let index = match (left, right, up, down) {
            (true, _, false, false) => 0,
            (_, true, false, false) => 1,
            (false, false, true, _) => 2,
            (false, false, _, true) => 3,
            (true, _, true, _) => 4,
            (_, true, true, _) => 5,
            (true, _, _, true) => 6,
            (_, true, _, true) => 7,
            _ => return true,
        };
        self.usable[index]
    }
}

#[cfg(test)]
mod tests {
    use super::{Offsets, Plane, Reach, apply};

    const OPEN: Reach = Reach { usable: [true; 8] };

    fn run(source: &[u16], offsets: Offsets, width: usize, height: usize) -> Vec<u16> {
        let mut into = source.to_vec();
        let mut plane = Plane {
            source,
            into: &mut into,
            stride: width,
            width,
            height,
            depth: 8,
        };
        apply(
            &mut plane,
            offsets,
            (0, 0),
            (width, height),
            |_, _| false,
            OPEN,
        );
        into
    }

    #[test]
    fn a_band_offset_moves_four_bands_and_nothing_else() {
        let source: Vec<u16> = (0..64u16).map(|at| at * 4).collect();
        let offsets = Offsets {
            kind: 1,
            value: [0, 5, -5, 7, -7],
            band: 4,
            class: 0,
        };
        let out = run(&source, offsets, 64, 1);
        for (at, (&before, &after)) in source.iter().zip(&out).enumerate() {
            let band = (before >> 3) as i32 - 4;
            let wanted = if (0..4).contains(&band) {
                (i32::from(before) + [5, -5, 7, -7][band as usize]).clamp(0, 255)
            } else {
                i32::from(before)
            };
            assert_eq!(i32::from(after), wanted, "at {at} value {before}");
        }
    }

    #[test]
    fn an_edge_offset_lifts_a_valley_and_flattens_a_peak() {
        let source: Vec<u16> = vec![100, 90, 80, 60, 80, 70, 60, 50];
        let offsets = Offsets {
            kind: 2,
            value: [0, 3, 1, -1, -3],
            band: 0,
            class: 0,
        };
        let out = run(&source, offsets, 8, 1);
        assert_eq!(out[0], 100);
        assert_eq!(out[7], 50);
        for at in 1..7 {
            let value = i32::from(source[at]);
            let index = 2
                + (value - i32::from(source[at - 1])).signum()
                + (value - i32::from(source[at + 1])).signum();
            let offset = match index {
                0 => 3,
                1 => 1,
                2 => 0,
                3 => -1,
                _ => -3,
            };
            assert_eq!(i32::from(out[at]), value + offset, "at {at}");
        }
        assert_eq!(out[3], 63);
        assert_eq!(out[4], 77);
    }

    #[test]
    fn the_four_classes_look_the_four_ways_of_table_8_13() {
        let source: Vec<u16> = vec![90, 90, 90, 90, 50, 90, 90, 90, 90];
        for class in 0..4u8 {
            let offsets = Offsets {
                kind: 2,
                value: [0, 9, 0, 0, 0],
                band: 0,
                class,
            };
            let out = run(&source, offsets, 3, 3);
            assert_eq!(out[4], 59, "class {class}");
        }
        let source: Vec<u16> = vec![50, 50, 50, 90, 90, 90, 50, 50, 50];
        let peak = Offsets {
            kind: 2,
            value: [0, 0, 0, 0, -9],
            band: 0,
            class: 1,
        };
        assert_eq!(run(&source, peak, 3, 3)[4], 81);
        let across = Offsets { class: 0, ..peak };
        assert_eq!(run(&source, across, 3, 3)[4], 90);
    }

    #[test]
    fn a_block_with_no_offset_is_left_alone() {
        let source: Vec<u16> = (0..16u16).collect();
        let out = run(&source, Offsets::default(), 4, 4);
        assert_eq!(out, source);
    }

    #[test]
    fn what_skipped_the_quantiser_is_not_offset() {
        let source: Vec<u16> = vec![100; 16];
        let mut into = source.clone();
        let mut plane = Plane {
            source: &source,
            into: &mut into,
            stride: 4,
            width: 4,
            height: 4,
            depth: 8,
        };
        let offsets = Offsets {
            kind: 1,
            value: [0, 9, 9, 9, 9],
            band: 100 >> 3,
            class: 0,
        };
        apply(&mut plane, offsets, (0, 0), (4, 4), |x, _| x < 2, OPEN);
        assert_eq!(into[0], 100);
        assert_eq!(into[1], 100);
        assert_eq!(into[2], 109);
    }
}
