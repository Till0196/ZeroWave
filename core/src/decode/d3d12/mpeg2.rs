// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 を DXVA の構造体へ書いてハードウェアへ渡す。
//! ビットストリームバッファにはスライスだけを入れる。単一のテクスチャ配列は受け付けない。
//! フィールドの対は一枚のテクスチャへデコードする（`bSecondField` で反対のラインに書かせる）。

use mediadecoder::bits::Reader;
use mediadecoder::es::{Format, Picture, StartCodes, Structure};
use mediadecoder::video::mpeg2::headers::{self, Chroma, Matrices, Order, PictureCoding};
use windows::Win32::Media::MediaFoundation::{
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL, DXVA_PictureParameters,
};

use crate::decode::Error;
use crate::decode::d3d12::dx;
use crate::decode::d3d12::dxva::{QmatrixData, SliceInfo};
use crate::decode::d3d12::pool::Pool;
use crate::decode::d3d12::{Held, References, Session, argument, slice_argument};
use crate::decode::frames;

fn matrix_data(matrices: &Matrices) -> QmatrixData {
    let widen = |from: &[u8; 64]| std::array::from_fn(|at| u16::from(from[at]));
    QmatrixData {
        new_matrix: [1; 4],
        matrix: [
            widen(&matrices.intra),
            widen(&matrices.non_intra),
            widen(&matrices.chroma_intra),
            widen(&matrices.chroma_non_intra),
        ],
    }
}

/// `wBitstreamFcodes`。使わない所は 0 でなく `0xF`。
fn f_codes(coded: &PictureCoding) -> u16 {
    let mut codes = [
        coded.f_code[0][0],
        coded.f_code[0][1],
        coded.f_code[1][0],
        coded.f_code[1][1],
    ];
    if coded.kind == 1 {
        codes = [0xf; 4];
    } else if coded.kind != 3 {
        codes[2] = 0xf;
        codes[3] = 0xf;
    }
    ((codes[0] << 12) | (codes[1] << 8) | (codes[2] << 4) | codes[3]) as u16
}

/// `wBitstreamPCEelements`。最上位ビットから詰める（VA-API と逆）。
fn extension(coded: &PictureCoding) -> u16 {
    let bit = |value: bool, at: u32| u16::from(value) << at;
    (((coded.intra_dc_precision & 3) as u16) << 14)
        | (((coded.picture_structure & 3) as u16) << 12)
        | bit(coded.top_field_first, 11)
        | bit(coded.frame_pred_frame_dct, 10)
        | bit(coded.concealment_motion_vectors, 9)
        | bit(coded.q_scale_type, 8)
        | bit(coded.intra_vlc_format, 7)
        | bit(coded.alternate_scan, 6)
        | bit(coded.repeat_first_field, 5)
        | bit(coded.chroma_420_type, 4)
        | bit(coded.progressive_frame, 3)
}

#[derive(Debug)]
pub struct Mpeg2 {
    matrices: Matrices,
    order: frames::Mpeg2Order<usize>,
    /// 初期化用ピクチャーをデコードさせたサーフェス（[`Mpeg2::prime`]）。本物の参照ができるまで返さない。
    primer: Option<usize>,
}

