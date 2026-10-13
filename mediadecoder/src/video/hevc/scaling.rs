// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Quantiser matrices as a platform decoder wants them: the list the bitstream sent, in raster
//! (VA-API, V4L2) or the clause 6.5.3 diagonal scan (DXVA, NVDEC). A wrong order is no error,
//! just a nearly-right picture. The DC of 16x16/32x32 is lost; drivers take `ScalingListDC`.

use super::params::Scaling;
use super::tables;

const fn raster<const N: usize>(scan: &[[u8; 2]; N], side: usize) -> [usize; N] {
    let mut out = [0usize; N];
    let mut at = 0;
    while at < N {
        out[at] = scan[at][1] as usize * side + scan[at][0] as usize;
        at += 1;
    }
    out
}

/// Clause 6.5.3 over a 4x4 block: where the `i`th value sent belongs in the raster.
pub const DIAGONAL_4: [usize; 16] = raster(&tables::SCAN_4[0], 4);

/// The same for the 8x8 that every list above 4x4 is made of.
pub const DIAGONAL_8: [usize; 64] = raster(&tables::SCAN_8[0], 8);

pub fn list4(scaling: &Scaling, matrix: usize) -> [u8; 16] {
    let source = scaling.matrix(0, matrix);
    std::array::from_fn(|at| source[at])
}

/// One list of sixty-four, as the raster of the 8x8 the matrix repeats. `size` is `sizeId`.
pub fn list(scaling: &Scaling, size: usize, matrix: usize) -> [u8; 64] {
    let side = 4usize << size;
    let repeat = side / 8;
    let source = scaling.matrix(size, matrix);
    std::array::from_fn(|at| source[(at % 8) * repeat + (at / 8) * repeat * side])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagonal_scan_is_the_one_the_standard_writes() {
        assert_eq!(&DIAGONAL_4[..6], &[0, 4, 1, 8, 5, 2]);
        assert_eq!(DIAGONAL_4[15], 15);
        assert_eq!(&DIAGONAL_8[..3], &[0, 8, 1]);
        assert_eq!(DIAGONAL_8[63], 63);
        let mut seen = DIAGONAL_8.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 64);
    }

    #[test]
    fn a_list_above_eight_is_read_out_of_the_corner_of_each_repeated_block() {
        let scaling = Scaling::default();
        for size in 1..4 {
            let matrix = if size == 3 { 0 } else { 1 };
            let list = list(&scaling, size, matrix);
            let side = 4usize << size;
            let repeat = side / 8;
            let source = scaling.matrix(size, matrix);
            for y in 0..8 {
                for x in 0..8 {
                    assert_eq!(list[y * 8 + x], source[y * repeat * side + x * repeat]);
                }
            }
        }
    }
}
