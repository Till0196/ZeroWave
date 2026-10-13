// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ITU-T H.262 Annex B と第 7 節のテーブル。規格と見比べられるよう `(code, length, value)` で書き、
//! 符号は `u16` の下位 `length` ビットに上位から入れる。

/// `macroblock_escape`: adds 33 and another code follows.
pub const ESCAPE: u16 = 0xffff;
/// `macroblock_stuffing` (MPEG-1 only).
pub const STUFFING: u16 = 0xfffe;

/// Table B.1: `macroblock_address_increment`.
pub const ADDRESS_INCREMENT: &[(u16, u8, u16)] = &[
    (0x001, 1, 1),
    (0x003, 3, 2),
    (0x002, 3, 3),
    (0x003, 4, 4),
    (0x002, 4, 5),
    (0x003, 5, 6),
    (0x002, 5, 7),
    (0x007, 7, 8),
    (0x006, 7, 9),
    (0x00b, 8, 10),
    (0x00a, 8, 11),
    (0x009, 8, 12),
    (0x008, 8, 13),
    (0x007, 8, 14),
    (0x006, 8, 15),
    (0x017, 10, 16),
    (0x016, 10, 17),
    (0x015, 10, 18),
    (0x014, 10, 19),
    (0x013, 10, 20),
    (0x012, 10, 21),
    (0x023, 11, 22),
    (0x022, 11, 23),
    (0x021, 11, 24),
    (0x020, 11, 25),
    (0x01f, 11, 26),
    (0x01e, 11, 27),
    (0x01d, 11, 28),
    (0x01c, 11, 29),
    (0x01b, 11, 30),
    (0x01a, 11, 31),
    (0x019, 11, 32),
    (0x018, 11, 33),
    (0x008, 11, ESCAPE),
    (0x00f, 11, STUFFING),
];

/// Table B.9: `coded_block_pattern`.
pub const CODED_BLOCK_PATTERN: &[(u16, u8, u16)] = &[
    (0x001, 9, 0),
    (0x00b, 5, 1),
    (0x009, 5, 2),
    (0x00d, 6, 3),
    (0x00d, 4, 4),
    (0x017, 7, 5),
    (0x013, 7, 6),
    (0x01f, 8, 7),
    (0x00c, 4, 8),
    (0x016, 7, 9),
    (0x012, 7, 10),
    (0x01e, 8, 11),
    (0x013, 5, 12),
    (0x01b, 8, 13),
    (0x017, 8, 14),
    (0x013, 8, 15),
    (0x00b, 4, 16),
    (0x015, 7, 17),
    (0x011, 7, 18),
    (0x01d, 8, 19),
    (0x011, 5, 20),
    (0x019, 8, 21),
    (0x015, 8, 22),
    (0x011, 8, 23),
    (0x00f, 6, 24),
    (0x00f, 8, 25),
    (0x00d, 8, 26),
    (0x003, 9, 27),
    (0x00f, 5, 28),
    (0x00b, 8, 29),
    (0x007, 8, 30),
    (0x007, 9, 31),
    (0x00a, 4, 32),
    (0x014, 7, 33),
    (0x010, 7, 34),
    (0x01c, 8, 35),
    (0x00e, 6, 36),
    (0x00e, 8, 37),
    (0x00c, 8, 38),
    (0x002, 9, 39),
    (0x010, 5, 40),
    (0x018, 8, 41),
    (0x014, 8, 42),
    (0x010, 8, 43),
    (0x00e, 5, 44),
    (0x00a, 8, 45),
    (0x006, 8, 46),
    (0x006, 9, 47),
    (0x012, 5, 48),
    (0x01a, 8, 49),
    (0x016, 8, 50),
    (0x012, 8, 51),
    (0x00d, 5, 52),
    (0x009, 8, 53),
    (0x005, 8, 54),
    (0x005, 9, 55),
    (0x00c, 5, 56),
    (0x008, 8, 57),
    (0x004, 8, 58),
    (0x004, 9, 59),
    (0x007, 3, 60),
    (0x00a, 5, 61),
    (0x008, 5, 62),
    (0x00c, 6, 63),
];

