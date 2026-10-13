// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC: no field pictures; interlace is read from `profile_tier_level`, the VUI
//! (`field_seq_flag`) and picture timing (`pic_struct`). Every IRAP is a random access point.

use crate::bits::{Reader, unescape};
use crate::es::{Access, Coding, Format, Framing, Nals, Picture, Structure};

const IRAP: std::ops::RangeInclusive<u8> = 16..=23;

/// RASL: they read pictures from before their random access point ([`Picture::leading`]).
const RASL: std::ops::RangeInclusive<u8> = 8..=9;

const VCL_END: u8 = 31;

const SPS: u8 = 33;
const PPS: u8 = 34;
const AUD: u8 = 35;
const PREFIX_SEI: u8 = 39;

const PICTURE_TIMING: u32 = 1;

/// Recovery point: how a stream with no IRAP at all is entered.
const RECOVERY_POINT: u32 = 6;

/// Unescape limit (keeps a damaged unit from being copied whole).
const SPS_BYTES: usize = 1024;
const SEI_BYTES: usize = 1024;
const SHORT_BYTES: usize = 64;

/// Table E-1 (0 and 255 are not in it).
const SAMPLE_ASPECTS: [(u32, u32); 17] = [
    (0, 0),
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sequence {
    format: Format,
    progressive_source: bool,
    /// Both source flags set or both clear says nothing.
    interlaced_source: bool,
    field_seq: bool,
    /// Whether pictures carry `pic_struct`; where clear it must not be read.
    frame_field_info: bool,
    colour: super::Colour,
}

#[derive(Debug)]
struct Pending {
    start: usize,
    coding: Coding,
    random_access: bool,
    leading: bool,
    /// A first slice segment has been seen (a picture, not just parameter sets).
    coded: bool,
    timing: Option<(u32, u32)>,
    /// A recovery point named this picture (it stands before the slice).
    recovery: bool,
}

impl Pending {
    fn new(start: usize) -> Self {
        Self {
            start,
            coding: Coding::Unknown,
            random_access: false,
            leading: false,
            coded: false,
            timing: None,
            recovery: false,
        }
    }
}

#[derive(Debug)]
pub struct Splitter {
    sequence: Option<Sequence>,
    /// `num_extra_slice_header_bits` per PPS id (needed to reach `slice_type`).
    extra_slice_bits: [u8; 64],
    declared: bool,
    framing: Framing,
}

impl Default for Splitter {
    fn default() -> Self {
        Self::new(Framing::default())
    }
}

impl Splitter {
    pub fn new(framing: Framing) -> Self {
        Self {
            sequence: None,
            extra_slice_bits: [0; 64],
            declared: false,
            framing,
        }
    }

    /// Feeds one unit (for MMT one access unit) and appends the access units it held.
    pub fn push(&mut self, unit: &[u8], out: &mut Vec<Access>) {
        let mut pending: Option<Pending> = None;
        for nal in Nals::new(unit, self.framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let begin = nal.at;
            let payload = unit.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);

            // A first slice segment after a picture also opens an access unit.
            let slice = (kind <= VCL_END).then(|| self.slice(payload, kind));
            let opens =
                kind == AUD || matches!((&slice, &pending), (Some((true, _)), Some(p)) if p.coded);
            if opens && let Some(done) = pending.take() {
                self.finish(done, begin, out);
            }
            let au = pending.get_or_insert_with(|| Pending::new(begin));

            match kind {
                SPS => {
                    if let Some(sequence) = parse_sps(payload) {
                        self.sequence = Some(sequence);
                        self.declared = true;
                    }
                }
                PPS => parse_pps(payload, &mut self.extra_slice_bits),
                PREFIX_SEI => {
                    let (timing, recovery) = messages(payload);
                    if self.sequence.is_some_and(|s| s.frame_field_info) {
                        au.timing = timing.or(au.timing);
                    }
                    au.recovery |= recovery;
                }
                _ => {
                    if let Some((true, coding)) = slice {
                        au.coding = coding;
                        au.random_access = IRAP.contains(&kind);
                        au.leading = RASL.contains(&kind);
                        au.coded = true;
                    }
                }
            }
        }
        if let Some(done) = pending.take() {
            self.finish(done, unit.len(), out);
        }
    }

    pub fn format(&self) -> Option<Format> {
        self.sequence.map(|sequence| sequence.format)
    }

    fn slice(&self, payload: &[u8], kind: u8) -> (bool, Coding) {
        let bytes = unescape(payload, SHORT_BYTES);
        let mut reader = Reader::new(&bytes);
        if reader.bit() == 0 {
            return (false, Coding::Unknown);
        }
        if IRAP.contains(&kind) {
            reader.bit(); // no_output_of_prior_pics_flag
        }
        let pps = reader.ue() as usize;
        reader.skip(usize::from(
            self.extra_slice_bits.get(pps).copied().unwrap_or(0),
        ));
        let coding = match reader.ue() {
            0 => Coding::B,
            1 => Coding::P,
            2 => Coding::I,
            _ => Coding::Unknown,
        };
        (true, coding)
    }

    /// Reports one finished access unit if it holds a picture; otherwise its SPS carries forward.
    fn finish(&mut self, au: Pending, end: usize, out: &mut Vec<Access>) {
        if !au.coded {
            return;
        }
        let sequence = self.sequence;
        let declared = sequence.map(|s| s.progressive_source && !s.interlaced_source);
        let (structure, top_field_first, repeat_first_field) = match au.timing {
            Some((pic_struct, _)) => from_pic_struct(pic_struct),
            None => (Structure::Frame, false, false),
        };
        // In a field-only sequence a coded picture is whole, not half a pair; `Top` / `Bottom`
        // would make layers above deinterlace it. The field is kept in [`Picture::lone_field`].
        let field_seq = sequence.is_some_and(|s| s.field_seq);
        let lone_field = (field_seq && structure != Structure::Frame).then_some(structure);
        let (structure, top_field_first, repeat_first_field) = match field_seq {
            true => (Structure::Frame, false, false),
            false => (structure, top_field_first, repeat_first_field),
        };
        let progressive = match au.timing {
            // source_scan_type: 0 interlaced, 1 progressive, 2 unknown.
            Some((_, 0)) => false,
            Some((_, 1)) => true,
            _ => declared.unwrap_or(false),
        };
        out.push(Access {
            picture: Picture {
                coding: au.coding,
                structure,
                top_field_first,
                repeat_first_field,
                progressive,
                random_access: au.random_access || au.recovery,
                leading: au.leading,
                carries_format: std::mem::take(&mut self.declared),
                lone_field,
            },
            format: sequence.map(|s| s.format),
            range: au.start..end.max(au.start),
        });
    }
}

