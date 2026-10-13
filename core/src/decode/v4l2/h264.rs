// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! H.264 for a stateless V4L2 decoder. Pictures are named by timestamp; field
//! pairs are ordinary (some services send every picture as a field).

use std::ops::Range;

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::h264::dpb::{Marked, Reference};
use mediadecoder::video::h264::params::{self, Header, Pps, Sps};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::v4l2::ctrl::{self, Ctl, Kind};
use crate::decode::v4l2::pool::Pool;
use crate::decode::v4l2::{Held, Mode, Submission, Unit};

#[derive(Default)]
struct Progress {
    ts: u64,
    started: bool,
    /// The same for every slice.
    base: Vec<Ctl>,
    /// Each store's place in the DPB control; built before any list.
    dpb_index: Vec<Option<u8>>,
    units: Vec<Unit>,
    pieces: Vec<Range<usize>>,
}

#[derive(Debug)]
pub struct H264 {
    framing: Framing,
    mode: Mode,
    /// Start code bytes before every piece; `header_bit_size` counts over them.
    prefix: u32,
    sets: params::Sets,
    order: frames::H264Order<u64>,
}

impl H264 {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            mode: Mode::Slice,
            prefix: 0,
            sets: params::Sets::default(),
            order: frames::H264Order::new(),
        }
    }

    /// In frame mode the hardware parses headers and builds its own lists.
    pub fn configure(&mut self, mode: Mode, prefix: u32) {
        self.mode = mode;
        self.prefix = prefix;
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

    /// Not calling it stops the buffers moving.
    pub fn finished(&mut self, pool: &mut Pool) {
        self.order.finish(pool);
    }

    /// A damaged slice is reported only if no picture came of it.
    pub fn decode(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<Option<Submission>, Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        let mut progress = Progress::default();
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5) & 3;
            let bytes = data.get(nal.body.clone()).unwrap_or(&[]);
            match kind {
                7 | 8 => self.sets.absorb(kind, bytes),
                1 | 5 => {
                    let raw = nal.body.start.saturating_sub(1)..nal.body.end;
                    if let Err(error) = self.slice(
                        pool,
                        tag,
                        picture,
                        kind == 5,
                        idc,
                        bytes,
                        raw,
                        &mut progress,
                    ) {
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }

        if !progress.started {
            return match damage {
                Some(error) => Err(error),
                None => Ok(None),
            };
        }
        if matches!(self.mode, Mode::Frame) {
            progress.units.push(Unit {
                controls: std::mem::take(&mut progress.base),
                pieces: std::mem::take(&mut progress.pieces),
                zeroes: 0,
            });
        }
        Ok(Some(Submission {
            ts: progress.ts,
            units: progress.units,
            more: self.half_a_picture(),
        }))
    }

    /// A field whose mate has not arrived holds the capture buffer open.
    fn half_a_picture(&self) -> bool {
        let Some(current) = self.order.current.as_ref() else {
            return false;
        };
        if !current.field {
            return false;
        }
        self.order
            .dpb
            .index(current.serial)
            .is_some_and(|at| !self.order.dpb.stores[at].present[1 - current.parity])
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
        raw: Range<usize>,
        progress: &mut Progress,
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

        if !progress.started {
            self.order.start(pool, tag, picture, &header, &sps)?;
            let Some(current) = self.order.current.clone() else {
                return Err(Error::Damaged("a slice with no picture"));
            };
            let at = self
                .order
                .dpb
                .index(current.serial)
                .ok_or(Error::Damaged("a picture that fell out of the buffer"))?;
            // Clause 8.2.4.1, once per picture.
            self.order.dpb.number_frames(&sps, header.frame_num);
            let (dpb, index) = self.dpb(&header);
            progress.ts = self.order.dpb.stores[at].slot;
            progress.dpb_index = index;
            progress.base = vec![
                Ctl::of(Kind::H264Sps, &sequence_control(&sps)),
                Ctl::of(Kind::H264Pps, &picture_control(&pps)),
                Ctl::of(Kind::H264ScalingMatrix, &matrix_control(&sps, &pps)),
                Ctl::of(
                    Kind::H264DecodeParams,
                    &self.decode_control(&header, dpb, at),
                ),
            ];
            progress.started = true;
        }

        if matches!(self.mode, Mode::Frame) {
            progress.pieces.push(raw);
            return Ok(());
        }

        let mut controls = progress.base.clone();
        controls.push(Ctl::of(
            Kind::H264SliceParams,
            &self.slice_control(&header, &sps, &pps, &progress.dpb_index),
        ));
        if let Some(weights) = weight_control(&header, &pps) {
            controls.push(Ctl::of(Kind::H264PredWeights, &weights));
        }
        progress.units.push(Unit {
            controls,
            pieces: vec![raw],
            zeroes: 0,
        });
        Ok(())
    }

    fn decode_control(
        &self,
        header: &Header,
        dpb: [ctrl::H264DpbEntry; ctrl::V4L2_H264_NUM_DPB_ENTRIES],
        at: usize,
    ) -> ctrl::H264DecodeParams {
        let store = &self.order.dpb.stores[at];
        let mut flags = 0;
        if header.idr {
            flags |= ctrl::V4L2_H264_DECODE_PARAM_FLAG_IDR_PIC;
        }
        if header.field_pic {
            flags |= ctrl::V4L2_H264_DECODE_PARAM_FLAG_FIELD_PIC;
            if header.bottom_field {
                flags |= ctrl::V4L2_H264_DECODE_PARAM_FLAG_BOTTOM_FIELD;
            }
        }
        match header.kind {
            params::Kind::P | params::Kind::Sp => {
                flags |= ctrl::V4L2_H264_DECODE_PARAM_FLAG_PFRAME;
            }
            params::Kind::B => flags |= ctrl::V4L2_H264_DECODE_PARAM_FLAG_BFRAME,
            params::Kind::I | params::Kind::Si => {}
        }
        ctrl::H264DecodeParams {
            dpb,
            nal_ref_idc: header.nal_ref_idc as u16,
            frame_num: header.frame_num as u16,
            top_field_order_cnt: if store.has_poc[0] { store.poc[0] } else { 0 },
            bottom_field_order_cnt: if store.has_poc[1] { store.poc[1] } else { 0 },
            idr_pic_id: header.idr_pic_id as u16,
            pic_order_cnt_lsb: header.poc_lsb as u16,
            delta_pic_order_cnt_bottom: header.delta_poc_bottom,
            delta_pic_order_cnt0: header.delta_poc[0],
            delta_pic_order_cnt1: header.delta_poc[1],
            // Not published, so zero; suspect this first if P pictures drift on a
            // re-parsing driver.
            dec_ref_pic_marking_bit_size: 0,
            pic_order_cnt_bit_size: 0,
            slice_group_change_cycle: 0,
            reserved: 0,
            flags,
        }
    }

    /// For a second field it includes the frame being decoded.
    fn dpb(
        &self,
        header: &Header,
    ) -> (
        [ctrl::H264DpbEntry; ctrl::V4L2_H264_NUM_DPB_ENTRIES],
        Vec<Option<u8>>,
    ) {
        let mut dpb = [ctrl::H264DpbEntry::default(); ctrl::V4L2_H264_NUM_DPB_ENTRIES];
        let mut index = vec![None; self.order.dpb.stores.len()];
        let parity = usize::from(header.bottom_field);
        let mut count = 0usize;
        for (at, store) in self.order.dpb.stores.iter().enumerate() {
            if !store.reference() || count == ctrl::V4L2_H264_NUM_DPB_ENTRIES {
                continue;
            }
            let side = store.as_reference();
            let long = store.marked.contains(&Marked::Long);
            // Clause 8.2.4.1: a same-parity field is the nearer, so its number is odd.
            let base = if long { store.long_index } else { store.wrap };
            let pic_num = match side {
                Some(side) if header.field_pic => 2 * base + i32::from(side == parity),
                _ => base,
            };
            let fields = match side {
                None => ctrl::V4L2_H264_FRAME_REF,
                Some(0) => ctrl::V4L2_H264_TOP_FIELD_REF,
                Some(_) => ctrl::V4L2_H264_BOTTOM_FIELD_REF,
            };
            let mut flags =
                ctrl::V4L2_H264_DPB_ENTRY_FLAG_VALID | ctrl::V4L2_H264_DPB_ENTRY_FLAG_ACTIVE;
            if long {
                flags |= ctrl::V4L2_H264_DPB_ENTRY_FLAG_LONG_TERM;
            }
            if !store.complete() {
                flags |= ctrl::V4L2_H264_DPB_ENTRY_FLAG_FIELD;
            }
            dpb[count] = ctrl::H264DpbEntry {
                reference_ts: store.slot,
                pic_num: pic_num as u32,
                frame_num: store.frame_num as u16,
                fields,
                reserved: [0; 5],
                top_field_order_cnt: if store.has_poc[0] { store.poc[0] } else { 0 },
                bottom_field_order_cnt: if store.has_poc[1] { store.poc[1] } else { 0 },
                flags,
            };
            index[at] = Some(count as u8);
            count += 1;
        }
        (dpb, index)
    }

    fn slice_control(
        &self,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        index: &[Option<u8>],
    ) -> ctrl::H264SliceParams {
        let parity = usize::from(header.bottom_field);
        let poc = self
            .order
            .current
            .as_ref()
            .and_then(|current| self.order.dpb.index(current.serial))
            .map_or(0, |at| {
                if header.field_pic {
                    self.order.dpb.stores[at].poc[parity]
                } else {
                    self.order.dpb.stores[at].order()
                }
            });
        let mut initial = self.order.dpb.initial_lists(header, poc, parity);
        let full = initial.clone();
        self.order
            .dpb
            .reorder_lists(header, sps, &mut initial, &full);

        let mut flags = 0;
        if header.direct_spatial {
            flags |= ctrl::V4L2_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED;
        }
        ctrl::H264SliceParams {
            header_bit_size: header_bit_size(self.prefix, header),
            first_mb_in_slice: header.first_mb as u32,
            slice_type: match header.kind {
                params::Kind::P => ctrl::V4L2_H264_SLICE_TYPE_P,
                params::Kind::B => ctrl::V4L2_H264_SLICE_TYPE_B,
                params::Kind::I => ctrl::V4L2_H264_SLICE_TYPE_I,
                params::Kind::Sp => ctrl::V4L2_H264_SLICE_TYPE_SP,
                params::Kind::Si => ctrl::V4L2_H264_SLICE_TYPE_SI,
            },
            colour_plane_id: header.colour_plane as u8,
            redundant_pic_cnt: header.redundant as u8,
            cabac_init_idc: header.cabac_init_idc as u8,
            slice_qp_delta: (header.qp - pps.init_qp) as i8,
            slice_qs_delta: 0,
            disable_deblocking_filter_idc: header.disable_deblocking as u8,
            slice_alpha_c0_offset_div2: (header.alpha_offset / 2) as i8,
            slice_beta_offset_div2: (header.beta_offset / 2) as i8,
            num_ref_idx_l0_active_minus1: header.num_ref_idx[0].max(1) as u8 - 1,
            num_ref_idx_l1_active_minus1: header.num_ref_idx[1].max(1) as u8 - 1,
            reserved: 0,
            ref_pic_list0: list_control(&initial[0], index),
            ref_pic_list1: list_control(&initial[1], index),
            flags,
        }
    }
}

/// An entry for a picture not in the DPB is left zero (a used one never is).
fn list_control(
    list: &[Reference],
    index: &[Option<u8>],
) -> [ctrl::H264Reference; ctrl::V4L2_H264_REF_LIST_LEN] {
    let mut into = [ctrl::H264Reference::default(); ctrl::V4L2_H264_REF_LIST_LEN];
    for (at, reference) in list.iter().take(ctrl::V4L2_H264_REF_LIST_LEN).enumerate() {
        let Some(Some(slot)) = index.get(reference.at).copied() else {
            continue;
        };
        into[at] = ctrl::H264Reference {
            fields: match reference.parity {
                None => ctrl::V4L2_H264_FRAME_REF,
                Some(0) => ctrl::V4L2_H264_TOP_FIELD_REF,
                Some(_) => ctrl::V4L2_H264_BOTTOM_FIELD_REF,
            },
            index: slot,
        };
    }
    into
}

fn sequence_control(sps: &Sps) -> ctrl::H264Sps {
    let bit = |value: bool, flag: u32| if value { flag } else { 0 };
    let mut offset_for_ref_frame = [0i32; 255];
    let cycle = sps.cycle_length.min(255) as usize;
    offset_for_ref_frame[..cycle].copy_from_slice(&sps.cycle[..cycle]);
    ctrl::H264Sps {
        profile_idc: sps.profile as u8,
        constraint_set_flags: 0,
        level_idc: sps.level as u8,
        seq_parameter_set_id: sps.id as u8,
        chroma_format_idc: sps.chroma as u8,
        bit_depth_luma_minus8: sps.depth_luma.saturating_sub(8) as u8,
        bit_depth_chroma_minus8: sps.depth_chroma.saturating_sub(8) as u8,
        log2_max_frame_num_minus4: sps.log2_max_frame_num.saturating_sub(4) as u8,
        pic_order_cnt_type: sps.poc_type as u8,
        log2_max_pic_order_cnt_lsb_minus4: sps.log2_max_poc_lsb.saturating_sub(4) as u8,
        max_num_ref_frames: sps.max_ref_frames as u8,
        num_ref_frames_in_pic_order_cnt_cycle: cycle as u8,
        offset_for_ref_frame,
        offset_for_non_ref_pic: sps.offset_non_ref,
        offset_for_top_to_bottom_field: sps.offset_top_to_bottom,
        pic_width_in_mbs_minus1: (sps.mb_width.max(1) - 1) as u16,
        // In map units (halved for fields, clause 7.4.2.1.1); do not halve again.
        pic_height_in_map_units_minus1: (sps.map_units.max(1) - 1) as u16,
        flags: bit(
            sps.separate_planes,
            ctrl::V4L2_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE,
        ) | bit(
            sps.transform_bypass,
            ctrl::V4L2_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS,
        ) | bit(
            sps.poc_always_zero,
            ctrl::V4L2_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO,
        ) | bit(
            sps.gaps_allowed,
            ctrl::V4L2_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED,
        ) | bit(sps.frame_mbs_only, ctrl::V4L2_H264_SPS_FLAG_FRAME_MBS_ONLY)
            | bit(
                sps.mb_adaptive,
                ctrl::V4L2_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD,
            )
            | bit(
                sps.direct_8x8_inference,
                ctrl::V4L2_H264_SPS_FLAG_DIRECT_8X8_INFERENCE,
            ),
    }
}

fn picture_control(pps: &Pps) -> ctrl::H264Pps {
    let bit = |value: bool, flag: u16| if value { flag } else { 0 };
    ctrl::H264Pps {
        pic_parameter_set_id: pps.id as u8,
        seq_parameter_set_id: pps.sps_id as u8,
        num_slice_groups_minus1: pps.slice_groups.max(1) as u8 - 1,
        num_ref_idx_l0_default_active_minus1: pps.num_ref_idx[0].max(1) as u8 - 1,
        num_ref_idx_l1_default_active_minus1: pps.num_ref_idx[1].max(1) as u8 - 1,
        weighted_bipred_idc: pps.weighted_bipred as u8,
        pic_init_qp_minus26: (pps.init_qp - 26) as i8,
        pic_init_qs_minus26: (pps.init_qs - 26) as i8,
        chroma_qp_index_offset: pps.chroma_qp_offset[0] as i8,
        second_chroma_qp_index_offset: pps.chroma_qp_offset[1] as i8,
        flags: bit(pps.cabac, ctrl::V4L2_H264_PPS_FLAG_ENTROPY_CODING_MODE)
            | bit(
                pps.bottom_field_order_present,
                ctrl::V4L2_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT,
            )
            | bit(pps.weighted_pred, ctrl::V4L2_H264_PPS_FLAG_WEIGHTED_PRED)
            | bit(
                pps.deblocking_control,
                ctrl::V4L2_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT,
            )
            | bit(
                pps.constrained_intra,
                ctrl::V4L2_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED,
            )
            | bit(
                pps.redundant_present,
                ctrl::V4L2_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT,
            )
            | bit(pps.transform_8x8, ctrl::V4L2_H264_PPS_FLAG_TRANSFORM_8X8_MODE)
            // The matrix is always sent; without this the driver assumes flat lists.
            | ctrl::V4L2_H264_PPS_FLAG_SCALING_MATRIX_PRESENT,
    }
}

fn matrix_control(sps: &Sps, pps: &Pps) -> ctrl::H264ScalingMatrix {
    let weights = params::weights(sps, pps);
    ctrl::H264ScalingMatrix {
        scaling_list_4x4: weights.four,
        scaling_list_8x8: weights.eight,
    }
}

/// Only for explicit weighting: a table sent with implicit would be read as explicit.
fn weight_control(header: &Header, pps: &Pps) -> Option<ctrl::H264PredWeights> {
    let explicit = (pps.weighted_pred && matches!(header.kind, params::Kind::P | params::Kind::Sp))
        || (pps.weighted_bipred == 1 && header.kind == params::Kind::B);
    if !explicit {
        return None;
    }
    let mut weights = ctrl::H264PredWeights {
        luma_log2_weight_denom: header.log2_weight_denom[0] as u16,
        chroma_log2_weight_denom: header.log2_weight_denom[1] as u16,
        ..ctrl::H264PredWeights::default()
    };
    for (factors, table) in weights.weight_factors.iter_mut().zip(&header.weights) {
        for (at, weight) in table.iter().take(32).enumerate() {
            factors.luma_weight[at] = weight.luma.0 as i16;
            factors.luma_offset[at] = weight.luma.1 as i16;
            for side in 0..2 {
                factors.chroma_weight[at][side] = weight.chroma[side].0 as i16;
                factors.chroma_offset[at][side] = weight.chroma[side].1 as i16;
            }
        }
    }
    Some(weights)
}

/// Bits from the first byte of the output buffer (start code included) to the slice data.
fn header_bit_size(prefix: u32, header: &Header) -> u32 {
    prefix * 8 + 8 + header.bits as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::video::h264::dpb::Store;

    fn decoder() -> H264 {
        H264::new(Framing::StartCodes)
    }

    fn store(ts: u64, serial: i32, frame_num: u32) -> Store<u64, Held> {
        Store {
            slot: ts,
            serial,
            frame_num,
            wrap: frame_num as i32,
            long_index: 0,
            present: [true; 2],
            has_poc: [true; 2],
            poc: [2 * frame_num as i32, 2 * frame_num as i32 + 1],
            marked: [Marked::Short; 2],
            output: false,
            held: Held::default(),
        }
    }

    #[test]
    fn the_header_size_counts_the_start_code_and_the_unit_header() {
        let header = Header {
            bits: 37,
            ..Header::default()
        };
        assert_eq!(header_bit_size(3, &header), 24 + 8 + 37);
        assert_eq!(header_bit_size(0, &header), 8 + 37);
    }

    #[test]
    fn a_dpb_entry_says_which_fields_are_references() {
        let mut decoder = decoder();
        let mut single = store(3_000_000, 2, 5);
        single.present = [true, false];
        single.marked = [Marked::Short, Marked::No];
        let mut long = store(4_000_000, 3, 6);
        long.marked = [Marked::Long; 2];
        long.long_index = 2;
        decoder.order.dpb.stores = vec![store(2_000_000, 1, 4), single, long];

        let (dpb, index) = decoder.dpb(&Header::default());

        assert_eq!(index, vec![Some(0), Some(1), Some(2)]);
        assert_eq!(dpb[0].reference_ts, 2_000_000);
        assert_eq!(dpb[0].fields, ctrl::V4L2_H264_FRAME_REF);
        assert_eq!(dpb[0].frame_num, 4);
        assert_eq!(dpb[0].pic_num, 4);
        assert_eq!(
            dpb[0].flags,
            ctrl::V4L2_H264_DPB_ENTRY_FLAG_VALID | ctrl::V4L2_H264_DPB_ENTRY_FLAG_ACTIVE
        );
        assert_eq!(dpb[0].top_field_order_cnt, 8);
        assert_eq!(dpb[0].bottom_field_order_cnt, 9);

        assert_eq!(dpb[1].fields, ctrl::V4L2_H264_TOP_FIELD_REF);
        assert_eq!(
            dpb[1].flags & ctrl::V4L2_H264_DPB_ENTRY_FLAG_FIELD,
            ctrl::V4L2_H264_DPB_ENTRY_FLAG_FIELD,
            "a store with one field is a field reference"
        );

        assert_eq!(dpb[2].fields, ctrl::V4L2_H264_FRAME_REF);
        assert_eq!(
            dpb[2].flags & ctrl::V4L2_H264_DPB_ENTRY_FLAG_LONG_TERM,
            ctrl::V4L2_H264_DPB_ENTRY_FLAG_LONG_TERM
        );
        assert_eq!(dpb[2].pic_num, 2);
        assert_eq!(dpb[3].flags, 0);
        assert_eq!(dpb[3].reference_ts, 0);
    }

    #[test]
    fn a_field_picture_numbers_its_references_by_fields() {
        let mut decoder = decoder();
        let mut only_bottom = store(2_000_000, 1, 4);
        only_bottom.present = [false, true];
        only_bottom.marked = [Marked::No, Marked::Short];
        decoder.order.dpb.stores = vec![only_bottom];
        let bottom = Header {
            field_pic: true,
            bottom_field: true,
            ..Header::default()
        };
        let (dpb, _) = decoder.dpb(&bottom);
        assert_eq!(dpb[0].fields, ctrl::V4L2_H264_BOTTOM_FIELD_REF);
        assert_eq!(dpb[0].pic_num, 2 * 4 + 1);

        let top = Header {
            field_pic: true,
            ..Header::default()
        };
        let (dpb, _) = decoder.dpb(&top);
        assert_eq!(dpb[0].pic_num, 2 * 4, "the other parity is the far one");
    }

    #[test]
    fn a_list_entry_points_at_the_buffer_slot_of_its_own_picture() {
        let mut decoder = decoder();
        let mut spare = store(3_000_000, 2, 5);
        spare.marked = [Marked::No; 2];
        spare.output = true;
        decoder.order.dpb.stores = vec![store(2_000_000, 1, 4), spare, store(4_000_000, 3, 6)];

        let header = Header {
            kind: params::Kind::P,
            frame_num: 7,
            num_ref_idx: [2, 0],
            ..Header::default()
        };
        let (dpb, index) = decoder.dpb(&header);
        assert_eq!(index, vec![Some(0), None, Some(1)]);

        let lists = decoder.order.dpb.initial_lists(&header, 0, 0);
        let entries = list_control(&lists[0], &index);
        assert_eq!(entries[0].fields, ctrl::V4L2_H264_FRAME_REF);
        assert_eq!(
            dpb[entries[0].index as usize].reference_ts, 4_000_000,
            "the first entry has to name the picture the list chose"
        );
        assert_eq!(dpb[entries[1].index as usize].reference_ts, 2_000_000);
        assert_eq!(entries[2], ctrl::H264Reference::default());
    }

    #[test]
    fn the_sequence_flags_are_packed_where_the_header_says() {
        let sps = Sps {
            id: 3,
            profile: 100,
            level: 40,
            chroma: 1,
            depth_luma: 8,
            depth_chroma: 8,
            log2_max_frame_num: 8,
            log2_max_poc_lsb: 6,
            max_ref_frames: 4,
            mb_width: 120,
            map_units: 34,
            frame_mbs_only: false,
            mb_adaptive: true,
            direct_8x8_inference: true,
            gaps_allowed: false,
            ..Sps::default()
        };
        let control = sequence_control(&sps);
        assert_eq!(control.profile_idc, 100);
        assert_eq!(control.level_idc, 40);
        assert_eq!(control.seq_parameter_set_id, 3);
        assert_eq!(control.log2_max_frame_num_minus4, 4);
        assert_eq!(control.log2_max_pic_order_cnt_lsb_minus4, 2);
        assert_eq!(control.pic_width_in_mbs_minus1, 119);
        assert_eq!(control.pic_height_in_map_units_minus1, 33);
        assert_eq!(
            control.flags,
            ctrl::V4L2_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD
                | ctrl::V4L2_H264_SPS_FLAG_DIRECT_8X8_INFERENCE
        );

        let frames = Sps {
            frame_mbs_only: true,
            mb_adaptive: false,
            direct_8x8_inference: false,
            gaps_allowed: true,
            ..sps
        };
        assert_eq!(
            sequence_control(&frames).flags,
            ctrl::V4L2_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED
                | ctrl::V4L2_H264_SPS_FLAG_FRAME_MBS_ONLY
        );
    }

    #[test]
    fn the_picture_flags_are_packed_where_the_header_says() {
        let pps = Pps {
            id: 2,
            sps_id: 1,
            cabac: true,
            transform_8x8: true,
            weighted_pred: true,
            weighted_bipred: 2,
            init_qp: 30,
            init_qs: 26,
            chroma_qp_offset: [-2, 3],
            num_ref_idx: [3, 2],
            ..Pps::default()
        };
        let control = picture_control(&pps);
        assert_eq!(control.pic_parameter_set_id, 2);
        assert_eq!(control.seq_parameter_set_id, 1);
        assert_eq!(control.num_ref_idx_l0_default_active_minus1, 2);
        assert_eq!(control.num_ref_idx_l1_default_active_minus1, 1);
        assert_eq!(control.weighted_bipred_idc, 2);
        assert_eq!(control.pic_init_qp_minus26, 4);
        assert_eq!(control.pic_init_qs_minus26, 0);
        assert_eq!(control.chroma_qp_index_offset, -2);
        assert_eq!(control.second_chroma_qp_index_offset, 3);
        assert_eq!(
            control.flags,
            ctrl::V4L2_H264_PPS_FLAG_ENTROPY_CODING_MODE
                | ctrl::V4L2_H264_PPS_FLAG_WEIGHTED_PRED
                | ctrl::V4L2_H264_PPS_FLAG_TRANSFORM_8X8_MODE
                | ctrl::V4L2_H264_PPS_FLAG_SCALING_MATRIX_PRESENT
        );
    }

    #[test]
    fn the_scaling_matrix_carries_six_eight_by_eight_lists() {
        let matrix = matrix_control(&Sps::default(), &Pps::default());
        assert_eq!(matrix.scaling_list_4x4.len(), 6);
        assert_eq!(matrix.scaling_list_8x8.len(), 6);
        assert!(matrix.scaling_list_4x4.iter().flatten().all(|&v| v == 16));
        assert!(matrix.scaling_list_8x8.iter().flatten().all(|&v| v == 16));
    }

    #[test]
    fn weights_are_sent_only_where_the_slice_carries_them() {
        let explicit = Pps {
            weighted_pred: true,
            weighted_bipred: 1,
            ..Pps::default()
        };
        let implicit = Pps {
            weighted_pred: false,
            weighted_bipred: 2,
            ..Pps::default()
        };
        let predicted = Header {
            kind: params::Kind::P,
            log2_weight_denom: [5, 4],
            ..Header::default()
        };
        let bidirectional = Header {
            kind: params::Kind::B,
            ..Header::default()
        };
        let intra = Header {
            kind: params::Kind::I,
            ..Header::default()
        };
        let weights = weight_control(&predicted, &explicit).expect("explicit weights");
        assert_eq!(weights.luma_log2_weight_denom, 5);
        assert_eq!(weights.chroma_log2_weight_denom, 4);
        assert_eq!(weights.weight_factors[0].luma_weight[0], 1);
        assert_eq!(weights.weight_factors[1].luma_offset[0], 0);
        assert!(weight_control(&bidirectional, &explicit).is_some());
        assert!(weight_control(&bidirectional, &implicit).is_none());
        assert!(weight_control(&predicted, &implicit).is_none());
        assert!(weight_control(&intra, &explicit).is_none());
    }

    #[test]
    fn the_decode_parameters_say_what_the_picture_is() {
        let mut decoder = decoder();
        decoder.order.dpb.stores = vec![store(2_000_000, 1, 4)];
        let header = Header {
            kind: params::Kind::B,
            field_pic: true,
            bottom_field: true,
            frame_num: 9,
            nal_ref_idc: 2,
            poc_lsb: 12,
            idr_pic_id: 5,
            ..Header::default()
        };
        let (dpb, _) = decoder.dpb(&header);
        let control = decoder.decode_control(&header, dpb, 0);
        assert_eq!(control.frame_num, 9);
        assert_eq!(control.nal_ref_idc, 2);
        assert_eq!(control.pic_order_cnt_lsb, 12);
        assert_eq!(control.idr_pic_id, 5);
        assert_eq!(
            control.flags,
            ctrl::V4L2_H264_DECODE_PARAM_FLAG_FIELD_PIC
                | ctrl::V4L2_H264_DECODE_PARAM_FLAG_BOTTOM_FIELD
                | ctrl::V4L2_H264_DECODE_PARAM_FLAG_BFRAME
        );
        assert_eq!(control.dec_ref_pic_marking_bit_size, 0);
        assert_eq!(control.pic_order_cnt_bit_size, 0);
    }

    #[test]
    fn the_slice_kind_chooses_the_picture_flag() {
        let mut decoder = decoder();
        decoder.order.dpb.stores = vec![store(2_000_000, 1, 4)];
        let of = |kind| {
            let header = Header {
                kind,
                ..Header::default()
            };
            let (dpb, _) = decoder.dpb(&header);
            decoder.decode_control(&header, dpb, 0).flags
        };
        assert_eq!(of(params::Kind::I), 0);
        assert_eq!(
            of(params::Kind::P),
            ctrl::V4L2_H264_DECODE_PARAM_FLAG_PFRAME
        );
        assert_eq!(
            of(params::Kind::B),
            ctrl::V4L2_H264_DECODE_PARAM_FLAG_BFRAME
        );
    }
}