/// Table B.10: `motion_code`. A sign bit follows every value but zero.
pub const MOTION_CODE: &[(u16, u8, u16)] = &[
    (0x001, 1, 0),
    (0x001, 2, 1),
    (0x001, 3, 2),
    (0x001, 4, 3),
    (0x003, 6, 4),
    (0x005, 7, 5),
    (0x004, 7, 6),
    (0x003, 7, 7),
    (0x00b, 9, 8),
    (0x00a, 9, 9),
    (0x009, 9, 10),
    (0x011, 10, 11),
    (0x010, 10, 12),
    (0x00f, 10, 13),
    (0x00e, 10, 14),
    (0x00d, 10, 15),
    (0x00c, 10, 16),
];

/// Table B.12: `dct_dc_size_luminance`.
pub const DC_SIZE_LUMA: &[(u16, u8, u16)] = &[
    (0x004, 3, 0),
    (0x000, 2, 1),
    (0x001, 2, 2),
    (0x005, 3, 3),
    (0x006, 3, 4),
    (0x00e, 4, 5),
    (0x01e, 5, 6),
    (0x03e, 6, 7),
    (0x07e, 7, 8),
    (0x0fe, 8, 9),
    (0x1fe, 9, 10),
    (0x1ff, 9, 11),
];

/// Table B.13: `dct_dc_size_chrominance`.
pub const DC_SIZE_CHROMA: &[(u16, u8, u16)] = &[
    (0x000, 2, 0),
    (0x001, 2, 1),
    (0x002, 2, 2),
    (0x006, 3, 3),
    (0x00e, 4, 4),
    (0x01e, 5, 5),
    (0x03e, 6, 6),
    (0x07e, 7, 7),
    (0x0fe, 8, 8),
    (0x1fe, 9, 9),
    (0x3fe, 10, 10),
    (0x3ff, 10, 11),
];

