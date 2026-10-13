// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Intra prediction, clause 8.3. The caller gathers the neighbours and their availability.

/// The row above is twice the width; the caller fills a missing above-right with its last sample.
#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub top: [u8; 16],
    pub left: [u8; 16],
    pub corner: u8,
    pub has_top: bool,
    pub has_left: bool,
    pub has_corner: bool,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            top: [128; 16],
            left: [128; 16],
            corner: 128,
            has_top: false,
            has_left: false,
            has_corner: false,
        }
    }
}

impl Edge {
    /// Index -1 is the corner `p[-1, -1]`.
    fn top(&self, x: i32) -> i32 {
        if x < 0 {
            return i32::from(self.corner);
        }
        i32::from(self.top[x.min(15) as usize])
    }

    fn left(&self, y: i32) -> i32 {
        if y < 0 {
            return i32::from(self.corner);
        }
        i32::from(self.left[y.min(15) as usize])
    }

    fn corner(&self) -> i32 {
        i32::from(self.corner)
    }

    /// Clause 8.3.2.2.1: low-pass filter for 8x8; a missing edge changes the corner rule.
    pub fn filtered(&self) -> Self {
        let mut out = *self;
        if self.has_top {
            out.top[0] = if self.has_corner {
                ((self.corner() + 2 * self.top(0) + self.top(1) + 2) >> 2) as u8
            } else {
                ((3 * self.top(0) + self.top(1) + 2) >> 2) as u8
            };
            for x in 1..15 {
                out.top[x as usize] =
                    ((self.top(x - 1) + 2 * self.top(x) + self.top(x + 1) + 2) >> 2) as u8;
            }
            out.top[15] = ((self.top(14) + 3 * self.top(15) + 2) >> 2) as u8;
        }
        if self.has_corner {
            out.corner = match (self.has_top, self.has_left) {
                (true, true) => ((self.top(0) + 2 * self.corner() + self.left(0) + 2) >> 2) as u8,
                (true, false) => ((3 * self.corner() + self.top(0) + 2) >> 2) as u8,
                (false, true) => ((3 * self.corner() + self.left(0) + 2) >> 2) as u8,
                (false, false) => self.corner,
            };
        }
        if self.has_left {
            out.left[0] = if self.has_corner {
                ((self.corner() + 2 * self.left(0) + self.left(1) + 2) >> 2) as u8
            } else {
                ((3 * self.left(0) + self.left(1) + 2) >> 2) as u8
            };
            for y in 1..7 {
                out.left[y as usize] =
                    ((self.left(y - 1) + 2 * self.left(y) + self.left(y + 1) + 2) >> 2) as u8;
            }
            out.left[7] = ((self.left(6) + 3 * self.left(7) + 2) >> 2) as u8;
        }
        out
    }
}

pub struct Target<'a> {
    pub data: &'a mut [u8],
    pub at: usize,
    pub stride: usize,
}

impl Target<'_> {
    fn put(&mut self, x: usize, y: usize, value: i32) {
        self.data[self.at + y * self.stride + x] = value.clamp(0, 255) as u8;
    }
}

/// Mean of what is available, or mid-range. Clauses 8.3.1.2.3, 8.3.2.2.4 and 8.3.3.3.
fn mean(edge: &Edge, size: usize) -> i32 {
    let top: i32 = (0..size as i32).map(|x| edge.top(x)).sum();
    let left: i32 = (0..size as i32).map(|y| edge.left(y)).sum();
    let shift = size.trailing_zeros();
    match (edge.has_top, edge.has_left) {
        (true, true) => (top + left + size as i32) >> (shift + 1),
        (true, false) => (top + size as i32 / 2) >> shift,
        (false, true) => (left + size as i32 / 2) >> shift,
        (false, false) => 128,
    }
}