impl Default for Mpeg2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpeg2 {
    pub fn new() -> Self {
        Self {
            matrices: Matrices::new(Order::AsSent, Chroma::AsSent),
            order: frames::Mpeg2Order::new(),
            primer: None,
        }
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        if let Some(primer) = self.primer.take() {
            pool.release(primer);
        }
        self.order.reset(pool);
        self.matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        device: &mut dx::Device,
        session: &mut Session,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if !session.primed {
            session.primed = true;
            self.primer = None;
            if let Err(error) = self.prime(device, session, format) {
                tracing::warn!("D3D12: MPEG-2 の初期化用ピクチャーをデコードできない: {error:?}");
            }
        }
        let mut coded = PictureCoding::default();
        let mut parameters = DXVA_PictureParameters::default();
        let mut matrices = QmatrixData::default();
        let mut slices: Vec<SliceInfo> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut slot = None;
        let mut skip = false;
        let mut used: Vec<usize> = Vec::new();
        // DXVA はマクロブロックのアドレスを u16 で表す。収まらないサイズは開かない。
        let mb_count = (format.width as usize).div_ceil(16) * (format.height as usize).div_ceil(16);
        if mb_count > usize::from(u16::MAX) {
            return Err(Error::Unsupported("an MPEG-2 picture too large for DXVA"));
        }
        let mb_width = (format.width as usize).div_ceil(16) as u16;

        for (kind, body) in StartCodes::new(data) {
            let unit = &data[body.clone()];
            match kind {
                0xb3 => self.matrices.read_sequence(unit),
                0xb5 => match headers::extension_id(unit) {
                    headers::QUANT_MATRIX_EXTENSION => self.matrices.read_extension(unit),
                    headers::PICTURE_CODING_EXTENSION => coded.read_extension(unit),
                    _ => {}
                },
                0x00 => {
                    coded = PictureCoding {
                        kind: headers::read_picture(unit),
                        ..PictureCoding::default()
                    };
                }
                0x01..=0xaf if coded.kind != 0 => {
                    if slot.is_none() {
                        let (at, made, named, keep) =
                            self.begin(&mut session.pool, tag, picture, format, &coded)?;
                        parameters = made;
                        matrices = matrix_data(&self.matrices);
                        used = named;
                        slot = Some(at);
                        skip = !keep;
                    }
                    // スタートコードごと渡し、`wMBbitOffset` はその 32 ビットを数える。
                    // スライスは次の `00 00 01` の直前まで。0 を削ると残りを捨てるドライバーがある。
                    let (end, missing) = headers::slice_end(data, body.end);
                    let slice = &data[body.start.saturating_sub(4)..end];
                    let mut reader = Reader::new(&data[body.start..body.end]);
                    let quantiser = reader.bits(5) as u16;
                    let intra_slice_flag = reader.bit();
                    if intra_slice_flag == 1 {
                        reader.skip(8);
                        while reader.bit() == 1 {
                            reader.skip(8);
                        }
                    }
                    let offset = reader.at().min(usize::from(u16::MAX - 32));
                    let increment = address_increment(&mut reader).unwrap_or(1);
                    let horizontal =
                        (increment.saturating_sub(1) as u16).min(mb_width.saturating_sub(1));
                    slices.push(SliceInfo {
                        horizontal,
                        vertical: u16::from(kind) - 1,
                        bits_in_buffer: ((slice.len() + missing) * 8) as u32,
                        location: bytes.len() as u32,
                        start_code_bit_offset: 0,
                        reserved: 0,
                        mb_bit_offset: 32 + offset as u16,
                        // 名前に反して数でなくアドレス。数は下で次との差にする。
                        macroblocks: (u32::from(kind - 1) * u32::from(mb_width)
                            + u32::from(horizontal))
                        .min(u32::from(u16::MAX)) as u16,
                        quantiser_scale_code: quantiser,
                        bad_chopping: 0,
                    });
                    bytes.extend_from_slice(slice);
                    bytes.resize(bytes.len() + missing, 0);
                }
                _ => {}
            }
        }

        let Some(slot) = slot else {
            return Ok(());
        };
        let field = picture.structure != Structure::Frame;
        let count = mb_width * ((format.height as usize).div_ceil(16) >> u32::from(field)) as u16;
        for at in 0..slices.len() {
            let start = slices[at].macroblocks;
            let next = match slices.get(at + 1) {
                Some(next) => next.macroblocks,
                None => count,
            };
            slices[at].macroblocks = next.saturating_sub(start).max(1);
        }
        let arguments = [
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
                &parameters,
            ),
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
                &matrices,
            ),
            slice_argument(D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL, &slices),
        ];
        if !skip {
            session.submit(
                device,
                slot,
                References::BySlot(&used),
                &bytes,
                &arguments,
                u32::from(count),
            )?;
        }
        if self.order.pending() != Some(picture.structure) {
            self.finish(&mut session.pool);
        }
        Ok(())
    }

    /// 開いたばかりのデコーダーに、灰色の I のフレーム構造ピクチャーを一枚デコードさせて捨てる。
    /// 最初がそうでないとフィールド構造ピクチャーを書かないドライバーがある。
    fn prime(
        &mut self,
        device: &mut dx::Device,
        session: &mut Session,
        format: Format,
    ) -> Result<(), Error> {
        let mb_width = (format.width as usize).div_ceil(16);
        let mb_height = (format.height as usize).div_ceil(16);
        if mb_height == 0 || mb_height > 175 || mb_width == 0 {
            return Ok(());
        }
        let Some(slot) = session.pool.take() else {
            return Ok(());
        };
        let coded = PictureCoding {
            kind: 1,
            f_code: [[0xf; 2]; 2],
            intra_dc_precision: 0,
            picture_structure: 3,
            top_field_first: false,
            frame_pred_frame_dct: true,
            concealment_motion_vectors: false,
            q_scale_type: false,
            intra_vlc_format: false,
            alternate_scan: false,
            repeat_first_field: false,
            chroma_420_type: true,
            progressive_frame: true,
        };
        const QUANTISER: u16 = 8;
        let mut bytes = Vec::new();
        let mut slices = Vec::with_capacity(mb_height);
        for row in 0..mb_height {
            let location = bytes.len();
            bytes.extend_from_slice(&[0, 0, 1, row as u8 + 1]);
            let mut bits = Bits::default();
            bits.put(u32::from(QUANTISER), 5);
            bits.put(0, 1); // extra_bit_slice
            for _ in 0..mb_width {
                bits.put(1, 1); // macroblock_address_increment 1（テーブル B.1）
                bits.put(1, 1); // macroblock_type: intra（テーブル B.2）
                for _ in 0..4 {
                    bits.put(0b100, 3); // dct_dc_size_luminance 0（テーブル B.12）
                    bits.put(0b10, 2); // 係数の終わり（テーブル B.14）
                }
                for _ in 0..2 {
                    bits.put(0b00, 2); // dct_dc_size_chrominance 0（テーブル B.13）
                    bits.put(0b10, 2);
                }
            }
            bytes.extend_from_slice(&bits.finish());
            slices.push(SliceInfo {
                horizontal: 0,
                vertical: row as u16,
                bits_in_buffer: ((bytes.len() - location) * 8) as u32,
                location: location as u32,
                start_code_bit_offset: 0,
                reserved: 0,
                mb_bit_offset: 32 + 6,
                macroblocks: mb_width as u16,
                quantiser_scale_code: QUANTISER,
                bad_chopping: 0,
            });
        }
        if let Err(error) = session.pool.prepare(slot, &[]) {
            session.pool.release(slot);
            return Err(error);
        }
        let parameters = picture_parameters(
            &session.pool,
            slot,
            None,
            None,
            format,
            false,
            false,
            &coded,
        );
        let matrices = matrix_data(&Matrices::new(Order::AsSent, Chroma::AsSent));
        let arguments = [
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
                &parameters,
            ),
            argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
                &matrices,
            ),
            slice_argument(D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL, &slices),
        ];
        let submitted = session.submit(
            device,
            slot,
            References::BySlot(&[slot]),
            &bytes,
            &arguments,
            (mb_width * mb_height) as u32,
        );
        match submitted {
            Ok(()) => self.primer = Some(slot),
            Err(_) => {
                session.pool.release(slot);
            }
        }
        submitted
    }

    fn begin(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        coded: &PictureCoding,
    ) -> Result<(usize, DXVA_PictureParameters, Vec<usize>, bool), Error> {
        // 対のフィールドが来なかった先のフィールドは捨て、完了しなかったピクチャーのサーフェスは返す（返さないとプールが枯渇する）。
        let second = self.order.begin(pool, tag, picture)?;
        let held = self
            .order
            .current()
            .ok_or(Error::Damaged("a slice with no picture"))?;
        if coded.picture_structure == 0 || coded.kind == 0 {
            return Err(Error::Damaged("a picture with no coding extension"));
        }

        // 前方参照は先に表示する方。種類はフィールドの種類で決める。
        let (forward, backward) = match coded.kind {
            1 => (None, None),
            _ => self.order.references(picture.coding),
        };
        if let Some(primer) = self.primer
            && self
                .order
                .references(mediadecoder::es::Coding::P)
                .0
                .is_some()
        {
            self.primer = None;
            pool.release(primer);
        }
        // 前方参照の無い B はデコードさせない（次の I まで映像が乱れるドライバーがある）。
        let keep = !(coded.kind == 3 && forward.is_none());
        // 番号は参照に使われる間は変えない（途中で変えると、それを参照するピクチャーを正しくデコードしない
        // ドライバーがある）。I でも、後の B が読む前のアンカーは持たせておく。
        let (older, newer) = self.order.references(mediadecoder::es::Coding::B);
        let anchors: Vec<usize> = older
            .into_iter()
            .chain(newer)
            .map(|anchor| anchor.slot)
            .filter(|slot| *slot != held.slot)
            .collect();
        pool.prepare(held.slot, &anchors)?;
        let used: Vec<usize> = forward
            .into_iter()
            .chain(backward)
            .map(|held| held.slot)
            .chain(std::iter::once(held.slot))
            .collect();
        let field = picture.structure != Structure::Frame;
        let parameters = picture_parameters(
            pool, held.slot, forward, backward, format, field, second, coded,
        );
        Ok((held.slot, parameters, used, keep))
    }

    fn finish(&mut self, pool: &mut Pool) {
        self.order.close(pool);
    }
}