/// Table B.14: `(code, length, run, level)`; a sign bit follows each.
pub const COEFFICIENTS_ZERO: &[(u16, u8, u8, u8)] = &[
    (0x0003, 2, 0, 1),
    (0x0004, 4, 0, 2),
    (0x0005, 5, 0, 3),
    (0x0006, 7, 0, 4),
    (0x0026, 8, 0, 5),
    (0x0021, 8, 0, 6),
    (0x000a, 10, 0, 7),
    (0x001d, 12, 0, 8),
    (0x0018, 12, 0, 9),
    (0x0013, 12, 0, 10),
    (0x0010, 12, 0, 11),
    (0x001a, 13, 0, 12),
    (0x0019, 13, 0, 13),
    (0x0018, 13, 0, 14),
    (0x0017, 13, 0, 15),
    (0x001f, 14, 0, 16),
    (0x001e, 14, 0, 17),
    (0x001d, 14, 0, 18),
    (0x001c, 14, 0, 19),
    (0x001b, 14, 0, 20),
    (0x001a, 14, 0, 21),
    (0x0019, 14, 0, 22),
    (0x0018, 14, 0, 23),
    (0x0017, 14, 0, 24),
    (0x0016, 14, 0, 25),
    (0x0015, 14, 0, 26),
    (0x0014, 14, 0, 27),
    (0x0013, 14, 0, 28),
    (0x0012, 14, 0, 29),
    (0x0011, 14, 0, 30),
    (0x0010, 14, 0, 31),
    (0x0018, 15, 0, 32),
    (0x0017, 15, 0, 33),
    (0x0016, 15, 0, 34),
    (0x0015, 15, 0, 35),
    (0x0014, 15, 0, 36),
    (0x0013, 15, 0, 37),
    (0x0012, 15, 0, 38),
    (0x0011, 15, 0, 39),
    (0x0010, 15, 0, 40),
    (0x0003, 3, 1, 1),
    (0x0006, 6, 1, 2),
    (0x0025, 8, 1, 3),
    (0x000c, 10, 1, 4),
    (0x001b, 12, 1, 5),
    (0x0016, 13, 1, 6),
    (0x0015, 13, 1, 7),
    (0x001f, 15, 1, 8),
    (0x001e, 15, 1, 9),
    (0x001d, 15, 1, 10),
    (0x001c, 15, 1, 11),
    (0x001b, 15, 1, 12),
    (0x001a, 15, 1, 13),
    (0x0019, 15, 1, 14),
    (0x0013, 16, 1, 15),
    (0x0012, 16, 1, 16),
    (0x0011, 16, 1, 17),
    (0x0010, 16, 1, 18),
    (0x0005, 4, 2, 1),
    (0x0004, 7, 2, 2),
    (0x000b, 10, 2, 3),
    (0x0014, 12, 2, 4),
    (0x0014, 13, 2, 5),
    (0x0007, 5, 3, 1),
    (0x0024, 8, 3, 2),
    (0x001c, 12, 3, 3),
    (0x0013, 13, 3, 4),
    (0x0006, 5, 4, 1),
    (0x000f, 10, 4, 2),
    (0x0012, 12, 4, 3),
    (0x0007, 6, 5, 1),
    (0x0009, 10, 5, 2),
    (0x0012, 13, 5, 3),
    (0x0005, 6, 6, 1),
    (0x001e, 12, 6, 2),
    (0x0014, 16, 6, 3),
    (0x0004, 6, 7, 1),
    (0x0015, 12, 7, 2),
    (0x0007, 7, 8, 1),
    (0x0011, 12, 8, 2),
    (0x0005, 7, 9, 1),
    (0x0011, 13, 9, 2),
    (0x0027, 8, 10, 1),
    (0x0010, 13, 10, 2),
    (0x0023, 8, 11, 1),
    (0x001a, 16, 11, 2),
    (0x0022, 8, 12, 1),
    (0x0019, 16, 12, 2),
    (0x0020, 8, 13, 1),
    (0x0018, 16, 13, 2),
    (0x000e, 10, 14, 1),
    (0x0017, 16, 14, 2),
    (0x000d, 10, 15, 1),
    (0x0016, 16, 15, 2),
    (0x0008, 10, 16, 1),
    (0x0015, 16, 16, 2),
    (0x001f, 12, 17, 1),
    (0x001a, 12, 18, 1),
    (0x0019, 12, 19, 1),
    (0x0017, 12, 20, 1),
    (0x0016, 12, 21, 1),
    (0x001f, 13, 22, 1),
    (0x001e, 13, 23, 1),
    (0x001d, 13, 24, 1),
    (0x001c, 13, 25, 1),
    (0x001b, 13, 26, 1),
    (0x001f, 16, 27, 1),
    (0x001e, 16, 28, 1),
    (0x001d, 16, 29, 1),
    (0x001c, 16, 30, 1),
    (0x001b, 16, 31, 1),
];

