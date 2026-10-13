// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 参照ピクチャーから予測を取り出す。フィールドはストライド二倍・高さ半分の [`View`] として
//! 扱い、フレーム予測と同じ処理を通す。

#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    pub data: &'a [u8],
    /// Where the first row begins (tells a bottom field from a top one).
    pub at: usize,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl View<'_> {
    fn row(&self, y: usize) -> &[u8] {
        let at = self.at + y * self.stride;
        &self.data[at..at + self.width]
    }
}

/// Where a prediction is written. Field prediction in a frame picture uses a
/// stride of twice the width.
pub struct Target<'a> {
    pub data: &'a mut [u8],
    pub at: usize,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// Whole-sample start of a block and whether the axis is at a half sample.
fn split(position: i32, span: usize, extent: usize) -> (usize, bool) {
    let half = position & 1 != 0;
    // Arithmetic shift, not division: -1 is half a sample above sample -1.
    let whole = position >> 1;
    // Out-of-picture vectors are forbidden; clamping keeps damage to one macroblock.
    let last = extent.saturating_sub(span + usize::from(half));
    (whole.clamp(0, last as i32) as usize, half)
}

/// Fills `target` from `view` at `x`, `y` in half samples, rounding up at a half (7.6.4).
pub fn fetch(target: &mut Target<'_>, view: &View<'_>, x: i32, y: i32) {
    let (left, half_x) = split(x, target.width, view.width);
    let (top, half_y) = split(y, target.height, view.height);
    for row in 0..target.height {
        let source = view.row(top + row);
        let below = view.row(top + row + usize::from(half_y));
        let at = target.at + row * target.stride;
        let width = target.width;
        let line = &mut target.data[at..at + width];
        match (half_x, half_y) {
            (false, false) => line.copy_from_slice(&source[left..left + width]),
            (true, false) => {
                for (column, sample) in line.iter_mut().enumerate() {
                    let a = u32::from(source[left + column]);
                    let b = u32::from(source[left + column + 1]);
                    *sample = ((a + b + 1) >> 1) as u8;
                }
            }
            (false, true) => {
                for (column, sample) in line.iter_mut().enumerate() {
                    let a = u32::from(source[left + column]);
                    let b = u32::from(below[left + column]);
                    *sample = ((a + b + 1) >> 1) as u8;
                }
            }
            (true, true) => {
                for (column, sample) in line.iter_mut().enumerate() {
                    let a = u32::from(source[left + column]);
                    let b = u32::from(source[left + column + 1]);
                    let c = u32::from(below[left + column]);
                    let d = u32::from(below[left + column + 1]);
                    *sample = ((a + b + c + d + 2) >> 2) as u8;
                }
            }
        }
    }
}

/// Averages a second prediction into one already fetched (B and dual prime),
/// after each has been rounded, as the standard says.
pub fn fetch_average(target: &mut Target<'_>, view: &View<'_>, x: i32, y: i32) {
    let mut samples = [0u8; 256];
    let mut second = Target {
        data: &mut samples,
        at: 0,
        stride: target.width,
        width: target.width,
        height: target.height,
    };
    fetch(&mut second, view, x, y);
    let width = target.width;
    for row in 0..target.height {
        let at = target.at + row * target.stride;
        let line = &mut target.data[at..at + width];
        let other = &samples[row * width..row * width + width];
        for (sample, &value) in line.iter_mut().zip(other) {
            *sample = ((u32::from(*sample) + u32::from(value) + 1) >> 1) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, View, fetch, fetch_average};

    /// Sample `(x, y)` is `16 * y + x`.
    fn ramp(width: usize, height: usize) -> Vec<u8> {
        (0..width * height)
            .map(|at| (16 * (at / width) + at % width) as u8)
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
    fn a_whole_sample_vector_copies_the_samples_it_points_at() {
        let data = ramp(16, 16);
        let mut block = [0u8; 256];
        let mut target = Target {
            data: &mut block,
            at: 0,
            stride: 8,
            width: 8,
            height: 2,
        };
        fetch(&mut target, &view(&data, 16, 16), 2 * 3, 2 * 4);
        assert_eq!(
            &block[0..8],
            &[0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a]
        );
        assert_eq!(
            &block[8..16],
            &[0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a]
        );
    }

    #[test]
    fn a_half_sample_vector_interpolates_and_rounds_up() {
        let data = ramp(16, 16);
        let mut block = [0u8; 256];
        let mut one = |x, y| {
            let mut target = Target {
                data: &mut block,
                at: 0,
                stride: 4,
                width: 4,
                height: 1,
            };
            fetch(&mut target, &view(&data, 16, 16), x, y);
            block[0..4].to_vec()
        };
        assert_eq!(one(1, 0), [1, 2, 3, 4]);
        assert_eq!(one(0, 1), [8, 9, 10, 11]);
        assert_eq!(one(1, 1), [9, 10, 11, 12]);
    }

    #[test]
    fn a_field_is_a_view_with_twice_the_stride() {
        let data = ramp(16, 16);
        let bottom = View {
            data: &data,
            at: 16,
            stride: 32,
            width: 16,
            height: 8,
        };
        let mut block = [0u8; 256];
        let mut target = Target {
            data: &mut block,
            at: 0,
            stride: 4,
            width: 4,
            height: 2,
        };
        fetch(&mut target, &bottom, 0, 0);
        // Rows one and three of the picture.
        assert_eq!(&block[0..4], &[0x10, 0x11, 0x12, 0x13]);
        assert_eq!(&block[4..8], &[0x30, 0x31, 0x32, 0x33]);
    }

    #[test]
    fn a_vector_that_reaches_outside_the_picture_is_held_inside_it() {
        let data = ramp(16, 16);
        let mut block = [0u8; 256];
        let mut one = |x, y| {
            let mut target = Target {
                data: &mut block,
                at: 0,
                stride: 4,
                width: 4,
                height: 1,
            };
            fetch(&mut target, &view(&data, 16, 16), x, y);
            block[0..4].to_vec()
        };
        assert_eq!(one(-100, -100), [0, 1, 2, 3]);
        assert_eq!(one(1000, 1000), [0xfc, 0xfd, 0xfe, 0xff]);
    }

    #[test]
    fn averaging_a_second_prediction_rounds_up_as_well() {
        let data = ramp(16, 16);
        let mut block = [0u8; 256];
        let mut target = Target {
            data: &mut block,
            at: 0,
            stride: 4,
            width: 4,
            height: 1,
        };
        fetch(&mut target, &view(&data, 16, 16), 0, 0);
        fetch_average(&mut target, &view(&data, 16, 16), 2, 0);
        assert_eq!(&block[0..4], &[1, 2, 3, 4]);
    }
}
