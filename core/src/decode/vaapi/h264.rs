// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! H.264, handed to the hardware. Clause 8.2 is [`mediadecoder::video::h264::dpb`], shared with
//! the software decoder so both put pictures out in the same order.

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::h264::dpb::{Marked, Store};
use mediadecoder::video::h264::params::{self, Header, Kind, Pps, Sps};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::vaapi::pool::Pool;
use crate::decode::vaapi::{Held, va};
use va::{VABufferID, VAContextID, VASurfaceID};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PictureH264 {
    picture_id: VASurfaceID,
    frame_idx: u32,
    flags: u32,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    va_reserved: [u32; 4],
}

impl Default for PictureH264 {
    fn default() -> Self {
        Self {
            picture_id: va::VA_INVALID_SURFACE,
            frame_idx: 0,
            flags: va::VA_PICTURE_H264_INVALID,
            top_field_order_cnt: 0,
            bottom_field_order_cnt: 0,
            va_reserved: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct PictureParameters {
    current: PictureH264,
    references: [PictureH264; 16],
    width_in_mbs_minus1: u16,
    height_in_mbs_minus1: u16,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    num_ref_frames: u8,
    seq_fields: u32,
    deprecated: [u8; 4],
    pic_init_qp_minus26: i8,
    pic_init_qs_minus26: i8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    pic_fields: u32,
    frame_num: u16,
    va_reserved: [u32; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct IqMatrix {
    four: [[u8; 16]; 6],
    eight: [[u8; 64]; 2],
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SliceParameters {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    slice_data_bit_offset: u16,
    first_mb_in_slice: u16,
    slice_type: u8,
    direct_spatial_mv_pred_flag: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    cabac_init_idc: u8,
    slice_qp_delta: i8,
    disable_deblocking_filter_idc: u8,
    slice_alpha_c0_offset_div2: i8,
    slice_beta_offset_div2: i8,
    list0: [PictureH264; 32],
    list1: [PictureH264; 32],
    luma_log2_weight_denom: u8,
    chroma_log2_weight_denom: u8,
    luma_weight_l0_flag: u8,
    luma_weight_l0: [i16; 32],
    luma_offset_l0: [i16; 32],
    chroma_weight_l0_flag: u8,
    chroma_weight_l0: [[i16; 2]; 32],
    chroma_offset_l0: [[i16; 2]; 32],
    luma_weight_l1_flag: u8,
    luma_weight_l1: [i16; 32],
    luma_offset_l1: [i16; 32],
    chroma_weight_l1_flag: u8,
    chroma_weight_l1: [[i16; 2]; 32],
    chroma_offset_l1: [[i16; 2]; 32],
    va_reserved: [u32; 4],
}

impl Default for SliceParameters {
    fn default() -> Self {
        Self {
            slice_data_size: 0,
            slice_data_offset: 0,
            slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
            slice_data_bit_offset: 0,
            first_mb_in_slice: 0,
            slice_type: 0,
            direct_spatial_mv_pred_flag: 0,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            list0: [PictureH264::default(); 32],
            list1: [PictureH264::default(); 32],
            luma_log2_weight_denom: 0,
            chroma_log2_weight_denom: 0,
            luma_weight_l0_flag: 0,
            luma_weight_l0: [0; 32],
            luma_offset_l0: [0; 32],
            chroma_weight_l0_flag: 0,
            chroma_weight_l0: [[0; 2]; 32],
            chroma_offset_l0: [[0; 2]; 32],
            luma_weight_l1_flag: 0,
            luma_weight_l1: [0; 32],
            luma_offset_l1: [0; 32],
            chroma_weight_l1_flag: 0,
            chroma_weight_l1: [[0; 2]; 32],
            chroma_offset_l1: [[0; 2]; 32],
            va_reserved: [0; 4],
        }
    }
}

fn picture(store: &Store<usize, Held>, pool: &Pool, parity: Option<usize>) -> PictureH264 {
    let long = store.marked.contains(&Marked::Long);
    let mut flags = 0;
    match parity {
        Some(0) => flags |= va::VA_PICTURE_H264_TOP_FIELD,
        Some(_) => flags |= va::VA_PICTURE_H264_BOTTOM_FIELD,
        None => {}
    }
    if store.reference() {
        flags |= if long {
            va::VA_PICTURE_H264_LONG_TERM_REFERENCE
        } else {
            va::VA_PICTURE_H264_SHORT_TERM_REFERENCE
        };
    }
    PictureH264 {
        picture_id: pool.id(store.slot),
        frame_idx: if long {
            store.long_index as u32
        } else {
            store.frame_num
        },
        flags,
        top_field_order_cnt: if store.has_poc[0] { store.poc[0] } else { 0 },
        bottom_field_order_cnt: if store.has_poc[1] { store.poc[1] } else { 0 },
        va_reserved: [0; 4],
    }
}

fn as_reference(store: &Store<usize, Held>, pool: &Pool) -> PictureH264 {
    picture(store, pool, store.as_reference())
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
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        let mut buffers = va::Buffers::new(display);
        let mut surface = va::VA_INVALID_SURFACE;
        let mut started = false;
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5) & 3;
            let bytes = data.get(nal.body.clone()).unwrap_or(&[]);
            match kind {
                7 | 8 => self.sets.absorb(kind, bytes),
                1 | 5 => {
                    // The whole unit, escapes left in, goes to the driver; the bit offset is
                    // counted in the unescaped copy, as `va.h` asks.
                    let raw = data
                        .get(nal.body.start.saturating_sub(1)..nal.body.end)
                        .unwrap_or(&[]);
                    if let Err(error) = self.slice(
                        display,
                        context,
                        pool,
                        tag,
                        picture,
                        kind == 5,
                        idc,
                        bytes,
                        raw,
                        &mut buffers,
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
        display.decode(context, surface, &mut buffers)?;
        self.order.finish(pool);
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
        idr: bool,
        idc: u32,
        bytes: &[u8],
        raw: &[u8],
        buffers: &mut Vec<VABufferID>,
        surface: &mut VASurfaceID,
        started: &mut bool,
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

        if !*started {
            self.order.start(pool, tag, picture, &header, &sps)?;
            let Some(current) = self.order.current.clone() else {
                return Err(Error::Damaged("a slice with no picture"));
            };
            let at = self
                .order
                .dpb
                .index(current.serial)
                .ok_or(Error::Damaged("a picture that fell out of the buffer"))?;
            *surface = pool.id(self.order.dpb.stores[at].slot);
            let parameters = self.picture_parameters(pool, &header, &sps, &pps, at);
            buffers.push(display.parameters(
                context,
                va::VA_PICTURE_PARAMETER_BUFFER_TYPE,
                &parameters,
            )?);
            let weights = params::weights(&sps, &pps);
            let matrix = IqMatrix {
                four: weights.four,
                eight: [weights.eight[0], weights.eight[1]],
                va_reserved: [0; 4],
            };
            buffers.push(display.parameters(context, va::VA_IQ_MATRIX_BUFFER_TYPE, &matrix)?);
            *started = true;
        }

        self.order.dpb.number_frames(&sps, header.frame_num);
        let parameters = self.slice_parameters(pool, &header, &sps, &pps, raw);
        buffers.push(display.parameters(
            context,
            va::VA_SLICE_PARAMETER_BUFFER_TYPE,
            &parameters,
        )?);
        buffers.push(display.coded(context, raw)?);
        Ok(())
    }

    fn picture_parameters(
        &self,
        pool: &Pool,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        at: usize,
    ) -> PictureParameters {
        let store = &self.order.dpb.stores[at];
        let mut current = picture(
            store,
            pool,
            if header.field_pic {
                Some(usize::from(header.bottom_field))
            } else {
                None
            },
        );
        if header.nal_ref_idc != 0 {
            current.flags |= if header.long_term_reference {
                va::VA_PICTURE_H264_LONG_TERM_REFERENCE
            } else {
                va::VA_PICTURE_H264_SHORT_TERM_REFERENCE
            };
        }
        // For the second field of a pair this includes the frame being decoded (its first field).
        let mut references = [PictureH264::default(); 16];
        let mut count = 0;
        for other in &self.order.dpb.stores {
            if !other.reference() || count == 16 {
                continue;
            }
            references[count] = as_reference(other, pool);
            count += 1;
        }
        let bit = |value: bool, at: u32| u32::from(value) << at;
        let seq_fields = (sps.chroma & 3)
            | bit(sps.separate_planes, 2)
            | bit(sps.gaps_allowed, 3)
            | bit(sps.frame_mbs_only, 4)
            | bit(sps.mb_adaptive, 5)
            | bit(sps.direct_8x8_inference, 6)
            // MinLumaBiPredSize8x8, clause A.3.3.2.
            | bit(sps.level >= 31, 7)
            | ((sps.log2_max_frame_num - 4) & 15) << 8
            | (sps.poc_type & 3) << 12
            | (sps.log2_max_poc_lsb.saturating_sub(4) & 15) << 14
            | bit(sps.poc_always_zero, 18);
        let pic_fields = bit(pps.cabac, 0)
            | bit(pps.weighted_pred, 1)
            | ((pps.weighted_bipred & 3) << 2)
            | bit(pps.transform_8x8, 4)
            | bit(header.field_pic, 5)
            | bit(pps.constrained_intra, 6)
            | bit(pps.bottom_field_order_present, 7)
            | bit(pps.deblocking_control, 8)
            | bit(pps.redundant_present, 9)
            | bit(header.nal_ref_idc != 0, 10);
        PictureParameters {
            current,
            references,
            width_in_mbs_minus1: (sps.mb_width - 1) as u16,
            // The frame height even for a field (clause 7.4.2.1.1).
            height_in_mbs_minus1: (sps.mb_height().max(1) - 1) as u16,
            bit_depth_luma_minus8: (sps.depth_luma.saturating_sub(8)) as u8,
            bit_depth_chroma_minus8: (sps.depth_chroma.saturating_sub(8)) as u8,
            num_ref_frames: sps.max_ref_frames as u8,
            seq_fields,
            deprecated: [0; 4],
            pic_init_qp_minus26: (pps.init_qp - 26) as i8,
            pic_init_qs_minus26: (pps.init_qs - 26) as i8,
            chroma_qp_index_offset: pps.chroma_qp_offset[0] as i8,
            second_chroma_qp_index_offset: pps.chroma_qp_offset[1] as i8,
            pic_fields,
            frame_num: header.frame_num as u16,
            va_reserved: [0; 8],
        }
    }

    fn slice_parameters(
        &self,
        pool: &Pool,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        raw: &[u8],
    ) -> SliceParameters {
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

        let mut parameters = SliceParameters {
            slice_data_size: raw.len() as u32,
            slice_data_offset: 0,
            slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
            // Counted including the unit's own header byte.
            slice_data_bit_offset: (header.bits + 8) as u16,
            first_mb_in_slice: header.first_mb as u16,
            slice_type: match header.kind {
                Kind::P => 0,
                Kind::B => 1,
                Kind::I => 2,
                Kind::Sp => 3,
                Kind::Si => 4,
            },
            direct_spatial_mv_pred_flag: u8::from(header.direct_spatial),
            num_ref_idx_l0_active_minus1: header.num_ref_idx[0].max(1) as u8 - 1,
            num_ref_idx_l1_active_minus1: header.num_ref_idx[1].max(1) as u8 - 1,
            cabac_init_idc: header.cabac_init_idc as u8,
            slice_qp_delta: (header.qp - pps.init_qp) as i8,
            disable_deblocking_filter_idc: header.disable_deblocking as u8,
            slice_alpha_c0_offset_div2: (header.alpha_offset / 2) as i8,
            slice_beta_offset_div2: (header.beta_offset / 2) as i8,
            ..SliceParameters::default()
        };
        for (list, into) in [&mut parameters.list0, &mut parameters.list1]
            .into_iter()
            .enumerate()
        {
            for (at, reference) in initial[list].iter().take(32).enumerate() {
                into[at] = picture(&self.order.dpb.stores[reference.at], pool, reference.parity);
            }
        }

        let explicit = (pps.weighted_pred && matches!(header.kind, Kind::P | Kind::Sp))
            || (pps.weighted_bipred == 1 && header.kind == Kind::B);
        if explicit {
            parameters.luma_log2_weight_denom = header.log2_weight_denom[0] as u8;
            parameters.chroma_log2_weight_denom = header.log2_weight_denom[1] as u8;
            parameters.luma_weight_l0_flag = 1;
            parameters.chroma_weight_l0_flag = u8::from(sps.chroma != 0);
            if header.kind == Kind::B {
                parameters.luma_weight_l1_flag = 1;
                parameters.chroma_weight_l1_flag = u8::from(sps.chroma != 0);
            }
            for at in 0..32.min(params::REFS) {
                for list in 0..2 {
                    let weight = header.weights[list][at];
                    let (luma, luma_offset, chroma, chroma_offset) = if list == 0 {
                        (
                            &mut parameters.luma_weight_l0,
                            &mut parameters.luma_offset_l0,
                            &mut parameters.chroma_weight_l0,
                            &mut parameters.chroma_offset_l0,
                        )
                    } else {
                        (
                            &mut parameters.luma_weight_l1,
                            &mut parameters.luma_offset_l1,
                            &mut parameters.chroma_weight_l1,
                            &mut parameters.chroma_offset_l1,
                        )
                    };
                    luma[at] = weight.luma.0 as i16;
                    luma_offset[at] = weight.luma.1 as i16;
                    for side in 0..2 {
                        chroma[at][side] = weight.chroma[side].0 as i16;
                        chroma_offset[at][side] = weight.chroma[side].1 as i16;
                    }
                }
            }
        }
        parameters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_the_size_libva_declares() {
        assert_eq!(size_of::<PictureH264>(), 36);
        assert_eq!(size_of::<PictureParameters>(), 672);
        assert_eq!(size_of::<IqMatrix>(), 240);
        assert_eq!(size_of::<SliceParameters>(), 3128);
    }
}