/// Table D-2. 7 and 8 repeat the frame, not a field.
fn from_pic_struct(value: u32) -> (Structure, bool, bool) {
    match value {
        1 | 9 | 11 => (Structure::Top, false, false),
        2 | 10 | 12 => (Structure::Bottom, false, false),
        3 => (Structure::Frame, true, false),
        4 => (Structure::Frame, false, false),
        5 => (Structure::Frame, true, true),
        6 => (Structure::Frame, false, true),
        _ => (Structure::Frame, false, false),
    }
}

/// Reads an SPS through the VUI timing. Nothing where it ran off the end.
fn parse_sps(payload: &[u8]) -> Option<Sequence> {
    let bytes = unescape(payload, SPS_BYTES);
    let mut reader = Reader::new(&bytes);
    let mut sequence = Sequence {
        format: Format::default(),
        progressive_source: false,
        interlaced_source: false,
        field_seq: false,
        frame_field_info: false,
        colour: super::Colour::default(),
    };
    reader.skip(4); // sps_video_parameter_set_id
    let sub_layers = reader.bits(3) as usize; // sps_max_sub_layers_minus1
    reader.skip(1); // sps_temporal_id_nesting_flag
    parse_profile_tier_level(&mut reader, sub_layers, &mut sequence);

    reader.ue(); // sps_seq_parameter_set_id
    let chroma = reader.ue();
    if chroma == 3 {
        reader.skip(1); // separate_colour_plane_flag
    }
    let width = reader.ue();
    let height = reader.ue();
    // The conformance window is in chroma samples.
    let (sub_width, sub_height) = match chroma {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    };
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    if reader.bit() == 1 {
        left = reader.ue();
        right = reader.ue();
        top = reader.ue();
        bottom = reader.ue();
    }
    let depth = reader.ue().saturating_add(8); // bit_depth_luma_minus8
    reader.ue(); // bit_depth_chroma_minus8
    let poc_bits = (reader.ue() as usize).saturating_add(4);

    let first = if reader.bit() == 1 { 0 } else { sub_layers };
    for _ in first..=sub_layers {
        reader.ue(); // sps_max_dec_pic_buffering_minus1
        reader.ue(); // sps_max_num_reorder_pics
        reader.ue(); // sps_max_latency_increase_plus1
    }
    reader.ue(); // log2_min_luma_coding_block_size_minus3
    reader.ue(); // log2_diff_max_min_luma_coding_block_size
    reader.ue(); // log2_min_luma_transform_block_size_minus2
    reader.ue(); // log2_diff_max_min_luma_transform_block_size
    reader.ue(); // max_transform_hierarchy_depth_inter
    reader.ue(); // max_transform_hierarchy_depth_intra
    if reader.bit() == 1 && reader.bit() == 1 {
        parse_scaling_list(&mut reader);
    }
    reader.skip(1); // amp_enabled_flag
    reader.skip(1); // sample_adaptive_offset_enabled_flag
    if reader.bit() == 1 {
        reader.skip(8); // the two sample bit depths
        reader.ue(); // log2_min_pcm_luma_coding_block_size_minus3
        reader.ue(); // log2_diff_max_min_pcm_luma_coding_block_size
        reader.skip(1); // pcm_loop_filter_disabled_flag
    }

    let sets = (reader.ue() as usize).min(64);
    let mut deltas = [0u32; 65];
    for set in 0..sets {
        parse_short_term_ref_pic_set(&mut reader, set, sets, &mut deltas);
    }
    if reader.bit() == 1 {
        let count = (reader.ue() as usize).min(32);
        for _ in 0..count {
            reader.skip(poc_bits); // lt_ref_pic_poc_lsb_sps
            reader.skip(1); // used_by_curr_pic_lt_sps_flag
        }
    }
    reader.skip(1); // sps_temporal_mvp_enabled_flag
    reader.skip(1); // strong_intra_smoothing_enabled_flag

    let mut aspect = (0, 1);
    let mut rate = (0, 1);
    if reader.bit() == 1 {
        let (sample_aspect, timing) = parse_vui(&mut reader, &mut sequence);
        aspect = sample_aspect;
        rate = timing;
    }
    if reader.at() > bytes.len() * 8 {
        return None;
    }

    // 上限を超えるものは開かない（[`crate::video::fits`]）。
    if !crate::video::fits(width.into(), height.into()) {
        return None;
    }
    let crop = |sub: u32, a: u32, b: u32| u64::from(sub) * (u64::from(a) + u64::from(b));
    let width = u64::from(width).saturating_sub(crop(sub_width, left, right)) as u32;
    let height = u64::from(height).saturating_sub(crop(sub_height, top, bottom)) as u32;
    if width == 0 || height == 0 {
        return None;
    }
    // What is shown is the frame: a field-only 1920x540 with square samples is 16:9, not 32:9.
    let rows = if sequence.field_seq {
        height.saturating_mul(2)
    } else {
        height
    };
    sequence.format = Format {
        width,
        height,
        depth: depth.min(255) as u8,
        chroma: chroma.min(255) as u8,
        rate,
        progressive: sequence.progressive_source
            && !sequence.interlaced_source
            && !sequence.field_seq,
        aspect: reduce(width * aspect.0, rows * aspect.1),
        field_pictures: sequence.field_seq,
        woven: false,
        colour: sequence.colour,
    };
    Some(sequence)
}