/// Table B.15, for intra blocks when `intra_vlc_format` is one.
pub const COEFFICIENTS_ONE: &[(u16, u8, u8, u8)] = &[
    (0x0002, 2, 0, 1),
    (0x0006, 3, 0, 2),
    (0x0007, 4, 0, 3),
    (0x001c, 5, 0, 4),
    (0x001d, 5, 0, 5),
    (0x0005, 6, 0, 6),
    (0x0004, 6, 0, 7),
    (0x007b, 7, 0, 8),
    (0x007c, 7, 0, 9),
    (0x0023, 8, 0, 10),
    (0x0022, 8, 0, 11),
    (0x00fa, 8, 0, 12),
    (0x00fb, 8, 0, 13),
    (0x00fe, 8, 0, 14),
    (0x00ff, 8, 0, 15),
    (0x001f, 14, 0, 16),
    (0x001e, 14, 0, 17),
    (0x001d, 14, 0, 18),
    (0x001c, 14, 0, 19),
    (0x001b, 14, 0, 20),
    (0x001a, 14, 0, 21),
    (0x0019, 14, 0, 22),
    (0x0018, 14, 0, 23),
    (0x0017, 14, 0, 24),
    (0x0016, 14, 0, 25),
    (0x0015, 14, 0, 26),
    (0x0014, 14, 0, 27),
    (0x0013, 14, 0, 28),
    (0x0012, 14, 0, 29),
    (0x0011, 14, 0, 30),
    (0x0010, 14, 0, 31),
    (0x0018, 15, 0, 32),
    (0x0017, 15, 0, 33),
    (0x0016, 15, 0, 34),
    (0x0015, 15, 0, 35),
    (0x0014, 15, 0, 36),
    (0x0013, 15, 0, 37),
    (0x0012, 15, 0, 38),
    (0x0011, 15, 0, 39),
    (0x0010, 15, 0, 40),
    (0x0002, 3, 1, 1),
    (0x0006, 5, 1, 2),
    (0x0079, 7, 1, 3),
    (0x0027, 8, 1, 4),
    (0x0020, 8, 1, 5),
    (0x0016, 13, 1, 6),
    (0x0015, 13, 1, 7),
    (0x001f, 15, 1, 8),
    (0x001e, 15, 1, 9),
    (0x001d, 15, 1, 10),
    (0x001c, 15, 1, 11),
    (0x001b, 15, 1, 12),
    (0x001a, 15, 1, 13),
    (0x0019, 15, 1, 14),
    (0x0013, 16, 1, 15),
    (0x0012, 16, 1, 16),
    (0x0011, 16, 1, 17),
    (0x0010, 16, 1, 18),
    (0x0005, 5, 2, 1),
    (0x0007, 7, 2, 2),
    (0x00fc, 8, 2, 3),
    (0x000c, 10, 2, 4),
    (0x0014, 13, 2, 5),
    (0x0007, 5, 3, 1),
    (0x0026, 8, 3, 2),
    (0x001c, 12, 3, 3),
    (0x0013, 13, 3, 4),
    (0x0006, 6, 4, 1),
    (0x00fd, 8, 4, 2),
    (0x0012, 12, 4, 3),
    (0x0007, 6, 5, 1),
    (0x0004, 9, 5, 2),
    (0x0012, 13, 5, 3),
    (0x0006, 7, 6, 1),
    (0x001e, 12, 6, 2),
    (0x0014, 16, 6, 3),
    (0x0004, 7, 7, 1),
    (0x0015, 12, 7, 2),
    (0x0005, 7, 8, 1),
    (0x0011, 12, 8, 2),
    (0x0078, 7, 9, 1),
    (0x0011, 13, 9, 2),
    (0x007a, 7, 10, 1),
    (0x0010, 13, 10, 2),
    (0x0021, 8, 11, 1),
    (0x001a, 16, 11, 2),
    (0x0025, 8, 12, 1),
    (0x0019, 16, 12, 2),
    (0x0024, 8, 13, 1),
    (0x0018, 16, 13, 2),
    (0x0005, 9, 14, 1),
    (0x0017, 16, 14, 2),
    (0x0007, 9, 15, 1),
    (0x0016, 16, 15, 2),
    (0x000d, 10, 16, 1),
    (0x0015, 16, 16, 2),
    (0x001f, 12, 17, 1),
    (0x001a, 12, 18, 1),
    (0x0019, 12, 19, 1),
    (0x0017, 12, 20, 1),
    (0x0016, 12, 21, 1),
    (0x001f, 13, 22, 1),
    (0x001e, 13, 23, 1),
    (0x001d, 13, 24, 1),
    (0x001c, 13, 25, 1),
    (0x001b, 13, 26, 1),
    (0x001f, 16, 27, 1),
    (0x001e, 16, 28, 1),
    (0x001d, 16, 29, 1),
    (0x001c, 16, 30, 1),
    (0x001b, 16, 31, 1),
];

