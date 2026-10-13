// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC for a stateless V4L2 decoder. Pictures are named by timestamp, and the
//! three sets of clause 8.3.2 are indices into `dpb`, not POCs.

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::hevc::dpb::{Entry, Marked};
use mediadecoder::video::hevc::params::{self, Header, Pps, Sps};
use mediadecoder::video::hevc::{escapes_before, scaling};

use crate::decode::Error;
use crate::decode::frames;
use crate::decode::v4l2::ctrl::{self, Ctl, Kind};
use crate::decode::v4l2::pool::Pool;
use crate::decode::v4l2::{Held, Mode, Submission, Unit};

const FRAMES: usize = ctrl::V4L2_HEVC_DPB_ENTRIES_NUM_MAX;

/// 「無い」を示す値。DPB のインデックスとしてカーネルへは送らない（[`Hevc::fill`]）。
const NONE: u8 = 0xff;

#[derive(Debug, Clone)]
struct Current {
    ts: u64,
    poc: i32,
    held: Held,
    output: bool,
    header: Box<Header>,
    /// Cloned onto each request: slice mode wants all of them every time.
    shared: Vec<Ctl>,
}

#[derive(Debug)]
pub struct Hevc {
    framing: Framing,
    mode: Mode,
    /// Start code bytes written before every piece; offsets count over them.
    prefix: u32,
    sets: params::Sets,
    order: frames::HevcOrder<u64>,
    current: Option<Current>,
    dpb: [ctrl::HevcDpbEntry; FRAMES],
    active: usize,
    /// Some drivers refuse a WPP or tiled slice without `HEVC_ENTRY_POINT_OFFSETS`.
    entry_points: bool,
}

