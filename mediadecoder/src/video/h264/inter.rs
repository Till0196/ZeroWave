// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Inter prediction, clause 8.4.2. Vectors may point outside; the edge is repeated.

/// A plane of a reference picture, or one field of it (twice the stride, half the height).
#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    pub data: &'a [u8],
    pub at: usize,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl View<'_> {
    fn sample(&self, x: i32, y: i32) -> i32 {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        i32::from(self.data[self.at + y * self.stride + x])
    }
}

const SPAN: usize = 16 + 5;

/// Reused per slice: clearing seven kilobytes per 4x4 partition costs more than the filter.
pub struct Scratch {
    src: [i32; SPAN * SPAN],
    half_x: [i32; SPAN * SPAN],
    half_y: [i32; SPAN * SPAN],
    both: [i32; SPAN * SPAN],
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            src: [0; SPAN * SPAN],
            half_x: [0; SPAN * SPAN],
            half_y: [0; SPAN * SPAN],
            both: [0; SPAN * SPAN],
        }
    }
}

fn clip(value: i32) -> i32 {
    value.clamp(0, 255)
}

/// `x`, `y` in quarter samples; `out` is `width` by `height`, stride `width`.
pub fn luma(
    out: &mut [u8],
    width: usize,
    height: usize,
    view: &View<'_>,
    x: i32,
    y: i32,
    scratch: &mut Scratch,
) {
    let (whole_x, frac_x) = (x >> 2, x & 3);
    let (whole_y, frac_y) = (y >> 2, y & 3);

    if frac_x == 0 && frac_y == 0 {
        for row in 0..height {
            for column in 0..width {
                out[row * width + column] =
                    view.sample(whole_x + column as i32, whole_y + row as i32) as u8;
            }
        }
        return;
    }

    let span = width + 5;
    let across = width;

    // Which filtered values this position needs (Table 8-12); none uses all three.
    let need_j = (frac_x == 2 && frac_y != 0) || (frac_y == 2 && frac_x != 0);
    let need_b = frac_x != 0 && frac_y != 2;
    let need_h = frac_y != 0 && frac_x != 2;
    let need_half_x = need_b || need_j;

    let (row_from, row_to) = if need_h || need_j {
        (0, height + 5)
    } else {
        (2, height + 2)
    };
    let (column_from, column_to) = if need_half_x {
        (0, span)
    } else {
        (2, width + 3)
    };
    let (left, top) = (whole_x - 2, whole_y - 2);
    let inside = left + column_from as i32 >= 0
        && top + row_from as i32 >= 0
        && left + column_to as i32 <= view.width as i32
        && top + row_to as i32 <= view.height as i32;
    let src = &mut scratch.src;
    if inside {
        for row in row_from..row_to {
            // Signed: with no padding in one axis the corner may lie off the edge.
            let row_at = view.at as isize
                + (top + row as i32) as isize * view.stride as isize
                + left as isize;
            let from = (row_at + column_from as isize) as usize;
            let taken = &view.data[from..from + (column_to - column_from)];
            let into = &mut src[row * span + column_from..row * span + column_to];
            for (slot, &sample) in into.iter_mut().zip(taken) {
                *slot = i32::from(sample);
            }
        }
    } else {
        for row in row_from..row_to {
            for column in column_from..column_to {
                src[row * span + column] = view.sample(left + column as i32, top + row as i32);
            }
        }
    }

    let half_x = &mut scratch.half_x;
    if need_half_x {
        let (from, to) = if need_j {
            (0, height + 5)
        } else if frac_y == 3 {
            (2, height + 3)
        } else {
            (2, height + 2)
        };
        for row in from..to {
            let taken = &src[row * span..row * span + width + 5];
            let into = &mut half_x[row * across..row * across + across];
            for (column, slot) in into.iter_mut().enumerate() {
                *slot = taken[column] + taken[column + 5]
                    - 5 * (taken[column + 1] + taken[column + 4])
                    + 20 * (taken[column + 2] + taken[column + 3]);
            }
        }
    }
    let half_y = &mut scratch.half_y;
    if need_h {
        let to = width + 2 + usize::from(frac_x == 3);
        for row in 0..height {
            let base = row * span;
            let taken = &src[base..base + 5 * span + to];
            for column in 2..to {
                half_y[base + column] = taken[column] + taken[column + 5 * span]
                    - 5 * (taken[column + span] + taken[column + 4 * span])
                    + 20 * (taken[column + 2 * span] + taken[column + 3 * span]);
            }
        }
    }
    let both = &mut scratch.both;
    if need_j {
        for row in 0..height {
            let base = row * across;
            let taken = &half_x[base..base + 6 * across];
            let into = &mut both[base..base + across];
            for (column, slot) in into.iter_mut().enumerate() {
                *slot = taken[column] + taken[column + 5 * across]
                    - 5 * (taken[column + across] + taken[column + 4 * across])
                    + 20 * (taken[column + 2 * across] + taken[column + 3 * across]);
            }
        }
    }

    for row in 0..height {
        for column in 0..width {
            let whole = |dx: usize, dy: usize| src[(row + 2 + dy) * span + column + 2 + dx];
            let b = |dy: usize| clip((half_x[(row + 2 + dy) * across + column] + 16) >> 5);
            let h = |dx: usize| clip((half_y[row * span + column + 2 + dx] + 16) >> 5);
            let j = || clip((both[row * across + column] + 512) >> 10);
            let value = match (frac_x, frac_y) {
                (0, 0) => whole(0, 0),
                (1, 0) => (whole(0, 0) + b(0) + 1) >> 1,
                (2, 0) => b(0),
                (3, 0) => (whole(1, 0) + b(0) + 1) >> 1,
                (0, 1) => (whole(0, 0) + h(0) + 1) >> 1,
                (1, 1) => (b(0) + h(0) + 1) >> 1,
                (2, 1) => (b(0) + j() + 1) >> 1,
                (3, 1) => (b(0) + h(1) + 1) >> 1,
                (0, 2) => h(0),
                (1, 2) => (h(0) + j() + 1) >> 1,
                (2, 2) => j(),
                (3, 2) => (j() + h(1) + 1) >> 1,
                (0, 3) => (whole(0, 1) + h(0) + 1) >> 1,
                (1, 3) => (h(0) + b(1) + 1) >> 1,
                (2, 3) => (j() + b(1) + 1) >> 1,
                _ => (h(1) + b(1) + 1) >> 1,
            };
            out[row * width + column] = value as u8;
        }
    }
}

