// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Scaling and the inverse transforms, clause 8.6. Exact to the bit; no floating point.
//! The four DCT sizes are one 32-point matrix read at a stride; the 4-point DST is for intra luma.

use super::tables::{DCT, DST, LEVEL_SCALE};

const MOST: usize = 32 * 32;

/// `CoeffMinY` and `CoeffMaxY` of clause 7.4.3.2.1.
const COEFF_MIN: i32 = -(1 << 15);
const COEFF_MAX: i32 = (1 << 15) - 1;

/// Clause 8.6.3: undoes the quantisation, in place. `matrix` is `None` for a flat
/// weighting: no scaling lists, or a skipped transform above 4x4.
pub fn scale(coefficients: &mut [i32], log2: usize, qp: i32, depth: u8, matrix: Option<&[u8]>) {
    let side = 1usize << log2;
    let shift = i32::from(depth) + log2 as i32 - 5;
    let round = 1i32 << (shift - 1);
    let level = LEVEL_SCALE[(qp % 6) as usize];
    let step = qp / 6;
    for y in 0..side {
        for x in 0..side {
            let at = x + y * side;
            let weight = match matrix {
                Some(matrix) => i32::from(matrix[at]),
                None => 16,
            };
            let value = coefficients[at]
                .saturating_mul(weight)
                .saturating_mul(level)
                .saturating_mul(1 << step);
            coefficients[at] = ((value.saturating_add(round)) >> shift).clamp(COEFF_MIN, COEFF_MAX);
        }
    }
}

/// Clause 8.6.4.2: one pass over `lines` lists of `side` values.
/// `terms` is how many of each list may be non-zero; the rest are skipped.
#[allow(clippy::too_many_arguments)]
fn pass(
    input: &[i32],
    output: &mut [i32],
    side: usize,
    terms: usize,
    lines: usize,
    dst: bool,
    shift: i32,
    clip: bool,
) {
    let round = 1i32 << (shift - 1);
    let step = 32 / side;
    for line in 0..lines {
        let source = &input[line * side..line * side + terms];
        let into = &mut output[line * side..line * side + side];
        for (i, slot) in into.iter_mut().enumerate() {
            let mut sum = 0i32;
            if dst {
                for (j, &value) in source.iter().enumerate() {
                    sum += DST[j][i] * value;
                }
            } else {
                for (j, &value) in source.iter().enumerate() {
                    sum += DCT[j * step][i] * value;
                }
            }
            let value = (sum + round) >> shift;
            *slot = if clip {
                value.clamp(COEFF_MIN, COEFF_MAX)
            } else {
                value
            };
        }
    }
}

fn transpose(block: &mut [i32], side: usize) {
    for y in 0..side {
        for x in (y + 1)..side {
            block.swap(x + y * side, y + x * side);
        }
    }
}

/// Clause 8.6.4.1: the two-dimensional inverse transform, in place.
/// Columns first; the intermediate is shifted by seven and clipped to stay in sixteen bits.
pub fn inverse(coefficients: &mut [i32], log2: usize, dst: bool, depth: u8) {
    let side = 1usize << log2;
    let count = side * side;
    // Past this everything is nought, in the input and the intermediate, so both passes stop early.
    let mut columns = 0usize;
    let mut rows = 0usize;
    for y in 0..side {
        for x in 0..side {
            if coefficients[x + y * side] != 0 {
                columns = columns.max(x + 1);
                rows = rows.max(y + 1);
            }
        }
    }
    if columns == 0 {
        coefficients[..count].fill(0);
        return;
    }
    let mut scratch = [0i32; MOST];
    // A pass runs along rows, so the block is transposed around the column pass.
    transpose(&mut coefficients[..count], side);
    pass(
        &coefficients[..count],
        &mut scratch[..count],
        side,
        rows,
        columns,
        dst,
        7,
        true,
    );
    scratch[columns * side..count].fill(0);
    transpose(&mut scratch[..count], side);
    // No clip after the second pass: equation 8-314 clips between passes, 8-299 does not.
    let shift = 20 - i32::from(depth);
    pass(
        &scratch[..count],
        &mut coefficients[..count],
        side,
        columns,
        side,
        dst,
        shift,
        false,
    );
}

/// Equation 8-298: a skipped transform shifts the coefficients up and back down like a transform.
pub fn skipped(coefficients: &mut [i32], log2: usize, depth: u8) {
    let count = 1usize << (2 * log2);
    let up = 5 + log2 as i32;
    let down = 20 - i32::from(depth);
    let round = 1i32 << (down - 1);
    for value in &mut coefficients[..count] {
        *value = ((*value << up) + round) >> down;
    }
}

