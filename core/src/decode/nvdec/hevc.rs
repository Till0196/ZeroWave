// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC を NVDEC へ。ヘッダーの解析と DPB は [`mediadecoder::video::hevc`] のものを使う。
//! 量子化行列は対角順（diag order）で渡す。逆だと壊れずに画質がずれるだけ。
//! 参照は DPB とインデックスの配列で渡し、並べ替え（8.3.4）は NVDEC がする。

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::hevc::dpb::{Entry, Marked};
use mediadecoder::video::hevc::params::{self, Header, Pps, Sps};
use mediadecoder::video::hevc::scaling;

use super::pool::Pool;
use super::{Held, cu};
use crate::decode::Error;
use crate::decode::frames;
use crate::decode::rps::{Shape, short_term_bits};

const FRAMES: usize = 16;

#[derive(Debug, Clone)]
struct Current {
    slot: usize,
    poc: i32,
    held: Held,
    output: bool,
    header: Box<Header>,
}

#[derive(Debug)]
pub struct Hevc {
    framing: Framing,
    sets: params::Sets,
    order: frames::HevcOrder<usize>,
    current: Option<Current>,
    frames: [Option<(usize, i32)>; FRAMES],
}

impl Hevc {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            sets: params::Sets::default(),
            order: frames::HevcOrder::new(),
            current: None,
            frames: [None; FRAMES],
        }
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.finish(pool);
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        let open = self.current.take().map(|current| current.slot);
        self.order.reset(pool, open);
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
            return Err(Error::Unsupported("a chroma format other than 4:2:0"));
        }
        let mut params: Option<cu::PicParams> = None;
        let mut bitstream: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let second = data.get(nal.body.start).copied().unwrap_or(0);
            let temporal = (second & 7).saturating_sub(1);
            if (nal.header & 1) != 0 || (second >> 3) != 0 {
                continue;
            }
            let bytes = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            match kind {
                params::SPS_NUT | params::PPS_NUT => {
                    let _ = self.sets.absorb(kind, bytes);
                }
                kind if params::is_slice(kind) => {
                    let unit = data.get(nal.body.start - 1..nal.body.end).unwrap_or(&[]);
                    if let Err(error) = self.slice(
                        pool,
                        tag,
                        picture,
                        kind,
                        temporal,
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
        self.finish(pool);
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
        kind: u8,
        temporal: u8,
        bytes: &[u8],
        unit: &[u8],
        params: &mut Option<cu::PicParams>,
        bitstream: &mut Vec<u8>,
        offsets: &mut Vec<u32>,
    ) -> Result<(), Error> {
        let data = unescape(bytes, bytes.len());
        let mut reader = mediadecoder::bits::Reader::new(&data);
        reader.bit();
        if params::is_irap(kind) {
            reader.bit();
        }
        let pps_id = reader.ue();
        let Some((pps, sps)) = self.sets.find(pps_id) else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        let previous = self
            .current
            .as_ref()
            .map(|current| (*current.header).clone());
        let header = params::header(&data, kind, &sps, &pps, previous.as_ref())?;

        if header.first_segment {
            // 投入が失敗すると `finish` を通らないので、前のフレームのスロットをここで返す。
            if let Some(stale) = self.current.take() {
                pool.release(stale.slot);
            }
            let capacity = sps.max_dec_pic_buffering.clamp(1, 16);
            self.order
                .dpb
                .limits(sps.max_num_reorder.min(capacity), capacity);
            let Some((slot, poc)) = self.order.begin(pool, &sps, &header, kind, temporal)? else {
                // デコードを始める前のピクチャーを参照するリーディングピクチャー。
                return Ok(());
            };
            self.current = Some(Current {
                slot,
                poc,
                held: frames::HevcOrder::held(slot, tag, picture, poc),
                output: header.pic_output_flag
                    && !(params::is_rasl(kind) && self.order.dpb.drop_rasl()),
                header: Box::new(header.clone()),
            });
            let rps = short_term_bits(&data, kind, &Shape::of(&sps, &pps), &sps.short_term);
            *params = Some(self.picture_parameters(pool, tag, &sps, &pps, &header, kind, poc, rps));
        } else if self.order.skipping || params.is_none() {
            return Ok(());
        } else if self.current.is_none() {
            return Err(Error::Damaged("a slice segment with no picture"));
        }
        if !header.dependent
            && !header.first_segment
            && let Some(current) = self.current.as_mut()
        {
            *current.header = header.clone();
        }

        offsets.push(bitstream.len() as u32);
        // スタートコードはこちらで 3 バイト書く（MMT の長さ前置きでも）。4 バイトだとフレームが出ない。
        bitstream.extend_from_slice(&[0, 0, 1]);
        bitstream.extend_from_slice(unit);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn picture_parameters(
        &mut self,
        pool: &Pool,
        tag: u64,
        sps: &Sps,
        pps: &Pps,
        header: &Header,
        kind: u8,
        poc: i32,
        rps: (i32, i32),
    ) -> cu::PicParams {
        let mut frames = [None; FRAMES];
        let mut ref_pic_idx = [-1i32; 16];
        let mut pic_order_cnt_val = [0i32; 16];
        let mut is_long_term = [0u8; 16];
        for (at, store) in self.order.dpb.references().take(FRAMES).enumerate() {
            frames[at] = Some((store.id, store.poc));
            ref_pic_idx[at] = pool.id(store.id);
            pic_order_cnt_val[at] = store.poc;
            is_long_term[at] = u8::from(store.marked == Marked::Long);
        }
        self.frames = frames;

        let sets = self.order.dpb.sets.clone();
        let mut indices = [[0u8; 8]; 3];
        for (which, list) in sets.iter().enumerate() {
            for (at, entry) in list.iter().take(8).enumerate() {
                // 一度も持っていない参照。0xff は仕様に無いので 0 を置く（映像は壊れる）。
                let found = self.index_of(entry);
                indices[which][at] = if found == 0xff { 0 } else { found };
            }
        }

        let mut out = cu::PicParams {
            width_in_mbs: (sps.width as i32).div_euclid(16),
            height_in_mbs: (sps.height as i32).div_euclid(16),
            curr_pic_idx: pool.id(self.current.as_ref().map_or(0, |current| current.slot)),
            field_pic_flag: 0,
            bottom_field_flag: 0,
            second_field: 0,
            ref_pic_flag: 1,
            intra_pic_flag: i32::from(header.kind.intra()),
            timestamp: tag as i64,
            ..cu::PicParams::default()
        };
        let scaling = pps.scaling.as_ref().unwrap_or(&sps.scaling);
        let lists = scaling_lists(scaling);
        let mut hevc = cu::HevcPicParams {
            pic_width_in_luma_samples: sps.width as i32,
            pic_height_in_luma_samples: sps.height as i32,
            log2_min_luma_coding_block_size_minus3: (sps.min_cb_log2 - 3) as u8,
            log2_diff_max_min_luma_coding_block_size: (sps.ctb_log2 - sps.min_cb_log2) as u8,
            log2_min_transform_block_size_minus2: (sps.min_tb_log2 - 2) as u8,
            log2_diff_max_min_transform_block_size: (sps.max_tb_log2 - sps.min_tb_log2) as u8,
            pcm_enabled_flag: u8::from(sps.pcm_enabled),
            log2_min_pcm_luma_coding_block_size_minus3: (sps.pcm_min_log2.max(3) - 3) as u8,
            log2_diff_max_min_pcm_luma_coding_block_size: sps
                .pcm_max_log2
                .saturating_sub(sps.pcm_min_log2)
                as u8,
            pcm_sample_bit_depth_luma_minus1: sps.pcm_depth[0].saturating_sub(1),
            pcm_sample_bit_depth_chroma_minus1: sps.pcm_depth[1].saturating_sub(1),
            pcm_loop_filter_disabled_flag: u8::from(sps.pcm_loop_filter_disabled),
            strong_intra_smoothing_enabled_flag: u8::from(sps.strong_intra_smoothing),
            max_transform_hierarchy_depth_intra: sps.max_transform_depth_intra as u8,
            max_transform_hierarchy_depth_inter: sps.max_transform_depth_inter as u8,
            amp_enabled_flag: u8::from(sps.amp_enabled),
            separate_colour_plane_flag: 0,
            log2_max_pic_order_cnt_lsb_minus4: (sps.log2_max_poc_lsb - 4) as u8,
            num_short_term_ref_pic_sets: sps.short_term.len() as u8,
            long_term_ref_pics_present_flag: u8::from(sps.long_term_present),
            num_long_term_ref_pics_sps: sps.long_term_poc_lsb.len() as u8,
            sps_temporal_mvp_enabled_flag: u8::from(sps.temporal_mvp_enabled),
            sample_adaptive_offset_enabled_flag: u8::from(sps.sao_enabled),
            scaling_list_enable_flag: u8::from(sps.scaling_list_enabled),
            irap_pic_flag: u8::from(params::is_irap(kind)),
            idr_pic_flag: u8::from(params::is_idr(kind)),
            bit_depth_luma_minus8: sps.depth[0].saturating_sub(8),
            bit_depth_chroma_minus8: sps.depth[1].saturating_sub(8),
            dependent_slice_segments_enabled_flag: u8::from(pps.dependent_slice_segments_enabled),
            slice_segment_header_extension_present_flag: u8::from(
                pps.slice_header_extension_present,
            ),
            sign_data_hiding_enabled_flag: u8::from(pps.sign_data_hiding),
            cu_qp_delta_enabled_flag: u8::from(pps.cu_qp_delta_enabled),
            diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth as u8,
            init_qp_minus26: (pps.init_qp - 26) as i8,
            pps_cb_qp_offset: pps.qp_offset[0] as i8,
            pps_cr_qp_offset: pps.qp_offset[1] as i8,
            constrained_intra_pred_flag: u8::from(pps.constrained_intra_pred),
            weighted_pred_flag: u8::from(pps.weighted_pred),
            weighted_bipred_flag: u8::from(pps.weighted_bipred),
            transform_skip_enabled_flag: u8::from(pps.transform_skip_enabled),
            transquant_bypass_enabled_flag: u8::from(pps.transquant_bypass_enabled),
            entropy_coding_sync_enabled_flag: u8::from(pps.entropy_coding_sync_enabled),
            log2_parallel_merge_level_minus2: (pps.log2_parallel_merge_level - 2) as u8,
            num_extra_slice_header_bits: pps.num_extra_slice_header_bits as u8,
            loop_filter_across_tiles_enabled_flag: u8::from(pps.loop_filter_across_tiles),
            loop_filter_across_slices_enabled_flag: u8::from(pps.loop_filter_across_slices),
            output_flag_present_flag: u8::from(pps.output_flag_present),
            num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_default[0].max(1) as u8 - 1,
            num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_default[1].max(1) as u8 - 1,
            lists_modification_present_flag: u8::from(pps.lists_modification_present),
            cabac_init_present_flag: u8::from(pps.cabac_init_present),
            pps_slice_chroma_qp_offsets_present_flag: u8::from(pps.slice_chroma_qp_offsets_present),
            deblocking_filter_override_enabled_flag: u8::from(pps.deblocking_override_enabled),
            pps_deblocking_filter_disabled_flag: u8::from(pps.deblocking_disabled),
            pps_beta_offset_div2: (pps.beta_offset / 2) as i8,
            pps_tc_offset_div2: (pps.tc_offset / 2) as i8,
            tiles_enabled_flag: u8::from(pps.tiles_enabled),
            // タイルが無いときに 0 にすると幅 0 のタイル列が一本あることになる。
            uniform_spacing_flag: u8::from(pps.column_width.is_empty()),
            num_tile_columns_minus1: pps.column_width.len().max(1) as u8 - 1,
            num_tile_rows_minus1: pps.row_height.len().max(1) as u8 - 1,
            num_bits_for_short_term_rps_in_slice: rps.0,
            num_delta_pocs_of_ref_rps_idx: rps.1,
            num_poc_total_curr: self.order.dpb.total as i32,
            num_poc_st_curr_before: sets[0].len() as i32,
            num_poc_st_curr_after: sets[1].len() as i32,
            num_poc_lt_curr: sets[2].len() as i32,
            curr_pic_order_cnt_val: poc,
            ref_pic_idx,
            pic_order_cnt_val,
            is_long_term,
            ref_pic_set_st_curr_before: indices[0],
            ref_pic_set_st_curr_after: indices[1],
            ref_pic_set_lt_curr: indices[2],
            scaling_list_4x4: lists.four,
            scaling_list_8x8: lists.eight,
            scaling_list_16x16: lists.sixteen,
            scaling_list_32x32: lists.thirty_two,
            scaling_list_dc_16x16: lists.dc_sixteen,
            scaling_list_dc_32x32: lists.dc_thirty_two,
            ..cu::HevcPicParams::default()
        };
        for (at, width) in pps.column_width.iter().take(21).enumerate() {
            hevc.column_width_minus1[at] = width.max(&1).saturating_sub(1) as u16;
        }
        for (at, height) in pps.row_height.iter().take(21).enumerate() {
            hevc.row_height_minus1[at] = height.max(&1).saturating_sub(1) as u16;
        }
        out.codec.put(&hevc);
        out
    }

    fn index_of(&self, entry: &Entry<usize>) -> u8 {
        let Some(slot) = entry.id else {
            return 0xff;
        };
        for (at, frame) in self.frames.iter().enumerate() {
            if *frame == Some((slot, entry.poc)) {
                return at as u8;
            }
        }
        0xff
    }
}

impl Hevc {
    fn finish(&mut self, pool: &mut Pool) {
        let Some(current) = self.current.take() else {
            return;
        };
        self.order.push(
            pool,
            current.slot,
            current.poc,
            current.output,
            current.held,
        );
    }
}

#[derive(Debug)]
struct ScalingLists {
    four: [[u8; 16]; 6],
    eight: [[u8; 64]; 6],
    sixteen: [[u8; 64]; 6],
    thirty_two: [[u8; 64]; 2],
    dc_sixteen: [u8; 6],
    dc_thirty_two: [u8; 2],
}

/// The quantiser matrices, as NVDEC wants them: in diagonal scan (6.5.3), not raster.
/// 16x16 / 32x32 の DC は `ScalingListDC` で上書きする（7.4.5）。
fn scaling_lists(matrices: &params::Scaling) -> ScalingLists {
    let list = |size: usize, matrix: usize| -> [u8; 64] {
        let raster = scaling::list(matrices, size, matrix);
        std::array::from_fn(|at| raster[scaling::DIAGONAL_8[at]])
    };
    ScalingLists {
        four: std::array::from_fn(|matrix| {
            let raster = scaling::list4(matrices, matrix);
            std::array::from_fn(|at| raster[scaling::DIAGONAL_4[at]])
        }),
        eight: std::array::from_fn(|matrix| list(1, matrix)),
        sixteen: std::array::from_fn(|matrix| list(2, matrix)),
        thirty_two: std::array::from_fn(|matrix| list(3, matrix * 3)),
        dc_sixteen: std::array::from_fn(|matrix| matrices.matrix(2, matrix)[0]),
        dc_thirty_two: std::array::from_fn(|matrix| matrices.matrix(3, matrix * 3)[0]),
    }
}