/// The same for 4:2:0 chroma, at `x` and `y` in eighth samples.
pub fn chroma(out: &mut [u8], width: usize, height: usize, view: &View<'_>, x: i32, y: i32) {
    let (whole_x, frac_x) = (x >> 3, x & 7);
    let (whole_y, frac_y) = (y >> 3, y & 7);
    // Equation 8-266.
    let (ka, kb, kc, kd) = (
        (8 - frac_x) * (8 - frac_y),
        frac_x * (8 - frac_y),
        (8 - frac_x) * frac_y,
        frac_x * frac_y,
    );
    let inside = whole_x >= 0
        && whole_y >= 0
        && whole_x as usize + width < view.width
        && whole_y as usize + height < view.height;
    if inside {
        for row in 0..height {
            let top = view.at + (whole_y as usize + row) * view.stride + whole_x as usize;
            let bottom = top + view.stride;
            for column in 0..width {
                let a = i32::from(view.data[top + column]);
                let b = i32::from(view.data[top + column + 1]);
                let c = i32::from(view.data[bottom + column]);
                let d = i32::from(view.data[bottom + column + 1]);
                out[row * width + column] = ((ka * a + kb * b + kc * c + kd * d + 32) >> 6) as u8;
            }
        }
        return;
    }
    for row in 0..height as i32 {
        for column in 0..width as i32 {
            let a = view.sample(whole_x + column, whole_y + row);
            let b = view.sample(whole_x + column + 1, whole_y + row);
            let c = view.sample(whole_x + column, whole_y + row + 1);
            let d = view.sample(whole_x + column + 1, whole_y + row + 1);
            let value = (ka * a + kb * b + kc * c + kd * d + 32) >> 6;
            out[row as usize * width + column as usize] = value as u8;
        }
    }
}

/// Clause 8.4.2.3. Unweighted is weight 1, denominator 0, offset 0, not a separate path.
#[derive(Debug, Clone, Copy)]
pub struct Weighting {
    pub denominator: u32,
    pub weight: [i32; 2],
    pub offset: [i32; 2],
}