#[allow(clippy::too_many_arguments)]
fn picture_parameters(
    pool: &Pool,
    slot: usize,
    forward: Option<Held>,
    backward: Option<Held>,
    format: Format,
    field: bool,
    second: bool,
    coded: &PictureCoding,
) -> DXVA_PictureParameters {
    let index_of = |slot: usize| -> u16 { pool.index(slot).map_or(0xffff, |index| index as u16) };
    let index = |named: Option<Held>| -> u16 { named.map_or(0xffff, |named| index_of(named.slot)) };
    let height_in_mbs = (format.height as usize).div_ceil(16) >> u32::from(field);
    DXVA_PictureParameters {
        wDecodedPictureIndex: index_of(slot),
        // 仕様の説明は 0 だが 0xffff。0 は参照のインデックスとして解釈され、最初の参照へ書かれてしまう。
        wDeblockedPictureIndex: 0xffff,
        wForwardRefPictureIndex: index(forward),
        wBackwardRefPictureIndex: index(backward),
        wPicWidthInMBminus1: (format.width as usize).div_ceil(16).max(1) as u16 - 1,
        wPicHeightInMBminus1: height_in_mbs.max(1) as u16 - 1,
        bMacroblockWidthMinus1: 15,
        bMacroblockHeightMinus1: 15,
        bBlockWidthMinus1: 7,
        bBlockHeightMinus1: 7,
        bBPPminus1: 7,
        bPicStructure: coded.picture_structure as u8,
        bSecondField: u8::from(second),
        bPicIntra: u8::from(coded.kind == 1),
        bPicBackwardPrediction: u8::from(coded.kind == 3),
        bBidirectionalAveragingMode: 0,
        bMVprecisionAndChromaRelation: 0,
        bChromaFormat: 1,
        bPicScanFixed: 1,
        bPicScanMethod: u8::from(coded.alternate_scan),
        bPicReadbackRequests: 0,
        bRcontrol: 0,
        bPicSpatialResid8: 0,
        bPicOverflowBlocks: 0,
        bPicExtrapolation: 0,
        bPicDeblocked: 0,
        bPicDeblockConfined: 0,
        bPic4MVallowed: 0,
        bPicOBMC: 0,
        bPicBinPB: 0,
        bMV_RPS: 0,
        bReservedBits: 0,
        wBitstreamFcodes: f_codes(coded),
        wBitstreamPCEelements: extension(coded),
        bBitstreamConcealmentNeed: 0,
        bBitstreamConcealmentMethod: 0,
    }
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    held: u32,
    count: u32,
}

