// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Inverse quantisation and the inverse transforms of clause 8.5: 4x4, 8x8, and the 4x4 and
//! 2x2 Hadamard for DC. Written straight from the equations, same order and shifts.

use super::params::Weights;
use super::tables::{NORMALISE_4X4, NORMALISE_8X8};

#[derive(Debug, Clone, Copy)]
pub struct Scale {
    pub four: [[i32; 16]; 6],
    pub eight: [[i32; 64]; 6],
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            four: [[16; 16]; 6],
            eight: [[16; 64]; 6],
        }
    }
}

impl Scale {
    /// Equations 8-313 and 8-316.
    pub fn new(weights: &Weights, four: usize, eight: usize) -> Self {
        let mut scale = Self {
            four: [[0; 16]; 6],
            eight: [[0; 64]; 6],
        };
        for m in 0..6 {
            for at in 0..16 {
                let (i, j) = (at / 4, at % 4);
                let class = match (i % 2, j % 2) {
                    (0, 0) => 0,
                    (1, 1) => 1,
                    _ => 2,
                };
                scale.four[m][at] =
                    i32::from(weights.four[four][at]) * i32::from(NORMALISE_4X4[m][class]);
            }
            for ((into, &weight), &normalise) in scale.eight[m]
                .iter_mut()
                .zip(&weights.eight[eight])
                .zip(&NORMALISE_8X8[m])
            {
                *into = i32::from(weight) * i32::from(normalise);
            }
        }
        scale
    }
}

/// Equations 8-336 and 8-337. `keep_dc`: the DC was already scaled (16x16 intra, chroma).
pub fn scale_4x4(block: &mut [i32; 16], scale: &[i32; 16], qp: i32, keep_dc: bool) {
    let shift = qp / 6;
    let first = usize::from(keep_dc);
    if qp >= 24 {
        for at in first..16 {
            block[at] = (block[at] * scale[at]) << (shift - 4);
        }
    } else {
        let round = 1 << (3 - shift);
        for at in first..16 {
            block[at] = (block[at] * scale[at] + round) >> (4 - shift);
        }
    }
}

/// Equations 8-356 and 8-357.
pub fn scale_8x8(block: &mut [i32; 64], scale: &[i32; 64], qp: i32) {
    let shift = qp / 6;
    if qp >= 36 {
        for at in 0..64 {
            block[at] = (block[at] * scale[at]) << (shift - 6);
        }
    } else {
        let round = 1 << (5 - shift);
        for at in 0..64 {
            block[at] = (block[at] * scale[at] + round) >> (6 - shift);
        }
    }
}

/// Clause 8.5.12.2, rows then columns, with the rounding of equation 8-354.
pub fn transform_4x4(block: &mut [i32; 16]) {
    for row in 0..4 {
        let at = row * 4;
        let out = four(block[at..at + 4].try_into().expect("four of them"));
        block[at..at + 4].copy_from_slice(&out);
    }
    for column in 0..4 {
        let out = four(std::array::from_fn(|row| block[row * 4 + column]));
        for row in 0..4 {
            block[column + row * 4] = (out[row] + 32) >> 6;
        }
    }
}

pub(super) fn four(d: [i32; 4]) -> [i32; 4] {
    let e = [
        d[0] + d[2],
        d[0] - d[2],
        (d[1] >> 1) - d[3],
        d[1] + (d[3] >> 1),
    ];
    [e[0] + e[3], e[1] + e[2], e[1] - e[2], e[0] - e[3]]
}

pub(super) fn eight(d: [i32; 8]) -> [i32; 8] {
    let e = [
        d[0] + d[4],
        -d[3] + d[5] - d[7] - (d[7] >> 1),
        d[0] - d[4],
        d[1] + d[7] - d[3] - (d[3] >> 1),
        (d[2] >> 1) - d[6],
        -d[1] + d[7] + d[5] + (d[5] >> 1),
        d[2] + (d[6] >> 1),
        d[3] + d[5] + d[1] + (d[1] >> 1),
    ];
    let f = [
        e[0] + e[6],
        e[1] + (e[7] >> 2),
        e[2] + e[4],
        e[3] + (e[5] >> 2),
        e[2] - e[4],
        (e[3] >> 2) - e[5],
        e[0] - e[6],
        e[7] - (e[1] >> 2),
    ];
    [
        f[0] + f[7],
        f[2] + f[5],
        f[4] + f[3],
        f[6] + f[1],
        f[6] - f[1],
        f[4] - f[3],
        f[2] - f[5],
        f[0] - f[7],
    ]
}