#[cfg(test)]
mod tests {
    use super::{DCT, DST, inverse, skipped};

    /// The standard's integers are nudged rounded cosines, hence the tolerance of one.
    #[test]
    fn the_cosine_matrix_is_the_cosines_of_the_defining_equation() {
        let scale = 64.0 * std::f64::consts::SQRT_2;
        for (k, basis) in DCT.iter().enumerate() {
            for (n, &value) in basis.iter().enumerate() {
                let angle = (2.0 * n as f64 + 1.0) * k as f64 * std::f64::consts::PI / 64.0;
                let wanted = if k == 0 {
                    64.0
                } else {
                    (scale * angle.cos()).round()
                };
                assert!(
                    (f64::from(value) - wanted).abs() <= 1.0,
                    "basis {k} at {n}: {value} against {wanted}"
                );
            }
        }
        for (a, first) in DCT.iter().enumerate() {
            let norm: i64 = first.iter().map(|&value| i64::from(value).pow(2)).sum();
            assert!((norm - 131072).abs() < 1024, "basis {a} has length {norm}");
            for (b, second) in DCT.iter().enumerate().skip(a + 1) {
                let overlap: i64 = first
                    .iter()
                    .zip(second)
                    .map(|(&one, &two)| i64::from(one) * i64::from(two))
                    .sum();
                assert!(
                    overlap.abs() < 512,
                    "bases {a} and {b} overlap by {overlap}"
                );
            }
        }
    }

    #[test]
    fn the_sine_matrix_is_the_sines_of_the_defining_equation() {
        for (k, basis) in DST.iter().enumerate() {
            for (n, &value) in basis.iter().enumerate() {
                let angle = (2.0 * k as f64 + 1.0) * (n as f64 + 1.0) * std::f64::consts::PI / 9.0;
                let wanted = (256.0 / 3.0 * angle.sin()).round();
                assert_eq!(f64::from(value), wanted, "basis {k} at {n}");
            }
        }
    }

    #[test]
    fn the_inverse_undoes_a_forward_transform_written_from_the_same_matrices() {
        for (log2, dst) in [
            (2usize, true),
            (2, false),
            (3, false),
            (4, false),
            (5, false),
        ] {
            let side = 1usize << log2;
            let step = 32 / side;
            let weight = |k: usize, n: usize| -> i64 {
                if dst {
                    i64::from(DST[k][n])
                } else {
                    i64::from(DCT[k * step][n])
                }
            };
            let block: Vec<i32> = (0..side * side)
                .map(|at| ((at as i32 * 37) % 61) - 30)
                .collect();
            let mut columns = vec![0i64; side * side];
            for x in 0..side {
                for k in 0..side {
                    columns[x + k * side] = (0..side)
                        .map(|y| weight(k, y) * i64::from(block[x + y * side]))
                        .sum();
                }
            }
            let mut coefficients = vec![0i32; 1024];
            let shift = 2 * log2 + 5;
            for y in 0..side {
                for k in 0..side {
                    let sum: i64 = (0..side)
                        .map(|x| weight(k, x) * columns[x + y * side])
                        .sum();
                    coefficients[k + y * side] = (sum >> shift) as i32;
                }
            }
            inverse(&mut coefficients, log2, dst, 8);
            let slack = 2 + side as i32 / 8;
            for (at, &value) in block.iter().enumerate() {
                let got = coefficients[at];
                assert!(
                    (got - value).abs() <= slack,
                    "size {side}, sine {dst}, at {at}: {got} against {value}"
                );
            }
        }
    }

    #[test]
    fn a_skipped_transform_only_scales() {
        let mut eight = vec![0i32; 1024];
        eight[0] = 64;
        eight[5] = -64;
        skipped(&mut eight, 2, 8);
        assert_eq!(eight[0], ((64i32 << 7) + (1 << 11)) >> 12);
        assert_eq!(eight[0], 2);
        assert_eq!(eight[5], -2);
        let mut ten = vec![0i32; 1024];
        ten[0] = 64;
        skipped(&mut ten, 2, 10);
        assert_eq!(ten[0], 8);
        let mut wide = vec![0i32; 1024];
        wide[0] = 64;
        skipped(&mut wide, 3, 8);
        assert_eq!(wide[0], 4);
    }

    #[test]
    fn a_transform_of_nothing_is_nothing() {
        let mut coefficients = vec![7i32; 1024];
        coefficients[..64].fill(0);
        inverse(&mut coefficients, 3, false, 10);
        assert!(coefficients[..64].iter().all(|&value| value == 0));
    }
}