impl Hevc {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            mode: Mode::Slice,
            prefix: 0,
            sets: params::Sets::default(),
            order: frames::HevcOrder::new(),
            current: None,
            dpb: [ctrl::HevcDpbEntry::default(); FRAMES],
            active: 0,
            entry_points: false,
        }
    }

    /// In frame mode only the first segment's slice params are sent (broadcast HEVC
    /// is one slice per picture).
    pub fn configure(&mut self, mode: Mode, prefix: u32) {
        self.mode = mode;
        self.prefix = prefix;
    }

    /// The coded size of a sequence this access unit starts. Some drivers size the
    /// picture from the output format, so 1080 told as 1088 breaks.
    /// アクセスユニットにある最後の SPS。
    pub fn sequence(&self, data: &[u8]) -> Option<Sps> {
        let mut found = None;
        for nal in Nals::new(data, self.framing) {
            if (nal.header >> 1) & 0x3f != params::SPS_NUT {
                continue;
            }
            let bytes = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            let mut one = params::Sets::default();
            if one.absorb(params::SPS_NUT, bytes).is_ok() {
                found = one.sps.into_iter().flatten().next();
            }
        }
        found
    }

    pub fn take_entry_points(&mut self, offered: bool) {
        self.entry_points = offered;
    }

    pub fn take(&mut self) -> Option<Held> {
        self.order.take()
    }

    pub fn flush(&mut self, pool: &mut Pool) {
        self.finished(pool);
        self.order.flush(pool);
    }

    pub fn reset(&mut self, pool: &mut Pool) {
        let open = self.current.take().map(|current| current.ts);
        self.order.reset(pool, open);
    }

    /// The picture stays open until [`Hevc::finished`]. A damaged unit that still
    /// yielded slices is reported as success.
    pub fn decode(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<Option<Submission>, Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format other than 4:2:0"));
        }
        let mut ts = 0u64;
        let mut started = false;
        let mut damage = None;
        let mut slices: Vec<(Vec<Ctl>, std::ops::Range<usize>)> = Vec::new();

        for nal in Nals::new(data, self.framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let second = data.get(nal.body.start).copied().unwrap_or(0);
            let temporal_plus1 = second & 7;
            if (nal.header & 1) != 0 || (second >> 3) != 0 {
                continue;
            }
            let bytes = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            match kind {
                params::SPS_NUT | params::PPS_NUT => {
                    let _ = self.sets.absorb(kind, bytes);
                }
                kind if params::is_slice(kind) => {
                    let range = nal.body.start.saturating_sub(1)..nal.body.end;
                    let raw_len = range.len();
                    if let Err(error) = self.slice(
                        pool,
                        tag,
                        picture,
                        kind,
                        temporal_plus1,
                        bytes,
                        raw_len,
                        &mut slices,
                        range,
                        &mut ts,
                        &mut started,
                    ) {
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }

        if !started || slices.is_empty() {
            return match damage {
                Some(error) => Err(error),
                None => Ok(None),
            };
        }
        // The end of a picture is told by HOLD_CAPTURE_BUF, set by the caller.
        let units = match self.mode {
            Mode::Slice => slices
                .into_iter()
                .map(|(controls, range)| Unit {
                    controls,
                    pieces: vec![range],
                    zeroes: 0,
                })
                .collect(),
            Mode::Frame => {
                let pieces = slices.iter().map(|(_, range)| range.clone()).collect();
                let controls = slices
                    .into_iter()
                    .next()
                    .map_or_else(Vec::new, |(controls, _)| controls);
                vec![Unit {
                    controls,
                    pieces,
                    zeroes: 0,
                }]
            }
        };
        Ok(Some(Submission {
            ts,
            units,
            more: false,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn slice(
        &mut self,
        pool: &mut Pool,
        tag: u64,
        picture: &Picture,
        kind: u8,
        temporal_plus1: u8,
        bytes: &[u8],
        raw_len: usize,
        slices: &mut Vec<(Vec<Ctl>, std::ops::Range<usize>)>,
        range: std::ops::Range<usize>,
        ts: &mut u64,
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
            // 投入を拒否されて閉じられなかった前のフレームのバッファを返す。
            if let Some(stale) = self.current.take() {
                frames::Surfaces::release(pool, stale.ts);
            }
            self.order.skipping = false;
            let capacity = sps.max_dec_pic_buffering.clamp(1, 16);
            self.order
                .dpb
                .limits(sps.max_num_reorder.min(capacity), capacity);
            // Clause 8.1.3: leading pictures of the start point cannot be decoded.
            let Some(poc) = self.order.dpb.begin(
                &mut frames::Frames {
                    pool,
                    ready: &mut self.order.ready,
                },
                &sps,
                &header,
                kind,
                temporal_plus1.saturating_sub(1),
            ) else {
                self.order.skipping = true;
                return Ok(());
            };
            // 参照が欠けているピクチャー（途中から入ったときに前の GOP を参照するものなど）はデコードしない。
            if self
                .order
                .dpb
                .sets
                .iter()
                .flatten()
                .any(|entry| entry.id.is_none())
            {
                self.order.skipping = true;
                return Ok(());
            }
            let Some(picture_ts) = self.order.dpb.free(&mut frames::Frames {
                pool,
                ready: &mut self.order.ready,
            }) else {
                return Err(Error::Damaged(
                    "a picture buffer that cannot be made room in",
                ));
            };
            // The DPB first: list entries index into it.
            let decode = self.decode_parameters(&header, kind, poc);
            let mut shared = vec![
                Ctl::of(Kind::HevcSps, &sps_control(&sps)),
                Ctl::of(Kind::HevcPps, &pps_control(&sps, &pps)),
            ];
            if sps.scaling_list_enabled {
                let scaling = pps.scaling.as_ref().unwrap_or(&sps.scaling);
                shared.push(Ctl::of(Kind::HevcScalingMatrix, &scaling_matrix(scaling)));
            }
            shared.push(Ctl::of(Kind::HevcDecodeParams, &decode));
            self.current = Some(Current {
                ts: picture_ts,
                poc,
                held: frames::HevcOrder::held(picture_ts, tag, picture, poc),
                output: header.pic_output_flag
                    && !(params::is_rasl(kind) && self.order.dpb.drop_rasl()),
                header: Box::new(header.clone()),
                shared,
            });
            *ts = picture_ts;
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

        let parameters =
            self.slice_parameters(&sps, &pps, &header, kind, temporal_plus1, bytes, raw_len);
        let Some(current) = self.current.as_ref() else {
            return Err(Error::Damaged("a slice segment with no picture"));
        };
        let mut controls = current.shared.clone();
        controls.push(Ctl::of(Kind::HevcSliceParams, &parameters));
        if self.entry_points && !header.entry_points.is_empty() {
            controls.push(Ctl::words(
                Kind::HevcEntryPointOffsets,
                &entry_offsets(&header.entry_points),
            ));
        }
        slices.push((controls, range));
        Ok(())
    }

    fn decode_parameters(&mut self, header: &Header, kind: u8, poc: i32) -> ctrl::HevcDecodeParams {
        let mut dpb = [ctrl::HevcDpbEntry::default(); FRAMES];
        let mut active = 0usize;
        for (at, store) in self.order.dpb.references().take(FRAMES).enumerate() {
            dpb[at] = ctrl::HevcDpbEntry {
                timestamp: store.id,
                flags: if store.marked == Marked::Long {
                    ctrl::V4L2_HEVC_DPB_ENTRY_LONG_TERM_REFERENCE
                } else {
                    0
                },
                field_pic: 0,
                reserved: 0,
                pic_order_cnt_val: store.poc,
            };
            active = at + 1;
        }
        self.dpb = dpb;
        self.active = active;

        let mut parameters = ctrl::HevcDecodeParams {
            pic_order_cnt_val: poc,
            short_term_ref_pic_set_size: 0,
            long_term_ref_pic_set_size: 0,
            num_active_dpb_entries: active as u8,
            num_poc_st_curr_before: 0,
            num_poc_st_curr_after: 0,
            num_poc_lt_curr: 0,
            poc_st_curr_before: [0; FRAMES],
            poc_st_curr_after: [0; FRAMES],
            poc_lt_curr: [0; FRAMES],
            num_delta_pocs_of_ref_rps_idx: 0,
            reserved: [0; 3],
            dpb,
            flags: 0,
        };
        let sets = self.order.dpb.sets.clone();
        parameters.num_poc_st_curr_before = self.fill(&sets[0], &mut parameters.poc_st_curr_before);
        parameters.num_poc_st_curr_after = self.fill(&sets[1], &mut parameters.poc_st_curr_after);
        parameters.num_poc_lt_curr = self.fill(&sets[2], &mut parameters.poc_lt_curr);

        let bit = |value: bool, at: u64| u64::from(value) * at;
        parameters.flags = bit(
            params::is_irap(kind),
            ctrl::V4L2_HEVC_DECODE_PARAM_FLAG_IRAP_PIC,
        ) | bit(
            params::is_idr(kind),
            ctrl::V4L2_HEVC_DECODE_PARAM_FLAG_IDR_PIC,
        ) | bit(
            header.no_output_of_prior_pics,
            ctrl::V4L2_HEVC_DECODE_PARAM_FLAG_NO_OUTPUT_OF_PRIOR,
        );
        parameters
    }

    #[allow(clippy::too_many_arguments)]
    fn slice_parameters(
        &self,
        sps: &Sps,
        pps: &Pps,
        header: &Header,
        kind: u8,
        temporal_plus1: u8,
        bytes: &[u8],
        raw_len: usize,
    ) -> ctrl::HevcSliceParams {
        let lists = self.order.dpb.lists(header);
        let poc = self.current.as_ref().map_or(0, |current| current.poc);
        let structure = self
            .current
            .as_ref()
            .map_or(mediadecoder::es::Structure::Frame, |current| {
                current.held.structure
            });
        let mut parameters = ctrl::HevcSliceParams {
            bit_size: (self.prefix + raw_len as u32) * 8,
            data_byte_offset: data_byte_offset(self.prefix, bytes, header.data_at),
            num_entry_point_offsets: header.entry_points.len() as u32,
            nal_unit_type: kind,
            nuh_temporal_id_plus1: temporal_plus1,
            slice_type: match header.kind {
                params::Kind::B => ctrl::V4L2_HEVC_SLICE_TYPE_B,
                params::Kind::P => ctrl::V4L2_HEVC_SLICE_TYPE_P,
                params::Kind::I => ctrl::V4L2_HEVC_SLICE_TYPE_I,
            },
            colour_plane_id: 0,
            slice_pic_order_cnt: poc,
            num_ref_idx_l0_active_minus1: header.num_ref_idx[0].max(1) as u8 - 1,
            num_ref_idx_l1_active_minus1: header.num_ref_idx[1].max(1) as u8 - 1,
            collocated_ref_idx: if header.temporal_mvp_enabled {
                header.collocated_ref_idx as u8
            } else {
                NONE
            },
            five_minus_max_num_merge_cand: 5u8.saturating_sub(header.max_merge_cand as u8),
            slice_qp_delta: (header.qp - pps.init_qp) as i8,
            slice_cb_qp_offset: header.slice_qp_offset[0] as i8,
            slice_cr_qp_offset: header.slice_qp_offset[1] as i8,
            slice_act_y_qp_offset: 0,
            slice_act_cb_qp_offset: 0,
            slice_act_cr_qp_offset: 0,
            slice_beta_offset_div2: (header.beta_offset / 2) as i8,
            slice_tc_offset_div2: (header.tc_offset / 2) as i8,
            // `pic_struct` is in SEI; the container's structure is used instead.
            pic_struct: match structure {
                mediadecoder::es::Structure::Frame => 0,
                mediadecoder::es::Structure::Top => 1,
                mediadecoder::es::Structure::Bottom => 2,
            },
            reserved0: [0; 3],
            slice_segment_addr: header.segment_address as u32,
            ref_idx_l0: [NONE; FRAMES],
            ref_idx_l1: [NONE; FRAMES],
            // Not published, so zero; a driver that read them would decode wrongly.
            short_term_ref_pic_set_size: 0,
            long_term_ref_pic_set_size: 0,
            pred_weight_table: ctrl::HevcPredWeightTable {
                delta_luma_weight_l0: [0; FRAMES],
                luma_offset_l0: [0; FRAMES],
                delta_chroma_weight_l0: [[0; 2]; FRAMES],
                chroma_offset_l0: [[0; 2]; FRAMES],
                delta_luma_weight_l1: [0; FRAMES],
                luma_offset_l1: [0; FRAMES],
                delta_chroma_weight_l1: [[0; 2]; FRAMES],
                chroma_offset_l1: [[0; 2]; FRAMES],
                luma_log2_weight_denom: 0,
                delta_chroma_log2_weight_denom: 0,
            },
            reserved1: [0; 2],
            flags: 0,
        };

        let bit = |value: bool, at: u64| u64::from(value) * at;
        parameters.flags = bit(
            header.sao_luma,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_SAO_LUMA,
        ) | bit(
            header.sao_chroma,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_SAO_CHROMA,
        ) | bit(
            header.temporal_mvp_enabled,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_TEMPORAL_MVP_ENABLED,
        ) | bit(
            header.mvd_l1_zero,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_MVD_L1_ZERO,
        ) | bit(
            header.cabac_init_flag,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_CABAC_INIT,
        ) | bit(
            header.collocated_from_l0,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_COLLOCATED_FROM_L0,
        ) | bit(
            header.deblocking_disabled,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_DEBLOCKING_FILTER_DISABLED,
        ) | bit(
            header.loop_filter_across_slices,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_LOOP_FILTER_ACROSS_SLICES_ENABLED,
        ) | bit(
            header.dependent,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_DEPENDENT_SLICE_SEGMENT,
        );

        self.fill(&lists[0], &mut parameters.ref_idx_l0);
        self.fill(&lists[1], &mut parameters.ref_idx_l1);

        if (pps.weighted_pred && header.kind == params::Kind::P)
            || (pps.weighted_bipred && header.kind == params::Kind::B)
        {
            // The weight as a delta from unity, the offset scaled to 8 bits.
            let range = 1i32 << (sps.depth[0] - 8);
            let table = &mut parameters.pred_weight_table;
            table.luma_log2_weight_denom = header.luma_log2_denom as u8;
            table.delta_chroma_log2_weight_denom =
                (header.chroma_log2_denom - header.luma_log2_denom) as i8;
            for list in 0..2 {
                for at in 0..FRAMES {
                    let weight = header.weights[list][at];
                    let luma = weight[0].weight - (1 << header.luma_log2_denom);
                    let (delta_luma, luma_offset, delta_chroma, chroma_offset) = if list == 0 {
                        (
                            &mut table.delta_luma_weight_l0,
                            &mut table.luma_offset_l0,
                            &mut table.delta_chroma_weight_l0,
                            &mut table.chroma_offset_l0,
                        )
                    } else {
                        (
                            &mut table.delta_luma_weight_l1,
                            &mut table.luma_offset_l1,
                            &mut table.delta_chroma_weight_l1,
                            &mut table.chroma_offset_l1,
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

    /// 持っていないフレームを指すフィールドも位置は残し、中身は DPB の先頭にする（`0xff` はテーブルの外を読まれる）。
    fn fill(&self, entries: &[Entry<u64>], into: &mut [u8; FRAMES]) -> u8 {
        let mut count = 0u8;
        for (at, entry) in entries.iter().take(FRAMES).enumerate() {
            into[at] = match self.index_of(entry) {
                NONE => 0,
                index => index,
            };
            count += 1;
        }
        count
    }

    fn index_of(&self, entry: &Entry<u64>) -> u8 {
        let Some(ts) = entry.id else {
            return NONE;
        };
        for (at, held) in self.dpb.iter().take(self.active).enumerate() {
            if held.timestamp == ts && held.pic_order_cnt_val == entry.poc {
                return at as u8;
            }
        }
        NONE
    }
}

impl Hevc {
    /// Called once the submission has been sent.
    pub fn finished(&mut self, pool: &mut Pool) {
        let Some(current) = self.current.take() else {
            return;
        };
        self.order
            .push(pool, current.ts, current.poc, current.output, current.held);
    }
}

/// キャプチャーの形式を決める前にドライバーへ渡す SPS の制御。
pub fn sequence_control(sps: &Sps) -> Ctl {
    Ctl::of(Kind::HevcSps, &sps_control(sps))
}

fn sps_control(sps: &Sps) -> ctrl::HevcSps {
    let bit = |value: bool, at: u64| u64::from(value) * at;
    ctrl::HevcSps {
        video_parameter_set_id: 0,
        seq_parameter_set_id: sps.id as u8,
        pic_width_in_luma_samples: sps.width as u16,
        pic_height_in_luma_samples: sps.height as u16,
        bit_depth_luma_minus8: sps.depth[0].saturating_sub(8),
        bit_depth_chroma_minus8: sps.depth[1].saturating_sub(8),
        log2_max_pic_order_cnt_lsb_minus4: (sps.log2_max_poc_lsb - 4) as u8,
        sps_max_dec_pic_buffering_minus1: (sps.max_dec_pic_buffering.max(1) - 1) as u8,
        sps_max_num_reorder_pics: sps.max_num_reorder as u8,
        sps_max_latency_increase_plus1: 0,
        log2_min_luma_coding_block_size_minus3: (sps.min_cb_log2 - 3) as u8,
        log2_diff_max_min_luma_coding_block_size: (sps.ctb_log2 - sps.min_cb_log2) as u8,
        log2_min_luma_transform_block_size_minus2: (sps.min_tb_log2 - 2) as u8,
        log2_diff_max_min_luma_transform_block_size: (sps.max_tb_log2 - sps.min_tb_log2) as u8,
        max_transform_hierarchy_depth_inter: sps.max_transform_depth_inter as u8,
        max_transform_hierarchy_depth_intra: sps.max_transform_depth_intra as u8,
        pcm_sample_bit_depth_luma_minus1: sps.pcm_depth[0].saturating_sub(1),
        pcm_sample_bit_depth_chroma_minus1: sps.pcm_depth[1].saturating_sub(1),
        log2_min_pcm_luma_coding_block_size_minus3: (sps.pcm_min_log2.max(3) - 3) as u8,
        log2_diff_max_min_pcm_luma_coding_block_size: sps
            .pcm_max_log2
            .saturating_sub(sps.pcm_min_log2)
            as u8,
        num_short_term_ref_pic_sets: sps.short_term.len() as u8,
        num_long_term_ref_pics_sps: sps.long_term_poc_lsb.len() as u8,
        chroma_format_idc: sps.chroma_format as u8,
        sps_max_sub_layers_minus1: 0,
        reserved: [0; 6],
        flags: bit(
            sps.scaling_list_enabled,
            ctrl::V4L2_HEVC_SPS_FLAG_SCALING_LIST_ENABLED,
        ) | bit(sps.amp_enabled, ctrl::V4L2_HEVC_SPS_FLAG_AMP_ENABLED)
            | bit(
                sps.sao_enabled,
                ctrl::V4L2_HEVC_SPS_FLAG_SAMPLE_ADAPTIVE_OFFSET,
            )
            | bit(sps.pcm_enabled, ctrl::V4L2_HEVC_SPS_FLAG_PCM_ENABLED)
            | bit(
                sps.pcm_loop_filter_disabled,
                ctrl::V4L2_HEVC_SPS_FLAG_PCM_LOOP_FILTER_DISABLED,
            )
            | bit(
                sps.long_term_present,
                ctrl::V4L2_HEVC_SPS_FLAG_LONG_TERM_REF_PICS_PRESENT,
            )
            | bit(
                sps.temporal_mvp_enabled,
                ctrl::V4L2_HEVC_SPS_FLAG_SPS_TEMPORAL_MVP_ENABLED,
            )
            | bit(
                sps.strong_intra_smoothing,
                ctrl::V4L2_HEVC_SPS_FLAG_STRONG_INTRA_SMOOTHING_ENABLED,
            ),
    }
}

fn pps_control(sps: &Sps, pps: &Pps) -> ctrl::HevcPps {
    let bit = |value: bool, at: u64| u64::from(value) * at;
    let mut control = ctrl::HevcPps {
        pic_parameter_set_id: pps.id as u8,
        num_extra_slice_header_bits: pps.num_extra_slice_header_bits as u8,
        num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_default[0].max(1) as u8 - 1,
        num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_default[1].max(1) as u8 - 1,
        init_qp_minus26: (pps.init_qp - 26) as i8,
        diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth as u8,
        pps_cb_qp_offset: pps.qp_offset[0] as i8,
        pps_cr_qp_offset: pps.qp_offset[1] as i8,
        num_tile_columns_minus1: pps.column_width.len().max(1) as u8 - 1,
        num_tile_rows_minus1: pps.row_height.len().max(1) as u8 - 1,
        column_width_minus1: [0; 20],
        row_height_minus1: [0; 22],
        pps_beta_offset_div2: (pps.beta_offset / 2) as i8,
        pps_tc_offset_div2: (pps.tc_offset / 2) as i8,
        log2_parallel_merge_level_minus2: (pps.log2_parallel_merge_level - 2) as u8,
        reserved: 0,
        flags: 0,
    };
    // One byte each: wide tile columns are clamped.
    for (at, width) in pps.column_width.iter().take(20).enumerate() {
        control.column_width_minus1[at] = width.clamp(&1, &256).saturating_sub(1) as u8;
    }
    for (at, height) in pps.row_height.iter().take(22).enumerate() {
        control.row_height_minus1[at] = height.clamp(&1, &256).saturating_sub(1) as u8;
    }
    // No tiles is one picture-sized tile (6.5.1); some drivers read these for WPP too.
    if pps.column_width.is_empty() {
        control.column_width_minus1[0] = sps.ctb_width().clamp(1, 256).saturating_sub(1) as u8;
    }
    if pps.row_height.is_empty() {
        control.row_height_minus1[0] = sps.ctb_height().clamp(1, 256).saturating_sub(1) as u8;
    }
    control.flags = bit(
        pps.dependent_slice_segments_enabled,
        ctrl::V4L2_HEVC_PPS_FLAG_DEPENDENT_SLICE_SEGMENT_ENABLED,
    ) | bit(
        pps.output_flag_present,
        ctrl::V4L2_HEVC_PPS_FLAG_OUTPUT_FLAG_PRESENT,
    ) | bit(
        pps.sign_data_hiding,
        ctrl::V4L2_HEVC_PPS_FLAG_SIGN_DATA_HIDING_ENABLED,
    ) | bit(
        pps.cabac_init_present,
        ctrl::V4L2_HEVC_PPS_FLAG_CABAC_INIT_PRESENT,
    ) | bit(
        pps.constrained_intra_pred,
        ctrl::V4L2_HEVC_PPS_FLAG_CONSTRAINED_INTRA_PRED,
    ) | bit(
        pps.transform_skip_enabled,
        ctrl::V4L2_HEVC_PPS_FLAG_TRANSFORM_SKIP_ENABLED,
    ) | bit(
        pps.cu_qp_delta_enabled,
        ctrl::V4L2_HEVC_PPS_FLAG_CU_QP_DELTA_ENABLED,
    ) | bit(
        pps.slice_chroma_qp_offsets_present,
        ctrl::V4L2_HEVC_PPS_FLAG_PPS_SLICE_CHROMA_QP_OFFSETS_PRESENT,
    ) | bit(pps.weighted_pred, ctrl::V4L2_HEVC_PPS_FLAG_WEIGHTED_PRED)
        | bit(
            pps.weighted_bipred,
            ctrl::V4L2_HEVC_PPS_FLAG_WEIGHTED_BIPRED,
        )
        | bit(
            pps.transquant_bypass_enabled,
            ctrl::V4L2_HEVC_PPS_FLAG_TRANSQUANT_BYPASS_ENABLED,
        )
        | bit(pps.tiles_enabled, ctrl::V4L2_HEVC_PPS_FLAG_TILES_ENABLED)
        | bit(
            pps.entropy_coding_sync_enabled,
            ctrl::V4L2_HEVC_PPS_FLAG_ENTROPY_CODING_SYNC_ENABLED,
        )
        | bit(
            pps.loop_filter_across_tiles,
            ctrl::V4L2_HEVC_PPS_FLAG_LOOP_FILTER_ACROSS_TILES_ENABLED,
        )
        | bit(
            pps.loop_filter_across_slices,
            ctrl::V4L2_HEVC_PPS_FLAG_PPS_LOOP_FILTER_ACROSS_SLICES_ENABLED,
        )
        | bit(
            pps.deblocking_override_enabled,
            ctrl::V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_OVERRIDE_ENABLED,
        )
        | bit(
            pps.deblocking_disabled,
            ctrl::V4L2_HEVC_PPS_FLAG_PPS_DISABLE_DEBLOCKING_FILTER,
        )
        | bit(
            pps.lists_modification_present,
            ctrl::V4L2_HEVC_PPS_FLAG_LISTS_MODIFICATION_PRESENT,
        )
        | bit(
            pps.slice_header_extension_present,
            ctrl::V4L2_HEVC_PPS_FLAG_SLICE_SEGMENT_HEADER_EXTENSION_PRESENT,
        )
        | bit(
            deblocking_control_present(pps),
            ctrl::V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT,
        )
        | bit(
            uniform_spacing(sps, pps),
            ctrl::V4L2_HEVC_PPS_FLAG_UNIFORM_SPACING,
        );
    control
}

/// Inferred from the block's contents; an all-default block reads as absent, harmlessly.
fn deblocking_control_present(pps: &Pps) -> bool {
    pps.deblocking_override_enabled
        || pps.deblocking_disabled
        || pps.beta_offset != 0
        || pps.tc_offset != 0
}

/// Whether the tiling is the one equation 6-3 computes.
fn uniform_spacing(sps: &Sps, pps: &Pps) -> bool {
    if pps.column_width.is_empty() && pps.row_height.is_empty() {
        return true;
    }
    let even = |sizes: &[usize], whole: usize| {
        let count = sizes.len();
        count > 0
            && sizes
                .iter()
                .enumerate()
                .all(|(i, &size)| size == (i + 1) * whole / count - i * whole / count)
    };
    even(&pps.column_width, sps.ctb_width()) && even(&pps.row_height, sps.ctb_height())
}

/// In the block's raster, not the diagonal scan (despite the docs); drivers do not rescan.
fn scaling_matrix(matrices: &params::Scaling) -> ctrl::HevcScalingMatrix {
    ctrl::HevcScalingMatrix {
        scaling_list_4x4: std::array::from_fn(|matrix| scaling::list4(matrices, matrix)),
        scaling_list_8x8: std::array::from_fn(|matrix| scaling::list(matrices, 1, matrix)),
        scaling_list_16x16: std::array::from_fn(|matrix| scaling::list(matrices, 2, matrix)),
        // Intra and inter, at 0 and 3 of the six.
        scaling_list_32x32: std::array::from_fn(|matrix| scaling::list(matrices, 3, matrix * 3)),
        scaling_list_dc_coef_16x16: std::array::from_fn(|matrix| matrices.matrix(2, matrix)[0]),
        scaling_list_dc_coef_32x32: std::array::from_fn(|matrix| matrices.matrix(3, matrix * 3)[0]),
    }
}

/// Each substream's length in bytes, escapes counted, from the running positions.
fn entry_offsets(positions: &[usize]) -> Vec<u32> {
    let mut previous = 0;
    positions
        .iter()
        .map(|&at| {
            let offset = at.saturating_sub(previous);
            previous = at;
            offset as u32
        })
        .collect()
}

/// Counted over the escaped bytes from the start of the buffer (no escape count field).
fn data_byte_offset(prefix: u32, bytes: &[u8], data_at: usize) -> u32 {
    prefix + 2 + data_at as u32 + escapes_before(bytes, data_at) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::video::hevc::dpb::Store;
    use mediadecoder::video::hevc::params::{Scaling, ShortTerm};

    fn sequence() -> Sps {
        Sps {
            id: 0,
            chroma_format: 1,
            width: 3840,
            height: 2160,
            crop: [0; 4],
            depth: [10, 10],
            log2_max_poc_lsb: 8,
            max_dec_pic_buffering: 6,
            max_num_reorder: 2,
            min_cb_log2: 3,
            ctb_log2: 6,
            min_tb_log2: 2,
            max_tb_log2: 5,
            max_transform_depth_inter: 3,
            max_transform_depth_intra: 3,
            scaling_list_enabled: false,
            scaling: Scaling::default(),
            amp_enabled: false,
            sao_enabled: false,
            pcm_enabled: false,
            pcm_depth: [8, 8],
            pcm_min_log2: 3,
            pcm_max_log2: 3,
            pcm_loop_filter_disabled: false,
            short_term: Vec::new(),
            long_term_present: false,
            long_term_poc_lsb: Vec::new(),
            long_term_used: Vec::new(),
            temporal_mvp_enabled: false,
            strong_intra_smoothing: false,
        }
    }

    fn picture() -> Pps {
        Pps {
            id: 0,
            sps_id: 0,
            dependent_slice_segments_enabled: false,
            output_flag_present: false,
            num_extra_slice_header_bits: 0,
            sign_data_hiding: false,
            cabac_init_present: false,
            num_ref_idx_default: [1, 1],
            init_qp: 26,
            constrained_intra_pred: false,
            transform_skip_enabled: false,
            cu_qp_delta_enabled: false,
            diff_cu_qp_delta_depth: 0,
            qp_offset: [0, 0],
            slice_chroma_qp_offsets_present: false,
            weighted_pred: false,
            weighted_bipred: false,
            transquant_bypass_enabled: false,
            tiles_enabled: false,
            entropy_coding_sync_enabled: false,
            column_width: Vec::new(),
            row_height: Vec::new(),
            loop_filter_across_tiles: false,
            loop_filter_across_slices: false,
            deblocking_override_enabled: false,
            deblocking_disabled: false,
            beta_offset: 0,
            tc_offset: 0,
            scaling_list_present: false,
            scaling: None,
            lists_modification_present: false,
            log2_parallel_merge_level: 2,
            slice_header_extension_present: false,
        }
    }

    fn store(ts: u64, poc: i32, marked: Marked) -> Store<u64, Held> {
        Store {
            id: ts,
            poc,
            marked,
            output: true,
            held: Held::default(),
        }
    }

    fn found(ts: u64, poc: i32) -> Entry<u64> {
        Entry {
            poc,
            long: false,
            id: Some(ts),
        }
    }

    const MISSING: Entry<u64> = Entry {
        poc: 0,
        long: false,
        id: None,
    };

    #[test]
    fn entry_points_go_back_to_substream_lengths() {
        assert_eq!(entry_offsets(&[10, 30, 35]), vec![10, 20, 5]);
        assert!(entry_offsets(&[]).is_empty());
    }

    #[test]
    fn the_data_offset_is_the_start_code_the_unit_header_and_the_escaped_header() {
        let bytes = [0, 0, 3, 1, 2, 3, 0xaa, 0xbb];
        assert_eq!(data_byte_offset(0, &bytes, 6), 2 + 6 + 1);
        assert_eq!(data_byte_offset(3, &bytes, 6), 3 + 2 + 6 + 1);
        let plain = [1, 2, 3, 4, 5, 6, 7];
        assert_eq!(data_byte_offset(3, &plain, 5), 3 + 2 + 5);
    }

    #[test]
    fn the_three_sets_hold_indices_into_the_decoded_picture_buffer() {
        let mut hevc = Hevc::new(Framing::StartCodes);
        hevc.order.dpb.stores = vec![
            store(1_000_000, 4, Marked::Short),
            store(2_000_000, 8, Marked::No),
            store(3_000_000, 12, Marked::Short),
            store(4_000_000, 2, Marked::Long),
        ];
        hevc.order.dpb.sets = [
            vec![found(1_000_000, 4)],
            vec![found(3_000_000, 12)],
            vec![found(4_000_000, 2)],
        ];
        hevc.order.dpb.total = 3;
        let params = hevc.decode_parameters(&Header::default(), params::IDR_W_RADL, 16);

        assert_eq!(params.num_active_dpb_entries, 3);
        assert_eq!(params.dpb[0].timestamp, 1_000_000);
        assert_eq!(params.dpb[1].timestamp, 3_000_000);
        assert_eq!(params.dpb[2].timestamp, 4_000_000);
        assert_eq!(
            params.dpb[2].flags,
            ctrl::V4L2_HEVC_DPB_ENTRY_LONG_TERM_REFERENCE
        );

        assert_eq!(params.num_poc_st_curr_before, 1);
        assert_eq!(params.num_poc_st_curr_after, 1);
        assert_eq!(params.num_poc_lt_curr, 1);
        assert_eq!(params.poc_st_curr_before[0], 0);
        assert_eq!(params.poc_st_curr_after[0], 1);
        assert_eq!(params.poc_lt_curr[0], 2);
        assert_eq!(
            params.flags,
            ctrl::V4L2_HEVC_DECODE_PARAM_FLAG_IRAP_PIC | ctrl::V4L2_HEVC_DECODE_PARAM_FLAG_IDR_PIC
        );
    }

    #[test]
    fn a_reference_that_was_never_had_keeps_its_place_inside_the_dpb() {
        let mut hevc = Hevc::new(Framing::StartCodes);
        hevc.order.dpb.stores = vec![
            store(1_000_000, 4, Marked::Short),
            store(2_000_000, 8, Marked::Short),
        ];
        hevc.order.dpb.sets = [
            vec![MISSING, found(1_000_000, 4), found(2_000_000, 8)],
            Vec::new(),
            Vec::new(),
        ];
        hevc.order.dpb.total = 3;
        let params = hevc.decode_parameters(&Header::default(), params::RASL_R, 16);
        assert_eq!(params.num_poc_st_curr_before, 3);
        assert_eq!(params.poc_st_curr_before[0], 0);
        assert_eq!(params.poc_st_curr_before[1], 0);
        assert_eq!(params.poc_st_curr_before[2], 1);
        assert_eq!(hevc.index_of(&found(1_000_000, 5)), NONE);
    }

    #[test]
    fn a_reference_list_is_indices_with_a_stand_in_where_one_is_missing() {
        let mut hevc = Hevc::new(Framing::StartCodes);
        hevc.order.dpb.stores = vec![
            store(1_000_000, 4, Marked::Short),
            store(2_000_000, 8, Marked::Short),
        ];
        hevc.decode_parameters(&Header::default(), params::RASL_R, 16);
        let mut into = [0u8; FRAMES];
        let count = hevc.fill(
            &[found(2_000_000, 8), MISSING, found(1_000_000, 4)],
            &mut into,
        );
        assert_eq!(count, 3);
        assert_eq!(&into[..3], &[1, 0, 0]);
    }

    #[test]
    fn the_sequence_flags_are_at_the_bits_the_header_declares() {
        let mut sps = sequence();
        sps.scaling_list_enabled = true;
        sps.sao_enabled = true;
        sps.temporal_mvp_enabled = true;
        let control = sps_control(&sps);
        assert_eq!(
            control.flags,
            ctrl::V4L2_HEVC_SPS_FLAG_SCALING_LIST_ENABLED
                | ctrl::V4L2_HEVC_SPS_FLAG_SAMPLE_ADAPTIVE_OFFSET
                | ctrl::V4L2_HEVC_SPS_FLAG_SPS_TEMPORAL_MVP_ENABLED
        );
        assert_eq!(control.flags, (1 << 1) | (1 << 3) | (1 << 7));
        assert_eq!(sps_control(&sequence()).flags, 0);

        assert_eq!(control.pic_width_in_luma_samples, 3840);
        assert_eq!(control.bit_depth_luma_minus8, 2);
        assert_eq!(control.log2_max_pic_order_cnt_lsb_minus4, 4);
        assert_eq!(control.sps_max_dec_pic_buffering_minus1, 5);
        assert_eq!(control.log2_min_luma_coding_block_size_minus3, 0);
        assert_eq!(control.log2_diff_max_min_luma_coding_block_size, 3);
    }

    #[test]
    fn the_picture_flags_are_at_the_bits_the_header_declares() {
        let sps = sequence();
        let mut pps = picture();
        pps.weighted_pred = true;
        pps.tiles_enabled = true;
        pps.lists_modification_present = true;
        let control = pps_control(&sps, &pps);
        assert_eq!(
            control.flags,
            ctrl::V4L2_HEVC_PPS_FLAG_WEIGHTED_PRED
                | ctrl::V4L2_HEVC_PPS_FLAG_TILES_ENABLED
                | ctrl::V4L2_HEVC_PPS_FLAG_LISTS_MODIFICATION_PRESENT
                | ctrl::V4L2_HEVC_PPS_FLAG_UNIFORM_SPACING
        );
        assert_eq!(control.flags, (1 << 8) | (1 << 11) | (1 << 17) | (1 << 20));

        let mut off = picture();
        off.deblocking_disabled = true;
        assert_eq!(
            pps_control(&sps, &off).flags
                & ctrl::V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT,
            ctrl::V4L2_HEVC_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT
        );

        let mut uneven = picture();
        uneven.tiles_enabled = true;
        uneven.column_width = vec![20, 40];
        uneven.row_height = vec![34];
        let control = pps_control(&sps, &uneven);
        assert_eq!(control.flags & ctrl::V4L2_HEVC_PPS_FLAG_UNIFORM_SPACING, 0);
        assert_eq!(control.num_tile_columns_minus1, 1);
        assert_eq!(control.column_width_minus1[0], 19);
        assert_eq!(control.column_width_minus1[1], 39);
        let mut even = picture();
        even.tiles_enabled = true;
        even.column_width = vec![30, 30];
        even.row_height = vec![34];
        assert_ne!(
            pps_control(&sps, &even).flags & ctrl::V4L2_HEVC_PPS_FLAG_UNIFORM_SPACING,
            0
        );
    }

    #[test]
    fn the_slice_flags_are_at_the_bits_the_header_declares() {
        let sps = sequence();
        let pps = picture();
        let header = Header {
            kind: params::Kind::P,
            sao_luma: true,
            temporal_mvp_enabled: true,
            collocated_ref_idx: 1,
            cabac_init_flag: true,
            dependent: true,
            num_ref_idx: [2, 0],
            max_merge_cand: 5,
            qp: 30,
            data_at: 6,
            short_term: ShortTerm::default(),
            ..Header::default()
        };

        let hevc = Hevc::new(Framing::StartCodes);
        let bytes = [0, 0, 3, 1, 2, 3, 0xaa, 0xbb];
        let params = hevc.slice_parameters(
            &sps,
            &pps,
            &header,
            params::RASL_R,
            1,
            &bytes,
            bytes.len() + 2,
        );
        assert_eq!(
            params.flags,
            ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_SAO_LUMA
                | ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_SLICE_TEMPORAL_MVP_ENABLED
                | ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_CABAC_INIT
                | ctrl::V4L2_HEVC_SLICE_PARAMS_FLAG_DEPENDENT_SLICE_SEGMENT
        );
        assert_eq!(params.flags, 1 | (1 << 2) | (1 << 4) | (1 << 9));
        assert_eq!(params.slice_type, ctrl::V4L2_HEVC_SLICE_TYPE_P);
        assert_eq!(params.nal_unit_type, params::RASL_R);
        assert_eq!(params.nuh_temporal_id_plus1, 1);
        assert_eq!(params.num_ref_idx_l0_active_minus1, 1);
        assert_eq!(params.slice_qp_delta, 4);
        assert_eq!(params.collocated_ref_idx, 1);
        assert_eq!(params.bit_size, (bytes.len() as u32 + 2) * 8);
        assert_eq!(params.data_byte_offset, 2 + 6 + 1);
        assert_eq!(params.ref_idx_l1, [NONE; FRAMES]);

        let plain = Header {
            temporal_mvp_enabled: false,
            ..header.clone()
        };
        let params = hevc.slice_parameters(&sps, &pps, &plain, params::RASL_R, 1, &bytes, 10);
        assert_eq!(params.collocated_ref_idx, NONE);
    }

    #[test]
    fn the_scaling_matrix_comes_out_in_the_raster() {
        let scaling = Scaling::default();
        let matrix = scaling_matrix(&scaling);

        assert_eq!(matrix.scaling_list_4x4[0], [16; 16]);

        assert_eq!(
            &matrix.scaling_list_8x8[0][..8],
            &[16, 16, 16, 16, 17, 18, 21, 24]
        );
        assert_eq!(matrix.scaling_list_8x8[0][63], 115);
        assert_eq!(
            &matrix.scaling_list_8x8[3][..8],
            &[16, 16, 16, 16, 17, 18, 20, 24]
        );
        assert_eq!(
            &matrix.scaling_list_8x8[3][56..],
            &[24, 25, 28, 33, 41, 54, 71, 91]
        );

        assert_eq!(matrix.scaling_list_16x16[0], matrix.scaling_list_8x8[0]);
        assert_eq!(matrix.scaling_list_32x32[0], matrix.scaling_list_8x8[0]);
        assert_eq!(matrix.scaling_list_32x32[1], matrix.scaling_list_8x8[3]);

        assert_eq!(matrix.scaling_list_dc_coef_16x16, [16; 6]);
        assert_eq!(matrix.scaling_list_dc_coef_32x32, [16; 2]);
    }
}