fn parse_profile_tier_level(reader: &mut Reader, sub_layers: usize, sequence: &mut Sequence) {
    reader.skip(2); // general_profile_space
    reader.skip(1); // general_tier_flag
    reader.skip(5); // general_profile_idc, 2 for Main 10
    reader.skip(32); // general_profile_compatibility_flag[32]
    sequence.progressive_source = reader.bit() == 1;
    sequence.interlaced_source = reader.bit() == 1;
    reader.skip(1); // general_non_packed_constraint_flag
    reader.skip(1); // general_frame_only_constraint_flag
    reader.skip(43); // the constraint flags of the range extensions, reserved
    reader.skip(1); // general_inbld_flag, or reserved where no profile uses it
    reader.skip(8); // general_level_idc, 51 for Level 5.1

    // Sub-layer flags are padded out to eight so what follows is aligned.
    let mut present = [(false, false); 8];
    for entry in present.iter_mut().take(sub_layers) {
        *entry = (reader.bit() == 1, reader.bit() == 1);
    }
    if sub_layers > 0 {
        reader.skip(2 * (8 - sub_layers));
    }
    for &(profile, level) in present.iter().take(sub_layers) {
        if profile {
            reader.skip(88); // the same profile and constraint flags again
        }
        if level {
            reader.skip(8);
        }
    }
}

fn parse_scaling_list(reader: &mut Reader) {
    for size in 0..4 {
        let step = if size == 3 { 3 } else { 1 };
        let mut matrix = 0;
        while matrix < 6 {
            if reader.bit() == 0 {
                reader.ue();
            } else {
                let coefficients = (1usize << (4 + (size << 1))).min(64);
                if size > 1 {
                    reader.se(); // scaling_list_dc_coef_minus8
                }
                for _ in 0..coefficients {
                    reader.se();
                }
            }
            matrix += step;
        }
    }
}

fn parse_short_term_ref_pic_set(reader: &mut Reader, idx: usize, count: usize, deltas: &mut [u32]) {
    if idx != 0 && reader.bit() == 1 {
        let reference = if idx == count {
            (idx).checked_sub(1 + reader.ue() as usize)
        } else {
            idx.checked_sub(1)
        };
        reader.skip(1); // delta_rps_sign
        reader.ue(); // abs_delta_rps_minus1
        let held = reference
            .and_then(|at| deltas.get(at))
            .copied()
            .unwrap_or(0);
        let mut used = 0;
        for _ in 0..=held.min(64) {
            if reader.bit() == 1 || reader.bit() == 1 {
                used += 1;
            }
        }
        if let Some(slot) = deltas.get_mut(idx) {
            *slot = used;
        }
        return;
    }
    let negative = reader.ue().min(64);
    let positive = reader.ue().min(64);
    for _ in 0..negative + positive {
        reader.ue(); // delta_poc_s0_minus1 or delta_poc_s1_minus1
        reader.skip(1); // used_by_curr_pic_s0_flag or s1
    }
    if let Some(slot) = deltas.get_mut(idx) {
        *slot = negative + positive;
    }
}

