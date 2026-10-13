// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The SPS, PPS and slice header, read for a decoder.

use crate::bits::Reader;

pub const SEQUENCES: usize = 32;
pub const PICTURES: usize = 256;

pub const REFS: usize = 32;

pub const PARAMETER_BYTES: usize = 4096;

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
    /// `kind` 7 or 8. A set that does not read is ignored: the stream repeats its sets.
    pub fn absorb(&mut self, kind: u8, body: &[u8]) {
        match kind {
            7 => {
                if let Some(sps) = sequence(&crate::bits::unescape(body, PARAMETER_BYTES))
                    && (sps.id as usize) < SEQUENCES
                {
                    self.sps[sps.id as usize] = Some(sps);
                }
            }
            8 => {
                // A PPS is read against its SPS: the number of scaling lists depends on its chroma format.
                let bytes = crate::bits::unescape(body, PARAMETER_BYTES);
                let mut reader = Reader::new(&bytes);
                reader.ue();
                let sequence = reader.ue() as usize;
                let sps = self.sps.get(sequence).copied().flatten();
                if let Some(pps) = picture(&bytes, sps.as_ref())
                    && (pps.id as usize) < PICTURES
                {
                    self.pps[pps.id as usize] = Some(pps);
                }
            }
            _ => {}
        }
    }

    pub fn find(&self, pps_id: u32) -> Option<(Pps, Sps)> {
        let pps = self.pps.get(pps_id as usize).copied().flatten()?;
        let sps = self.sps.get(pps.sps_id as usize).copied().flatten()?;
        Some((pps, sps))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    P,
    B,
    I,
    /// Switching slices; differ from P and I only in a transform no broadcast uses.
    Sp,
    Si,
}

impl Kind {
    fn of(slice_type: u32) -> Option<Self> {
        match slice_type % 5 {
            0 => Some(Self::P),
            1 => Some(Self::B),
            2 => Some(Self::I),
            3 => Some(Self::Sp),
            4 => Some(Self::Si),
            _ => None,
        }
    }

    pub fn intra(self) -> bool {
        matches!(self, Self::I | Self::Si)
    }

    pub fn lists(self) -> usize {
        match self {
            Self::B => 2,
            Self::I | Self::Si => 0,
            _ => 1,
        }
    }
}

/// A scaling matrix as sent, in zig-zag order: fallbacks apply before the scan.
#[derive(Debug, Clone, Copy)]
pub struct Scaling {
    pub present: bool,
    pub sent: [bool; 12],
    pub default: [bool; 12],
    pub lists: [[u8; 64]; 12],
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            present: false,
            sent: [false; 12],
            default: [false; 12],
            lists: [[16; 64]; 12],
        }
    }
}

impl Scaling {
    fn read(reader: &mut Reader<'_>, chroma: u32, eights: usize) -> Self {
        let mut scaling = Self {
            present: true,
            ..Self::default()
        };
        let count = 6 + eights * if chroma == 3 { 6 } else { 2 };
        for list in 0..count.min(12) {
            let (index, size) = if list < 6 {
                (list, 16)
            } else {
                (6 + (list - 6), 64)
            };
            if reader.bit() == 0 {
                continue;
            }
            scaling.sent[index] = true;
            let (mut last, mut next) = (8i32, 8i32);
            for at in 0..size {
                if next != 0 {
                    next = (last + reader.se()).rem_euclid(256);
                    // A first delta of nought means the standard's list; elsewhere, repeat the rest.
                    if at == 0 && next == 0 {
                        scaling.default[index] = true;
                    }
                }
                if next != 0 {
                    last = next;
                }
                scaling.lists[index][at] = last as u8;
            }
        }
        scaling
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub four: [[u8; 16]; 6],
    pub eight: [[u8; 64]; 6],
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            four: [[16; 16]; 6],
            eight: [[16; 64]; 6],
        }
    }
}