/// Clause 8.5.13.2.
pub fn transform_8x8(block: &mut [i32; 64]) {
    for row in 0..8 {
        let at = row * 8;
        let out = eight(block[at..at + 8].try_into().expect("eight of them"));
        block[at..at + 8].copy_from_slice(&out);
    }
    for column in 0..8 {
        let d = std::array::from_fn(|row| block[row * 8 + column]);
        let out = eight(d);
        for row in 0..8 {
            block[row * 8 + column] = (out[row] + 32) >> 6;
        }
    }
}

fn hadamard(d: [i32; 4]) -> [i32; 4] {
    let e = [d[0] + d[1], d[0] - d[1], d[2] + d[3], d[2] - d[3]];
    [e[0] + e[2], e[0] - e[2], e[1] - e[3], e[1] + e[3]]
}

/// Equations 8-320 to 8-322: the 16 DC coefficients of an Intra16x16 macroblock.
pub fn luma_dc(block: &mut [i32; 16], scale: i32, qp: i32) {
    for row in 0..4 {
        let at = row * 4;
        let out = hadamard(block[at..at + 4].try_into().expect("four of them"));
        block[at..at + 4].copy_from_slice(&out);
    }
    let shift = qp / 6;
    for column in 0..4 {
        let out = hadamard(std::array::from_fn(|row| block[row * 4 + column]));
        for row in 0..4 {
            block[column + row * 4] = if qp >= 36 {
                (out[row] * scale) << (shift - 6)
            } else {
                (out[row] * scale + (1 << (5 - shift))) >> (6 - shift)
            };
        }
    }
}

