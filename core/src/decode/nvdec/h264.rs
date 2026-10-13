// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! H.264 を NVDEC へ。8.2 は [`mediadecoder::video::h264::dpb`] をソフトウェアデコーダーと共有する。
//! 参照の並べ替えは渡さず、DPB そのものを渡す。一枚欠けても順が違っても、エラーにならずに映像が壊れる。

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::h264::dpb::{Marked, Store};
use mediadecoder::video::h264::params::{self, Header, Pps, Sps};

use super::pool::Pool;
use super::{Held, cu};
use crate::decode::Error;
use crate::decode::frames;

/// `used_for_reference` is the pair of marks (1 = top, 2 = bottom, 3 = both).
fn entry(store: &Store<usize, Held>, pool: &Pool) -> cu::H264DpbEntry {
    let long = store.marked.contains(&Marked::Long);
    cu::H264DpbEntry {
        pic_idx: pool.id(store.slot),
        frame_idx: if long {
            store.long_index
        } else {
            store.frame_num as i32
        },
        is_long_term: i32::from(long),
        not_existing: 0,
        used_for_reference: i32::from(store.marked[0] != Marked::No)
            | (i32::from(store.marked[1] != Marked::No) << 1),
        field_order_cnt: [
            if store.has_poc[0] { store.poc[0] } else { 0 },
            if store.has_poc[1] { store.poc[1] } else { 0 },
        ],
    }
}

#[derive(Debug)]
pub struct H264 {
    framing: Framing,
    sets: params::Sets,
    order: frames::H264Order<usize>,
}

