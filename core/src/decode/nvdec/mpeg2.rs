// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 を NVDEC へ。ヘッダーの解析と並べ替えはソフトウェアデコーダーと同じものを使う。
//! 量子化行列はラスター順で渡す。転置しても壊れず画質がずれるだけ。
//! スライスはスタートコードごと連続させ、位置を `pSliceDataOffsets` で渡す。

use mediadecoder::es::{Format, Picture, StartCodes, Structure};
use mediadecoder::video::mpeg2::headers::{self, Chroma, Matrices, Order, PictureCoding};

use super::Held;
use super::cu;
use super::pool::Pool;
use crate::decode::Error;
use crate::decode::frames;

#[derive(Debug)]
pub struct Mpeg2 {
    matrices: Matrices,
    order: frames::Mpeg2Order<usize>,
}

impl Default for Mpeg2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpeg2 {
    pub fn new() -> Self {
        Self {
            // 色差の行列は渡す場所が無いが、輝度の行列しか送られないので失うものは無い。
            matrices: Matrices::new(Order::Raster, Chroma::AsSent),
            order: frames::Mpeg2Order::new(),
        }
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        self.order.reset(pool);
        self.matrices = Matrices::new(Order::Raster, Chroma::AsSent);
    }

    pub fn decode(
        &mut self,
        decoder: &cu::Decoder,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        let mut coded = PictureCoding::default();
        let mut params: Option<cu::PicParams> = None;
        let mut bitstream: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();

        for (kind, body) in StartCodes::new(data) {
            let bytes = &data[body.clone()];
            match kind {
                0xb3 => self.matrices.read_sequence(bytes),
                0xb5 => match headers::extension_id(bytes) {
                    headers::QUANT_MATRIX_EXTENSION => self.matrices.read_extension(bytes),
                    headers::PICTURE_CODING_EXTENSION => coded.read_extension(bytes),
                    _ => {}
                },
                0x00 => {
                    coded = PictureCoding {
                        kind: headers::read_picture(bytes),
                        ..PictureCoding::default()
                    };
                }
                0x01..=0xaf if coded.kind != 0 => {
                    if params.is_none() {
                        params = Some(self.begin(pool, tag, picture, format, &coded)?);
                    }
                    let from = body.start.saturating_sub(4);
                    offsets.push(bitstream.len() as u32);
                    bitstream.extend_from_slice(&data[from..body.end]);
                }
                _ => {}
            }
        }

        let Some(mut params) = params else {
            return Ok(());
        };
        params.num_slices = offsets.len() as u32;
        params.bitstream_data_len = bitstream.len() as u32;
        params.bitstream_data = bitstream.as_ptr();
        params.slice_data_offsets = offsets.as_ptr();
        let submitted = decoder.decode(&mut params);
        drop(bitstream);
        drop(offsets);
        submitted?;
        if self.order.pending() != Some(picture.structure) {
            self.finish(pool);
        }
        Ok(())
    }

    fn begin(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        coded: &PictureCoding,
    ) -> Result<cu::PicParams, Error> {
        // ペアの二つ目のフィールドは同じサーフェスに書き、一つ目から予測しうる。
        let second = self.order.begin(pool, tag, picture)?;
        let held = self
            .order
            .current()
            .ok_or(Error::Damaged("a slice with no picture"))?;

        let (forward, backward) = self.order.references(picture.coding);
        let mut params = cu::PicParams {
            width_in_mbs: format.width.div_ceil(16) as i32,
            // フィールドピクチャーでもフレームの高さ。
            height_in_mbs: format.height.div_ceil(16) as i32,
            curr_pic_idx: pool.id(held.slot),
            field_pic_flag: i32::from(picture.structure != Structure::Frame),
            bottom_field_flag: i32::from(picture.structure == Structure::Bottom),
            second_field: i32::from(second),
            ref_pic_flag: i32::from(coded.kind != 3),
            intra_pic_flag: i32::from(coded.kind == 1),
            timestamp: tag as i64,
            ..cu::PicParams::default()
        };
        params.codec.put(&cu::Mpeg2PicParams {
            forward_ref_idx: forward.map_or(-1, |held| pool.id(held.slot)),
            backward_ref_idx: backward.map_or(-1, |held| pool.id(held.slot)),
            picture_coding_type: coded.kind as i32,
            full_pel_forward_vector: 0,
            full_pel_backward_vector: 0,
            f_code: [
                [coded.f_code[0][0] as i32, coded.f_code[0][1] as i32],
                [coded.f_code[1][0] as i32, coded.f_code[1][1] as i32],
            ],
            intra_dc_precision: coded.intra_dc_precision as i32,
            frame_pred_frame_dct: i32::from(coded.frame_pred_frame_dct),
            concealment_motion_vectors: i32::from(coded.concealment_motion_vectors),
            q_scale_type: i32::from(coded.q_scale_type),
            intra_vlc_format: i32::from(coded.intra_vlc_format),
            alternate_scan: i32::from(coded.alternate_scan),
            top_field_first: i32::from(coded.top_field_first),
            quant_matrix_intra: self.matrices.intra,
            quant_matrix_inter: self.matrices.non_intra,
        });
        Ok(params)
    }

    fn finish(&mut self, pool: &mut Pool) {
        self.order.close(pool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrix_nvdec_is_given_is_the_raster_of_the_standards_table() {
        let matrices = Matrices::new(Order::Raster, Chroma::AsSent);
        assert_eq!(matrices.intra, headers::DEFAULT_INTRA);
        assert_eq!(matrices.non_intra, headers::DEFAULT_NON_INTRA);
    }
}