fn parse_vui(reader: &mut Reader, sequence: &mut Sequence) -> ((u32, u32), (u32, u32)) {
    let mut aspect = (0, 1);
    if reader.bit() == 1 {
        let idc = reader.bits(8);
        aspect = if idc == 255 {
            (reader.bits(16), reader.bits(16))
        } else {
            SAMPLE_ASPECTS.get(idc as usize).copied().unwrap_or((0, 0))
        };
        if aspect.0 == 0 || aspect.1 == 0 {
            aspect = (0, 1);
        }
    }
    if reader.bit() == 1 {
        reader.skip(1); // overscan_appropriate_flag
    }
    if reader.bit() == 1 {
        reader.skip(3); // video_format
        sequence.colour.full_range = reader.bit() == 1;
        if reader.bit() == 1 {
            sequence.colour.primaries = reader.bits(8) as u8;
            sequence.colour.transfer = reader.bits(8) as u8;
            sequence.colour.matrix = reader.bits(8) as u8;
        }
    }
    if reader.bit() == 1 {
        reader.ue();
        reader.ue();
    }
    reader.skip(1); // neutral_chroma_indication_flag
    sequence.field_seq = reader.bit() == 1;
    sequence.frame_field_info = reader.bit() == 1;
    if reader.bit() == 1 {
        // default_display_window_flag: a hint only; the conformance window's size is kept.
        reader.ue();
        reader.ue();
        reader.ue();
        reader.ue();
    }
    let mut rate = (0, 1);
    if reader.bit() == 1 {
        let units = reader.bits(32);
        let scale = reader.bits(32);
        if units != 0 {
            rate = reduce(scale, units);
        }
    }
    (aspect, rate)
}

fn parse_pps(payload: &[u8], extra: &mut [u8; 64]) {
    let bytes = unescape(payload, SHORT_BYTES);
    let mut reader = Reader::new(&bytes);
    let id = reader.ue() as usize;
    reader.ue(); // pps_seq_parameter_set_id
    reader.skip(1); // dependent_slice_segments_enabled_flag
    reader.skip(1); // output_flag_present_flag
    if let Some(slot) = extra.get_mut(id) {
        *slot = reader.bits(3) as u8;
    }
}

/// Walks an SEI unit for `pic_struct`, `source_scan_type` and a recovery point. `pic_struct`
/// comes first in HEVC, so no HRD lengths are needed. Some streams send no IRAP at all.
fn messages(payload: &[u8]) -> (Option<(u32, u32)>, bool) {
    let bytes = unescape(payload, SEI_BYTES);
    let mut timing = None;
    let mut recovery = false;
    let mut at = 0;
    while at < bytes.len() {
        let Some(kind) = read_extended(&bytes, &mut at) else {
            break;
        };
        let Some(size) = read_extended(&bytes, &mut at) else {
            break;
        };
        let Some(end) = (at.checked_add(size as usize)).filter(|&end| end <= bytes.len()) else {
            break;
        };
        match kind {
            PICTURE_TIMING => {
                let mut reader = Reader::new(&bytes[at..end]);
                timing = Some((reader.bits(4), reader.bits(2)));
            }
            RECOVERY_POINT => recovery = true,
            _ => {}
        }
        at = end;
    }
    (timing, recovery)
}

fn read_extended(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    while bytes.get(*at) == Some(&0xff) {
        value = value.saturating_add(255);
        *at += 1;
    }
    let last = *bytes.get(*at)?;
    *at += 1;
    Some(value.saturating_add(u32::from(last)))
}

fn reduce(numerator: u32, denominator: u32) -> (u32, u32) {
    if numerator == 0 || denominator == 0 {
        return (0, 1);
    }
    let (mut a, mut b) = (numerator, denominator);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    (numerator / a, denominator / a)
}

#[cfg(test)]
mod tests {
    use super::{Framing, Splitter, from_pic_struct, reduce};
    use crate::es::{Access, Coding, Structure};

    #[derive(Debug, Default)]
    struct Writer {
        bytes: Vec<u8>,
        at: usize,
    }

    impl Writer {
        fn bit(&mut self, value: u32) {
            if self.at.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if value & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 1 << (7 - self.at % 8);
            }
            self.at += 1;
        }

        fn bits(&mut self, count: usize, value: u32) {
            for shift in (0..count).rev() {
                self.bit(value >> shift);
            }
        }

        fn flag(&mut self, value: bool) {
            self.bit(u32::from(value));
        }

        fn ue(&mut self, value: u32) {
            let width = (value + 1).ilog2() as usize;
            self.bits(width, 0);
            self.bits(width + 1, value + 1);
        }