pub fn square(target: &mut Target<'_>, size: usize, mode: usize, edge: &Edge) {
    let end = size as i32 - 1;
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let value = match mode {
                0 => edge.top(x),
                1 => edge.left(y),
                2 => mean(edge, size),
                3 if x == end && y == end => {
                    (edge.top(2 * end) + 3 * edge.top(2 * end + 1) + 2) >> 2
                }
                3 => (edge.top(x + y) + 2 * edge.top(x + y + 1) + edge.top(x + y + 2) + 2) >> 2,
                4 if x > y => {
                    (edge.top(x - y - 2) + 2 * edge.top(x - y - 1) + edge.top(x - y) + 2) >> 2
                }
                4 if x < y => {
                    (edge.left(y - x - 2) + 2 * edge.left(y - x - 1) + edge.left(y - x) + 2) >> 2
                }
                4 => (edge.top(0) + 2 * edge.corner() + edge.left(0) + 2) >> 2,
                5 => {
                    let z = 2 * x - y;
                    if z >= 0 && z % 2 == 0 {
                        (edge.top(x - (y >> 1) - 1) + edge.top(x - (y >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (edge.top(x - (y >> 1) - 2)
                            + 2 * edge.top(x - (y >> 1) - 1)
                            + edge.top(x - (y >> 1))
                            + 2)
                            >> 2
                    } else if z == -1 {
                        (edge.left(0) + 2 * edge.corner() + edge.top(0) + 2) >> 2
                    } else {
                        (edge.left(y - 2 * x - 1)
                            + 2 * edge.left(y - 2 * x - 2)
                            + edge.left(y - 2 * x - 3)
                            + 2)
                            >> 2
                    }
                }
                6 => {
                    let z = 2 * y - x;
                    if z >= 0 && z % 2 == 0 {
                        (edge.left(y - (x >> 1) - 1) + edge.left(y - (x >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (edge.left(y - (x >> 1) - 2)
                            + 2 * edge.left(y - (x >> 1) - 1)
                            + edge.left(y - (x >> 1))
                            + 2)
                            >> 2
                    } else if z == -1 {
                        (edge.left(0) + 2 * edge.corner() + edge.top(0) + 2) >> 2
                    } else {
                        (edge.top(x - 2 * y - 1)
                            + 2 * edge.top(x - 2 * y - 2)
                            + edge.top(x - 2 * y - 3)
                            + 2)
                            >> 2
                    }
                }
                7 if y % 2 == 0 => (edge.top(x + (y >> 1)) + edge.top(x + (y >> 1) + 1) + 1) >> 1,
                7 => {
                    (edge.top(x + (y >> 1))
                        + 2 * edge.top(x + (y >> 1) + 1)
                        + edge.top(x + (y >> 1) + 2)
                        + 2)
                        >> 2
                }
                _ => {
                    let z = x + 2 * y;
                    if z == 2 * size as i32 - 3 {
                        (edge.left(end - 1) + 3 * edge.left(end) + 2) >> 2
                    } else if z > 2 * size as i32 - 3 {
                        edge.left(end)
                    } else if z % 2 == 0 {
                        (edge.left(y + (x >> 1)) + edge.left(y + (x >> 1) + 1) + 1) >> 1
                    } else {
                        (edge.left(y + (x >> 1))
                            + 2 * edge.left(y + (x >> 1) + 1)
                            + edge.left(y + (x >> 1) + 2)
                            + 2)
                            >> 2
                    }
                }
            };
            target.put(x as usize, y as usize, value);
        }
    }
}

/// Plane prediction, clauses 8.3.3.4 and 8.3.4.4.
fn plane(target: &mut Target<'_>, width: usize, height: usize, edge: &Edge, scale: i32) {
    let half_x = width as i32 / 2 - 1;
    let half_y = height as i32 / 2 - 1;
    let a = 16 * (edge.left(height as i32 - 1) + edge.top(width as i32 - 1));
    let mut h = 0;
    for step in 0..=half_x {
        h += (step + 1) * (edge.top(half_x + 1 + step) - edge.top(half_x - 1 - step));
    }
    let mut v = 0;
    for step in 0..=half_y {
        v += (step + 1) * (edge.left(half_y + 1 + step) - edge.left(half_y - 1 - step));
    }
    let b = (scale * h + 32) >> 6;
    let c = (scale * v + 32) >> 6;
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let value = (a + b * (x - half_x) + c * (y - half_y) + 16) >> 5;
            target.put(x as usize, y as usize, value);
        }
    }
}

/// Clause 8.3.3: Intra16x16.
pub fn luma_16x16(target: &mut Target<'_>, mode: usize, edge: &Edge) {
    match mode {
        0 => {
            for y in 0..16 {
                for x in 0..16 {
                    target.put(x, y, edge.top(x as i32));
                }
            }
        }
        1 => {
            for y in 0..16 {
                for x in 0..16 {
                    target.put(x, y, edge.left(y as i32));
                }
            }
        }
        2 => {
            let value = mean(edge, 16);
            for y in 0..16 {
                for x in 0..16 {
                    target.put(x, y, value);
                }
            }
        }
        _ => plane(target, 16, 16, edge, 5),
    }
}

/// Clause 8.3.4: 4:2:0 chroma. Mode 0 is DC here, not vertical as in luma.
pub fn chroma(target: &mut Target<'_>, mode: usize, edge: &Edge) {
    match mode {
        0 => chroma_dc(target, edge),
        1 => {
            for y in 0..8 {
                for x in 0..8 {
                    target.put(x, y, edge.left(y as i32));
                }
            }
        }
        2 => {
            for y in 0..8 {
                for x in 0..8 {
                    target.put(x, y, edge.top(x as i32));
                }
            }
        }
        _ => plane(target, 8, 8, edge, 34),
    }
}

/// Clause 8.3.4.1: DC per 4x4 quarter.
fn chroma_dc(target: &mut Target<'_>, edge: &Edge) {
    for quarter in 0..4 {
        let (x0, y0) = ((quarter % 2) * 4, (quarter / 2) * 4);
        let top: i32 = (0..4).map(|x| edge.top((x0 + x) as i32)).sum();
        let left: i32 = (0..4).map(|y| edge.left((y0 + y) as i32)).sum();
        let (has_top, has_left) = (edge.has_top, edge.has_left);
        let both = (x0 == 0 && y0 == 0) || (x0 > 0 && y0 > 0);
        let prefer_top = x0 > 0 && y0 == 0;
        let value = if both && has_top && has_left {
            (top + left + 4) >> 3
        } else if (both || prefer_top) && has_top {
            (top + 2) >> 2
        } else if both || !prefer_top {
            if has_left {
                (left + 2) >> 2
            } else if has_top {
                (top + 2) >> 2
            } else {
                128
            }
        } else if has_left {
            (left + 2) >> 2
        } else {
            128
        };
        for y in 0..4 {
            for x in 0..4 {
                target.put(x0 + x, y0 + y, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge() -> Edge {
        Edge {
            top: std::array::from_fn(|x| (10 + x * 3) as u8),
            left: std::array::from_fn(|y| (200 - y * 7) as u8),
            corner: 100,
            has_top: true,
            has_left: true,
            has_corner: true,
        }
    }

    fn block(size: usize, mode: usize, edge: &Edge) -> Vec<u8> {
        let mut data = vec![0u8; size * size];
        let mut target = Target {
            data: &mut data,
            at: 0,
            stride: size,
        };
        super::square(&mut target, size, mode, edge);
        data
    }

    #[test]
    fn vertical_and_horizontal_are_the_edges_repeated() {
        let edge = edge();
        for size in [4, 8] {
            let vertical = block(size, 0, &edge);
            let horizontal = block(size, 1, &edge);
            for y in 0..size {
                for x in 0..size {
                    assert_eq!(vertical[y * size + x], edge.top[x]);
                    assert_eq!(horizontal[y * size + x], edge.left[y]);
                }
            }
        }
    }

    #[test]
    fn the_mean_is_the_mean_of_whichever_edges_are_there() {
        let mut edge = edge();
        for size in [4, 8] {
            let top: u32 = (0..size).map(|x| u32::from(edge.top[x])).sum();
            let left: u32 = (0..size).map(|y| u32::from(edge.left[y])).sum();
            let count = size as u32;
            assert_eq!(
                u32::from(block(size, 2, &edge)[0]),
                (top + left + count) / (2 * count)
            );
            edge.has_left = false;
            assert_eq!(
                u32::from(block(size, 2, &edge)[0]),
                (top + count / 2) / count
            );
            edge.has_top = false;
            edge.has_left = true;
            assert_eq!(
                u32::from(block(size, 2, &edge)[0]),
                (left + count / 2) / count
            );
            edge.has_left = false;
            assert_eq!(block(size, 2, &edge)[0], 128);
            edge.has_top = true;
            edge.has_left = true;
        }
    }

    #[test]
    fn the_diagonal_modes_are_the_equations_of_clause_8_3_1_2() {
        let edge = edge();
        let p = |x: i32, y: i32| -> i32 {
            if y == -1 && x == -1 {
                i32::from(edge.corner)
            } else if y == -1 {
                i32::from(edge.top[x as usize])
            } else {
                i32::from(edge.left[y as usize])
            }
        };
        let down_left = block(4, 3, &edge);
        let down_right = block(4, 4, &edge);
        for y in 0..4i32 {
            for x in 0..4i32 {
                let wanted = if x == 3 && y == 3 {
                    (p(6, -1) + 3 * p(7, -1) + 2) >> 2
                } else {
                    (p(x + y, -1) + 2 * p(x + y + 1, -1) + p(x + y + 2, -1) + 2) >> 2
                };
                assert_eq!(i32::from(down_left[(y * 4 + x) as usize]), wanted);
                let wanted = match x.cmp(&y) {
                    std::cmp::Ordering::Greater => {
                        (p(x - y - 2, -1) + 2 * p(x - y - 1, -1) + p(x - y, -1) + 2) >> 2
                    }
                    std::cmp::Ordering::Less => {
                        (p(-1, y - x - 2) + 2 * p(-1, y - x - 1) + p(-1, y - x) + 2) >> 2
                    }
                    std::cmp::Ordering::Equal => (p(0, -1) + 2 * p(-1, -1) + p(-1, 0) + 2) >> 2,
                };
                assert_eq!(i32::from(down_right[(y * 4 + x) as usize]), wanted);
            }
        }
    }

    #[test]
    fn horizontal_up_repeats_the_last_sample_once_the_column_runs_out() {
        let edge = edge();
        for size in [4, 8] {
            let got = block(size, 8, &edge);
            assert_eq!(got[size * size - 1], edge.left[size - 1]);
        }
    }

    #[test]
    fn a_flat_neighbourhood_predicts_flat_in_every_mode() {
        let edge = Edge {
            top: [37; 16],
            left: [37; 16],
            corner: 37,
            has_top: true,
            has_left: true,
            has_corner: true,
        };
        for size in [4, 8] {
            for mode in 0..9 {
                assert!(
                    block(size, mode, &edge).iter().all(|&value| value == 37),
                    "size {size} mode {mode}"
                );
            }
        }
        let mut data = vec![0u8; 256];
        for mode in 0..4 {
            let mut target = Target {
                data: &mut data,
                at: 0,
                stride: 16,
            };
            luma_16x16(&mut target, mode, &edge);
            assert!(data.iter().all(|&value| value == 37), "16x16 mode {mode}");
        }
        for mode in 0..4 {
            let mut target = Target {
                data: &mut data,
                at: 0,
                stride: 16,
            };
            chroma(&mut target, mode, &edge);
            assert!(
                (0..8).all(|y| (0..8).all(|x| data[y * 16 + x] == 37)),
                "chroma mode {mode}"
            );
        }
    }

    #[test]
    fn the_plane_mode_follows_a_gradient_exactly() {
        let edge = Edge {
            top: std::array::from_fn(|x| (103 + x * 2) as u8),
            left: std::array::from_fn(|y| (98 - (y as i32) * 3) as u8),
            corner: 101,
            has_top: true,
            has_left: true,
            has_corner: true,
        };
        let mut data = vec![0u8; 256];
        let mut target = Target {
            data: &mut data,
            at: 0,
            stride: 16,
        };
        luma_16x16(&mut target, 3, &edge);
        for y in 0..16i32 {
            for x in 0..16i32 {
                assert_eq!(
                    i32::from(data[(y * 16 + x) as usize]),
                    100 + 2 * x - 3 * y,
                    "at {x}, {y}"
                );
            }
        }

        let edge = Edge {
            top: std::array::from_fn(|x| (103 + x * 2) as u8),
            left: std::array::from_fn(|y| (98 - (y as i32) * 3) as u8),
            corner: 101,
            has_top: true,
            has_left: true,
            has_corner: true,
        };
        let mut target = Target {
            data: &mut data,
            at: 0,
            stride: 16,
        };
        chroma(&mut target, 3, &edge);
        for y in 0..8i32 {
            for x in 0..8i32 {
                assert_eq!(
                    i32::from(data[(y * 16 + x) as usize]),
                    100 + 2 * x - 3 * y,
                    "chroma at {x}, {y}"
                );
            }
        }
    }

    #[test]
    fn the_8x8_edge_is_filtered_and_the_4x4_edge_is_not() {
        let mut edge = edge();
        edge.top[0] = 40;
        let filtered = edge.filtered();
        assert_eq!(
            i32::from(filtered.top[0]),
            (i32::from(edge.corner) + 2 * 40 + i32::from(edge.top[1]) + 2) >> 2
        );
        assert_eq!(
            i32::from(filtered.top[15]),
            (i32::from(edge.top[14]) + 3 * i32::from(edge.top[15]) + 2) >> 2
        );
        let flat = Edge {
            top: [90; 16],
            left: [90; 16],
            corner: 90,
            has_top: true,
            has_left: true,
            has_corner: true,
        };
        let filtered = flat.filtered();
        assert_eq!(filtered.top, [90; 16]);
        assert_eq!(filtered.left[..8], [90; 8]);
        assert_eq!(filtered.corner, 90);
    }
}
