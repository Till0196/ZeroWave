// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 逆 DCT。H.262 Annex A は IEEE 1180-1990 の精度だけを求めるので、正しいデコーダー同士でも
//! 一、二ずれる（映像は SNR で比べる）。下のテストがその適合手順。f32 の偶奇分解で計算する。

/// `cos(k * pi / 16)`.
const C1: f32 = 0.980_785_25;
const C2: f32 = 0.923_879_5;
const C3: f32 = 0.831_469_6;
const C4: f32 = 0.707_106_77;
const C5: f32 = 0.555_570_25;
const C6: f32 = 0.382_683_43;
const C7: f32 = 0.195_090_32;

/// `1 / (2 * sqrt(2))`.
const SCALE_DC: f32 = 0.353_553_4;
const SCALE_AC: f32 = 0.5;

/// One 1-D inverse transform of eight; the definition's scaling is done here.
#[inline]
fn line(input: [f32; 8], output: &mut [f32; 8]) {
    let a0 = input[0] * SCALE_DC;
    let a1 = input[1] * SCALE_AC;
    let a2 = input[2] * SCALE_AC;
    let a3 = input[3] * SCALE_AC;
    let a4 = input[4] * SCALE_AC;
    let a5 = input[5] * SCALE_AC;
    let a6 = input[6] * SCALE_AC;
    let a7 = input[7] * SCALE_AC;

    let p = a0 + C4 * a4;
    let q = a0 - C4 * a4;
    let r = C2 * a2 + C6 * a6;
    let s = C6 * a2 - C2 * a6;
    let even = [p + r, q + s, q - s, p - r];

    let odd = [
        C1 * a1 + C3 * a3 + C5 * a5 + C7 * a7,
        C3 * a1 - C7 * a3 - C1 * a5 - C5 * a7,
        C5 * a1 - C1 * a3 + C7 * a5 + C3 * a7,
        C7 * a1 - C5 * a3 + C3 * a5 - C1 * a7,
    ];

    // Even part is symmetric and odd part antisymmetric about the middle.
    for at in 0..4 {
        output[at] = even[at] + odd[at];
        output[7 - at] = even[at] - odd[at];
    }
}

/// Inverse transform of one block in place: raster coefficients in, saturated samples out.
pub fn transform(block: &mut [i16; 64]) {
    let mut middle = [0f32; 64];
    for row in 0..8 {
        let at = row * 8;
        let mut input = [0f32; 8];
        for (value, &coefficient) in input.iter_mut().zip(&block[at..at + 8]) {
            *value = f32::from(coefficient);
        }
        let mut output = [0f32; 8];
        line(input, &mut output);
        middle[at..at + 8].copy_from_slice(&output);
    }
    for column in 0..8 {
        let mut input = [0f32; 8];
        for (row, value) in input.iter_mut().enumerate() {
            *value = middle[row * 8 + column];
        }
        let mut output = [0f32; 8];
        line(input, &mut output);
        for (row, &value) in output.iter().enumerate() {
            // Round half away from zero, as the conformance procedure expects.
            block[row * 8 + column] = round(value);
        }
    }
}

fn round(value: f32) -> i16 {
    let rounded = if value < 0.0 {
        (value - 0.5) as i32
    } else {
        (value + 0.5) as i32
    };
    rounded.clamp(-256, 255) as i16
}

#[cfg(test)]
mod tests {
    use super::transform;