impl H264 {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            sets: params::Sets::default(),
            order: frames::H264Order::new(),
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
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        let mut params: Option<cu::PicParams> = None;
        let mut bitstream: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5) & 3;
            let bytes = data.get(nal.body.clone()).unwrap_or(&[]);
            match kind {
                7 | 8 => self.sets.absorb(kind, bytes),
                1 | 5 => {
                    // スタートコードはこちらで 3 バイト書く（長さ前置きの MMT でも同じ形）。
                    let unit = data
                        .get(nal.body.start.saturating_sub(1)..nal.body.end)
                        .unwrap_or(&[]);
                    if let Err(error) = self.slice(
                        pool,
                        tag,
                        picture,
                        kind == 5,
                        idc,
                        bytes,
                        unit,
                        &mut params,
                        &mut bitstream,
                        &mut offsets,
                    ) {
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }

        let Some(mut params) = params else {
            return match damage {
                Some(error) => Err(error),
                None => Ok(()),
            };
        };
        params.num_slices = offsets.len() as u32;
        params.bitstream_data_len = bitstream.len() as u32;
        params.bitstream_data = bitstream.as_ptr();
        params.slice_data_offsets = offsets.as_ptr();
        let submitted = decoder.decode(&mut params);
        drop(bitstream);
        drop(offsets);
        submitted?;
        self.order.finish(pool);
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slice(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        idr: bool,
        idc: u32,
        bytes: &[u8],
        unit: &[u8],
        params: &mut Option<cu::PicParams>,
        bitstream: &mut Vec<u8>,
        offsets: &mut Vec<u32>,
    ) -> Result<(), Error> {
        let data = unescape(bytes, bytes.len());
        let found = params::header(&data, idr, idc, |id| self.sets.find(id));
        let Some((header, pps, sps)) = found else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        if header.redundant != 0 {
            return Ok(());
        }

        if params.is_none() {
            self.order.start(pool, tag, picture, &header, &sps)?;
            let Some(current) = self.order.current.clone() else {
                return Err(Error::Damaged("a slice with no picture"));
            };
            let at = self
                .order
                .dpb
                .index(current.serial)
                .ok_or(Error::Damaged("a picture that fell out of the buffer"))?;
            *params = Some(self.picture_parameters(pool, tag, &header, &sps, &pps, at));
        }

        self.order.dpb.number_frames(&sps, header.frame_num);
        offsets.push(bitstream.len() as u32);
        bitstream.extend_from_slice(&[0, 0, 1]);
        bitstream.extend_from_slice(unit);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn picture_parameters(
        &self,
        pool: &Pool,
        tag: u64,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        at: usize,
    ) -> cu::PicParams {
        let store = &self.order.dpb.stores[at];
        let parity = usize::from(header.bottom_field);
        let mut dpb = [cu::H264DpbEntry {
            // 使っていないスロットは -1。0 だと 0 番のサーフェスを指す。
            pic_idx: -1,
            ..cu::H264DpbEntry::default()
        }; 16];
        let mut count = 0;
        for other in &self.order.dpb.stores {
            if !other.reference() || count == 16 {
                continue;
            }
            dpb[count] = entry(other, pool);
            count += 1;
        }
        let weights = params::weights(sps, pps);
        let mut out = cu::PicParams {
            width_in_mbs: sps.mb_width as i32,
            // フィールドピクチャーでもフレームの高さ（7.4.2.1.1）。
            height_in_mbs: sps.mb_height().max(1) as i32,
            curr_pic_idx: pool.id(store.slot),
            field_pic_flag: i32::from(header.field_pic),
            bottom_field_flag: i32::from(header.bottom_field),
            second_field: i32::from(header.field_pic && store.present[1 - parity]),
            ref_pic_flag: i32::from(header.nal_ref_idc != 0),
            intra_pic_flag: i32::from(header.kind.intra()),
            timestamp: tag as i64,
            ..cu::PicParams::default()
        };
        out.codec.put(&cu::H264PicParams {
            log2_max_frame_num_minus4: sps.log2_max_frame_num as i32 - 4,
            pic_order_cnt_type: sps.poc_type as i32,
            log2_max_pic_order_cnt_lsb_minus4: sps.log2_max_poc_lsb.saturating_sub(4) as i32,
            delta_pic_order_always_zero_flag: i32::from(sps.poc_always_zero),
            frame_mbs_only_flag: i32::from(sps.frame_mbs_only),
            direct_8x8_inference_flag: i32::from(sps.direct_8x8_inference),
            num_ref_frames: sps.max_ref_frames as i32,
            residual_colour_transform_flag: u8::from(sps.separate_planes),
            bit_depth_luma_minus8: sps.depth_luma.saturating_sub(8) as u8,
            bit_depth_chroma_minus8: sps.depth_chroma.saturating_sub(8) as u8,
            qpprime_y_zero_transform_bypass_flag: 0,
            entropy_coding_mode_flag: i32::from(pps.cabac),
            pic_order_present_flag: i32::from(pps.bottom_field_order_present),
            num_ref_idx_l0_active_minus1: pps.num_ref_idx[0].max(1) as i32 - 1,
            num_ref_idx_l1_active_minus1: pps.num_ref_idx[1].max(1) as i32 - 1,
            weighted_pred_flag: i32::from(pps.weighted_pred),
            weighted_bipred_idc: pps.weighted_bipred as i32,
            pic_init_qp_minus26: pps.init_qp - 26,
            deblocking_filter_control_present_flag: i32::from(pps.deblocking_control),
            redundant_pic_cnt_present_flag: i32::from(pps.redundant_present),
            transform_8x8_mode_flag: i32::from(pps.transform_8x8),
            mbaff_frame_flag: i32::from(sps.mb_adaptive && !header.field_pic),
            constrained_intra_pred_flag: i32::from(pps.constrained_intra),
            chroma_qp_index_offset: pps.chroma_qp_offset[0],
            second_chroma_qp_index_offset: pps.chroma_qp_offset[1],
            ref_pic_flag: i32::from(header.nal_ref_idc != 0),
            frame_num: header.frame_num as i32,
            curr_field_order_cnt: [
                if store.has_poc[0] { store.poc[0] } else { 0 },
                if store.has_poc[1] { store.poc[1] } else { 0 },
            ],
            dpb,
            // `params::weights` は既にラスター順。HEVC と違い VA-API と同じ並び。
            weight_scale_4x4: weights.four,
            weight_scale_8x8: [weights.eight[0], weights.eight[1]],
            pic_init_qs_minus26: (pps.init_qs - 26) as i8,
            // 下位ビットがフラグ、その上は「下位ビットを読め」。立てないと無視される。
            mb_adaptive_frame_field_flag: u32::from(sps.mb_adaptive) | 2,
            ..cu::H264PicParams::default()
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unused_buffer_entry_names_no_picture() {
        assert_eq!(cu::H264DpbEntry::default().pic_idx, 0);
        let store: Store<usize, Held> = Store {
            slot: 3,
            serial: 0,
            frame_num: 7,
            wrap: 7,
            long_index: 0,
            present: [true, false],
            has_poc: [true, false],
            poc: [4, 0],
            marked: [Marked::Short, Marked::No],
            output: true,
            held: Held::default(),
        };
        let entry = entry(&store, &Pool::new(8));
        assert_eq!(entry.pic_idx, 3);
        assert_eq!(entry.frame_idx, 7);
        assert_eq!(entry.used_for_reference, 1);
        assert_eq!(entry.field_order_cnt, [4, 0]);
    }
}