/// Clauses 7.4.2.1.1 and 7.4.2.2 fallbacks, then the inverse scan.
/// An unsent PPS list falls back to the SPS only if it sent a matrix; otherwise to the
/// standard's lists, not flat.
pub fn weights(sps: &Sps, pps: &Pps) -> Weights {
    use super::tables::{DEFAULT_4X4, DEFAULT_8X8, ZIGZAG_4X4, ZIGZAG_8X8};

    let mut lists = [[16u8; 64]; 12];
    for (from, rule_b) in [(&sps.scaling, false), (&pps.scaling, sps.scaling.present)] {
        if !from.present {
            continue;
        }
        for index in 0..12 {
            let size = if index < 6 { 16 } else { 64 };
            let default: &[u8] = if index < 6 {
                &DEFAULT_4X4[usize::from(index >= 3)]
            } else {
                &DEFAULT_8X8[index % 2]
            };
            if from.sent[index] && !from.default[index] {
                lists[index] = from.lists[index];
                continue;
            }
            let fallback: [u8; 64] = if from.default[index] {
                let mut out = [16u8; 64];
                out[..size].copy_from_slice(default);
                out
            } else if index == 0 || index == 3 || index == 6 || index == 7 {
                if rule_b {
                    lists[index]
                } else {
                    let mut out = [16u8; 64];
                    out[..size].copy_from_slice(default);
                    out
                }
            } else if index < 6 {
                lists[index - 1]
            } else {
                lists[index - 2]
            };
            lists[index] = fallback;
        }
    }

    let mut out = Weights::default();
    for list in 0..6 {
        for at in 0..16 {
            out.four[list][usize::from(ZIGZAG_4X4[at])] = lists[list][at];
        }
        for at in 0..64 {
            out.eight[list][usize::from(ZIGZAG_8X8[at])] = lists[6 + list][at];
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub struct Sps {
    pub id: u32,
    pub profile: u32,
    pub level: u32,
    /// 1 for 4:2:0; nought with separate planes.
    pub chroma: u32,
    pub separate_planes: bool,
    pub depth_luma: u32,
    pub depth_chroma: u32,
    pub transform_bypass: bool,
    pub scaling: Scaling,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub poc_always_zero: bool,
    pub offset_non_ref: i32,
    pub offset_top_to_bottom: i32,
    pub cycle: [i32; 256],
    pub cycle_length: u32,
    pub max_ref_frames: u32,
    pub gaps_allowed: bool,
    pub mb_width: usize,
    /// In macroblock pairs for a field stream, which makes 1080 into 1088.
    pub map_units: usize,
    pub frame_mbs_only: bool,
    pub mb_adaptive: bool,
    pub direct_8x8_inference: bool,
    pub crop: [u32; 4],
    pub max_reorder: Option<u32>,
    pub max_dec_frame_buffering: Option<u32>,
}

impl Default for Sps {
    fn default() -> Self {
        Self {
            id: 0,
            profile: 66,
            level: 10,
            chroma: 1,
            separate_planes: false,
            depth_luma: 8,
            depth_chroma: 8,
            transform_bypass: false,
            scaling: Scaling::default(),
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 4,
            poc_always_zero: false,
            offset_non_ref: 0,
            offset_top_to_bottom: 0,
            cycle: [0; 256],
            cycle_length: 0,
            max_ref_frames: 1,
            gaps_allowed: false,
            mb_width: 0,
            map_units: 0,
            frame_mbs_only: true,
            mb_adaptive: false,
            direct_8x8_inference: true,
            crop: [0; 4],
            max_reorder: None,
            max_dec_frame_buffering: None,
        }
    }
}

impl Sps {
    pub fn mb_height(&self) -> usize {
        self.map_units * if self.frame_mbs_only { 1 } else { 2 }
    }

    pub fn chroma_array(&self) -> u32 {
        if self.separate_planes { 0 } else { self.chroma }
    }
}

pub fn sequence(bytes: &[u8]) -> Option<Sps> {
    let mut reader = Reader::new(bytes);
    let mut sps = Sps {
        profile: reader.bits(8),
        ..Sps::default()
    };
    reader.skip(8); // the constraint flags
    sps.level = reader.bits(8);
    sps.id = reader.ue();
    if sps.id as usize >= SEQUENCES {
        return None;
    }
    if matches!(
        sps.profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        sps.chroma = reader.ue();
        if sps.chroma > 3 {
            return None;
        }
        if sps.chroma == 3 {
            sps.separate_planes = reader.bit() == 1;
        }
        let (luma, chroma) = (reader.ue(), reader.ue());
        if luma > 6 || chroma > 6 {
            return None;
        }
        sps.depth_luma = luma + 8;
        sps.depth_chroma = chroma + 8;
        sps.transform_bypass = reader.bit() == 1;
        if reader.bit() == 1 {
            sps.scaling = Scaling::read(&mut reader, sps.chroma, 1);
        }
    }
    // 0〜12 を超えると後のシフトがあふれる。
    let frame_num = reader.ue();
    if frame_num > 12 {
        return None;
    }
    sps.log2_max_frame_num = frame_num + 4;
    sps.poc_type = reader.ue();
    match sps.poc_type {
        0 => {
            let lsb = reader.ue();
            if lsb > 12 {
                return None;
            }
            sps.log2_max_poc_lsb = lsb + 4;
        }
        1 => {
            sps.poc_always_zero = reader.bit() == 1;
            sps.offset_non_ref = reader.se();
            sps.offset_top_to_bottom = reader.se();
            sps.cycle_length = reader.ue().min(255);
            for at in 0..sps.cycle_length as usize {
                sps.cycle[at] = reader.se();
            }
        }
        2 => {}
        _ => return None,
    }
    sps.max_ref_frames = reader.ue().min(REFS as u32);
    sps.gaps_allowed = reader.bit() == 1;
    let (mb_width, map_units) = (u64::from(reader.ue()) + 1, u64::from(reader.ue()) + 1);
    sps.frame_mbs_only = reader.bit() == 1;
    let mb_height = map_units * if sps.frame_mbs_only { 1 } else { 2 };
    if !crate::video::fits(mb_width * 16, mb_height * 16) {
        return None;
    }
    sps.mb_width = mb_width as usize;
    sps.map_units = map_units as usize;
    if !sps.frame_mbs_only {
        sps.mb_adaptive = reader.bit() == 1;
    }
    sps.direct_8x8_inference = reader.bit() == 1;
    if reader.bit() == 1 {
        sps.crop = [reader.ue(), reader.ue(), reader.ue(), reader.ue()];
        // クロップがピクチャーより大きいものは拒否する（単位は色差の画素で、2 倍を超えることはない）。
        let [left, right, top, bottom] = sps.crop.map(u64::from);
        if (left + right) * 2 >= mb_width * 16 || (top + bottom) * 4 >= mb_height * 16 {
            return None;
        }
    }
    if reader.bit() == 1 {
        usability(&mut reader, &mut sps);
    }
    if sps.mb_width == 0 || sps.map_units == 0 {
        return None;
    }
    Some(sps)
}

/// Walks the VUI for how far the stream reorders (also walked by [`crate::es::h264`]).
fn usability(reader: &mut Reader<'_>, sps: &mut Sps) {
    if reader.bit() == 1 && reader.bits(8) == 255 {
        reader.skip(16 + 16); // the shape of a sample, written out
    }
    if reader.bit() == 1 {
        reader.skip(1); // overscan_appropriate_flag
    }
    if reader.bit() == 1 {
        reader.skip(3 + 1);
        if reader.bit() == 1 {
            reader.skip(8 * 3);
        }
    }
    if reader.bit() == 1 {
        reader.ue();
        reader.ue();
    }
    if reader.bit() == 1 {
        reader.skip(32 + 32 + 1);
    }
    let nal = reader.bit() == 1;
    if nal {
        reference_decoder(reader);
    }
    let vcl = reader.bit() == 1;
    if vcl {
        reference_decoder(reader);
    }
    if nal || vcl {
        reader.skip(1); // low_delay_hrd_flag
    }
    reader.skip(1); // pic_struct_present_flag
    if reader.bit() == 1 {
        reader.skip(1); // motion_vectors_over_pic_boundaries_flag
        reader.ue();
        reader.ue();
        reader.ue();
        reader.ue();
        sps.max_reorder = Some(reader.ue().min(REFS as u32));
        sps.max_dec_frame_buffering = Some(reader.ue().min(REFS as u32));
    }
}

fn reference_decoder(reader: &mut Reader<'_>) {
    let count = reader.ue().min(32) + 1;
    reader.skip(4 + 4);
    for _ in 0..count {
        reader.ue();
        reader.ue();
        reader.skip(1);
    }
    reader.skip(5 + 5 + 5 + 5);
}

#[derive(Debug, Clone, Copy)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub cabac: bool,
    pub bottom_field_order_present: bool,
    pub slice_groups: u32,
    pub num_ref_idx: [u32; 2],
    pub weighted_pred: bool,
    pub weighted_bipred: u32,
    pub init_qp: i32,
    pub init_qs: i32,
    pub chroma_qp_offset: [i32; 2],
    pub deblocking_control: bool,
    pub constrained_intra: bool,
    pub redundant_present: bool,
    pub transform_8x8: bool,
    pub scaling: Scaling,
}

impl Default for Pps {
    fn default() -> Self {
        Self {
            id: 0,
            sps_id: 0,
            cabac: false,
            bottom_field_order_present: false,
            slice_groups: 1,
            num_ref_idx: [1, 1],
            weighted_pred: false,
            weighted_bipred: 0,
            init_qp: 26,
            init_qs: 26,
            chroma_qp_offset: [0, 0],
            deblocking_control: false,
            constrained_intra: false,
            redundant_present: false,
            transform_8x8: false,
            scaling: Scaling::default(),
        }
    }
}

pub fn picture(bytes: &[u8], sps: Option<&Sps>) -> Option<Pps> {
    let mut reader = Reader::new(bytes);
    let mut pps = Pps {
        id: reader.ue(),
        sps_id: reader.ue(),
        ..Pps::default()
    };
    if pps.id as usize >= PICTURES || pps.sps_id as usize >= SEQUENCES {
        return None;
    }
    pps.cabac = reader.bit() == 1;
    pps.bottom_field_order_present = reader.bit() == 1;
    pps.slice_groups = reader.ue() + 1;
    if pps.slice_groups > 1 {
        // No broadcast uses slice groups; refuse rather than misplace every macroblock.
        return None;
    }
    pps.num_ref_idx = [reader.ue() + 1, reader.ue() + 1];
    if pps.num_ref_idx[0] as usize > REFS || pps.num_ref_idx[1] as usize > REFS {
        return None;
    }
    pps.weighted_pred = reader.bit() == 1;
    pps.weighted_bipred = reader.bits(2);
    // 7.4.2.2 の範囲（qp は深さ 14 ビットで -62 まで）。範囲外の値は後の足し算をあふれさせる。
    let (qp, qs) = (reader.se(), reader.se());
    if !(-62..=25).contains(&qp) || !(-26..=25).contains(&qs) {
        return None;
    }
    pps.init_qp = qp + 26;
    pps.init_qs = qs + 26;
    pps.chroma_qp_offset[0] = reader.se();
    pps.chroma_qp_offset[1] = pps.chroma_qp_offset[0];
    pps.deblocking_control = reader.bit() == 1;
    pps.constrained_intra = reader.bit() == 1;
    pps.redundant_present = reader.bit() == 1;
    if more_data(&reader, bytes) {
        pps.transform_8x8 = reader.bit() == 1;
        if reader.bit() == 1 {
            let chroma = sps.map_or(1, |sps| sps.chroma);
            pps.scaling = Scaling::read(&mut reader, chroma, usize::from(pps.transform_8x8));
        }
        pps.chroma_qp_offset[1] = reader.se();
    }
    if pps
        .chroma_qp_offset
        .iter()
        .any(|offset| !(-12..=12).contains(offset))
    {
        return None;
    }
    Some(pps)
}

fn more_data(reader: &Reader<'_>, bytes: &[u8]) -> bool {
    let at = reader.at();
    let mut last = bytes.len() * 8;
    while last > at {
        last -= 1;
        if bytes[last / 8] >> (7 - last % 8) & 1 == 1 {
            return last > at;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Reorder {
    pub op: u32,
    pub value: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Marking {
    pub op: u32,
    pub first: u32,
    pub second: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Weight {
    pub luma: (i32, i32),
    pub chroma: [(i32, i32); 2],
}

impl Default for Weight {
    fn default() -> Self {
        Self {
            luma: (1, 0),
            chroma: [(1, 0); 2],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    pub first_mb: usize,
    pub kind: Kind,
    /// Whether `slice_type` was in the second five values (all slices of the picture alike).
    pub all_same: bool,
    pub pps_id: u32,
    pub colour_plane: u32,
    pub frame_num: u32,
    pub field_pic: bool,
    pub bottom_field: bool,
    pub idr: bool,
    pub nal_ref_idc: u32,
    pub idr_pic_id: u32,
    pub poc_lsb: u32,
    pub delta_poc_bottom: i32,
    pub delta_poc: [i32; 2],
    pub redundant: u32,
    pub direct_spatial: bool,
    pub num_ref_idx: [usize; 2],
    pub reorder: [[Reorder; REFS + 1]; 2],
    pub reorders: [usize; 2],
    pub log2_weight_denom: [u32; 2],
    pub weights: [[Weight; REFS]; 2],
    pub weighted: bool,
    pub no_output_of_prior_pics: bool,
    pub long_term_reference: bool,
    pub adaptive_marking: bool,
    pub marking: [Marking; REFS + 1],
    pub markings: usize,
    pub cabac_init_idc: u32,
    pub qp: i32,
    pub disable_deblocking: u32,
    pub alpha_offset: i32,
    pub beta_offset: i32,
    pub bits: usize,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            first_mb: 0,
            kind: Kind::I,
            all_same: false,
            pps_id: 0,
            colour_plane: 0,
            frame_num: 0,
            field_pic: false,
            bottom_field: false,
            idr: false,
            nal_ref_idc: 0,
            idr_pic_id: 0,
            poc_lsb: 0,
            delta_poc_bottom: 0,
            delta_poc: [0; 2],
            redundant: 0,
            direct_spatial: false,
            num_ref_idx: [0; 2],
            reorder: [[Reorder::default(); REFS + 1]; 2],
            reorders: [0; 2],
            log2_weight_denom: [0; 2],
            weights: [[Weight::default(); REFS]; 2],
            weighted: false,
            no_output_of_prior_pics: false,
            long_term_reference: false,
            adaptive_marking: false,
            marking: [Marking::default(); REFS + 1],
            markings: 0,
            cabac_init_idc: 0,
            qp: 26,
            disable_deblocking: 0,
            alpha_offset: 0,
            beta_offset: 0,
            bits: 0,
        }
    }
}

pub fn header(
    bytes: &[u8],
    idr: bool,
    nal_ref_idc: u32,
    find: impl Fn(u32) -> Option<(Pps, Sps)>,
) -> Option<(Header, Pps, Sps)> {
    let mut reader = Reader::new(bytes);
    let mut header = Header {
        idr,
        nal_ref_idc,
        ..Header::default()
    };
    header.first_mb = reader.ue() as usize;
    let slice_type = reader.ue();
    if slice_type > 9 {
        return None;
    }
    header.all_same = slice_type >= 5;
    header.kind = Kind::of(slice_type)?;
    header.pps_id = reader.ue();
    let (pps, sps) = find(header.pps_id)?;

    if sps.separate_planes {
        header.colour_plane = reader.bits(2);
    }
    header.frame_num = reader.bits(sps.log2_max_frame_num as usize);
    if !sps.frame_mbs_only {
        header.field_pic = reader.bit() == 1;
        if header.field_pic {
            header.bottom_field = reader.bit() == 1;
        }
    }
    if idr {
        header.idr_pic_id = reader.ue();
    }
    match sps.poc_type {
        0 => {
            header.poc_lsb = reader.bits(sps.log2_max_poc_lsb as usize);
            if pps.bottom_field_order_present && !header.field_pic {
                header.delta_poc_bottom = reader.se();
            }
        }
        1 if !sps.poc_always_zero => {
            header.delta_poc[0] = reader.se();
            if pps.bottom_field_order_present && !header.field_pic {
                header.delta_poc[1] = reader.se();
            }
        }
        _ => {}
    }
    if pps.redundant_present {
        header.redundant = reader.ue();
    }
    if header.kind == Kind::B {
        header.direct_spatial = reader.bit() == 1;
    }
    header.num_ref_idx = [pps.num_ref_idx[0] as usize, pps.num_ref_idx[1] as usize];
    if matches!(header.kind, Kind::P | Kind::Sp | Kind::B) && reader.bit() == 1 {
        header.num_ref_idx[0] = reader.ue() as usize + 1;
        if header.kind == Kind::B {
            header.num_ref_idx[1] = reader.ue() as usize + 1;
        }
    }
    for list in 0..2 {
        if list >= header.kind.lists() {
            header.num_ref_idx[list] = 0;
            continue;
        }
        // A field picture may name twice as many references: each frame offers two fields.
        let most = if header.field_pic { REFS } else { REFS / 2 };
        if header.num_ref_idx[list] > most {
            return None;
        }
    }

    for list in 0..header.kind.lists() {
        if reader.bit() == 0 {
            continue;
        }
        loop {
            let op = reader.ue();
            if op == 3 || header.reorders[list] > REFS {
                break;
            }
            if op > 3 {
                return None;
            }
            header.reorder[list][header.reorders[list]] = Reorder {
                op,
                value: reader.ue(),
            };
            header.reorders[list] += 1;
        }
    }

    header.weighted = (pps.weighted_pred && matches!(header.kind, Kind::P | Kind::Sp))
        || (pps.weighted_bipred == 1 && header.kind == Kind::B);
    if header.weighted {
        read_weights(&mut reader, &mut header, sps.chroma_array() != 0);
    } else {
        for list in 0..2 {
            for weight in &mut header.weights[list] {
                *weight = Weight::default();
            }
        }
    }

    if nal_ref_idc != 0 {
        if idr {
            header.no_output_of_prior_pics = reader.bit() == 1;
            header.long_term_reference = reader.bit() == 1;
        } else {
            header.adaptive_marking = reader.bit() == 1;
            if header.adaptive_marking {
                loop {
                    let op = reader.ue();
                    if op == 0 || header.markings > REFS {
                        break;
                    }
                    if op > 6 {
                        return None;
                    }
                    let first = if matches!(op, 1 | 3) { reader.ue() } else { 0 };
                    let second = match op {
                        2 => reader.ue(),
                        3 | 6 => reader.ue(),
                        4 => reader.ue(),
                        _ => 0,
                    };
                    header.marking[header.markings] = Marking { op, first, second };
                    header.markings += 1;
                }
            }
        }
    }

    if pps.cabac && !header.kind.intra() {
        header.cabac_init_idc = reader.ue();
        if header.cabac_init_idc > 2 {
            return None;
        }
    }
    header.qp = pps.init_qp.saturating_add(reader.se());
    if !(-36..=51).contains(&header.qp) {
        return None;
    }
    if matches!(header.kind, Kind::Sp | Kind::Si) {
        if header.kind == Kind::Sp {
            reader.skip(1); // sp_for_switch_flag
        }
        reader.se(); // slice_qs_delta
    }
    if pps.deblocking_control {
        header.disable_deblocking = reader.ue();
        if header.disable_deblocking > 2 {
            return None;
        }
        if header.disable_deblocking != 1 {
            let (alpha, beta) = (reader.se(), reader.se());
            if !(-6..=6).contains(&alpha) || !(-6..=6).contains(&beta) {
                return None;
            }
            header.alpha_offset = alpha * 2;
            header.beta_offset = beta * 2;
        }
    }
    header.bits = reader.at();
    Some((header, pps, sps))
}

fn read_weights(reader: &mut Reader<'_>, header: &mut Header, chroma: bool) {
    header.log2_weight_denom[0] = reader.ue().min(7);
    if chroma {
        header.log2_weight_denom[1] = reader.ue().min(7);
    }
    let unweighted = [
        1 << header.log2_weight_denom[0],
        1 << header.log2_weight_denom[1],
    ];
    for list in 0..2 {
        for at in 0..REFS {
            header.weights[list][at] = Weight {
                luma: (unweighted[0], 0),
                chroma: [(unweighted[1], 0); 2],
            };
        }
    }
    // 重みと足す値は -128〜127（7.4.3.2）。範囲外の値は予測の掛け算をあふれさせるので丸める。
    let pair =
        |reader: &mut Reader<'_>| (reader.se().clamp(-128, 127), reader.se().clamp(-128, 127));
    for list in 0..header.kind.lists() {
        for at in 0..header.num_ref_idx[list].min(REFS) {
            if reader.bit() == 1 {
                header.weights[list][at].luma = pair(reader);
            }
            if chroma && reader.bit() == 1 {
                for part in 0..2 {
                    header.weights[list][at].chroma[part] = pair(reader);
                }
            }
        }
    }
}
