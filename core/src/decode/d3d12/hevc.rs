// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC, handed to the hardware. Parsing and the DPB come from [`mediadecoder::video::hevc`];
//! what is left is `RefPicList` and the three sets of clause 8.3.2 as indices into it.

use mediadecoder::bits::unescape;
use mediadecoder::es::{Format, Framing, Nals, Picture};
use mediadecoder::video::hevc::dpb::Marked;
use mediadecoder::video::hevc::params::{self, Header, Kind, Pps, Sps};
use mediadecoder::video::hevc::scaling;
use windows::Win32::Media::MediaFoundation::{
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL,
};

use crate::decode::Error;
use crate::decode::d3d12::dx;
use crate::decode::d3d12::dxva::{
    FRAMES, PicEntry, PicParamsHevc, QmatrixHevc, SliceShort, bit, diagonal,
};
use crate::decode::d3d12::pool::Pool;
use crate::decode::d3d12::{Held, References, Session, argument, slice_argument};
use crate::decode::frames;
use crate::decode::rps::{Shape, short_term_bits};

/// SPS が示す DPB の枚数（`sps_max_dec_pic_buffering`）。サーフェスはこれで数える
/// （規格上限の 16 では 4K で 1 GB 近くになる）。
pub fn buffering(data: &[u8], framing: Framing) -> Option<usize> {
    Nals::new(data, framing)
        .filter(|nal| (nal.header >> 1) & 0x3f == params::SPS_NUT)
        .filter_map(|nal| {
            let body = data.get(nal.body.start + 1..nal.body.end)?;
            params::sequence(&unescape(body, body.len())).ok()
        })
        .map(|sps| sps.max_dec_pic_buffering)
        .max()
}

#[derive(Debug, Clone)]
struct Current {
    slot: usize,
    poc: i32,
    held: Held,
    output: bool,
}

#[derive(Debug)]
pub struct Hevc {
    framing: Framing,
    sets: params::Sets,
    order: frames::HevcOrder<usize>,
    current: Option<Current>,
    report: u32,
}

