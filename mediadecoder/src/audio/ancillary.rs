// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `MPEG4_ancillary_data`：ダウンミックスの係数
//! （ISO/IEC 14496-3:2009/AMD 4、ARIB STD-B32 第 2 部 6.2.3(3)）。

use super::Mixdown;
use crate::bits::Reader;

const SYNC: u32 = 0xBC;
const SYNC2: u32 = 0xBD;

/// Reads one from a DSE payload. `None` where it is not one.
pub fn read(payload: &[u8]) -> Option<Mixdown> {
    let mut reader = Reader::new(payload);
    if reader.left() < 24 || reader.bits(8) != SYNC {
        return None;
    }
    let mut out = Mixdown::default();
    reader.skip(6);
    out.pseudo_surround = reader.bit() == 1;
    reader.skip(1);
    reader.skip(3);
    let levels = reader.bit() == 1;
    let extension = reader.bit() == 1;
    let compression = reader.bit() == 1;
    let coarse = reader.bit() == 1;
    let fine = reader.bit() == 1;
    if levels {
        if reader.left() < 8 {
            return Some(out);
        }
        let centre_on = reader.bit() == 1;
        let centre = reader.bits(3) as u8;
        let surround_on = reader.bit() == 1;
        let surround = reader.bits(3) as u8;
        if centre_on {
            out.centre = Some(Mixdown::level(centre));
        }
        if surround_on {
            out.surround = Some(Mixdown::level(surround));
        }
    }
    if compression {
        reader.skip(16);
    }
    if coarse {
        reader.skip(16);
    }
    if fine {
        reader.skip(16);
    }
    if extension {
        if reader.left() < 8 {
            return Some(out);
        }
        reader.skip(5);
        let ext_levels = reader.bit() == 1;
        let ext_gains = reader.bit() == 1;
        let ext_lfe = reader.bit() == 1;
        if ext_levels {
            // dmix_a and dmix_b: levels for 7/6 channel layouts; they win over the other pair.
            if reader.left() < 8 {
                return Some(out);
            }
            out.centre = Some(Mixdown::level(reader.bits(3) as u8));
            out.surround = Some(Mixdown::level(reader.bits(3) as u8));
            reader.skip(2);
        }
        if ext_gains {
            // Global gains after the fold: not applied.
            reader.skip(16);
        }
        if ext_lfe {
            if reader.left() < 8 {
                return Some(out);
            }
            out.lfe = Some(Mixdown::lfe_level(reader.bits(4) as u8));
            reader.skip(4);
        }
    }
    if reader.left() >= 9 && reader.bits(8) == SYNC2 && reader.bit() == 1 && reader.left() >= 19 {
        let mut g = [0.0f32; 6];
        for gain in g.iter_mut().take(5) {
            *gain = Mixdown::level(reader.bits(3) as u8);
        }
        g[5] = Mixdown::lfe_level(reader.bits(4) as u8);
        out.twenty_two = Some(g);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 2e-3
    }

    #[test]
    fn a_stereo_asset_says_nothing_about_folding() {
        let read = read(&[0xbc, 0xc0, 0x04, 0x01, 0x80]).unwrap();
        assert_eq!(read, Mixdown::default());
    }

    #[test]
    fn the_twenty_two_asset_carries_the_arib_defaults() {
        let read = read(&[
            0xbc, 0xc0, 0x1c, 0xaa, 0x01, 0x00, 0x00, 0xbd, 0xb6, 0x22, 0x70,
        ])
        .unwrap();
        let minus_3 = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(read.centre.unwrap(), minus_3));
        assert!(near(read.surround.unwrap(), minus_3));
        assert_eq!(read.lfe, None);
        let g = read.twenty_two.unwrap();
        assert!(near(g[0], Mixdown::level(3)), "g1 −4.5 dB");
        assert!(near(g[1], Mixdown::level(3)), "g2 −4.5 dB");
        assert!(near(g[2], 1.0), "g3 0 dB");
        assert!(near(g[3], Mixdown::level(4)), "g4 −6 dB");
        assert!(near(g[4], minus_3), "g5 −3 dB");
        assert!(near(g[5], minus_3), "LFE −3 dB");
    }

    #[test]
    fn the_variant_with_compression_reads_the_same() {
        let read = read(&[0xbc, 0xc0, 0x1c, 0xaa, 0x01, 0x80, 0x00]).unwrap();
        assert!(near(read.centre.unwrap(), std::f32::consts::FRAC_1_SQRT_2));
        assert_eq!(read.twenty_two, None);
    }

    #[test]
    #[allow(clippy::unusual_byte_groupings)] // 3・3・2 ビットのフィールドの境界で区切っている
    fn the_extension_levels_win_and_the_lfe_is_read() {
        // flags: extension only; ext status: levels + lfe; a=1 (−1.5), b=4 (−6); lfe 5 (0 dB)
        let read = read(&[0xbc, 0xc0, 0x08, 0b0000_0101, 0b001_100_00, 0b0101_0000]).unwrap();
        assert!(near(read.centre.unwrap(), Mixdown::level(1)));
        assert!(near(read.surround.unwrap(), Mixdown::level(4)));
        assert!(near(read.lfe.unwrap(), 1.0));
    }

    #[test]
    fn something_else_in_a_dse_is_not_one() {
        assert_eq!(read(&[0x01, 0x02, 0x03]), None);
        assert_eq!(read(&[0xbc]), None);
    }

    #[test]
    fn the_tables_agree_with_the_standard() {
        assert!(near(Mixdown::level(0), 1.0));
        assert!(near(Mixdown::level(2), std::f32::consts::FRAC_1_SQRT_2));
        assert_eq!(Mixdown::level(7), 0.0);
        assert!(near(Mixdown::lfe_level(5), 1.0));
        assert!(near(Mixdown::lfe_level(7), std::f32::consts::FRAC_1_SQRT_2));
        assert_eq!(Mixdown::lfe_level(15), 0.0);
        assert!(near(Mixdown::matrix(0), std::f32::consts::FRAC_1_SQRT_2));
        assert!(near(Mixdown::matrix(1), 0.5));
        assert_eq!(Mixdown::matrix(3), 0.0);
    }
}