        fn finish(mut self) -> Vec<u8> {
            self.bit(1);
            while !self.at.is_multiple_of(8) {
                self.bit(0);
            }
            self.bytes
        }
    }

    fn escape(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut zeroes = 0;
        for &byte in bytes {
            if zeroes >= 2 && byte <= 3 {
                out.push(3);
                zeroes = 0;
            }
            zeroes = if byte == 0 { zeroes + 1 } else { 0 };
            out.push(byte);
        }
        out
    }

    /// What an SPS is built to say; defaults are what an MMT broadcast sends.
    #[derive(Debug, Clone, Copy)]
    struct Spec {
        width: u32,
        height: u32,
        window: Option<(u32, u32, u32, u32)>,
        field_seq: bool,
        frame_field_info: bool,
        progressive_source: bool,
        interlaced_source: bool,
        aspect_idc: u32,
        colour: Option<(u32, u32, u32)>,
    }

    impl Default for Spec {
        fn default() -> Self {
            Self {
                width: 3840,
                height: 2160,
                window: None,
                field_seq: false,
                frame_field_info: true,
                progressive_source: true,
                interlaced_source: false,
                aspect_idc: 1,
                colour: None,
            }
        }
    }

    fn sps(spec: Spec) -> Vec<u8> {
        let mut w = Writer::default();
        w.bits(4, 0); // sps_video_parameter_set_id
        w.bits(3, 0); // sps_max_sub_layers_minus1
        w.flag(true); // sps_temporal_id_nesting_flag

        w.bits(2, 0); // general_profile_space
        w.flag(false); // general_tier_flag, main tier
        w.bits(5, 2); // general_profile_idc, Main 10
        w.bits(32, 0); // general_profile_compatibility_flag[32]
        w.flag(spec.progressive_source);
        w.flag(spec.interlaced_source);
        w.flag(false); // general_non_packed_constraint_flag
        w.flag(true); // general_frame_only_constraint_flag
        w.bits(32, 0); // 43 reserved bits, in two goes
        w.bits(11, 0);
        w.flag(false); // general_inbld_flag
        w.bits(8, 51); // general_level_idc, Level 5.1

        w.ue(0); // sps_seq_parameter_set_id
        w.ue(1); // chroma_format_idc, 4:2:0
        w.ue(spec.width);
        w.ue(spec.height);
        match spec.window {
            Some((left, right, top, bottom)) => {
                w.flag(true);
                w.ue(left);
                w.ue(right);
                w.ue(top);
                w.ue(bottom);
            }
            None => w.flag(false),
        }
        w.ue(2); // bit_depth_luma_minus8, ten bits
        w.ue(2); // bit_depth_chroma_minus8
        w.ue(4); // log2_max_pic_order_cnt_lsb_minus4
        w.flag(true); // sps_sub_layer_ordering_info_present_flag
        w.ue(5); // sps_max_dec_pic_buffering_minus1
        w.ue(3); // sps_max_num_reorder_pics
        w.ue(0); // sps_max_latency_increase_plus1
        w.ue(0); // log2_min_luma_coding_block_size_minus3
        w.ue(3); // log2_diff_max_min_luma_coding_block_size
        w.ue(0); // log2_min_luma_transform_block_size_minus2
        w.ue(3); // log2_diff_max_min_luma_transform_block_size
        w.ue(2); // max_transform_hierarchy_depth_inter
        w.ue(2); // max_transform_hierarchy_depth_intra
        w.flag(false); // scaling_list_enabled_flag
        w.flag(true); // amp_enabled_flag
        w.flag(true); // sample_adaptive_offset_enabled_flag
        w.flag(false); // pcm_enabled_flag
        w.ue(0); // num_short_term_ref_pic_sets
        w.flag(false); // long_term_ref_pics_present_flag
        w.flag(true); // sps_temporal_mvp_enabled_flag
        w.flag(true); // strong_intra_smoothing_enabled_flag

        w.flag(true); // vui_parameters_present_flag
        w.flag(spec.aspect_idc != 0); // aspect_ratio_info_present_flag
        if spec.aspect_idc != 0 {
            w.bits(8, spec.aspect_idc);
        }
        w.flag(false); // overscan_info_present_flag
        w.flag(spec.colour.is_some()); // video_signal_type_present_flag
        if let Some((primaries, transfer, matrix)) = spec.colour {
            w.bits(3, 0); // video_format, component
            w.flag(false); // video_full_range_flag
            w.flag(true); // colour_description_present_flag
            w.bits(8, primaries);
            w.bits(8, transfer);
            w.bits(8, matrix);
        }
        w.flag(false); // chroma_loc_info_present_flag
        w.flag(false); // neutral_chroma_indication_flag
        w.flag(spec.field_seq);
        w.flag(spec.frame_field_info);
        w.flag(false); // default_display_window_flag
        w.flag(true); // vui_timing_info_present_flag
        w.bits(32, 2002); // vui_num_units_in_tick
        w.bits(32, 120_000); // vui_time_scale
        w.flag(false); // vui_poc_proportional_to_timing_flag
        w.flag(false); // vui_hrd_parameters_present_flag
        w.flag(false); // bitstream_restriction_flag
        w.flag(false); // sps_extension_present_flag
        w.finish()
    }

    fn pps(extra: u32) -> Vec<u8> {
        let mut w = Writer::default();
        w.ue(0); // pps_pic_parameter_set_id
        w.ue(0); // pps_seq_parameter_set_id
        w.flag(false); // dependent_slice_segments_enabled_flag
        w.flag(false); // output_flag_present_flag
        w.bits(3, extra); // num_extra_slice_header_bits
        w.finish()
    }

    fn slice(first: bool, irap: bool, slice_type: u32) -> Vec<u8> {
        let mut w = Writer::default();
        w.flag(first);
        if irap {
            w.flag(false); // no_output_of_prior_pics_flag
        }
        w.ue(0); // slice_pic_parameter_set_id
        w.ue(slice_type);
        w.finish()
    }

    fn picture_timing(pic_struct: u32, scan: u32) -> Vec<u8> {
        let mut body = Writer::default();
        body.bits(4, pic_struct);
        body.bits(2, scan);
        body.flag(false); // duplicate_flag
        let body = body.finish();
        let mut out = vec![1, body.len() as u8];
        out.extend_from_slice(&body);
        out
    }

    fn nal(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, kind << 1, 1];
        out.extend_from_slice(&escape(payload));
        out
    }

    fn aud() -> Vec<u8> {
        nal(35, &[0x50])
    }

    fn split(unit: &[u8]) -> (Splitter, Vec<Access>) {
        let mut splitter = Splitter::new(Framing::StartCodes);
        let mut out = Vec::new();
        splitter.push(unit, &mut out);
        (splitter, out)
    }

    #[test]
    fn a_sequence_parameter_set_gives_what_a_4k_broadcast_declares() {
        let (splitter, _) = split(&nal(33, &sps(Spec::default())));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!(format.width, 3840);
        assert_eq!(format.height, 2160);
        assert_eq!(format.depth, 10);
        assert_eq!(format.chroma, 1);
        assert_eq!(format.rate, (60000, 1001));
        assert_eq!(format.aspect, (16, 9));
        assert!(format.progressive);
        assert_eq!(format.colour, super::super::Colour::default());
        assert!(!format.colour.is_hdr());
    }

    /// HLG (STD-B32 Part 1 Table 4-6: 9, 18, 9); the VUI after it still reads right.
    #[test]
    fn the_colour_description_is_read_and_what_follows_it_still_is() {
        let spec = Spec {
            colour: Some((9, 18, 9)),
            field_seq: true,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!(format.colour.primaries, 9);
        assert_eq!(format.colour.transfer, 18);
        assert_eq!(format.colour.matrix, 9);
        assert!(!format.colour.full_range);
        assert!(format.colour.is_hdr());
        assert!(format.field_pictures);
        assert_eq!(format.rate, (60000, 1001));
    }

    #[test]
    fn a_conformance_window_crops_the_coded_size() {
        let spec = Spec {
            width: 1920,
            height: 1088,
            window: Some((0, 0, 0, 4)),
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!((format.width, format.height), (1920, 1080));
        assert_eq!(format.aspect, (16, 9));
    }

    #[test]
    fn a_left_and_right_window_crops_in_chroma_samples() {
        let spec = Spec {
            window: Some((8, 8, 0, 0)),
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!(format.width, 3840 - 32);
    }

    #[test]
    fn a_field_sequence_is_not_progressive_however_the_source_was_described() {
        let spec = Spec {
            field_seq: true,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        let format = splitter.format().expect("the sequence should be read");
        assert!(!format.progressive);
        assert_eq!((format.width, format.height), (3840, 2160));
    }

    #[test]
    fn an_interlaced_source_is_not_progressive() {
        let spec = Spec {
            progressive_source: false,
            interlaced_source: true,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        assert!(!splitter.format().expect("read").progressive);
    }

    #[test]
    fn both_source_flags_set_says_nothing_and_is_not_taken_as_progressive() {
        let spec = Spec {
            progressive_source: true,
            interlaced_source: true,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        assert!(!splitter.format().expect("read").progressive);
    }

    #[test]
    fn an_inserted_byte_does_not_shift_the_fields() {
        let payload = sps(Spec::default());
        assert!(
            escape(&payload).len() > payload.len(),
            "this stream should need the inserted byte for the test to mean anything"
        );
        let (splitter, _) = split(&nal(33, &payload));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!((format.width, format.height), (3840, 2160));
        assert_eq!(format.depth, 10);
        assert_eq!(format.rate, (60000, 1001));
    }

    #[test]
    fn an_unspecified_aspect_is_left_unsaid_rather_than_guessed() {
        let spec = Spec {
            aspect_idc: 0,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        assert_eq!(splitter.format().expect("read").aspect, (0, 1));
    }

    #[test]
    fn an_access_unit_comes_out_with_what_its_pieces_said() {
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(39, &picture_timing(0, 1)));
        unit.extend(nal(21, &slice(true, true, 2)));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        let access = &out[0];
        assert_eq!(access.picture.coding, Coding::I);
        assert_eq!(access.picture.structure, Structure::Frame);
        assert!(access.picture.progressive);
        assert!(access.picture.random_access);
        assert!(access.picture.carries_format);
        assert_eq!(access.range, 0..unit.len());
        assert_eq!(access.format.expect("declared").width, 3840);
    }

    #[test]
    fn a_delimiter_cuts_one_unit_into_two_access_units() {
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(21, &slice(true, true, 2)));
        let first = unit.len();
        unit.extend(aud());
        unit.extend(nal(39, &picture_timing(0, 1)));
        unit.extend(nal(1, &slice(true, false, 0)));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].range, 0..first);
        assert_eq!(out[1].range, first..unit.len());
        assert_eq!(out[0].picture.coding, Coding::I);
        assert!(out[0].picture.random_access);
        assert!(out[0].picture.carries_format);
        assert_eq!(out[1].picture.coding, Coding::B);
        assert!(!out[1].picture.random_access);
        assert!(!out[1].picture.carries_format);
    }

    #[test]
    fn a_stream_with_no_delimiters_is_still_cut_at_its_pictures() {
        let mut unit = nal(33, &sps(Spec::default()));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(1, &slice(true, false, 1)));
        unit.extend(nal(1, &slice(false, false, 1)));
        unit.extend(nal(1, &slice(true, false, 0)));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].picture.coding, Coding::P);
        assert_eq!(out[1].picture.coding, Coding::B);
    }

    #[test]
    fn extra_slice_header_bits_stand_before_the_slice_type() {
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(2)));
        let mut header = Writer::default();
        header.flag(true); // first_slice_segment_in_pic_flag
        header.ue(0); // slice_pic_parameter_set_id
        header.bits(2, 3); // the two extra bits the picture parameter set said
        header.ue(1); // slice_type, P
        unit.extend(nal(1, &header.finish()));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].picture.coding, Coding::P);
    }

    #[test]
    fn a_picture_timing_message_says_what_the_picture_is() {
        for (pic_struct, structure, tff, rff) in [
            (0u32, Structure::Frame, false, false),
            (1, Structure::Top, false, false),
            (2, Structure::Bottom, false, false),
            (3, Structure::Frame, true, false),
            (4, Structure::Frame, false, false),
            (5, Structure::Frame, true, true),
            (6, Structure::Frame, false, true),
        ] {
            let spec = Spec {
                field_seq: false,
                progressive_source: false,
                interlaced_source: true,
                ..Spec::default()
            };
            let mut unit = aud();
            unit.extend(nal(33, &sps(spec)));
            unit.extend(nal(34, &pps(0)));
            unit.extend(nal(39, &picture_timing(pic_struct, 0)));
            unit.extend(nal(1, &slice(true, false, 0)));

            let (_, out) = split(&unit);
            assert_eq!(out.len(), 1, "pic_struct {pic_struct}");
            assert_eq!(
                out[0].picture.structure, structure,
                "pic_struct {pic_struct}"
            );
            assert_eq!(
                out[0].picture.top_field_first, tff,
                "pic_struct {pic_struct}"
            );
            assert_eq!(
                out[0].picture.repeat_first_field, rff,
                "pic_struct {pic_struct}"
            );
            assert!(!out[0].picture.progressive, "pic_struct {pic_struct}");
        }
    }

    fn recovery_point(count: u32) -> Vec<u8> {
        let mut body = Writer::default();
        body.ue(count); // recovery_poc_cnt, written signed as se(v)
        body.flag(true); // exact_match_flag
        body.flag(false); // broken_link_flag
        let body = body.finish();
        let mut out = vec![6, body.len() as u8];
        out.extend_from_slice(&body);
        out
    }

    /// No IRAP: the only entry is a recovery point with an all-intra picture.
    #[test]
    fn a_recovery_point_is_a_place_to_begin_where_no_random_access_point_is() {
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(39, &recovery_point(0)));
        unit.extend(nal(1, &slice(true, false, 2)));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert!(out[0].picture.random_access);
    }

    #[test]
    fn a_trailing_picture_with_no_recovery_point_is_not_a_place_to_begin() {
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(1, &slice(true, false, 2)));

        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert!(!out[0].picture.random_access);
    }

    #[test]
    fn a_field_only_sequence_is_shown_at_the_aspect_of_the_frame() {
        let spec = Spec {
            width: 1920,
            height: 544,
            window: Some((0, 0, 0, 2)),
            field_seq: true,
            progressive_source: false,
            interlaced_source: true,
            ..Spec::default()
        };
        let (splitter, _) = split(&nal(33, &sps(spec)));
        let format = splitter.format().expect("the sequence should be read");
        assert_eq!((format.width, format.height), (1920, 540));
        assert_eq!(format.aspect, (16, 9));
        assert!(format.field_pictures);
    }

    #[test]
    fn a_field_only_sequence_hands_over_whole_pictures() {
        let spec = Spec {
            width: 1920,
            height: 544,
            window: Some((0, 0, 0, 2)),
            field_seq: true,
            progressive_source: false,
            interlaced_source: true,
            ..Spec::default()
        };
        // Which field it is stays known, for weaving after decoding.
        for (pic_struct, field) in [(11, Structure::Top), (10, Structure::Bottom)] {
            let mut unit = aud();
            unit.extend(nal(33, &sps(spec)));
            unit.extend(nal(34, &pps(0)));
            unit.extend(nal(39, &picture_timing(pic_struct, 0)));
            unit.extend(nal(1, &slice(true, false, 2)));

            let (_, out) = split(&unit);
            assert_eq!(out.len(), 1, "pic_struct {pic_struct}");
            assert_eq!(
                out[0].picture.structure,
                Structure::Frame,
                "pic_struct {pic_struct}"
            );
            assert_eq!(
                out[0].picture.lone_field,
                Some(field),
                "pic_struct {pic_struct}"
            );
        }
    }

    #[test]
    fn the_scan_type_of_the_picture_beats_what_the_sequence_said() {
        let spec = Spec {
            progressive_source: false,
            interlaced_source: true,
            ..Spec::default()
        };
        let mut unit = aud();
        unit.extend(nal(33, &sps(spec)));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(39, &picture_timing(0, 1)));
        unit.extend(nal(1, &slice(true, false, 0)));

        let (_, out) = split(&unit);
        assert!(out[0].picture.progressive);
    }

    #[test]
    fn a_message_before_the_picture_timing_one_is_stepped_over() {
        let mut sei = vec![5u8, 4, 1, 2, 3, 4]; // user data unregistered
        sei.extend(picture_timing(3, 0));
        let mut unit = aud();
        unit.extend(nal(33, &sps(Spec::default())));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(39, &sei));
        unit.extend(nal(1, &slice(true, false, 0)));

        let (_, out) = split(&unit);
        assert_eq!(out[0].picture.structure, Structure::Frame);
        assert!(out[0].picture.top_field_first);
    }

    #[test]
    fn picture_timing_is_not_read_where_the_sequence_said_it_is_absent() {
        let spec = Spec {
            frame_field_info: false,
            ..Spec::default()
        };
        let mut unit = aud();
        unit.extend(nal(33, &sps(spec)));
        unit.extend(nal(34, &pps(0)));
        unit.extend(nal(39, &picture_timing(2, 0)));
        unit.extend(nal(1, &slice(true, false, 0)));

        let (_, out) = split(&unit);
        assert_eq!(out[0].picture.structure, Structure::Frame);
        assert!(out[0].picture.progressive);
    }

    #[test]
    fn a_truncated_sequence_parameter_set_is_not_believed() {
        let payload = sps(Spec::default());
        for len in 1..payload.len() {
            let mut splitter = Splitter::new(Framing::StartCodes);
            let mut out = Vec::new();
            splitter.push(&nal(33, &payload[..len]), &mut out);
            if let Some(format) = splitter.format() {
                assert!(format.width > 0 && format.height > 0);
            }
        }
        let mut splitter = Splitter::new(Framing::StartCodes);
        splitter.push(&nal(33, &payload[..8]), &mut Vec::new());
        assert!(splitter.format().is_none());
    }

    #[test]
    fn damaged_and_empty_units_give_back_nothing_rather_than_panicking() {
        for unit in [
            vec![],
            vec![0, 0, 1],
            vec![0, 0, 1, 0x42],
            vec![0xff; 64],
            vec![0, 0, 1, 0x42, 1],
            vec![0, 0, 0, 1, 0x46, 1, 0, 0, 0, 1],
        ] {
            let (splitter, out) = split(&unit);
            assert!(out.is_empty(), "{unit:?}");
            assert!(splitter.format().is_none(), "{unit:?}");
        }
    }

    #[test]
    fn a_picture_with_no_sequence_yet_is_still_reported() {
        let mut unit = aud();
        unit.extend(nal(1, &slice(true, false, 0)));
        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert!(out[0].format.is_none());
        assert!(!out[0].picture.carries_format);
    }

    #[test]
    fn a_sequence_declared_in_one_unit_reaches_the_picture_in_the_next() {
        let mut splitter = Splitter::new(Framing::StartCodes);
        let mut out = Vec::new();
        let mut first = nal(33, &sps(Spec::default()));
        first.extend(nal(34, &pps(0)));
        splitter.push(&first, &mut out);
        assert!(out.is_empty(), "parameter sets alone are not a picture");

        let mut second = aud();
        second.extend(nal(21, &slice(true, true, 2)));
        splitter.push(&second, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].picture.carries_format);
        assert_eq!(out[0].range, 0..second.len());
    }

    #[test]
    fn ranges_are_relative_to_the_unit_that_was_fed() {
        let mut splitter = Splitter::new(Framing::StartCodes);
        let mut out = Vec::new();
        let mut unit = aud();
        unit.extend(nal(1, &slice(true, false, 0)));
        splitter.push(&unit, &mut out);
        splitter.push(&unit, &mut out);
        assert_eq!(out[0].range, out[1].range);
    }

    #[test]
    fn every_intra_random_access_point_is_an_entry_and_nothing_else_is() {
        for kind in 0u8..=31 {
            let mut unit = aud();
            unit.extend(nal(33, &sps(Spec::default())));
            unit.extend(nal(34, &pps(0)));
            unit.extend(nal(kind, &slice(true, (16..=23).contains(&kind), 2)));
            let (_, out) = split(&unit);
            assert_eq!(out.len(), 1, "type {kind}");
            assert_eq!(
                out[0].picture.random_access,
                (16..=23).contains(&kind),
                "type {kind}"
            );
        }
    }

    #[test]
    fn the_leading_pictures_of_an_entry_point_are_marked() {
        for kind in 0u8..=31 {
            let mut unit = aud();
            unit.extend(nal(33, &sps(Spec::default())));
            unit.extend(nal(34, &pps(0)));
            unit.extend(nal(kind, &slice(true, (16..=23).contains(&kind), 2)));
            let (_, out) = split(&unit);
            assert_eq!(out.len(), 1, "type {kind}");
            assert_eq!(
                out[0].picture.leading,
                (8..=9).contains(&kind),
                "type {kind}"
            );
        }
    }

    #[test]
    fn ratios_come_out_in_their_lowest_terms() {
        assert_eq!(reduce(120_000, 2002), (60000, 1001));
        assert_eq!(reduce(3840, 2160), (16, 9));
        assert_eq!(reduce(0, 30), (0, 1));
        assert_eq!(reduce(30, 0), (0, 1));
    }

    #[test]
    fn reserved_picture_structures_are_read_as_a_frame() {
        for value in [7, 8, 13, 14, 15] {
            assert_eq!(from_pic_struct(value).0, Structure::Frame);
        }
        assert_eq!(from_pic_struct(9).0, Structure::Top);
        assert_eq!(from_pic_struct(12).0, Structure::Bottom);
    }
}
