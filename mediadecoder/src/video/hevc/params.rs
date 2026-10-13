// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Parameter sets and the slice segment header, clause 7.3.
//! Separate from [`crate::es::hevc`]. Anything beyond 4:2:0 at up to ten bits without
//! extensions is refused: it adds syntax below the slice header that is not read here.

use crate::bits::Reader;
use crate::video::Error;

use super::tables::{DEFAULT_SCALING, SCAN_8, scan_order};

pub const SEQUENCES: usize = 16;
pub const PICTURES: usize = 64;

/// タイルの列と行の数の上限（Table A.8 の最上位の level）。
pub const TILE_COLUMNS: usize = 20;
pub const TILE_ROWS: usize = 22;

pub const PARAMETER_BYTES: usize = 4096;

/// The parameter sets sent so far, by identifier; out-of-range identifiers are dropped.
#[derive(Debug, Clone)]
pub struct Sets {
    pub sps: Vec<Option<Sps>>,
    pub pps: Vec<Option<Pps>>,
}

impl Default for Sets {
    fn default() -> Self {
        Self {
            sps: vec![None; SEQUENCES],
            pps: vec![None; PICTURES],
        }
    }
}

impl Sets {
    /// Reads a parameter set from a NAL unit (`body` after the two header bytes); others ignored.
    pub fn absorb(&mut self, kind: u8, body: &[u8]) -> Result<(), Error> {
        match kind {
            SPS_NUT => {
                let sps = sequence(&crate::bits::unescape(body, PARAMETER_BYTES))?;
                let id = sps.id as usize;
                if id < SEQUENCES {
                    self.sps[id] = Some(sps);
                }
            }
            PPS_NUT => {
                let bytes = crate::bits::unescape(body, PARAMETER_BYTES);
                let mut reader = Reader::new(&bytes);
                reader.ue();
                let sequence = reader.ue() as usize;
                let sps = self.sps.get(sequence).cloned().flatten();
                let pps = picture(&bytes, sps.as_ref())?;
                let id = pps.id as usize;
                if id < PICTURES {
                    self.pps[id] = Some(pps);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn find(&self, pps_id: u32) -> Option<(Pps, Sps)> {
        let pps = self.pps.get(pps_id as usize).cloned().flatten()?;
        let sps = self.sps.get(pps.sps_id as usize).cloned().flatten()?;
        Some((pps, sps))
    }
}

pub const RADL_N: u8 = 6;
pub const RADL_R: u8 = 7;
pub const RASL_N: u8 = 8;
pub const RASL_R: u8 = 9;
pub const BLA_W_LP: u8 = 16;
pub const BLA_N_LP: u8 = 18;
pub const IDR_W_RADL: u8 = 19;
pub const IDR_N_LP: u8 = 20;
pub const CRA_NUT: u8 = 21;
pub const RSV_IRAP_VCL23: u8 = 23;
pub const SPS_NUT: u8 = 33;
pub const PPS_NUT: u8 = 34;
pub const EOS_NUT: u8 = 36;
pub const EOB_NUT: u8 = 37;

pub fn is_slice(kind: u8) -> bool {
    kind <= RSV_IRAP_VCL23
}

pub fn is_irap(kind: u8) -> bool {
    (BLA_W_LP..=RSV_IRAP_VCL23).contains(&kind)
}

/// Japanese MMT broadcasts send no IDR; its entry points are all CRA.
pub fn is_idr(kind: u8) -> bool {
    kind == IDR_W_RADL || kind == IDR_N_LP
}

pub fn is_bla(kind: u8) -> bool {
    (BLA_W_LP..=BLA_N_LP).contains(&kind)
}

pub fn is_cra(kind: u8) -> bool {
    kind == CRA_NUT
}

pub fn is_rasl(kind: u8) -> bool {
    kind == RASL_N || kind == RASL_R
}

pub fn is_radl(kind: u8) -> bool {
    kind == RADL_N || kind == RADL_R
}

/// Clause 7.4.2.2: odd types below the reserved range are references.
pub fn is_reference(kind: u8) -> bool {
    if kind > RSV_IRAP_VCL23 {
        return false;
    }
    kind >= BLA_W_LP || kind % 2 == 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    B = 0,
    P = 1,
    #[default]
    I = 2,
}

impl Kind {
    pub fn intra(self) -> bool {
        self == Self::I
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortTerm {
    /// POC differences to earlier pictures, in increasing distance, as positive numbers.
    pub before: Vec<i32>,
    pub before_used: Vec<bool>,
    pub after: Vec<i32>,
    pub after_used: Vec<bool>,
}

impl ShortTerm {
    fn count(&self) -> usize {
        self.before.len() + self.after.len()
    }

    /// How many pictures this one may predict from; sets the width of list modification indices.
    pub fn used(&self) -> usize {
        self.before_used.iter().filter(|&&used| used).count()
            + self.after_used.iter().filter(|&&used| used).count()
    }
}

/// `ScalingFactor[sizeId][matrixId][x][y]`, one flat run per size so a clone does not copy 30 KB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scaling {
    lists: Vec<u8>,
}

const SCALING_AT: [usize; 5] = [0, 6 * 16, 6 * 16 + 6 * 64, 6 * 16 + 6 * 64 + 6 * 256, 0];

impl Default for Scaling {
    fn default() -> Self {
        let mut scaling = Self {
            lists: vec![16; 6 * (16 + 64 + 256 + 1024)],
        };
        for size in 1..4 {
            for matrix in 0..6 {
                scaling.set_default(size, matrix);
            }
        }
        scaling
    }
}

impl Scaling {
    pub fn matrix(&self, size: usize, matrix: usize) -> &[u8] {
        let side = 4 << size;
        let at = SCALING_AT[size] + matrix * side * side;
        &self.lists[at..at + side * side]
    }

    fn matrix_mut(&mut self, size: usize, matrix: usize) -> &mut [u8] {
        let side = 4 << size;
        let at = SCALING_AT[size] + matrix * side * side;
        &mut self.lists[at..at + side * side]
    }

    fn set_default(&mut self, size: usize, matrix: usize) {
        let source: [u8; 64] = if size == 0 {
            [16; 64]
        } else {
            DEFAULT_SCALING[usize::from(matrix >= 3)]
        };
        self.expand(size, matrix, &source, source[0] as i32);
    }

    /// Equations 7-44 to 7-49; the DC term is written separately, not as a repeated corner.
    fn expand(&mut self, size: usize, matrix: usize, list: &[u8], dc: i32) {
        let side = 4usize << size;
        let count = if size == 0 { 16 } else { 64 };
        let scan: &[[u8; 2]] = if size == 0 {
            scan_order(2, 0)
        } else {
            &SCAN_8[0]
        };
        let repeat = side / if size == 0 { 4 } else { 8 };
        let into = self.matrix_mut(size, matrix);
        for i in 0..count {
            let x = scan[i][0] as usize * repeat;
            let y = scan[i][1] as usize * repeat;
            for j in 0..repeat {
                for k in 0..repeat {
                    into[(x + k) + (y + j) * side] = list[i];
                }
            }
        }
        if size > 1 {
            into[0] = dc.clamp(1, 255) as u8;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sps {
    pub id: u32,
    pub chroma_format: u32,
    pub width: usize,
    pub height: usize,
    /// The conformance window, in chroma samples (2176 rows coded, 2160 shown).
    pub crop: [u32; 4],
    pub depth: [u8; 2],
    pub log2_max_poc_lsb: u32,
    pub max_dec_pic_buffering: usize,
    pub max_num_reorder: usize,
    pub min_cb_log2: usize,
    pub ctb_log2: usize,
    pub min_tb_log2: usize,
    pub max_tb_log2: usize,
    pub max_transform_depth_inter: usize,
    pub max_transform_depth_intra: usize,
    pub scaling_list_enabled: bool,
    pub scaling: Scaling,
    pub amp_enabled: bool,
    pub sao_enabled: bool,
    pub pcm_enabled: bool,
    pub pcm_depth: [u8; 2],
    pub pcm_min_log2: usize,
    pub pcm_max_log2: usize,
    pub pcm_loop_filter_disabled: bool,
    pub short_term: Vec<ShortTerm>,
    /// Not implied by the sequence holding any: slices may name their own.
    pub long_term_present: bool,
    pub long_term_poc_lsb: Vec<u32>,
    pub long_term_used: Vec<bool>,
    pub temporal_mvp_enabled: bool,
    pub strong_intra_smoothing: bool,
}

impl Sps {
    pub fn ctb_width(&self) -> usize {
        self.width.div_ceil(1 << self.ctb_log2)
    }

    pub fn ctb_height(&self) -> usize {
        self.height.div_ceil(1 << self.ctb_log2)
    }

    pub fn ctbs(&self) -> usize {
        self.ctb_width() * self.ctb_height()
    }

    pub fn qp_offset(&self, chroma: bool) -> i32 {
        6 * (i32::from(self.depth[usize::from(chroma)]) - 8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub dependent_slice_segments_enabled: bool,
    pub output_flag_present: bool,
    pub num_extra_slice_header_bits: usize,
    pub sign_data_hiding: bool,
    pub cabac_init_present: bool,
    pub num_ref_idx_default: [usize; 2],
    pub init_qp: i32,
    pub constrained_intra_pred: bool,
    pub transform_skip_enabled: bool,
    pub cu_qp_delta_enabled: bool,
    pub diff_cu_qp_delta_depth: usize,
    pub qp_offset: [i32; 2],
    pub slice_chroma_qp_offsets_present: bool,
    pub weighted_pred: bool,
    pub weighted_bipred: bool,
    pub transquant_bypass_enabled: bool,
    pub tiles_enabled: bool,
    pub entropy_coding_sync_enabled: bool,
    /// Tile column bounds in CTBs; empty where the picture is one tile.
    pub column_width: Vec<usize>,
    pub row_height: Vec<usize>,
    pub loop_filter_across_tiles: bool,
    pub loop_filter_across_slices: bool,
    pub deblocking_override_enabled: bool,
    pub deblocking_disabled: bool,
    pub beta_offset: i32,
    pub tc_offset: i32,
    pub scaling_list_present: bool,
    pub scaling: Option<Scaling>,
    pub lists_modification_present: bool,
    pub log2_parallel_merge_level: usize,
    pub slice_header_extension_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weight {
    pub used: bool,
    pub weight: i32,
    pub offset: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Header {
    pub first_segment: bool,
    pub no_output_of_prior_pics: bool,
    pub pps_id: u32,
    pub dependent: bool,
    pub segment_address: usize,
    pub kind: Kind,
    pub pic_output_flag: bool,
    pub poc_lsb: u32,
    pub short_term: ShortTerm,
    pub long_term: Vec<LongTerm>,
    pub temporal_mvp_enabled: bool,
    pub sao_luma: bool,
    pub sao_chroma: bool,
    pub num_ref_idx: [usize; 2],
    /// Which initial list entry each final entry takes, where the slice reordered them.
    pub list_entry: [Vec<usize>; 2],
    pub modified: [bool; 2],
    pub mvd_l1_zero: bool,
    pub cabac_init_flag: bool,
    pub collocated_from_l0: bool,
    pub collocated_ref_idx: usize,
    pub luma_log2_denom: i32,
    pub chroma_log2_denom: i32,
    pub weights: [[[Weight; 3]; 16]; 2],
    pub max_merge_cand: usize,
    pub qp: i32,
    pub slice_qp_offset: [i32; 2],
    pub deblocking_disabled: bool,
    pub beta_offset: i32,
    pub tc_offset: i32,
    pub loop_filter_across_slices: bool,
    pub entry_points: Vec<usize>,
    /// Where the arithmetic coded data begins, in bytes into the unescaped payload.
    pub data_at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LongTerm {
    pub poc_lsb: u32,
    pub used: bool,
    /// Where the MSBs were sent, the reference is found by the whole POC.
    pub msb: Option<i32>,
}

fn profile_tier_level(reader: &mut Reader, sub_layers: usize) {
    reader.skip(2 + 1 + 5 + 32 + 4 + 43 + 1);
    reader.skip(8);
    let mut profile = [false; 8];
    let mut level = [false; 8];
    for i in 0..sub_layers.min(8) {
        profile[i] = reader.bit() == 1;
        level[i] = reader.bit() == 1;
    }
    if sub_layers > 0 {
        reader.skip(2 * (8 - sub_layers.min(8)));
    }
    for i in 0..sub_layers.min(8) {
        if profile[i] {
            reader.skip(2 + 1 + 5 + 32 + 4 + 43 + 1);
        }
        if level[i] {
            reader.skip(8);
        }
    }
}

fn scaling_list_data(reader: &mut Reader) -> Scaling {
    let mut scaling = Scaling::default();
    for size in 0..4usize {
        let mut matrix = 0usize;
        while matrix < 6 {
            if reader.bit() == 0 {
                // Not written out: the default, or a copy of a matrix already read.
                let delta = reader.ue() as usize;
                if delta == 0 {
                    scaling.set_default(size, matrix);
                } else {
                    let step = if size == 3 { 3 } else { 1 };
                    let from = matrix.saturating_sub(delta * step);
                    let copy = scaling.matrix(size, from).to_vec();
                    scaling.matrix_mut(size, matrix).copy_from_slice(&copy);
                }
            } else {
                let count = 64.min(1 << (4 + (size << 1)));
                let mut next = 8i32;
                let mut dc = 8i32;
                if size > 1 {
                    dc = reader.se() + 8;
                    next = dc;
                }
                let mut list = [16u8; 64];
                for entry in list.iter_mut().take(count) {
                    next = (next + reader.se() + 256).rem_euclid(256);
                    *entry = next as u8;
                }
                scaling.expand(size, matrix, &list, dc);
            }
            matrix += if size == 3 { 3 } else { 1 };
        }
    }
    // 32x32 chroma never arises at 4:2:0, but the default keeps every index in range.
    for matrix in [1, 2, 4, 5] {
        let copy = scaling.matrix(3, if matrix < 3 { 0 } else { 3 }).to_vec();
        scaling.matrix_mut(3, matrix).copy_from_slice(&copy);
    }
    scaling
}

fn short_term_set(
    reader: &mut Reader,
    index: usize,
    sets: &[ShortTerm],
    count: usize,
) -> Result<ShortTerm, Error> {
    // delta_poc_s*_minus1 と abs_delta_rps_minus1 は 0〜2^15-1。超える値を足すと i32 があふれる。
    let delta = |reader: &mut Reader| {
        let value = reader.ue();
        if value >= 1 << 15 {
            return Err(Error::Damaged(
                "a reference picture set with too large a step",
            ));
        }
        Ok(value as i32 + 1)
    };
    let predicted = index != 0 && reader.bit() == 1;
    if !predicted {
        let negative = reader.ue().min(16) as usize;
        let positive = reader.ue().min(16) as usize;
        let mut set = ShortTerm::default();
        let mut poc = 0i32;
        for _ in 0..negative {
            poc += delta(reader)?;
            set.before.push(poc);
            set.before_used.push(reader.bit() == 1);
        }
        poc = 0;
        for _ in 0..positive {
            poc += delta(reader)?;
            set.after.push(poc);
            set.after_used.push(reader.bit() == 1);
        }
        return Ok(set);
    }

    let step = if index == count {
        (reader.ue() as usize).saturating_add(1)
    } else {
        1
    };
    let sign = reader.bit();
    let magnitude = delta(reader)?;
    let delta_rps = if sign == 1 { -magnitude } else { magnitude };
    let Some(source) = sets.get(index.wrapping_sub(step)) else {
        return Ok(ShortTerm::default());
    };
    let entries = source.count();
    let mut used = vec![false; entries + 1];
    let mut use_delta = vec![true; entries + 1];
    for j in 0..=entries {
        used[j] = reader.bit() == 1;
        if !used[j] {
            use_delta[j] = reader.bit() == 1;
        }
    }

    let mut set = ShortTerm::default();
    // Kept positive, so an earlier entry's signed difference is minus the stored one.
    let negatives = source.before.len();
    for j in (0..source.after.len()).rev() {
        let poc = source.after[j] + delta_rps;
        if poc < 0 && use_delta[negatives + j] {
            set.before.push(-poc);
            set.before_used.push(used[negatives + j]);
        }
    }
    if delta_rps < 0 && use_delta[entries] {
        set.before.push(-delta_rps);
        set.before_used.push(used[entries]);
    }
    for j in 0..negatives {
        let poc = -source.before[j] + delta_rps;
        if poc < 0 && use_delta[j] {
            set.before.push(-poc);
            set.before_used.push(used[j]);
        }
    }
    for j in (0..negatives).rev() {
        let poc = -source.before[j] + delta_rps;
        if poc > 0 && use_delta[j] {
            set.after.push(poc);
            set.after_used.push(used[j]);
        }
    }
    if delta_rps > 0 && use_delta[entries] {
        set.after.push(delta_rps);
        set.after_used.push(used[entries]);
    }
    for j in 0..source.after.len() {
        let poc = source.after[j] + delta_rps;
        if poc > 0 && use_delta[negatives + j] {
            set.after.push(poc);
            set.after_used.push(used[negatives + j]);
        }
    }
    Ok(set)
}

pub fn sequence(payload: &[u8]) -> Result<Sps, Error> {
    let mut reader = Reader::new(payload);
    reader.skip(4 + 3 + 1);
    let sub_layers = ((payload.first().copied().unwrap_or(0) >> 1) & 7) as usize;
    profile_tier_level(&mut reader, sub_layers);
    let id = reader.ue();
    if id as usize >= SEQUENCES {
        return Err(Error::Damaged("a sequence parameter set with no number"));
    }
    let chroma_format = reader.ue();
    if chroma_format == 3 && reader.bit() == 1 {
        return Err(Error::Unsupported("separately coded colour planes"));
    }
    if chroma_format != 1 {
        return Err(Error::Unsupported("a chroma format other than 4:2:0"));
    }
    let (width, height) = (reader.ue(), reader.ue());
    // 上限を超えるサイズは確保の前に拒否する（[`crate::video::fits`]）。
    if !crate::video::fits(width.into(), height.into()) {
        return Err(Error::Unsupported("a picture larger than any level allows"));
    }
    let (width, height) = (width as usize, height as usize);
    let mut crop = [0u32; 4];
    if reader.bit() == 1 {
        for offset in &mut crop {
            *offset = reader.ue();
        }
    }
    // 4:2:0 のクロップは色差の画素（2 画素）で数える。ピクチャーより大きいものは拒否する。
    let [left, right, top, bottom] = crop.map(u64::from);
    if (left + right) * 2 >= width as u64 || (top + bottom) * 2 >= height as u64 {
        return Err(Error::Damaged(
            "a conformance window larger than the picture",
        ));
    }
    // 大きな値を u8 に入れると桁あふれして検査を通ってしまうので、入れる前に確認する。
    let (luma, chroma) = (reader.ue(), reader.ue());
    if luma > 2 || chroma > 2 || luma != chroma {
        return Err(Error::Unsupported("a sample deeper than ten bits"));
    }
    let depth = [luma as u8 + 8, chroma as u8 + 8];
    let lsb = reader.ue();
    if lsb > 12 {
        return Err(Error::Damaged(
            "a picture order count wider than sixteen bits",
        ));
    }
    let log2_max_poc_lsb = lsb + 4;
    let ordering = reader.bit() == 1;
    let mut max_dec_pic_buffering = 1;
    let mut max_num_reorder = 0;
    let first = if ordering { 0 } else { sub_layers };
    for i in first..=sub_layers {
        let buffering = reader.ue() as usize + 1;
        let reorder = reader.ue() as usize;
        reader.ue();
        // Only the highest sub-layer's numbers are used: every temporal layer is decoded.
        if i == sub_layers {
            max_dec_pic_buffering = buffering.clamp(1, 16);
            max_num_reorder = reorder.min(16);
        }
    }
    // 足し算があふれないよう、どれも小さく抑えてから足す（範囲外は下の検査で拒否する）。
    let small = |value: u32| value.min(64) as usize;
    let min_cb_log2 = small(reader.ue()) + 3;
    let ctb_log2 = min_cb_log2 + small(reader.ue());
    let min_tb_log2 = small(reader.ue()) + 2;
    let max_tb_log2 = min_tb_log2 + small(reader.ue());
    if !(4..=6).contains(&ctb_log2)
        || min_cb_log2 > ctb_log2
        || max_tb_log2 > 5
        || min_tb_log2 >= min_cb_log2
        || max_tb_log2 > ctb_log2
    {
        return Err(Error::Damaged("a sequence with impossible block sizes"));
    }
    if width == 0 || height == 0 || !width.is_multiple_of(1 << min_cb_log2) {
        return Err(Error::Damaged("a sequence with a size that is not whole"));
    }
    let max_transform_depth_inter = reader.ue() as usize;
    let max_transform_depth_intra = reader.ue() as usize;
    if max_transform_depth_inter > ctb_log2 - min_tb_log2
        || max_transform_depth_intra > ctb_log2 - min_tb_log2
    {
        return Err(Error::Damaged(
            "a transform tree deeper than the blocks allow",
        ));
    }
    let scaling_list_enabled = reader.bit() == 1;
    let mut scaling = Scaling::default();
    if scaling_list_enabled && reader.bit() == 1 {
        scaling = scaling_list_data(&mut reader);
    }
    let amp_enabled = reader.bit() == 1;
    let sao_enabled = reader.bit() == 1;
    let pcm_enabled = reader.bit() == 1;
    let mut pcm_depth = [8u8; 2];
    let mut pcm_min_log2 = 3;
    let mut pcm_max_log2 = 3;
    let mut pcm_loop_filter_disabled = false;
    if pcm_enabled {
        pcm_depth = [reader.bits(4) as u8 + 1, reader.bits(4) as u8 + 1];
        pcm_min_log2 = small(reader.ue()) + 3;
        pcm_max_log2 = pcm_min_log2 + small(reader.ue());
        pcm_loop_filter_disabled = reader.bit() == 1;
        // PCM のビット深度はピクチャーのビット深度まで、サイズは 8〜32 で CTB を超えない（7.4.3.2.1）。
        if pcm_depth[0] > depth[0]
            || pcm_depth[1] > depth[1]
            || pcm_min_log2 < min_cb_log2.min(5)
            || pcm_max_log2 > ctb_log2.min(5)
        {
            return Err(Error::Damaged(
                "a PCM block outside what the sequence allows",
            ));
        }
    }
    let count = reader.ue().min(64) as usize;
    let mut short_term: Vec<ShortTerm> = Vec::with_capacity(count);
    for i in 0..count {
        let set = short_term_set(&mut reader, i, &short_term, count)?;
        short_term.push(set);
    }
    let mut long_term_poc_lsb = Vec::new();
    let mut long_term_used = Vec::new();
    let long_term_present = reader.bit() == 1;
    if long_term_present {
        let count = reader.ue().min(32) as usize;
        for _ in 0..count {
            long_term_poc_lsb.push(reader.bits(log2_max_poc_lsb as usize));
            long_term_used.push(reader.bit() == 1);
        }
    }
    let temporal_mvp_enabled = reader.bit() == 1;
    let strong_intra_smoothing = reader.bit() == 1;
    // VUI is skipped and reading stops here; the extensions after it are refused by their effect.
    if reader.left() == 0 {
        return Err(Error::Damaged("a sequence parameter set cut short"));
    }
    Ok(Sps {
        id,
        chroma_format,
        width,
        height,
        crop,
        depth,
        log2_max_poc_lsb,
        max_dec_pic_buffering,
        max_num_reorder,
        min_cb_log2,
        ctb_log2,
        min_tb_log2,
        max_tb_log2,
        max_transform_depth_inter,
        max_transform_depth_intra,
        scaling_list_enabled,
        scaling,
        amp_enabled,
        sao_enabled,
        pcm_enabled,
        pcm_depth,
        pcm_min_log2,
        pcm_max_log2,
        pcm_loop_filter_disabled,
        short_term,
        long_term_present,
        long_term_poc_lsb,
        long_term_used,
        temporal_mvp_enabled,
        strong_intra_smoothing,
    })
}

pub fn picture(payload: &[u8], sps: Option<&Sps>) -> Result<Pps, Error> {
    let mut reader = Reader::new(payload);
    let id = reader.ue();
    let sps_id = reader.ue();
    if id as usize >= PICTURES || sps_id as usize >= SEQUENCES {
        return Err(Error::Damaged("a picture parameter set with no number"));
    }
    let dependent_slice_segments_enabled = reader.bit() == 1;
    let output_flag_present = reader.bit() == 1;
    let num_extra_slice_header_bits = reader.bits(3) as usize;
    let sign_data_hiding = reader.bit() == 1;
    let cabac_init_present = reader.bit() == 1;
    let num_ref_idx_default = [reader.ue() as usize + 1, reader.ue() as usize + 1];
    if num_ref_idx_default.iter().any(|count| *count > 15) {
        return Err(Error::Damaged("a reference list longer than fifteen"));
    }
    // init_qp_minus26 は -(26 + QpBdOffsetY)〜25。深さは 10 ビットまでなので -38 まで。
    let init_qp = reader.se();
    if !(-38..=25).contains(&init_qp) {
        return Err(Error::Damaged("a picture parameter set with no usable QP"));
    }
    let init_qp = init_qp + 26;
    let constrained_intra_pred = reader.bit() == 1;
    let transform_skip_enabled = reader.bit() == 1;
    let cu_qp_delta_enabled = reader.bit() == 1;
    let diff_cu_qp_delta_depth = if cu_qp_delta_enabled {
        reader.ue() as usize
    } else {
        0
    };
    let qp_offset = [reader.se(), reader.se()];
    if qp_offset.iter().any(|offset| !(-12..=12).contains(offset)) {
        return Err(Error::Damaged("a chroma QP offset outside -12 to 12"));
    }
    if let Some(sps) = sps
        && diff_cu_qp_delta_depth > sps.ctb_log2 - sps.min_cb_log2
    {
        return Err(Error::Damaged("a QP group smaller than a coding block"));
    }
    let slice_chroma_qp_offsets_present = reader.bit() == 1;
    let weighted_pred = reader.bit() == 1;
    let weighted_bipred = reader.bit() == 1;
    let transquant_bypass_enabled = reader.bit() == 1;
    let tiles_enabled = reader.bit() == 1;
    let entropy_coding_sync_enabled = reader.bit() == 1;
    let mut column_width = Vec::new();
    let mut row_height = Vec::new();
    let mut loop_filter_across_tiles = true;
    if tiles_enabled {
        let columns = reader.ue() as usize + 1;
        let rows = reader.ue() as usize + 1;
        // 最上位の level でも 20 列・22 行まで（Table A.8）。ドライバーへ渡すテーブルもこのサイズ。
        if columns > TILE_COLUMNS || rows > TILE_ROWS {
            return Err(Error::Unsupported("more tiles than any level allows"));
        }
        let uniform = reader.bit() == 1;
        let (ctb_width, ctb_height) = match sps {
            Some(sps) => (sps.ctb_width(), sps.ctb_height()),
            None => return Err(Error::Damaged("tiles with no sequence to size them")),
        };
        if columns > ctb_width.max(1) || rows > ctb_height.max(1) {
            return Err(Error::Damaged("more tiles than coding tree blocks"));
        }
        if uniform {
            // Equation 6-3: not the same as giving the remainder to the last column.
            for i in 0..columns {
                column_width.push((i + 1) * ctb_width / columns - i * ctb_width / columns);
            }
            for i in 0..rows {
                row_height.push((i + 1) * ctb_height / rows - i * ctb_height / rows);
            }
        } else {
            let mut left = ctb_width;
            for _ in 0..columns - 1 {
                let width = reader.ue() as usize + 1;
                column_width.push(width);
                left = left.saturating_sub(width);
            }
            column_width.push(left);
            let mut left = ctb_height;
            for _ in 0..rows - 1 {
                let height = reader.ue() as usize + 1;
                row_height.push(height);
                left = left.saturating_sub(height);
            }
            row_height.push(left);
        }
        loop_filter_across_tiles = reader.bit() == 1;
    }
    let loop_filter_across_slices = reader.bit() == 1;
    let mut deblocking_override_enabled = false;
    let mut deblocking_disabled = false;
    let mut beta_offset = 0;
    let mut tc_offset = 0;
    if reader.bit() == 1 {
        deblocking_override_enabled = reader.bit() == 1;
        deblocking_disabled = reader.bit() == 1;
        if !deblocking_disabled {
            beta_offset = reader.se().saturating_mul(2);
            tc_offset = reader.se().saturating_mul(2);
            if !(-12..=12).contains(&beta_offset) || !(-12..=12).contains(&tc_offset) {
                return Err(Error::Damaged("a deblocking offset outside -6 to 6"));
            }
        }
    }
    let scaling_list_present = reader.bit() == 1;
    let scaling = scaling_list_present.then(|| scaling_list_data(&mut reader));
    let lists_modification_present = reader.bit() == 1;
    let log2_parallel_merge_level = (reader.ue() as usize).saturating_add(2);
    if sps.is_some_and(|sps| log2_parallel_merge_level > sps.ctb_log2) {
        return Err(Error::Damaged(
            "a merge level larger than a coding tree block",
        ));
    }
    let slice_header_extension_present = reader.bit() == 1;
    if reader.left() == 0 {
        return Err(Error::Damaged("a picture parameter set cut short"));
    }
    if reader.bit() == 1 {
        let range = reader.bit() == 1;
        let multilayer = reader.bit() == 1;
        let three_d = reader.bit() == 1;
        let scc = reader.bit() == 1;
        if range || multilayer || three_d || scc {
            return Err(Error::Unsupported("a picture parameter set extension"));
        }
    }
    Ok(Pps {
        id,
        sps_id,
        dependent_slice_segments_enabled,
        output_flag_present,
        num_extra_slice_header_bits,
        sign_data_hiding,
        cabac_init_present,
        num_ref_idx_default,
        init_qp,
        constrained_intra_pred,
        transform_skip_enabled,
        cu_qp_delta_enabled,
        diff_cu_qp_delta_depth,
        qp_offset,
        slice_chroma_qp_offsets_present,
        weighted_pred,
        weighted_bipred,
        transquant_bypass_enabled,
        tiles_enabled,
        entropy_coding_sync_enabled,
        column_width,
        row_height,
        loop_filter_across_tiles,
        loop_filter_across_slices,
        deblocking_override_enabled,
        deblocking_disabled,
        beta_offset,
        tc_offset,
        scaling_list_present,
        scaling,
        lists_modification_present,
        log2_parallel_merge_level,
        slice_header_extension_present,
    })
}

fn width_of(most: usize) -> usize {
    let mut bits = 0;
    while (1usize << bits) < most {
        bits += 1;
    }
    bits
}

/// Reads a slice segment header; a dependent segment inherits all but its address.
pub fn header(
    payload: &[u8],
    kind: u8,
    sps: &Sps,
    pps: &Pps,
    previous: Option<&Header>,
) -> Result<Header, Error> {
    let mut reader = Reader::new(payload);
    let mut header = Header {
        first_segment: reader.bit() == 1,
        pic_output_flag: true,
        max_merge_cand: 5,
        ..Header::default()
    };
    if is_irap(kind) {
        header.no_output_of_prior_pics = reader.bit() == 1;
    }
    header.pps_id = reader.ue();
    if !header.first_segment {
        if pps.dependent_slice_segments_enabled {
            header.dependent = reader.bit() == 1;
        }
        header.segment_address = reader.bits(width_of(sps.ctbs())) as usize;
        if header.segment_address >= sps.ctbs() {
            return Err(Error::Damaged("a slice segment outside the picture"));
        }
    }

    if header.dependent {
        let Some(previous) = previous else {
            return Err(Error::Damaged("a dependent slice segment with no slice"));
        };
        let address = header.segment_address;
        let prior = header.no_output_of_prior_pics;
        header = previous.clone();
        header.segment_address = address;
        header.first_segment = false;
        header.no_output_of_prior_pics = prior;
        header.dependent = true;
    } else {
        reader.skip(pps.num_extra_slice_header_bits);
        header.kind = match reader.ue() {
            0 => Kind::B,
            1 => Kind::P,
            2 => Kind::I,
            _ => return Err(Error::Damaged("a slice of no known kind")),
        };
        if pps.output_flag_present {
            header.pic_output_flag = reader.bit() == 1;
        }
        if !is_idr(kind) {
            header.poc_lsb = reader.bits(sps.log2_max_poc_lsb as usize);
            if reader.bit() == 1 {
                let index = if sps.short_term.len() > 1 {
                    reader.bits(width_of(sps.short_term.len())) as usize
                } else {
                    0
                };
                header.short_term = sps
                    .short_term
                    .get(index)
                    .cloned()
                    .ok_or(Error::Damaged("a short-term set the sequence never sent"))?;
            } else {
                header.short_term = short_term_set(
                    &mut reader,
                    sps.short_term.len(),
                    &sps.short_term,
                    sps.short_term.len(),
                )?;
            }
            if sps.long_term_present {
                header.long_term = read_long_term(&mut reader, sps);
            }
            if sps.temporal_mvp_enabled {
                header.temporal_mvp_enabled = reader.bit() == 1;
            }
        }
        if sps.sao_enabled {
            header.sao_luma = reader.bit() == 1;
            header.sao_chroma = reader.bit() == 1;
        }
        header.num_ref_idx = pps.num_ref_idx_default;
        if header.kind != Kind::I {
            if reader.bit() == 1 {
                header.num_ref_idx[0] = reader.ue() as usize + 1;
                if header.kind == Kind::B {
                    header.num_ref_idx[1] = reader.ue() as usize + 1;
                }
            }
            if header.num_ref_idx[0] > 15 || header.num_ref_idx[1] > 15 {
                return Err(Error::Damaged("a reference list longer than fifteen"));
            }
            let total =
                header.short_term.used() + header.long_term.iter().filter(|l| l.used).count();
            if pps.lists_modification_present && total > 1 {
                let bits = width_of(total);
                for list in 0..if header.kind == Kind::B { 2 } else { 1 } {
                    header.modified[list] = reader.bit() == 1;
                    if header.modified[list] {
                        for _ in 0..header.num_ref_idx[list] {
                            header.list_entry[list].push(reader.bits(bits) as usize);
                        }
                    }
                }
            }
            if header.kind == Kind::B {
                header.mvd_l1_zero = reader.bit() == 1;
            }
            if pps.cabac_init_present {
                header.cabac_init_flag = reader.bit() == 1;
            }
            header.collocated_from_l0 = true;
            if header.temporal_mvp_enabled {
                if header.kind == Kind::B {
                    header.collocated_from_l0 = reader.bit() == 1;
                }
                let list = usize::from(!header.collocated_from_l0);
                if header.num_ref_idx[list] > 1 {
                    header.collocated_ref_idx = reader.ue() as usize;
                    if header.collocated_ref_idx >= header.num_ref_idx[list] {
                        return Err(Error::Damaged("a collocated picture outside its list"));
                    }
                }
            }
            if (pps.weighted_pred && header.kind == Kind::P)
                || (pps.weighted_bipred && header.kind == Kind::B)
            {
                read_weights(&mut reader, sps, &mut header)?;
            }
            let five_minus = reader.ue() as usize;
            if five_minus > 4 {
                return Err(Error::Damaged("more merge candidates than there are"));
            }
            header.max_merge_cand = 5 - five_minus;
        }
        header.qp = pps.init_qp.saturating_add(reader.se());
        if !(-12..=51).contains(&header.qp) {
            return Err(Error::Damaged("a slice with no usable QP"));
        }
        if pps.slice_chroma_qp_offsets_present {
            header.slice_qp_offset = [reader.se(), reader.se()];
            if header
                .slice_qp_offset
                .iter()
                .any(|offset| !(-12..=12).contains(offset))
            {
                return Err(Error::Damaged("a chroma QP offset outside -12 to 12"));
            }
        }
        header.deblocking_disabled = pps.deblocking_disabled;
        header.beta_offset = pps.beta_offset;
        header.tc_offset = pps.tc_offset;
        let mut override_flag = false;
        if pps.deblocking_override_enabled {
            override_flag = reader.bit() == 1;
        }
        if override_flag {
            header.deblocking_disabled = reader.bit() == 1;
            if !header.deblocking_disabled {
                header.beta_offset = reader.se().saturating_mul(2);
                header.tc_offset = reader.se().saturating_mul(2);
                if !(-12..=12).contains(&header.beta_offset)
                    || !(-12..=12).contains(&header.tc_offset)
                {
                    return Err(Error::Damaged("a deblocking offset outside -6 to 6"));
                }
            }
        }
        header.loop_filter_across_slices = pps.loop_filter_across_slices;
        if pps.loop_filter_across_slices
            && (header.sao_luma || header.sao_chroma || !header.deblocking_disabled)
        {
            header.loop_filter_across_slices = reader.bit() == 1;
        }
    }

    header.entry_points.clear();
    if pps.tiles_enabled || pps.entropy_coding_sync_enabled {
        let count = reader.ue() as usize;
        if count > 0 {
            if count > sps.ctbs() {
                return Err(Error::Damaged("more entry points than coding tree blocks"));
            }
            let bits = reader.ue() as usize + 1;
            if bits > 32 {
                return Err(Error::Damaged("an entry point offset too wide to read"));
            }
            let mut at = 0usize;
            for _ in 0..count {
                at += reader.bits(bits) as usize + 1;
                header.entry_points.push(at);
            }
        }
    }
    if pps.slice_header_extension_present {
        let length = reader.ue() as usize;
        reader.skip(8 * length);
    }
    reader.skip(1);
    reader.align();
    header.data_at = reader.at() / 8;
    if header.data_at >= payload.len() {
        return Err(Error::Damaged("a slice segment with no data"));
    }
    Ok(header)
}

fn read_long_term(reader: &mut Reader, sps: &Sps) -> Vec<LongTerm> {
    let from_sps = if sps.long_term_poc_lsb.is_empty() {
        0
    } else {
        reader.ue() as usize
    };
    let own = reader.ue() as usize;
    let mut out = Vec::new();
    let mut previous_msb = 0i32;
    for i in 0..(from_sps + own).min(32) {
        let mut entry = LongTerm::default();
        if i < from_sps {
            let index = if sps.long_term_poc_lsb.len() > 1 {
                reader.bits(width_of(sps.long_term_poc_lsb.len())) as usize
            } else {
                0
            };
            entry.poc_lsb = sps.long_term_poc_lsb.get(index).copied().unwrap_or(0);
            entry.used = sps.long_term_used.get(index).copied().unwrap_or(false);
        } else {
            entry.poc_lsb = reader.bits(sps.log2_max_poc_lsb as usize);
            entry.used = reader.bit() == 1;
        }
        if reader.bit() == 1 {
            // Equation 7-52: the cycles are cumulative within each run.
            let cycle = reader.ue() as i32;
            if i == 0 || i == from_sps {
                previous_msb = cycle;
            } else {
                previous_msb += cycle;
            }
            entry.msb = Some(previous_msb);
        }
        out.push(entry);
    }
    out
}

fn read_weights(reader: &mut Reader, sps: &Sps, header: &mut Header) -> Result<(), Error> {
    // 分母は 0〜7、重みの差と足す値は -128〜127（7.4.7.3）。範囲外の値はシフトと掛け算をあふれさせる。
    let byte = |value: i32| {
        (-128..=127)
            .contains(&value)
            .then_some(value)
            .ok_or(Error::Damaged("a prediction weight outside -128 to 127"))
    };
    let luma = reader.ue();
    let chroma = i64::from(luma) + i64::from(reader.se());
    if luma > 7 || !(0..=7).contains(&chroma) {
        return Err(Error::Damaged(
            "a prediction weight denominator above seven",
        ));
    }
    header.luma_log2_denom = luma as i32;
    header.chroma_log2_denom = chroma as i32;
    let lists = if header.kind == Kind::B { 2 } else { 1 };
    let range = 1i32 << (sps.depth[0] - 8);
    for list in 0..lists {
        let count = header.num_ref_idx[list].min(16);
        let mut luma = [false; 16];
        let mut chroma = [false; 16];
        for entry in luma.iter_mut().take(count) {
            *entry = reader.bit() == 1;
        }
        for entry in chroma.iter_mut().take(count) {
            *entry = reader.bit() == 1;
        }
        for i in 0..count {
            let weights = &mut header.weights[list][i];
            weights[0] = Weight {
                used: luma[i],
                weight: 1 << header.luma_log2_denom,
                offset: 0,
            };
            if luma[i] {
                weights[0].weight = (1 << header.luma_log2_denom) + byte(reader.se())?;
                weights[0].offset = byte(reader.se())? * range;
            }
            for slot in weights.iter_mut().skip(1) {
                *slot = Weight {
                    used: chroma[i],
                    weight: 1 << header.chroma_log2_denom,
                    offset: 0,
                };
                if chroma[i] {
                    let delta_weight = byte(reader.se())?;
                    let delta_offset = reader.se().clamp(-512, 511);
                    let weight = (1 << header.chroma_log2_denom) + delta_weight;
                    // Equation 7-56: the chroma offset is against the middle of the range.
                    let half = 1i32 << (sps.depth[1] - 1);
                    let offset = (half + delta_offset
                        - ((half * weight) >> header.chroma_log2_denom))
                        .clamp(-128, 127);
                    slot.weight = weight;
                    slot.offset = offset * range;
                }
            }
        }
    }
    Ok(())
}
