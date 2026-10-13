// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC, handed to the hardware. Parsing and clause 8.3 live in [`mediadecoder::video::hevc`];
//! what is left here is copying into VA-API's buffers and `ReferenceFrames` indices.

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::hevc::dpb::{Entry, Marked};
use mediadecoder::video::hevc::params::{self, Header, Kind, Pps, Sps};
use mediadecoder::video::hevc::{escapes_before, scaling};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::vaapi::pool::Pool;
use crate::decode::vaapi::{Held, va};
use va::{VABufferID, VAContextID, VASurfaceID};

const FRAMES: usize = 15;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PictureHEVC {
    picture_id: VASurfaceID,
    pic_order_cnt: i32,
    flags: u32,
    va_reserved: [u32; 4],
}

impl Default for PictureHEVC {
    fn default() -> Self {
        Self {
            picture_id: va::VA_INVALID_SURFACE,
            pic_order_cnt: 0,
            flags: va::VA_PICTURE_HEVC_INVALID,
            va_reserved: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PictureParameters {
    current: PictureHEVC,
    references: [PictureHEVC; FRAMES],
    width: u16,
    height: u16,
    pic_fields: u32,
    max_dec_pic_buffering_minus1: u8,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    pcm_depth_luma_minus1: u8,
    pcm_depth_chroma_minus1: u8,
    log2_min_cb_minus3: u8,
    log2_diff_max_min_cb: u8,
    log2_min_tb_minus2: u8,
    log2_diff_max_min_tb: u8,
    log2_min_pcm_cb_minus3: u8,
    log2_diff_max_min_pcm_cb: u8,
    max_transform_hierarchy_depth_intra: u8,
    max_transform_hierarchy_depth_inter: u8,
    init_qp_minus26: i8,
    diff_cu_qp_delta_depth: u8,
    pps_cb_qp_offset: i8,
    pps_cr_qp_offset: i8,
    log2_parallel_merge_level_minus2: u8,
    num_tile_columns_minus1: u8,
    num_tile_rows_minus1: u8,
    column_width_minus1: [u16; 19],
    row_height_minus1: [u16; 21],
    slice_parsing_fields: u32,
    log2_max_poc_lsb_minus4: u8,
    num_short_term_ref_pic_sets: u8,
    num_long_term_ref_pic_sps: u8,
    num_ref_idx_l0_default_active_minus1: u8,
    num_ref_idx_l1_default_active_minus1: u8,
    pps_beta_offset_div2: i8,
    pps_tc_offset_div2: i8,
    num_extra_slice_header_bits: u8,
    st_rps_bits: u32,
    va_reserved: [u32; 8],
}

super::va::zeroed_types!(PictureParameters, SliceParameters);

impl Default for PictureParameters {
    fn default() -> Self {
        // 予約フィールドも 0 でなければならない。
        let mut out: Self = super::va::zeroed();
        out.current = PictureHEVC::default();
        out.references = [PictureHEVC::default(); FRAMES];
        out
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct IqMatrix {
    four: [[u8; 16]; 6],
    eight: [[u8; 64]; 6],
    sixteen: [[u8; 64]; 6],
    thirty_two: [[u8; 64]; 2],
    dc_sixteen: [u8; 6],
    dc_thirty_two: [u8; 2],
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SliceParameters {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    slice_data_byte_offset: u32,
    slice_segment_address: u32,
    lists: [[u8; FRAMES]; 2],
    long_slice_flags: u32,
    collocated_ref_idx: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    slice_qp_delta: i8,
    slice_cb_qp_offset: i8,
    slice_cr_qp_offset: i8,
    slice_beta_offset_div2: i8,
    slice_tc_offset_div2: i8,
    luma_log2_weight_denom: u8,
    delta_chroma_log2_weight_denom: i8,
    delta_luma_weight_l0: [i8; FRAMES],
    luma_offset_l0: [i8; FRAMES],
    delta_chroma_weight_l0: [[i8; 2]; FRAMES],
    chroma_offset_l0: [[i8; 2]; FRAMES],
    delta_luma_weight_l1: [i8; FRAMES],
    luma_offset_l1: [i8; FRAMES],
    delta_chroma_weight_l1: [[i8; 2]; FRAMES],
    chroma_offset_l1: [[i8; 2]; FRAMES],
    five_minus_max_num_merge_cand: u8,
    num_entry_point_offsets: u16,
    entry_offset_to_subset_array: u16,
    slice_data_num_emu_prevn_bytes: u16,
    va_reserved: [u32; 2],
}

impl Default for SliceParameters {
    fn default() -> Self {
        let mut out: Self = super::va::zeroed();
        out.lists = [[0xff; FRAMES]; 2];
        out.collocated_ref_idx = 0xff;
        out
    }
}

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
    frames: [PictureHEVC; FRAMES],
}

impl Hevc {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            sets: params::Sets::default(),
            order: frames::HevcOrder::new(),
            current: None,
            frames: [PictureHEVC::default(); FRAMES],
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

    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        display: &va::Display,
        context: VAContextID,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format other than 4:2:0"));
        }
        let mut buffers = va::Buffers::new(display);
        let mut surface = va::VA_INVALID_SURFACE;
        let mut started = false;
        let mut damage = None;
        // Kept until the unit is read, so exactly one slice says it is the last; otherwise the
        // slices decode as pictures over each other.
        let mut slices: Vec<(SliceParameters, std::ops::Range<usize>)> = Vec::new();

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
                    let raw = data.get(nal.body.start - 1..nal.body.end).unwrap_or(&[]);
                    if let Err(error) = self.slice(
                        display,
                        context,
                        pool,
                        tag,
                        picture,
                        kind,
                        temporal,
                        bytes,
                        raw,
                        &mut buffers,
                        &mut slices,
                        nal.body.start - 1..nal.body.end,
                        &mut surface,
                        &mut started,
                    ) {
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }

        if !started {
            return match damage {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        if let Some((last, _)) = slices.last_mut() {
            last.long_slice_flags |= 1;
        }
        for (parameters, range) in &slices {
            buffers.push(display.parameters(
                context,
                va::VA_SLICE_PARAMETER_BUFFER_TYPE,
                parameters,
            )?);
            buffers.push(display.coded(context, &data[range.clone()])?);
        }
        display.decode(context, surface, &mut buffers)?;
        // A picture stays open until the next one begins (one picture over several access units).
        self.finish(pool);
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slice(
        &mut self,
        display: &va::Display,
        context: VAContextID,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        kind: u8,
        temporal: u8,
        bytes: &[u8],
        raw: &[u8],
        buffers: &mut Vec<VABufferID>,
        slices: &mut Vec<(SliceParameters, std::ops::Range<usize>)>,
        range: std::ops::Range<usize>,
        surface: &mut VASurfaceID,
        started: &mut bool,
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
            let capacity = sps.max_dec_pic_buffering.clamp(1, 16);
            self.order
                .dpb
                .limits(sps.max_num_reorder.min(capacity), capacity);
            let Some((slot, poc)) = self.order.begin(pool, &sps, &header, kind, temporal)? else {
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
            *surface = pool.id(slot);
            let parameters = self.picture_parameters(pool, &sps, &pps, &header, kind, poc);
            buffers.push(display.parameters(
                context,
                va::VA_PICTURE_PARAMETER_BUFFER_TYPE,
                &parameters,
            )?);
            if sps.scaling_list_enabled {
                let scaling = pps.scaling.as_ref().unwrap_or(&sps.scaling);
                buffers.push(display.parameters(
                    context,
                    va::VA_IQ_MATRIX_BUFFER_TYPE,
                    &iq_matrix(scaling),
                )?);
            }
            *started = true;
        } else if self.order.skipping || !*started {
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

        slices.push((
            self.slice_parameters(&sps, &pps, &header, bytes, raw),
            range,
        ));
        Ok(())
    }

    fn picture_parameters(
        &mut self,
        pool: &Pool,
        sps: &Sps,
        pps: &Pps,
        header: &Header,
        kind: u8,
        poc: i32,
    ) -> PictureParameters {
        // Each entry names its clause 8.3.2 set; the driver reads list indices through it.
        let mut frames = [PictureHEVC::default(); FRAMES];
        for (at, store) in self.order.dpb.references().take(FRAMES).enumerate() {
            let mut flags = if store.marked == Marked::Long {
                va::VA_PICTURE_HEVC_LONG_TERM_REFERENCE
            } else {
                0
            };
            for (which, bit) in [
                va::VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE,
                va::VA_PICTURE_HEVC_RPS_ST_CURR_AFTER,
                va::VA_PICTURE_HEVC_RPS_LT_CURR,
            ]
            .into_iter()
            .enumerate()
            {
                if self.order.dpb.sets[which]
                    .iter()
                    .any(|entry| entry.id == Some(store.id) && entry.poc == store.poc)
                {
                    flags |= bit;
                }
            }
            frames[at] = PictureHEVC {
                picture_id: pool.id(store.id),
                pic_order_cnt: store.poc,
                flags,
                va_reserved: [0; 4],
            };
        }
        self.frames = frames;

        let bit = |value: bool, at: u32| u32::from(value) << at;
        let pic_fields = (sps.chroma_format & 3)
            | bit(sps.pcm_enabled, 3)
            | bit(sps.scaling_list_enabled, 4)
            | bit(pps.transform_skip_enabled, 5)
            | bit(sps.amp_enabled, 6)
            | bit(sps.strong_intra_smoothing, 7)
            | bit(pps.sign_data_hiding, 8)
            | bit(pps.constrained_intra_pred, 9)
            | bit(pps.cu_qp_delta_enabled, 10)
            | bit(pps.weighted_pred, 11)
            | bit(pps.weighted_bipred, 12)
            | bit(pps.transquant_bypass_enabled, 13)
            | bit(pps.tiles_enabled, 14)
            | bit(pps.entropy_coding_sync_enabled, 15)
            | bit(pps.loop_filter_across_slices, 16)
            | bit(pps.loop_filter_across_tiles, 17)
            | bit(sps.pcm_loop_filter_disabled, 18)
            | bit(sps.max_num_reorder == 0, 19);
        let slice_parsing = bit(pps.lists_modification_present, 0)
            | bit(sps.long_term_present, 1)
            | bit(sps.temporal_mvp_enabled, 2)
            | bit(pps.cabac_init_present, 3)
            | bit(pps.output_flag_present, 4)
            | bit(pps.dependent_slice_segments_enabled, 5)
            | bit(pps.slice_chroma_qp_offsets_present, 6)
            | bit(sps.sao_enabled, 7)
            | bit(pps.deblocking_override_enabled, 8)
            | bit(pps.deblocking_disabled, 9)
            | bit(pps.slice_header_extension_present, 10)
            | bit(params::is_irap(kind), 11)
            | bit(params::is_idr(kind), 12)
            | bit(header.kind.intra(), 13);

        let mut parameters = PictureParameters {
            current: PictureHEVC {
                picture_id: pool.id(self.current.as_ref().map_or(0, |current| current.slot)),
                pic_order_cnt: poc,
                flags: 0,
                va_reserved: [0; 4],
            },
            references: frames,
            width: sps.width as u16,
            height: sps.height as u16,
            pic_fields,
            max_dec_pic_buffering_minus1: (sps.max_dec_pic_buffering.max(1) - 1) as u8,
            bit_depth_luma_minus8: sps.depth[0].saturating_sub(8),
            bit_depth_chroma_minus8: sps.depth[1].saturating_sub(8),
            pcm_depth_luma_minus1: sps.pcm_depth[0].saturating_sub(1),
            pcm_depth_chroma_minus1: sps.pcm_depth[1].saturating_sub(1),
            log2_min_cb_minus3: (sps.min_cb_log2 - 3) as u8,
            log2_diff_max_min_cb: (sps.ctb_log2 - sps.min_cb_log2) as u8,
            log2_min_tb_minus2: (sps.min_tb_log2 - 2) as u8,
            log2_diff_max_min_tb: (sps.max_tb_log2 - sps.min_tb_log2) as u8,
            log2_min_pcm_cb_minus3: (sps.pcm_min_log2.max(3) - 3) as u8,
            log2_diff_max_min_pcm_cb: (sps.pcm_max_log2.saturating_sub(sps.pcm_min_log2)) as u8,
            max_transform_hierarchy_depth_intra: sps.max_transform_depth_intra as u8,
            max_transform_hierarchy_depth_inter: sps.max_transform_depth_inter as u8,
            init_qp_minus26: (pps.init_qp - 26) as i8,
            diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth as u8,
            pps_cb_qp_offset: pps.qp_offset[0] as i8,
            pps_cr_qp_offset: pps.qp_offset[1] as i8,
            log2_parallel_merge_level_minus2: (pps.log2_parallel_merge_level - 2) as u8,
            num_tile_columns_minus1: pps.column_width.len().max(1) as u8 - 1,
            num_tile_rows_minus1: pps.row_height.len().max(1) as u8 - 1,
            column_width_minus1: [0; 19],
            row_height_minus1: [0; 21],
            slice_parsing_fields: slice_parsing,
            log2_max_poc_lsb_minus4: (sps.log2_max_poc_lsb - 4) as u8,
            num_short_term_ref_pic_sets: sps.short_term.len() as u8,
            num_long_term_ref_pic_sps: sps.long_term_poc_lsb.len() as u8,
            num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_default[0].max(1) as u8 - 1,
            num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_default[1].max(1) as u8 - 1,
            pps_beta_offset_div2: (pps.beta_offset / 2) as i8,
            pps_tc_offset_div2: (pps.tc_offset / 2) as i8,
            num_extra_slice_header_bits: pps.num_extra_slice_header_bits as u8,
            // Only a short-format driver skips the set.
            st_rps_bits: 0,
            va_reserved: [0; 8],
        };
        for (at, width) in pps.column_width.iter().take(19).enumerate() {
            parameters.column_width_minus1[at] = width.max(&1).saturating_sub(1) as u16;
        }
        for (at, height) in pps.row_height.iter().take(21).enumerate() {
            parameters.row_height_minus1[at] = height.max(&1).saturating_sub(1) as u16;
        }
        parameters
    }

    fn slice_parameters(
        &self,
        sps: &Sps,
        pps: &Pps,
        header: &Header,
        bytes: &[u8],
        raw: &[u8],
    ) -> SliceParameters {
        let lists = self.order.dpb.lists(header);
        let mut parameters = SliceParameters {
            slice_data_size: raw.len() as u32,
            slice_data_offset: 0,
            slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
            // Counted including the two byte unit header, over the payload with escapes removed.
            slice_data_byte_offset: (header.data_at + 2) as u32,
            slice_segment_address: header.segment_address as u32,
            collocated_ref_idx: if header.temporal_mvp_enabled {
                header.collocated_ref_idx as u8
            } else {
                0xff
            },
            num_ref_idx_l0_active_minus1: header.num_ref_idx[0].max(1) as u8 - 1,
            num_ref_idx_l1_active_minus1: header.num_ref_idx[1].max(1) as u8 - 1,
            slice_qp_delta: (header.qp - pps.init_qp) as i8,
            slice_cb_qp_offset: header.slice_qp_offset[0] as i8,
            slice_cr_qp_offset: header.slice_qp_offset[1] as i8,
            slice_beta_offset_div2: (header.beta_offset / 2) as i8,
            slice_tc_offset_div2: (header.tc_offset / 2) as i8,
            five_minus_max_num_merge_cand: 5u8.saturating_sub(header.max_merge_cand as u8),
            num_entry_point_offsets: 0,
            entry_offset_to_subset_array: 0,
            slice_data_num_emu_prevn_bytes: escapes_before(bytes, header.data_at) as u16,
            ..SliceParameters::default()
        };
        let bit = |value: bool, at: u32| u32::from(value) << at;
        parameters.long_slice_flags = bit(header.dependent, 1)
            | ((match header.kind {
                Kind::B => 0u32,
                Kind::P => 1,
                Kind::I => 2,
            }) << 2)
            | bit(header.sao_luma, 6)
            | bit(header.sao_chroma, 7)
            | bit(header.mvd_l1_zero, 8)
            | bit(header.cabac_init_flag, 9)
            | bit(header.temporal_mvp_enabled, 10)
            | bit(header.deblocking_disabled, 11)
            | bit(header.collocated_from_l0, 12)
            | bit(header.loop_filter_across_slices, 13);

        for (list, entries) in lists.iter().enumerate() {
            for (at, entry) in entries.iter().take(FRAMES).enumerate() {
                parameters.lists[list][at] = self.index_of(entry);
            }
        }

        if (pps.weighted_pred && header.kind == Kind::P)
            || (pps.weighted_bipred && header.kind == Kind::B)
        {
            let range = 1i32 << (sps.depth[0] - 8);
            parameters.luma_log2_weight_denom = header.luma_log2_denom as u8;
            parameters.delta_chroma_log2_weight_denom =
                (header.chroma_log2_denom - header.luma_log2_denom) as i8;
            for list in 0..2 {
                for at in 0..FRAMES {
                    let weight = header.weights[list][at];
                    let luma = weight[0].weight - (1 << header.luma_log2_denom);
                    let (delta_luma, luma_offset, delta_chroma, chroma_offset) = if list == 0 {
                        (
                            &mut parameters.delta_luma_weight_l0,
                            &mut parameters.luma_offset_l0,
                            &mut parameters.delta_chroma_weight_l0,
                            &mut parameters.chroma_offset_l0,
                        )
                    } else {
                        (
                            &mut parameters.delta_luma_weight_l1,
                            &mut parameters.luma_offset_l1,
                            &mut parameters.delta_chroma_weight_l1,
                            &mut parameters.chroma_offset_l1,
                        )
                    };
                    delta_luma[at] = luma.clamp(-128, 127) as i8;
                    luma_offset[at] = (weight[0].offset / range).clamp(-128, 127) as i8;
                    for side in 0..2 {
                        let chroma = weight[side + 1].weight - (1 << header.chroma_log2_denom);
                        delta_chroma[at][side] = chroma.clamp(-128, 127) as i8;
                        chroma_offset[at][side] =
                            (weight[side + 1].offset / range).clamp(-128, 127) as i8;
                    }
                }
            }
        }
        parameters
    }

    fn index_of(&self, entry: &Entry<usize>) -> u8 {
        let Some(slot) = entry.id else {
            return 0xff;
        };
        for (at, frame) in self.frames.iter().enumerate() {
            if frame.flags & va::VA_PICTURE_HEVC_INVALID == 0
                && frame.pic_order_cnt == entry.poc
                && self.order.dpb.holds(slot, entry.poc)
                && frame.picture_id != va::VA_INVALID_SURFACE
            {
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

/// In block raster, not the diagonal scan `va_dec_hevc.h` implies. DC terms come separately.
fn iq_matrix(matrices: &params::Scaling) -> IqMatrix {
    IqMatrix {
        four: std::array::from_fn(|matrix| scaling::list4(matrices, matrix)),
        eight: std::array::from_fn(|matrix| scaling::list(matrices, 1, matrix)),
        sixteen: std::array::from_fn(|matrix| scaling::list(matrices, 2, matrix)),
        thirty_two: std::array::from_fn(|matrix| scaling::list(matrices, 3, matrix * 3)),
        dc_sixteen: std::array::from_fn(|matrix| matrices.matrix(2, matrix)[0]),
        dc_thirty_two: std::array::from_fn(|matrix| matrices.matrix(3, matrix * 3)[0]),
        va_reserved: [0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_the_size_libva_declares() {
        assert_eq!(size_of::<PictureHEVC>(), 28);
        assert_eq!(size_of::<PictureParameters>(), 604);
        assert_eq!(size_of::<IqMatrix>(), 1016);
        assert_eq!(size_of::<SliceParameters>(), 264);
    }
}