/// Shared escape: six bits of run and twelve of signed level follow.
pub const COEFFICIENT_ESCAPE: (u16, u8) = (0x01, 6);
/// End of block, coded differently in B.14 and B.15.
pub const END_OF_BLOCK_ZERO: (u16, u8) = (0x02, 2);
pub const END_OF_BLOCK_ONE: (u16, u8) = (0x06, 4);

pub const QUANT: u8 = 1;
pub const FORWARD: u8 = 2;
pub const BACKWARD: u8 = 4;
pub const PATTERN: u8 = 8;
pub const INTRA: u8 = 16;

/// Table B.2: `macroblock_type` in an I picture.
pub const MACROBLOCK_TYPE_I: &[(u16, u8, u16)] =
    &[(0b1, 1, INTRA as u16), (0b01, 2, (QUANT | INTRA) as u16)];

/// Table B.3: `macroblock_type` in a P picture. Code `01` (pattern, no forward
/// motion) is not intra: zero-vector prediction, and it resets the predictors.
pub const MACROBLOCK_TYPE_P: &[(u16, u8, u16)] = &[
    (0b1, 1, (FORWARD | PATTERN) as u16),
    (0b01, 2, PATTERN as u16),
    (0b001, 3, FORWARD as u16),
    (0b00011, 5, INTRA as u16),
    (0b00010, 5, (QUANT | FORWARD | PATTERN) as u16),
    (0b00001, 5, (QUANT | PATTERN) as u16),
    (0b000001, 6, (QUANT | INTRA) as u16),
];

/// Table B.4.
pub const MACROBLOCK_TYPE_B: &[(u16, u8, u16)] = &[
    (0b10, 2, (FORWARD | BACKWARD) as u16),
    (0b11, 2, (FORWARD | BACKWARD | PATTERN) as u16),
    (0b010, 3, BACKWARD as u16),
    (0b011, 3, (BACKWARD | PATTERN) as u16),
    (0b0010, 4, FORWARD as u16),
    (0b0011, 4, (FORWARD | PATTERN) as u16),
    (0b00011, 5, INTRA as u16),
    (0b00010, 5, (QUANT | FORWARD | BACKWARD | PATTERN) as u16),
    (0b000011, 6, (QUANT | FORWARD | PATTERN) as u16),
    (0b000010, 6, (QUANT | BACKWARD | PATTERN) as u16),
    (0b000001, 6, (QUANT | INTRA) as u16),
];

/// Figure 7-2: zigzag scan, scan place to raster place. Quantiser matrices
/// are always sent in this order.
pub const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, //
    17, 24, 32, 25, 18, 11, 4, 5, //
    12, 19, 26, 33, 40, 48, 41, 34, //
    27, 20, 13, 6, 7, 14, 21, 28, //
    35, 42, 49, 56, 57, 50, 43, 36, //
    29, 22, 15, 23, 30, 37, 44, 51, //
    58, 59, 52, 45, 38, 31, 39, 46, //
    53, 60, 61, 54, 47, 55, 62, 63,
];

/// Figure 7-3: the alternate scan.
pub const ALTERNATE: [u8; 64] = [
    0, 8, 16, 24, 1, 9, 2, 10, //
    17, 25, 32, 40, 48, 56, 57, 49, //
    41, 33, 26, 18, 3, 11, 4, 12, //
    19, 27, 34, 42, 50, 58, 35, 43, //
    51, 59, 20, 28, 5, 13, 6, 14, //
    21, 29, 36, 44, 52, 60, 37, 45, //
    53, 61, 22, 30, 7, 15, 23, 31, //
    38, 46, 54, 62, 39, 47, 55, 63,
];