    /// `cos((2x + 1) u pi / 16)`, scaled for a 1-D transform.
    fn matrix() -> [[f64; 8]; 8] {
        let mut out = [[0f64; 8]; 8];
        for (x, row) in out.iter_mut().enumerate() {
            for (u, value) in row.iter_mut().enumerate() {
                let scale = if u == 0 {
                    std::f64::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                *value =
                    scale * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos() / 2.0;
            }
        }
        out
    }

    /// IEEE 1180 reference inverse transform in double precision.
    fn reference(block: &[i16; 64], matrix: &[[f64; 8]; 8]) -> [i16; 64] {
        let mut middle = [0f64; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut sum = 0f64;
                for u in 0..8 {
                    sum += matrix[x][u] * f64::from(block[y * 8 + u]);
                }
                middle[y * 8 + x] = sum;
            }
        }
        let mut out = [0i16; 64];
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0f64;
                for v in 0..8 {
                    sum += matrix[y][v] * middle[v * 8 + x];
                }
                out[y * 8 + x] = sum.round().clamp(-256.0, 255.0) as i16;
            }
        }
        out
    }

    /// Forward transform for making test blocks, held to twelve bits.
    fn forward(samples: &[i16; 64], matrix: &[[f64; 8]; 8]) -> [i16; 64] {
        let mut middle = [0f64; 64];
        for y in 0..8 {
            for u in 0..8 {
                let mut sum = 0f64;
                for x in 0..8 {
                    sum += matrix[x][u] * f64::from(samples[y * 8 + x]);
                }
                middle[y * 8 + u] = sum;
            }
        }
        let mut out = [0i16; 64];
        for u in 0..8 {
            for v in 0..8 {
                let mut sum = 0f64;
                for y in 0..8 {
                    sum += matrix[y][v] * middle[y * 8 + u];
                }
                out[v * 8 + u] = sum.round().clamp(-2048.0, 2047.0) as i16;
            }
        }
        out
    }

    /// A fixed-seed xorshift; IEEE 1180 only asks for uniform pseudo-random data.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn sample(&mut self, range: i32) -> i16 {
            let span = (2 * range + 1) as u64;
            ((self.next() % span) as i32 - range) as i16
        }
    }

    struct Error {
        peak: i32,
        sum: [f64; 64],
        squares: [f64; 64],
        blocks: f64,
    }

    impl Default for Error {
        fn default() -> Self {
            Self {
                peak: 0,
                sum: [0.0; 64],
                squares: [0.0; 64],
                blocks: 0.0,
            }
        }
    }

    impl Error {
        fn add(&mut self, ours: &[i16; 64], theirs: &[i16; 64]) {
            for at in 0..64 {
                let difference = i32::from(ours[at]) - i32::from(theirs[at]);
                self.peak = self.peak.max(difference.abs());
                self.sum[at] += f64::from(difference);
                self.squares[at] += f64::from(difference) * f64::from(difference);
            }
            self.blocks += 1.0;
        }

        fn measures(&self) -> (f64, f64, f64, f64) {
            let per_place_square = (0..64)
                .map(|at| self.squares[at] / self.blocks)
                .fold(0f64, f64::max);
            let overall_square = self.squares.iter().sum::<f64>() / (self.blocks * 64.0);
            let per_place_mean = (0..64)
                .map(|at| (self.sum[at] / self.blocks).abs())
                .fold(0f64, f64::max);
            let overall_mean = (self.sum.iter().sum::<f64>() / (self.blocks * 64.0)).abs();
            (
                per_place_square,
                overall_square,
                per_place_mean,
                overall_mean,
            )
        }
    }

    fn run(range: i32, negate: bool, seed: u64) -> Error {
        let mut random = Random(seed);
        let mut error = Error::default();
        let matrix = matrix();
        for _ in 0..10_000 {
            let mut samples = [0i16; 64];
            for sample in &mut samples {
                let value = random.sample(range);
                *sample = if negate { -value } else { value };
            }
            let coefficients = forward(&samples, &matrix);
            let theirs = reference(&coefficients, &matrix);
            let mut ours = coefficients;
            transform(&mut ours);
            error.add(&ours, &theirs);
        }
        error
    }

    #[test]
    fn the_transform_meets_ieee_1180() {
        for (range, negate, seed) in [
            (256, false, 0x2545_f491_4f6c_dd1d),
            (256, true, 0x9e37_79b9_7f4a_7c15),
            (5, false, 0x0123_4567_89ab_cdef),
            (5, true, 0xdead_beef_cafe_babe),
            (300, false, 0x1234_5678_9abc_def0),
            (300, true, 0xfedc_ba98_7654_3210),
        ] {
            let error = run(range, negate, seed);
            let (place_square, square, place_mean, mean) = error.measures();
            tracing::warn!(
                "IEEE 1180 range {range} negate {negate}: ppe {} pmse {place_square:.5} \
                 omse {square:.5} pme {place_mean:.5} ome {mean:.6}",
                error.peak
            );
            assert!(error.peak <= 1, "peak error {} exceeds one", error.peak);
            assert!(
                place_square <= 0.06,
                "mean square error at one place {place_square}"
            );
            assert!(square <= 0.02, "mean square error over the block {square}");
            assert!(place_mean <= 0.015, "mean error at one place {place_mean}");
            assert!(mean <= 0.0015, "mean error over the block {mean}");
        }
    }

    #[test]
    fn a_block_of_nothing_transforms_to_nothing() {
        let mut block = [0i16; 64];
        transform(&mut block);
        assert_eq!(block, [0i16; 64]);
    }

    #[test]
    fn a_direct_current_coefficient_alone_gives_a_flat_block() {
        // A coded 1024 DC alone gives a flat 128.
        let mut block = [0i16; 64];
        block[0] = 1024;
        transform(&mut block);
        assert_eq!(block, [128i16; 64]);
    }
}