impl Bits {
    fn put(&mut self, value: u32, width: u32) {
        for at in (0..width).rev() {
            self.held = (self.held << 1) | ((value >> at) & 1);
            self.count += 1;
            if self.count == 8 {
                self.bytes.push(self.held as u8);
                self.held = 0;
                self.count = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push((self.held << (8 - self.count)) as u8);
        }
        self.bytes
    }
}

/// テーブル B.1：マクロブロックアドレス増分。エスケープは 33 を足して読み直す。
fn address_increment(reader: &mut Reader<'_>) -> Option<u32> {
    let mut total = 0u32;
    loop {
        let mut code = 0u32;
        let mut found = None;
        for length in 1..=11usize {
            if reader.empty() {
                return None;
            }
            code = (code << 1) | reader.bit();
            match (length, code) {
                (1, 0b1) => found = Some(1),
                (3, 0b011) => found = Some(2),
                (3, 0b010) => found = Some(3),
                (4, 0b0011) => found = Some(4),
                (4, 0b0010) => found = Some(5),
                (5, 0b00011) => found = Some(6),
                (5, 0b00010) => found = Some(7),
                (7, 0b0000111) => found = Some(8),
                (7, 0b0000110) => found = Some(9),
                (8, 0b00001011) => found = Some(10),
                (8, 0b00001010) => found = Some(11),
                (8, 0b00001001) => found = Some(12),
                (8, 0b00001000) => found = Some(13),
                (8, 0b00000111) => found = Some(14),
                (8, 0b00000110) => found = Some(15),
                (10, 0b0000010111) => found = Some(16),
                (10, 0b0000010110) => found = Some(17),
                (10, 0b0000010101) => found = Some(18),
                (10, 0b0000010100) => found = Some(19),
                (10, 0b0000010011) => found = Some(20),
                (10, 0b0000010010) => found = Some(21),
                (11, 0b00000100011) => found = Some(22),
                (11, 0b00000100010) => found = Some(23),
                (11, 0b00000100001) => found = Some(24),
                (11, 0b00000100000) => found = Some(25),
                (11, 0b00000011111) => found = Some(26),
                (11, 0b00000011110) => found = Some(27),
                (11, 0b00000011101) => found = Some(28),
                (11, 0b00000011100) => found = Some(29),
                (11, 0b00000011011) => found = Some(30),
                (11, 0b00000011010) => found = Some(31),
                (11, 0b00000011001) => found = Some(32),
                (11, 0b00000011000) => found = Some(33),
                (11, 0b00000001000) => {
                    total += 33;
                    break;
                }
                _ => {}
            }
            if let Some(found) = found {
                return Some(total + found);
            }
        }
        if code != 0b00000001000 {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_coding_extension_is_packed_from_the_top() {
        let coded = PictureCoding {
            intra_dc_precision: 2,
            picture_structure: 3,
            top_field_first: true,
            alternate_scan: true,
            progressive_frame: true,
            ..PictureCoding::default()
        };
        let bits = extension(&coded);
        assert_eq!((bits >> 14) & 3, 2);
        assert_eq!((bits >> 12) & 3, 3);
        assert_eq!((bits >> 11) & 1, 1);
        assert_eq!((bits >> 6) & 1, 1);
        assert_eq!((bits >> 3) & 1, 1);
    }

    #[test]
    fn the_address_increment_reads_its_own_table() {
        let one = [0b10000000u8];
        assert_eq!(address_increment(&mut Reader::new(&one)), Some(1));
        let two = [0b01100000u8];
        assert_eq!(address_increment(&mut Reader::new(&two)), Some(2));
        let big = [0b00000011u8, 0b00000000];
        assert_eq!(address_increment(&mut Reader::new(&big)), Some(33));
        let escaped = [0b00000001u8, 0b00010000];
        assert_eq!(address_increment(&mut Reader::new(&escaped)), Some(34));
    }

    #[test]
    fn the_matrices_are_widened_to_sixteen_bits_in_the_scan() {
        let matrices = Matrices::new(Order::AsSent, Chroma::AsSent);
        let data = matrix_data(&matrices);
        assert_eq!({ data.matrix[0][1] }, 16);
    }
}