/// Equations 8-324 and 8-326: 4:2:0 chroma DC. Unlike luma, no rounding.
pub fn chroma_dc(block: &mut [i32; 4], scale: i32, qp: i32) {
    let f = [
        block[0] + block[1] + block[2] + block[3],
        block[0] - block[1] + block[2] - block[3],
        block[0] + block[1] - block[2] - block[3],
        block[0] - block[1] - block[2] + block[3],
    ];
    for at in 0..4 {
        block[at] = ((f[at] * scale) << (qp / 6)) >> 5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIS_4: [[f64; 4]; 4] = [
        [1.0, 1.0, 1.0, 1.0],
        [1.0, 0.5, -0.5, -1.0],
        [1.0, -1.0, -1.0, 1.0],
        [0.5, -1.0, 1.0, -0.5],
    ];

    const BASIS_8: [[f64; 8]; 8] = [
        [8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0],
        [12.0, 10.0, 6.0, 3.0, -3.0, -6.0, -10.0, -12.0],
        [8.0, 4.0, -4.0, -8.0, -8.0, -4.0, 4.0, 8.0],
        [10.0, -3.0, -12.0, -6.0, 6.0, 12.0, 3.0, -10.0],
        [8.0, -8.0, -8.0, 8.0, 8.0, -8.0, -8.0, 8.0],
        [6.0, -12.0, 3.0, 10.0, -10.0, -3.0, 12.0, -6.0],
        [4.0, -8.0, 8.0, -4.0, -4.0, 8.0, -8.0, 4.0],
        [3.0, -6.0, 10.0, -12.0, 12.0, -10.0, 6.0, -3.0],
    ];

    fn pass(d: &[i32], basis: &[&[f64]]) -> Vec<f64> {
        (0..d.len())
            .map(|x| (0..d.len()).map(|k| f64::from(d[k]) * basis[k][x]).sum())
            .collect()
    }

    fn awkward(count: usize, step: i32) -> Vec<i32> {
        (0..count)
            .map(|at| step * (((at as i32 * 37 + 11) % 23) - 11))
            .collect()
    }

    #[test]
    fn one_pass_of_the_4x4_transform_is_its_defining_matrix_product() {
        let basis: Vec<&[f64]> = BASIS_4.iter().map(|row| &row[..]).collect();
        for step in [2, 4, 16, -2] {
            let source = awkward(4, step);
            let wanted = pass(&source, &basis);
            let got = four(source.try_into().expect("four of them"));
            for (at, (&got, &want)) in got.iter().zip(&wanted).enumerate() {
                assert_eq!(f64::from(got), want, "sample {at} at step {step}");
            }
        }
    }

    #[test]
    fn one_pass_of_the_8x8_transform_is_its_defining_matrix_product() {
        let basis: Vec<&[f64]> = BASIS_8.iter().map(|row| &row[..]).collect();
        for step in [8, 16, -8] {
            let source = awkward(8, step);
            let wanted = pass(&source, &basis);
            let got = eight(source.try_into().expect("eight of them"));
            for (at, (&got, &want)) in got.iter().zip(&wanted).enumerate() {
                assert_eq!(f64::from(got), want / 8.0, "sample {at} at step {step}");
            }
        }
    }

    #[test]
    fn the_whole_transform_is_its_two_passes_and_the_final_rounding() {
        // Rows then columns; the other order truncates differently.
        let source = awkward(64, 3);
        let mut wanted: Vec<i32> = source.clone();
        for row in 0..8 {
            let out = eight(wanted[row * 8..row * 8 + 8].try_into().expect("eight"));
            wanted[row * 8..row * 8 + 8].copy_from_slice(&out);
        }
        for column in 0..8 {
            let out = eight(std::array::from_fn(|row| wanted[row * 8 + column]));
            for row in 0..8 {
                wanted[row * 8 + column] = (out[row] + 32) >> 6;
            }
        }
        let mut block: [i32; 64] = source.try_into().expect("sixty-four of them");
        transform_8x8(&mut block);
        assert_eq!(block.to_vec(), wanted);
    }

    #[test]
    fn a_flat_block_transforms_to_a_flat_block() {
        let mut block = [0i32; 16];
        block[0] = 64;
        transform_4x4(&mut block);
        assert_eq!(block, [1; 16]);

        let mut block = [0i32; 64];
        block[0] = 64;
        transform_8x8(&mut block);
        assert_eq!(block, [1; 64]);
    }

    #[test]
    fn the_direct_current_transforms_are_hadamard_transforms() {
        let mut block = [0i32; 16];
        block[0] = 16;
        luma_dc(&mut block, 16, 36);
        assert!(block.iter().all(|&value| value == block[0]));
        assert_ne!(block[0], 0);

        for coefficient in 1..16 {
            let mut block = [0i32; 16];
            block[coefficient] = 16;
            luma_dc(&mut block, 16, 36);
            assert_eq!(block.iter().sum::<i32>(), 0, "coefficient {coefficient}");
        }

        let mut block = [4i32, 0, 0, 0];
        chroma_dc(&mut block, 16, 30);
        assert!(block.iter().all(|&value| value == block[0]));
        for coefficient in 1..4 {
            let mut block = [0i32; 4];
            block[coefficient] = 64;
            chroma_dc(&mut block, 16, 30);
            assert_eq!(block.iter().sum::<i32>(), 0, "coefficient {coefficient}");
        }
    }

    #[test]
    fn a_quantiser_six_higher_doubles_what_a_coefficient_means() {
        let scale = Scale::default();
        for qp in 0..46 {
            let mut low = [0i32; 16];
            low[3] = 7;
            let mut high = low;
            scale_4x4(&mut low, &scale.four[(qp % 6) as usize], qp, false);
            scale_4x4(&mut high, &scale.four[(qp % 6) as usize], qp + 6, false);
            assert!((high[3] - low[3] * 2).abs() <= 1, "quantiser {qp}");
            if qp >= 24 {
                assert_eq!(high[3], low[3] * 2, "quantiser {qp}");
            }

            let mut low = [0i32; 64];
            low[9] = 7;
            let mut high = low;
            scale_8x8(&mut low, &scale.eight[(qp % 6) as usize], qp);
            scale_8x8(&mut high, &scale.eight[(qp % 6) as usize], qp + 6);
            assert!((high[9] - low[9] * 2).abs() <= 1, "quantiser {qp}");
            if qp >= 36 {
                assert_eq!(high[9], low[9] * 2, "quantiser {qp}");
            }
        }
    }
}
