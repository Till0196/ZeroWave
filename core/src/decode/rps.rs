// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! スライスヘッダーに直接書かれた短期参照ピクチャーセットのビット数（NVDEC と D3D12 が使う）。
//! 渡さないとスライスヘッダーの後続が全部ずれる。

use mediadecoder::video::hevc::params::{self, Pps, Sps};

/// `NumBitsForShortTermRPSInSlice` と `NumDeltaPocsOfRefRpsIdx`（7.3.6.1）。
/// 0 だと NVDEC は `cuvidDecodeStatus_Error` を返す（ヘッダーには書いていない）。
pub fn short_term_bits(
    payload: &[u8],
    kind: u8,
    shape: &Shape,
    sets: &[params::ShortTerm],
) -> (i32, i32) {
    if params::is_idr(kind) {
        return (0, 0);
    }
    let mut reader = mediadecoder::bits::Reader::new(payload);
    let first_segment = reader.bit() == 1;
    if params::is_irap(kind) {
        reader.bit();
    }
    reader.ue();
    let mut dependent = false;
    if !first_segment {
        if shape.dependent_enabled {
            dependent = reader.bit() == 1;
        }
        reader.skip(width_of(shape.ctbs));
    }
    // 従属スライスセグメントはヘッダーを持たず、前の独立スライスセグメントのものを継ぐ。
    if dependent {
        return (0, 0);
    }
    reader.skip(shape.extra_bits);
    reader.ue();
    if shape.output_flag_present {
        reader.bit();
    }
    reader.skip(shape.log2_max_poc_lsb);
    if reader.bit() == 1 {
        return (0, 0);
    }
    let from = reader.at();
    let count = sets.len();
    // 7.3.7 `st_ref_pic_set(stRpsIdx)`、`stRpsIdx == num_short_term_ref_pic_sets`。
    let mut deltas = 0i32;
    let inter = count != 0 && reader.bit() == 1;
    if inter {
        let delta_idx = reader.ue() as usize + 1;
        let referenced = count.saturating_sub(delta_idx);
        deltas = sets
            .get(referenced)
            .map_or(0, |set| (set.before.len() + set.after.len()) as i32);
        reader.bit();
        reader.ue();
        for _ in 0..=deltas {
            if reader.bit() == 0 {
                reader.bit();
            }
        }
    } else {
        let negative = reader.ue() as usize;
        let positive = reader.ue() as usize;
        // 壊れたストリームで延々と読まないように（7.4.8 の上限より大きい）。
        if negative > 32 || positive > 32 {
            return (0, 0);
        }
        for _ in 0..negative + positive {
            reader.ue();
            reader.bit();
        }
    }
    ((reader.at() - from) as i32, deltas)
}

#[derive(Debug)]
pub struct Shape {
    pub ctbs: usize,
    pub log2_max_poc_lsb: usize,
    pub dependent_enabled: bool,
    pub extra_bits: usize,
    pub output_flag_present: bool,
}

impl Shape {
    pub fn of(sps: &Sps, pps: &Pps) -> Self {
        Self {
            ctbs: sps.ctbs(),
            log2_max_poc_lsb: sps.log2_max_poc_lsb as usize,
            dependent_enabled: pps.dependent_slice_segments_enabled,
            extra_bits: pps.num_extra_slice_header_bits,
            output_flag_present: pps.output_flag_present,
        }
    }
}

fn width_of(most: usize) -> usize {
    let mut bits = 0;
    while (1usize << bits) < most {
        bits += 1;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_length_of_an_inline_reference_set_is_counted() {
        fn push(bits: &mut Vec<u8>, text: &str) {
            bits.extend(text.bytes().map(|b| b - b'0'));
        }
        let mut bits: Vec<u8> = Vec::new();
        for part in ["1", "1", "1", "011", "00000000", "0"] {
            push(&mut bits, part);
        }
        let before = bits.len();
        for part in ["010", "1", "1", "1"] {
            push(&mut bits, part);
        }
        let counted = bits.len() - before;
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (at, bit) in bits.iter().enumerate() {
            bytes[at / 8] |= bit << (7 - at % 8);
        }
        let shape = Shape {
            ctbs: 1,
            log2_max_poc_lsb: 8,
            dependent_enabled: false,
            extra_bits: 0,
            output_flag_present: false,
        };
        assert_eq!(
            short_term_bits(&bytes, 21, &shape, &[]),
            (counted as i32, 0)
        );
        assert_eq!(short_term_bits(&bytes, 19, &shape, &[]), (0, 0));
    }
}