impl Default for Weighting {
    fn default() -> Self {
        Self {
            denominator: 0,
            weight: [1; 2],
            offset: [0; 2],
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn single(
    into: &mut [u8],
    at: usize,
    stride: usize,
    from: &[u8],
    width: usize,
    height: usize,
    weighting: &Weighting,
    list: usize,
) {
    let denominator = weighting.denominator;
    let weight = weighting.weight[list];
    let offset = weighting.offset[list];
    if denominator == 0 && weight == 1 && offset == 0 {
        for row in 0..height {
            let corner = at + row * stride;
            into[corner..corner + width].copy_from_slice(&from[row * width..row * width + width]);
        }
        return;
    }
    for row in 0..height {
        for column in 0..width {
            let sample = i32::from(from[row * width + column]);
            let value = if denominator >= 1 {
                ((sample * weight + (1 << (denominator - 1))) >> denominator) + offset
            } else {
                sample * weight + offset
            };
            into[at + row * stride + column] = clip(value) as u8;
        }
    }
}

/// Bi-prediction, weighted. Averaged after each is interpolated and rounded, as specified.
#[allow(clippy::too_many_arguments)]
pub fn both(
    into: &mut [u8],
    at: usize,
    stride: usize,
    zero: &[u8],
    one: &[u8],
    width: usize,
    height: usize,
    weighting: &Weighting,
) {
    let denominator = weighting.denominator;
    let offset = (weighting.offset[0] + weighting.offset[1] + 1) >> 1;
    if denominator == 0 && weighting.weight == [1, 1] && offset == 0 {
        for row in 0..height {
            let corner = at + row * stride;
            for column in 0..width {
                let a = u32::from(zero[row * width + column]);
                let b = u32::from(one[row * width + column]);
                into[corner + column] = ((a + b + 1) >> 1) as u8;
            }
        }
        return;
    }
    for row in 0..height {
        for column in 0..width {
            let a = i32::from(zero[row * width + column]);
            let b = i32::from(one[row * width + column]);
            let value = ((a * weighting.weight[0] + b * weighting.weight[1] + (1 << denominator))
                >> (denominator + 1))
                + offset;
            into[at + row * stride + column] = clip(value) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAPS: [i32; 6] = [1, -5, 20, 20, -5, 1];

    fn picture(width: usize, height: usize) -> Vec<u8> {
        (0..width * height)
            .map(|at| ((at * 13 + (at / width) * 29) % 251) as u8)
            .collect()
    }

    fn view(data: &[u8], width: usize, height: usize) -> View<'_> {
        View {
            data,
            at: 0,
            stride: width,
            width,
            height,
        }
    }

    #[test]
    fn a_whole_sample_vector_copies_the_reference() {
        let data = picture(32, 32);
        let view = view(&data, 32, 32);
        let mut out = [0u8; 64];
        luma(&mut out, 8, 8, &view, 4 * 3, 4 * 5, &mut Scratch::default());
        for row in 0..8 {
            for column in 0..8 {
                assert_eq!(out[row * 8 + column], data[(5 + row) * 32 + 3 + column]);
            }
        }
    }

    #[test]
    fn the_half_sample_is_the_six_tap_filter_of_equation_8_249() {
        let data = picture(32, 32);
        let view = view(&data, 32, 32);
        let mut out = [0u8; 64];
        luma(
            &mut out,
            8,
            8,
            &view,
            4 * 4 + 2,
            4 * 6,
            &mut Scratch::default(),
        );
        for row in 0..8i32 {
            for column in 0..8i32 {
                let sum: i32 = (0..6)
                    .map(|tap| {
                        i32::from(data[((6 + row) * 32 + 4 + column + tap - 2) as usize])
                            * TAPS[tap as usize]
                    })
                    .sum();
                assert_eq!(
                    i32::from(out[(row * 8 + column) as usize]),
                    ((sum + 16) >> 5).clamp(0, 255)
                );
            }
        }
    }

    #[test]
    fn a_flat_reference_predicts_flat_at_every_one_of_the_sixteen_positions() {
        let data = vec![91u8; 32 * 32];
        let view = view(&data, 32, 32);
        let mut out = [0u8; 64];
        for y in 0..4 {
            for x in 0..4 {
                luma(
                    &mut out,
                    8,
                    8,
                    &view,
                    4 * 8 + x,
                    4 * 8 + y,
                    &mut Scratch::default(),
                );
                assert!(out.iter().all(|&value| value == 91), "at {x}, {y}");
            }
        }
        let mut out = [0u8; 16];
        for y in 0..8 {
            for x in 0..8 {
                chroma(&mut out, 4, 4, &view, 8 * 5 + x, 8 * 5 + y);
                assert!(out.iter().all(|&value| value == 91), "chroma at {x}, {y}");
            }
        }
    }

    #[test]
    fn a_vector_outside_the_picture_repeats_its_edge() {
        let data = picture(16, 16);
        let view = view(&data, 16, 16);
        let mut out = [0u8; 16];
        luma(
            &mut out,
            4,
            4,
            &view,
            -4 * 40,
            -4 * 40,
            &mut Scratch::default(),
        );
        assert!(out.iter().all(|&value| value == data[0]));
        luma(
            &mut out,
            4,
            4,
            &view,
            4 * 400,
            4 * 400,
            &mut Scratch::default(),
        );
        assert!(out.iter().all(|&value| value == data[16 * 16 - 1]));
    }

    #[test]
    fn a_field_of_a_frame_is_the_same_samples_at_twice_the_stride() {
        let data = picture(16, 16);
        let bottom = View {
            data: &data,
            at: 16,
            stride: 32,
            width: 16,
            height: 8,
        };
        let mut out = [0u8; 16];
        luma(&mut out, 4, 4, &bottom, 0, 0, &mut Scratch::default());
        for row in 0..4 {
            for column in 0..4 {
                assert_eq!(out[row * 4 + column], data[(2 * row + 1) * 16 + column]);
            }
        }
    }

    #[test]
    fn the_chroma_filter_is_the_bilinear_one_of_equation_8_266() {
        let data = picture(16, 16);
        let view = view(&data, 16, 16);
        let mut out = [0u8; 16];
        luma(&mut out, 4, 4, &view, 0, 0, &mut Scratch::default());
        let (x, y) = (8 * 3 + 5, 8 * 4 + 2);
        chroma(&mut out, 4, 4, &view, x, y);
        for row in 0..4usize {
            for column in 0..4usize {
                let a = i32::from(data[(4 + row) * 16 + 3 + column]);
                let b = i32::from(data[(4 + row) * 16 + 4 + column]);
                let c = i32::from(data[(5 + row) * 16 + 3 + column]);
                let d = i32::from(data[(5 + row) * 16 + 4 + column]);
                let wanted =
                    ((8 - 5) * (8 - 2) * a + 5 * (8 - 2) * b + (8 - 5) * 2 * c + 5 * 2 * d + 32)
                        >> 6;
                assert_eq!(i32::from(out[row * 4 + column]), wanted);
            }
        }
    }

    #[test]
    fn an_unweighted_prediction_is_a_copy_and_an_unweighted_pair_is_their_mean() {
        let weighting = Weighting::default();
        let from = [10u8, 20, 30, 40];
        let other = [11u8, 23, 30, 41];
        let mut into = [0u8; 4];
        single(&mut into, 0, 2, &from, 2, 2, &weighting, 0);
        assert_eq!(into, from);
        both(&mut into, 0, 2, &from, &other, 2, 2, &weighting);
        assert_eq!(into, [11, 22, 30, 41]);
    }

    #[test]
    fn a_weight_and_an_offset_are_applied_as_clause_8_4_2_3_2_writes_them() {
        let weighting = Weighting {
            denominator: 5,
            weight: [40, 24],
            offset: [-3, 7],
        };
        let from = [100u8; 4];
        let other = [200u8; 4];
        let mut into = [0u8; 4];
        single(&mut into, 0, 2, &from, 2, 2, &weighting, 0);
        assert_eq!(into[0], (((100 * 40 + 16) >> 5) - 3) as u8);
        single(&mut into, 0, 2, &other, 2, 2, &weighting, 1);
        assert_eq!(into[0], (((200 * 24 + 16) >> 5) + 7) as u8);
        both(&mut into, 0, 2, &from, &other, 2, 2, &weighting);
        assert_eq!(
            into[0],
            (((100 * 40 + 200 * 24 + 32) >> 6) + ((-3 + 7 + 1) >> 1)) as u8
        );
    }
}