impl Hevc {
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            sets: params::Sets::default(),
            order: frames::HevcOrder::new(),
            current: None,
            report: 0,
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
        device: &mut dx::Device,
        session: &mut Session,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format other than 4:2:0"));
        }
        let mut parameters = PicParamsHevc::default();
        let mut matrix: Option<QmatrixHevc> = None;
        let mut slices: Vec<SliceShort> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut slot = None;
        let mut used: Vec<usize> = Vec::new();
        let mut damage = None;

        for nal in Nals::new(data, self.framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let second = data.get(nal.body.start).copied().unwrap_or(0);
            let temporal = (second & 7).saturating_sub(1);
            if (nal.header & 1) != 0 || (second >> 3) != 0 {
                continue;
            }
            let body = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            match kind {
                params::SPS_NUT | params::PPS_NUT => {
                    let _ = self.sets.absorb(kind, body);
                }
                params::EOS_NUT | params::EOB_NUT => {
                    // The next picture starts a sequence, so its RASL pictures are dropped (as the
                    // software decoder does; otherwise the output order differs).
                    self.finish(&mut session.pool);
                    self.order.dpb.end_of_sequence();
                }
                kind if params::is_slice(kind) => {
                    let raw = data.get(nal.body.start - 1..nal.body.end).unwrap_or(&[]);
                    match self.slice(
                        session,
                        tag,
                        picture,
                        format,
                        kind,
                        temporal,
                        body,
                        slot.is_some(),
                    ) {
                        Ok(Some((at, made, weights, named))) => {
                            slot = Some(at);
                            parameters = made;
                            matrix = weights;
                            used = named;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            damage.get_or_insert(error);
                            continue;
                        }
                    }
                    if slot.is_none() || self.order.skipping {
                        continue;
                    }
                    slices.push(SliceShort {
                        location: bytes.len() as u32,
                        bytes: (raw.len() + 3) as u32,
                        bad_chopping: 0,
                    });
                    bytes.extend_from_slice(&[0, 0, 1]);
                    bytes.extend_from_slice(raw);
                }
                _ => {}
            }
        }

        let Some(slot) = slot else {
            return match damage {
                Some(error) => Err(error),
                None => Ok(()),
            };
        };
        let mut arguments = vec![argument(
            D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
            &parameters,
        )];
        if let Some(matrix) = matrix.as_ref() {
            arguments.push(argument(
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
                matrix,
            ));
        }
        arguments.push(slice_argument(
            D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL,
            &slices,
        ));
        session.fill_stand_ins(device);
        // HEVC にはマクロブロックが無いので D3D11 へも 0。
        session.submit(
            device,
            slot,
            References::BySlot(&used),
            &bytes,
            &arguments,
            0,
        )?;
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn slice(
        &mut self,
        session: &mut Session,
        tag: u64,
        picture: &Picture,
        format: Format,
        kind: u8,
        temporal: u8,
        body: &[u8],
        started: bool,
    ) -> Result<Option<(usize, PicParamsHevc, Option<QmatrixHevc>, Vec<usize>)>, Error> {
        if started {
            return Ok(None);
        }
        let data = unescape(body, body.len());
        let mut reader = mediadecoder::bits::Reader::new(&data);
        let first_segment = reader.bit() == 1;
        if !first_segment {
            return Ok(None);
        }
        // The previous picture is closed here, as the software decoder does.
        self.finish(&mut session.pool);
        if params::is_irap(kind) {
            reader.bit();
        }
        let pps_id = reader.ue();
        let Some((pps, sps)) = self.sets.find(pps_id) else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        let header = params::header(&data, kind, &sps, &pps, None)?;
        // サーフェスはストリームが示すサイズで作った。SPS がより大きいサイズを示すとドライバーがサーフェスの外へ書く。
        if sps.width > (format.width as usize).next_multiple_of(64)
            || sps.height > (format.height as usize).next_multiple_of(64)
        {
            return Err(Error::Damaged(
                "a sequence larger than the decoder was opened for",
            ));
        }

        self.order
            .dpb
            .limits(sps.max_num_reorder, sps.max_dec_pic_buffering);
        let Some((slot, poc)) =
            self.order
                .begin(&mut session.pool, &sps, &header, kind, temporal)?
        else {
            return Ok(None);
        };
        self.current = Some(Current {
            slot,
            poc,
            held: frames::HevcOrder::held(slot, tag, picture, poc),
            output: header.pic_output_flag
                && !(params::is_rasl(kind) && self.order.dpb.drop_rasl()),
        });
        let rps = short_term_bits(&data, kind, &Shape::of(&sps, &pps), &sps.short_term);
        let references: Vec<usize> = self
            .order
            .dpb
            .references()
            .take(FRAMES)
            .map(|store| store.id)
            .filter(|id| *id != slot)
            .collect();
        if let Err(error) = session.pool.prepare(slot, &references) {
            // スライスの無いピクチャーは DPB に入れない（後のピクチャーが参照すると配列の外を指す）。
            self.current = None;
            session.pool.release(slot);
            return Err(error);
        }
        let (parameters, used) =
            self.picture_parameters(&session.pool, &sps, &pps, &header, kind, poc, slot, rps);
        let matrix = sps.scaling_list_enabled.then(|| {
            let scaling = pps.scaling.as_ref().unwrap_or(&sps.scaling);
            iq_matrix(scaling)
        });
        Ok(Some((slot, parameters, matrix, used)))
    }

    #[allow(clippy::too_many_arguments)]
    fn picture_parameters(
        &mut self,
        pool: &Pool,
        sps: &Sps,
        pps: &Pps,
        header: &Header,
        kind: u8,
        poc: i32,
        slot: usize,
        rps: (i32, i32),
    ) -> (PicParamsHevc, Vec<usize>) {
        let mut parameters = PicParamsHevc {
            width_in_min_cbs: (sps.width >> sps.min_cb_log2) as u16,
            height_in_min_cbs: (sps.height >> sps.min_cb_log2) as u16,
            format_fields: PicParamsHevc::format_fields(
                sps.chroma_format,
                u32::from(sps.depth[0].saturating_sub(8)),
                u32::from(sps.depth[1].saturating_sub(8)),
                sps.log2_max_poc_lsb.saturating_sub(4),
                sps.max_num_reorder == 0,
            ),
            current: pool
                .index(slot)
                .map_or(PicEntry::NONE, |index| PicEntry::new(index, false)),
            max_dec_pic_buffering_minus1: (sps.max_dec_pic_buffering.max(1) - 1) as u8,
            log2_min_luma_cb_minus3: (sps.min_cb_log2 - 3) as u8,
            log2_diff_max_min_luma_cb: (sps.ctb_log2 - sps.min_cb_log2) as u8,
            log2_min_tb_minus2: (sps.min_tb_log2 - 2) as u8,
            log2_diff_max_min_tb: (sps.max_tb_log2 - sps.min_tb_log2) as u8,
            max_transform_hierarchy_depth_inter: sps.max_transform_depth_inter as u8,
            max_transform_hierarchy_depth_intra: sps.max_transform_depth_intra as u8,
            num_short_term_ref_pic_sets: sps.short_term.len() as u8,
            num_long_term_ref_pics_sps: sps.long_term_poc_lsb.len() as u8,
            num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_default[0].max(1) as u8 - 1,
            num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_default[1].max(1) as u8 - 1,
            init_qp_minus26: (pps.init_qp - 26) as i8,
            // For a driver to step over a short-term RPS written in the slice header
            // ([`crate::decode::rps`]). x265 writes it out every time.
            num_delta_pocs_of_ref_rps_idx: rps.1 as u8,
            bits_for_short_term_rps_in_slice: rps.0 as u16,
            tool_flags: bit(sps.scaling_list_enabled, 0)
                | bit(sps.amp_enabled, 1)
                | bit(sps.sao_enabled, 2)
                | bit(sps.pcm_enabled, 3)
                | ((u32::from(sps.pcm_depth[0].saturating_sub(1)) & 15) << 4)
                | ((u32::from(sps.pcm_depth[1].saturating_sub(1)) & 15) << 8)
                | (((sps.pcm_min_log2.max(3) - 3) as u32 & 3) << 12)
                | (((sps.pcm_max_log2.saturating_sub(sps.pcm_min_log2)) as u32 & 3) << 14)
                | bit(sps.pcm_loop_filter_disabled, 16)
                | bit(sps.long_term_present, 17)
                | bit(sps.temporal_mvp_enabled, 18)
                | bit(sps.strong_intra_smoothing, 19)
                | bit(pps.dependent_slice_segments_enabled, 20)
                | bit(pps.output_flag_present, 21)
                | ((pps.num_extra_slice_header_bits as u32 & 7) << 22)
                | bit(pps.sign_data_hiding, 25)
                | bit(pps.cabac_init_present, 26),
            picture_flags: bit(pps.constrained_intra_pred, 0)
                | bit(pps.transform_skip_enabled, 1)
                | bit(pps.cu_qp_delta_enabled, 2)
                | bit(pps.slice_chroma_qp_offsets_present, 3)
                | bit(pps.weighted_pred, 4)
                | bit(pps.weighted_bipred, 5)
                | bit(pps.transquant_bypass_enabled, 6)
                | bit(pps.tiles_enabled, 7)
                | bit(pps.entropy_coding_sync_enabled, 8)
                | bit(pps.column_width.is_empty() && pps.row_height.is_empty(), 9)
                | bit(pps.loop_filter_across_tiles, 10)
                | bit(pps.loop_filter_across_slices, 11)
                | bit(pps.deblocking_override_enabled, 12)
                | bit(pps.deblocking_disabled, 13)
                | bit(pps.lists_modification_present, 14)
                | bit(pps.slice_header_extension_present, 15)
                | bit(params::is_irap(kind), 16)
                | bit(params::is_idr(kind), 17)
                | bit(header.kind == Kind::I, 18),
            pps_cb_qp_offset: pps.qp_offset[0] as i8,
            pps_cr_qp_offset: pps.qp_offset[1] as i8,
            num_tile_columns_minus1: pps.column_width.len().max(1) as u8 - 1,
            num_tile_rows_minus1: pps.row_height.len().max(1) as u8 - 1,
            diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth as u8,
            pps_beta_offset_div2: (pps.beta_offset / 2) as i8,
            pps_tc_offset_div2: (pps.tc_offset / 2) as i8,
            log2_parallel_merge_level_minus2: (pps.log2_parallel_merge_level - 2) as u8,
            current_order: poc,
            status_report: {
                self.report = self.report.wrapping_add(1).max(1);
                self.report
            },
            ..PicParamsHevc::default()
        };
        for (at, width) in pps.column_width.iter().take(19).enumerate() {
            parameters.column_width_minus1[at] = width.max(&1).saturating_sub(1) as u16;
        }
        for (at, height) in pps.row_height.iter().take(21).enumerate() {
            parameters.row_height_minus1[at] = height.max(&1).saturating_sub(1) as u16;
        }

        let mut listed: Vec<(usize, i32)> = Vec::new();
        for store in self.order.dpb.references().take(FRAMES) {
            // 番号を持たないピクチャーは載せない（参照ピクチャーセットからは 0xff で「無い」になる）。
            let Some(index) = pool.index(store.id) else {
                continue;
            };
            let at = listed.len();
            parameters.reference_list[at] = PicEntry::new(index, store.marked == Marked::Long);
            parameters.order_list[at] = store.poc;
            listed.push((store.id, store.poc));
        }
        for (which, into) in [0usize, 1, 2].into_iter().zip([
            &mut parameters.before,
            &mut parameters.after,
            &mut parameters.long_term,
        ]) {
            for (at, entry) in self.order.dpb.sets[which].iter().take(8).enumerate() {
                into[at] = match entry.id {
                    Some(slot) => listed
                        .iter()
                        .position(|(held, poc)| *held == slot && *poc == entry.poc)
                        .map_or(0xff, |found| found as u8),
                    // A reference never received keeps its place so later indices still match.
                    None => 0xff,
                };
            }
        }
        let used = listed
            .iter()
            .map(|(held, _)| *held)
            .filter(|held| *held != slot)
            .collect();
        (parameters, used)
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

/// The quantiser matrices in the up-right diagonal scan, not raster (clause 7.4.5; the VA-API
/// path is the opposite). The driver takes `ScalingListDC` for the DC of 16x16 and 32x32.
fn iq_matrix(matrices: &params::Scaling) -> QmatrixHevc {
    let four: [usize; 16] = diagonal(4);
    let eight: [usize; 64] = diagonal(8);
    let list = |size: usize, matrix: usize| -> [u8; 64] {
        let raster = scaling::list(matrices, size, matrix);
        std::array::from_fn(|at| raster[eight[at]])
    };
    QmatrixHevc {
        four: std::array::from_fn(|matrix| {
            let raster = scaling::list4(matrices, matrix);
            std::array::from_fn(|at| raster[four[at]])
        }),
        eight: std::array::from_fn(|matrix| list(1, matrix)),
        sixteen: std::array::from_fn(|matrix| list(2, matrix)),
        thirty_two: std::array::from_fn(|matrix| list(3, matrix * 3)),
        dc_sixteen: std::array::from_fn(|matrix| matrices.matrix(2, matrix)[0]),
        dc_thirty_two: std::array::from_fn(|matrix| matrices.matrix(3, matrix * 3)[0]),
    }
}