pub const DEFAULT_INTRA: [u8; 64] = [
    8, 16, 19, 22, 26, 27, 29, 34, //
    16, 16, 22, 24, 27, 29, 34, 37, //
    19, 22, 26, 27, 29, 34, 34, 38, //
    22, 22, 26, 27, 29, 34, 37, 40, //
    22, 26, 27, 29, 32, 35, 40, 48, //
    26, 27, 29, 32, 35, 40, 48, 58, //
    26, 27, 29, 34, 38, 46, 56, 69, //
    27, 29, 35, 38, 46, 56, 69, 83,
];

pub const DEFAULT_NON_INTRA: [u8; 64] = [16; 64];

/// Table 7-6: `quantiser_scale_code` to scale when `q_scale_type` is one.
pub const NON_LINEAR_QUANTISER: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, //
    8, 10, 12, 14, 16, 18, 20, 22, //
    24, 28, 32, 36, 40, 44, 48, 52, //
    56, 64, 72, 80, 88, 96, 104, 112,
];

pub fn quantiser_scale(code: u32, non_linear: bool) -> i32 {
    if non_linear {
        i32::from(NON_LINEAR_QUANTISER[(code & 31) as usize])
    } else {
        (code as i32 & 31) * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_free(rows: impl Iterator<Item = (u16, u8)>) {
        let rows: Vec<_> = rows.collect();
        for &(a, la) in &rows {
            assert!(la > 0 && la <= 16);
            assert!(u32::from(a) < 1 << la, "code {a:x} does not fit {la} bits");
            for &(b, lb) in &rows {
                if (a, la) == (b, lb) {
                    continue;
                }
                let (short, long, bits) = if la <= lb { (a, b, la) } else { (b, a, lb) };
                assert_ne!(
                    short,
                    long >> (la.max(lb) - bits),
                    "{a:x}/{la} and {b:x}/{lb} share a prefix"
                );
            }
        }
    }

    #[test]
    fn the_tables_are_prefix_codes() {
        prefix_free(ADDRESS_INCREMENT.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(CODED_BLOCK_PATTERN.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(MOTION_CODE.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(DC_SIZE_LUMA.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(DC_SIZE_CHROMA.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(MACROBLOCK_TYPE_I.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(MACROBLOCK_TYPE_P.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(MACROBLOCK_TYPE_B.iter().map(|&(c, l, _)| (c, l)));
        prefix_free(
            COEFFICIENTS_ZERO
                .iter()
                .map(|&(c, l, _, _)| (c, l))
                .chain([COEFFICIENT_ESCAPE, END_OF_BLOCK_ZERO]),
        );
        prefix_free(
            COEFFICIENTS_ONE
                .iter()
                .map(|&(c, l, _, _)| (c, l))
                .chain([COEFFICIENT_ESCAPE, END_OF_BLOCK_ONE]),
        );
    }

    #[test]
    fn both_coefficient_tables_hold_the_same_runs_and_levels() {
        // B.15 is B.14 with the codes moved about.
        let zero: Vec<_> = COEFFICIENTS_ZERO
            .iter()
            .map(|&(_, _, r, l)| (r, l))
            .collect();
        let one: Vec<_> = COEFFICIENTS_ONE
            .iter()
            .map(|&(_, _, r, l)| (r, l))
            .collect();
        assert_eq!(zero, one);
        assert_eq!(zero.len(), 111);
    }

    #[test]
    fn every_scan_visits_every_place_once() {
        for scan in [ZIGZAG, ALTERNATE] {
            let mut seen = [false; 64];
            for &at in &scan {
                assert!(!seen[at as usize]);
                seen[at as usize] = true;
            }
        }
        assert_eq!((ZIGZAG[0], ALTERNATE[0]), (0, 0));
    }

    #[test]
    fn the_quantiser_scale_is_twice_the_code_until_it_is_not() {
        assert_eq!(quantiser_scale(1, false), 2);
        assert_eq!(quantiser_scale(31, false), 62);
        assert_eq!(quantiser_scale(1, true), 1);
        assert_eq!(quantiser_scale(8, true), 8);
        assert_eq!(quantiser_scale(31, true), 112);
    }
}
