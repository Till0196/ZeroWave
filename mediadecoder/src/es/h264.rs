// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! H.264 video: where one access unit begins, and what the SPS, slice header and picture
//! timing SEI say about it. 124/128° CS codes every picture as a field and sends no IDR.

use crate::bits::{Reader, unescape};
use crate::es::{Access, Coding, Format, Framing, Nals, Picture, Structure, ratio};

const SLICE: u8 = 1;
const IDR: u8 = 5;
const SEI: u8 = 6;
const SEQUENCE: u8 = 7;

const SEQUENCE_BYTES: usize = 512;
const SLICE_BYTES: usize = 32;
const SEI_BYTES: usize = 256;

const PICTURE_TIMING: u32 = 1;

/// Recovery point: the only entry point 124/128° CS gives (it sends no IDR).
const RECOVERY_POINT: u32 = 6;

#[derive(Debug, Default)]
pub struct Splitter {
    sequence: Option<Sequence>,
    framing: Framing,
}

impl Splitter {
    pub fn new(framing: Framing) -> Self {
        Self {
            sequence: None,
            framing,
        }
    }

    pub fn push(&mut self, unit: &[u8], out: &mut Vec<Access>) {
        // Where the current access unit began in this unit; one begun in an earlier unit is let go.
        let mut begins: Option<usize> = None;
        let mut open: Option<Open> = None;

        for nal in Nals::new(unit, self.framing) {
            let kind = nal.header & 0x1f;
            let at = nal.at;
            let bytes = &unit[nal.body];

            // Which units may head an access unit (only once the current one holds a picture).
            let heads = match kind {
                6 | 7 | 8 | 9 | 13 | 14 | 15 => true,
                SLICE | IDR => starts_a_picture(bytes),
                _ => false,
            };
            if heads
                && open.as_ref().is_some_and(Open::has_picture)
                && let Some(previous) = open.take()
                && let Some(from) = begins
            {
                out.push(previous.close(from, at, self.format()));
            }
            if open.is_none() {
                open = Some(Open::default());
                begins = Some(at);
            }
            let Some(open) = open.as_mut() else {
                continue;
            };

            match kind {
                SEQUENCE => {
                    self.sequence = Some(read_sequence(&unescape(bytes, SEQUENCE_BYTES)));
                    open.carries_format = true;
                }
                SEI => {
                    if let Some(sequence) = self.sequence.as_ref() {
                        let read = read_messages(&unescape(bytes, SEI_BYTES), sequence);
                        open.pic_struct = read.pic_struct.or(open.pic_struct);
                        open.recovery = open.recovery || read.recovery;
                    }
                }
                SLICE | IDR => {
                    if let Some(sequence) = self.sequence.as_ref()
                        && open.slice.is_none()
                        && starts_a_picture(bytes)
                    {
                        let mut slice = read_slice(&unescape(bytes, SLICE_BYTES), sequence);
                        slice.idr = kind == IDR;
                        open.slice = Some(slice);
                    }
                }
                _ => {}
            }
        }
        if let Some(open) = open.take()
            && let Some(from) = begins
            && open.has_picture()
        {
            out.push(open.close(from, unit.len(), self.format()));
        }
    }

    pub fn format(&self) -> Option<Format> {
        self.sequence.as_ref().map(Sequence::format)
    }
}

#[derive(Debug, Default)]
struct Open {
    slice: Option<Slice>,
    pic_struct: Option<u32>,
    recovery: bool,
    carries_format: bool,
}

impl Open {
    fn has_picture(&self) -> bool {
        self.slice.is_some()
    }

    fn close(self, from: usize, end: usize, format: Option<Format>) -> Access {
        let slice = self.slice.unwrap_or_default();
        let progressive = format.is_some_and(|format| format.progressive);
        Access {
            picture: Picture {
                coding: slice.coding,
                structure: slice.structure,
                // Field order comes from picture timing; with none, from the coded order.
                top_field_first: matches!(self.pic_struct, Some(3 | 5)),
                // Five and six are pull-down: the only way H.264 shows a field twice.
                repeat_first_field: matches!(self.pic_struct, Some(5 | 6)),
                // A frame in a stream that also codes fields is two fields woven, not progressive.
                progressive: !slice.structure.field() && progressive,
                random_access: slice.idr || self.recovery,
                // H.264 では NAL の型から判断できない。
                leading: false,
                carries_format: self.carries_format,
                lone_field: None,
            },
            format,
            range: from..end.max(from),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Slice {
    coding: Coding,
    structure: Structure,
    idr: bool,
}

fn starts_a_picture(bytes: &[u8]) -> bool {
    Reader::new(&unescape(bytes, SLICE_BYTES)).ue() == 0
}

fn read_slice(bytes: &[u8], sequence: &Sequence) -> Slice {
    let mut reader = Reader::new(bytes);
    reader.ue(); // first_mb_in_slice, known to be nought
    let kind = reader.ue();
    reader.ue(); // pic_parameter_set_id
    if sequence.separate_planes {
        reader.skip(2); // colour_plane_id
    }
    reader.skip(sequence.frame_number_bits);
    let mut structure = Structure::Frame;
    if !sequence.frame_only && reader.bit() == 1 {
        structure = if reader.bit() == 1 {
            Structure::Bottom
        } else {
            Structure::Top
        };
    }
    Slice {
        // Types 5..9 say every slice is of this type; switching slices act as P and I.
        coding: match kind % 5 {
            0 | 3 => Coding::P,
            1 => Coding::B,
            2 | 4 => Coding::I,
            _ => Coding::Unknown,
        },
        structure,
        idr: false,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Sei {
    /// `pic_struct`, where picture timing carried one and the SPS said it would.
    pic_struct: Option<u32>,
    recovery: bool,
}

/// Walks the messages of one SEI unit. The delays before `pic_struct` have the HRD's lengths.
fn read_messages(bytes: &[u8], sequence: &Sequence) -> Sei {
    let mut read = Sei::default();
    let mut at = 0;
    while at < bytes.len() {
        let Some(kind) = extend(bytes, &mut at) else {
            break;
        };
        let Some(size) = extend(bytes, &mut at) else {
            break;
        };
        let Some(end) = at.checked_add(size as usize) else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        match kind {
            PICTURE_TIMING if sequence.picture_timing => {
                let mut reader = Reader::new(&bytes[at..end]);
                if sequence.delays {
                    reader.skip(sequence.removal_delay_bits + sequence.output_delay_bits);
                }
                read.pic_struct = Some(reader.bits(4));
            }
            // Only its presence matters; `recovery_frame_cnt` is nought in broadcasts.
            RECOVERY_POINT => read.recovery = true,
            _ => {}
        }
        at = end;
    }
    read
}

/// A message's type or length: a run of 0xff bytes and then the remainder.
fn extend(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0;
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value += u32::from(byte);
        if byte != 0xff {
            return Some(value);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Sequence {
    width: u32,
    height: u32,
    depth: u8,
    chroma: u8,
    rate: (u32, u32),
    aspect: (u32, u32),
    frame_only: bool,
    separate_planes: bool,
    frame_number_bits: usize,
    picture_timing: bool,
    delays: bool,
    removal_delay_bits: usize,
    output_delay_bits: usize,
    colour: super::Colour,
}

impl Sequence {
    fn format(&self) -> Format {
        Format {
            width: self.width,
            height: self.height,
            depth: self.depth,
            chroma: self.chroma,
            rate: self.rate,
            progressive: self.frame_only,
            aspect: self.aspect,
            // A field pair is decoded into one full-height buffer, so a picture is always a frame.
            field_pictures: false,
            woven: false,
            colour: self.colour,
        }
    }
}

fn read_sequence(bytes: &[u8]) -> Sequence {
    let mut reader = Reader::new(bytes);
    let profile = reader.bits(8);
    reader.skip(8 + 8); // the constraint flags, and the level
    reader.ue(); // seq_parameter_set_id

    let mut chroma = 1;
    let mut depth = 8;
    let mut separate_planes = false;
    // Only these profiles write chroma format and depth; the rest are eight-bit 4:2:0.
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma = reader.ue();
        if chroma == 3 {
            separate_planes = reader.bit() == 1;
        }
        depth = reader.ue().saturating_add(8);
        reader.ue(); // bit_depth_chroma_minus8
        reader.skip(1); // qpprime_y_zero_transform_bypass_flag
        if reader.bit() == 1 {
            let lists = if chroma == 3 { 12 } else { 8 };
            for list in 0..lists {
                if reader.bit() == 0 {
                    continue;
                }
                let size = if list < 6 { 16 } else { 64 };
                let (mut last, mut next) = (8i32, 8i32);
                for _ in 0..size {
                    if next != 0 {
                        next = (last + reader.se()).rem_euclid(256);
                    }
                    if next != 0 {
                        last = next;
                    }
                }
            }
        }
    }

    let frame_number_bits = reader.ue() as usize + 4;
    match reader.ue() {
        0 => {
            reader.ue();
        }
        1 => {
            reader.skip(1);
            reader.se();
            reader.se();
            let count = reader.ue().min(256);
            for _ in 0..count {
                reader.se();
            }
        }
        _ => {}
    }
    reader.ue(); // max_num_ref_frames
    reader.skip(1); // gaps_in_frame_num_value_allowed_flag
    let width_mbs = u64::from(reader.ue()) + 1;
    let height_units = u64::from(reader.ue()) + 1;
    let frame_only = reader.bit() == 1;
    if !frame_only {
        reader.skip(1);
    }
    reader.skip(1); // direct_8x8_inference_flag

    // 1080 is not whole macroblocks: 1088 lines are coded and eight cropped.
    let mut width = width_mbs * 16;
    let mut height = height_units * 16 * if frame_only { 1 } else { 2 };
    // 上限を超えるものはサイズ 0（開けない）にする（[`crate::video::fits`]）。
    if !crate::video::fits(width, height) {
        (width, height) = (0, 0);
    }
    if reader.bit() == 1 {
        let (left, right) = (u64::from(reader.ue()), u64::from(reader.ue()));
        let (top, bottom) = (u64::from(reader.ue()), u64::from(reader.ue()));
        // The crop is counted in chroma samples, and vertically in fields where fields are coded.
        let (sub_width, sub_height) = subsampling(chroma);
        let (unit_x, unit_y) = if chroma == 0 || separate_planes {
            (1, 2 - u32::from(frame_only))
        } else {
            (sub_width, sub_height * (2 - u32::from(frame_only)))
        };
        width = width.saturating_sub(u64::from(unit_x) * (left + right));
        height = height.saturating_sub(u64::from(unit_y) * (top + bottom));
    }

    let mut sequence = Sequence {
        width: width as u32,
        height: height as u32,
        depth: depth.min(16) as u8,
        chroma: chroma.min(3) as u8,
        rate: (0, 1),
        aspect: (0, 1),
        frame_only,
        separate_planes,
        frame_number_bits,
        picture_timing: false,
        delays: false,
        removal_delay_bits: 0,
        output_delay_bits: 0,
        colour: super::Colour::default(),
    };
    if reader.bit() == 1 {
        read_usability(&mut reader, &mut sequence);
    }
    sequence
}

fn read_usability(reader: &mut Reader<'_>, sequence: &mut Sequence) {
    if reader.bit() == 1 {
        let idc = reader.bits(8);
        let (sample_width, sample_height) = if idc == 255 {
            (reader.bits(16), reader.bits(16))
        } else {
            sample_aspect(idc)
        };
        // H.264 names the sample shape; 1440x1080 with 4:3 samples is 16:9.
        if sample_width != 0 && sample_height != 0 {
            sequence.aspect = ratio(
                sequence.width * sample_width,
                sequence.height * sample_height,
            );
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
        reader.ue(); // the chroma sample positions
    }
    if reader.bit() == 1 {
        let units = reader.bits(32);
        let scale = reader.bits(32);
        reader.skip(1); // fixed_frame_rate_flag
        // The clock ticks twice a frame (it counts fields).
        if units != 0 && scale != 0 {
            sequence.rate = ratio(scale, units.saturating_mul(2));
        }
    }
    let nal = reader.bit() == 1;
    if nal {
        read_reference_decoder(reader, sequence);
    }
    let vcl = reader.bit() == 1;
    if vcl {
        read_reference_decoder(reader, sequence);
    }
    if nal || vcl {
        sequence.delays = true;
        reader.skip(1); // low_delay_hrd_flag
    }
    sequence.picture_timing = reader.bit() == 1;
}

fn read_reference_decoder(reader: &mut Reader<'_>, sequence: &mut Sequence) {
    let count = reader.ue().min(32) + 1;
    reader.skip(4 + 4); // the bit rate and buffer size scales
    for _ in 0..count {
        reader.ue();
        reader.ue();
        reader.skip(1);
    }
    reader.skip(5); // initial_cpb_removal_delay_length_minus1
    sequence.removal_delay_bits = reader.bits(5) as usize + 1;
    sequence.output_delay_bits = reader.bits(5) as usize + 1;
    reader.skip(5); // time_offset_length
}

fn subsampling(chroma: u32) -> (u32, u32) {
    match chroma {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    }
}

/// The sample shape from Table E-1 (14 is the one 1440x1080 uses).
fn sample_aspect(idc: u32) -> (u32, u32) {
    match idc {
        1 => (1, 1),
        2 => (12, 11),
        3 => (10, 11),
        4 => (16, 11),
        5 => (40, 33),
        6 => (24, 11),
        7 => (20, 11),
        8 => (32, 11),
        9 => (80, 33),
        10 => (18, 11),
        11 => (15, 11),
        12 => (64, 33),
        13 => (160, 99),
        14 => (4, 3),
        15 => (3, 2),
        16 => (2, 1),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::{Framing, Splitter};
    use crate::es::{Access, Coding, Structure};

    #[derive(Debug, Default)]
    struct Bits {
        bytes: Vec<u8>,
        at: usize,
    }

    impl Bits {
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

        fn ue(&mut self, value: u32) {
            let raised = value + 1;
            let width = 32 - raised.leading_zeros() as usize;
            self.bits(width - 1, 0);
            self.bits(width, raised);
        }

        fn stop(&mut self) {
            self.bit(1);
            while !self.at.is_multiple_of(8) {
                self.bit(0);
            }
        }
    }

    fn escape(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut zeroes = 0;
        for &byte in bytes {
            if zeroes == 2 && byte <= 3 {
                out.push(3);
                zeroes = 0;
            }
            zeroes = if byte == 0 { zeroes + 1 } else { 0 };
            out.push(byte);
        }
        out
    }

    fn nal(kind: u8, write: impl FnOnce(&mut Bits)) -> Vec<u8> {
        let mut bits = Bits::default();
        write(&mut bits);
        let mut out = vec![0, 0, 0, 1, 0x60 | kind];
        out.extend(escape(&bits.bytes));
        out
    }

    struct Shape {
        profile: u32,
        width_mbs: u32,
        height_units: u32,
        frame_only: bool,
        crop_bottom: u32,
        aspect: u32,
        timing: bool,
        picture_timing: bool,
        colour: Option<(u32, u32, u32)>,
    }

    impl Default for Shape {
        fn default() -> Self {
            Self {
                profile: 100,
                width_mbs: 90,
                height_units: 34,
                frame_only: false,
                crop_bottom: 2,
                aspect: 14,
                timing: true,
                picture_timing: true,
                colour: None,
            }
        }
    }

    fn sequence(shape: &Shape) -> Vec<u8> {
        nal(7, |bits| {
            bits.bits(8, shape.profile);
            bits.bits(8, 0); // the constraint flags
            bits.bits(8, 40); // level_idc
            bits.ue(0); // seq_parameter_set_id
            if shape.profile == 100 {
                bits.ue(1); // chroma_format_idc, 4:2:0
                bits.ue(0); // bit_depth_luma_minus8
                bits.ue(0); // bit_depth_chroma_minus8
                bits.bit(0); // qpprime_y_zero_transform_bypass_flag
                bits.bit(0); // seq_scaling_matrix_present_flag
            }
            bits.ue(4); // log2_max_frame_num_minus4
            bits.ue(0); // pic_order_cnt_type
            bits.ue(6); // log2_max_pic_order_cnt_lsb_minus4
            bits.ue(4); // max_num_ref_frames
            bits.bit(0); // gaps_in_frame_num_value_allowed_flag
            bits.ue(shape.width_mbs - 1);
            bits.ue(shape.height_units - 1);
            bits.bit(u32::from(shape.frame_only));
            if !shape.frame_only {
                bits.bit(0); // mb_adaptive_frame_field_flag
            }
            bits.bit(1); // direct_8x8_inference_flag
            bits.bit(u32::from(shape.crop_bottom != 0));
            if shape.crop_bottom != 0 {
                bits.ue(0);
                bits.ue(0);
                bits.ue(0);
                bits.ue(shape.crop_bottom);
            }
            bits.bit(1); // vui_parameters_present_flag
            bits.bit(u32::from(shape.aspect != 0));
            if shape.aspect != 0 {
                bits.bits(8, shape.aspect);
            }
            bits.bit(0); // overscan_info_present_flag
            bits.bit(u32::from(shape.colour.is_some())); // video_signal_type_present_flag
            if let Some((primaries, transfer, matrix)) = shape.colour {
                bits.bits(3, 0); // video_format, component
                bits.bit(0); // video_full_range_flag
                bits.bit(1); // colour_description_present_flag
                bits.bits(8, primaries);
                bits.bits(8, transfer);
                bits.bits(8, matrix);
            }
            bits.bit(0); // chroma_loc_info_present_flag
            bits.bit(u32::from(shape.timing));
            if shape.timing {
                bits.bits(32, 1001); // num_units_in_tick
                bits.bits(32, 60000); // time_scale
                bits.bit(1); // fixed_frame_rate_flag
            }
            bits.bit(0); // nal_hrd_parameters_present_flag
            bits.bit(0); // vcl_hrd_parameters_present_flag
            bits.bit(u32::from(shape.picture_timing));
            bits.bit(0); // bitstream_restriction_flag
            bits.stop();
        })
    }

    fn timing(pic_struct: u32) -> Vec<u8> {
        nal(6, |bits| {
            bits.bits(8, 1); // picture timing
            bits.bits(8, 1); // one byte of it
            bits.bits(4, pic_struct);
            bits.bits(4, 0);
            bits.stop();
        })
    }

    fn slice(kind: u8, coding: u32, field: Option<u32>) -> Vec<u8> {
        nal(kind, |bits| {
            bits.ue(0); // first_mb_in_slice
            bits.ue(coding + 5); // slice_type, said of every slice
            bits.ue(0); // pic_parameter_set_id
            bits.bits(8, 7); // frame_num
            if let Some(bottom) = field {
                bits.bit(1); // field_pic_flag
                bits.bit(bottom);
            } else {
                bits.bit(0);
            }
            bits.bits(32, 0x2244_6688); // whatever the picture is
            bits.stop();
        })
    }

    fn split(unit: &[u8]) -> (Splitter, Vec<Access>) {
        let mut splitter = Splitter::new(Framing::StartCodes);
        let mut out = Vec::new();
        splitter.push(unit, &mut out);
        (splitter, out)
    }

    #[test]
    fn a_1440_stream_is_shown_at_16_9_and_is_1080_after_the_crop() {
        let (splitter, _) = split(&sequence(&Shape::default()));
        let format = splitter.format().expect("a parameter set was read");
        assert_eq!((format.width, format.height), (1440, 1080));
        assert_eq!(format.aspect, (16, 9));
        assert_eq!(format.rate, (30000, 1001));
        assert_eq!((format.depth, format.chroma), (8, 1));
        assert!(!format.progressive);
    }

    #[test]
    fn a_top_field_and_the_bottom_field_after_it_are_two_access_units() {
        // Every picture a field and no IDR: neither may be dropped.
        let mut unit = sequence(&Shape::default());
        unit.extend(timing(1));
        unit.extend(slice(1, 2, Some(0)));
        let second = unit.len();
        unit.extend(timing(2));
        unit.extend(slice(1, 0, Some(1)));
        let (_, out) = split(&unit);

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].picture.structure, Structure::Top);
        assert_eq!(out[0].picture.coding, Coding::I);
        assert!(out[0].picture.carries_format);
        assert!(!out[0].picture.random_access);
        assert!(!out[0].picture.progressive);
        assert_eq!(out[1].picture.structure, Structure::Bottom);
        assert_eq!(out[1].picture.coding, Coding::P);
        assert!(!out[1].picture.carries_format);
        assert_eq!(out[0].range, 0..second);
        assert_eq!(out[1].range, second..unit.len());
    }

    #[test]
    fn a_refresh_in_a_frame_stream_is_where_a_decoder_may_begin() {
        let shape = Shape {
            profile: 66,
            width_mbs: 20,
            height_units: 12,
            frame_only: true,
            crop_bottom: 0,
            aspect: 0,
            timing: true,
            picture_timing: false,
            colour: None,
        };
        let mut unit = sequence(&shape);
        unit.extend(slice(5, 2, None));
        let (splitter, out) = split(&unit);

        let format = splitter.format().expect("a parameter set was read");
        assert_eq!((format.width, format.height), (320, 192));
        assert!(format.progressive);
        assert_eq!(format.aspect, (0, 1));
        assert_eq!(format.colour, super::super::Colour::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].picture.structure, Structure::Frame);
        assert!(out[0].picture.progressive);
        assert!(out[0].picture.random_access);
        assert!(out[0].picture.carries_format);
    }

    /// The colour description sits between aspect and timing; misreading it moves the frame rate.
    #[test]
    fn the_colour_description_is_read_and_the_timing_behind_it_still_is() {
        let shape = Shape {
            colour: Some((1, 1, 1)),
            ..Shape::default()
        };
        let (splitter, _) = split(&sequence(&shape));
        let format = splitter.format().expect("a parameter set was read");
        assert_eq!(format.colour.primaries, 1);
        assert_eq!(format.colour.transfer, 1);
        assert_eq!(format.colour.matrix, 1);
        assert!(!format.colour.is_hdr());
        assert_eq!(format.rate, (30000, 1001));
    }

    #[test]
    fn picture_timing_is_what_says_a_field_is_shown_twice() {
        for (pic_struct, top_first, repeat) in [
            (0, false, false),
            (3, true, false),
            (4, false, false),
            (5, true, true),
            (6, false, true),
        ] {
            let mut unit = sequence(&Shape::default());
            unit.extend(timing(pic_struct));
            unit.extend(slice(1, 2, None));
            let (_, out) = split(&unit);
            assert_eq!(out.len(), 1, "pic_struct {pic_struct}");
            assert_eq!(
                out[0].picture.top_field_first, top_first,
                "pic_struct {pic_struct}"
            );
            assert_eq!(
                out[0].picture.repeat_first_field, repeat,
                "pic_struct {pic_struct}"
            );
        }
    }

    #[test]
    fn a_picture_in_several_slices_is_one_access_unit() {
        let mut unit = sequence(&Shape::default());
        unit.extend(slice(1, 2, Some(0)));
        unit.extend(nal(1, |bits| {
            bits.ue(120); // first_mb_in_slice, so not a picture of its own
            bits.ue(7);
            bits.ue(0);
            bits.bits(8, 7);
            bits.bit(1);
            bits.bit(0);
            bits.stop();
        }));
        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].range, 0..unit.len());
    }

    #[test]
    fn a_parameter_set_in_one_unit_still_stands_over_the_next() {
        let mut splitter = Splitter::new(Framing::StartCodes);
        let mut out = Vec::new();
        splitter.push(&sequence(&Shape::default()), &mut out);
        assert!(out.is_empty());
        splitter.push(&slice(1, 1, Some(0)), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].picture.coding, Coding::B);
        assert!(!out[0].picture.carries_format);
        assert_eq!(out[0].format.map(|format| format.width), Some(1440));
    }

    #[test]
    fn a_slice_with_no_parameter_set_before_it_is_not_a_picture() {
        let (_, out) = split(&slice(1, 2, None));
        assert!(out.is_empty());
    }

    #[test]
    fn a_unit_cut_short_anywhere_gives_back_what_it_held() {
        let mut unit = sequence(&Shape::default());
        unit.extend(timing(1));
        unit.extend(slice(1, 2, Some(0)));
        unit.extend(timing(2));
        unit.extend(slice(1, 0, Some(1)));
        for end in 0..unit.len() {
            let (_, out) = split(&unit[..end]);
            for access in &out {
                assert!(access.range.end <= end);
                assert!(access.range.start <= access.range.end);
            }
        }
    }

    #[test]
    fn a_unit_that_is_not_a_stream_at_all_gives_nothing() {
        assert!(split(&[]).1.is_empty());
        assert!(split(&[0xff; 32]).1.is_empty());
        assert!(split(&[0, 0, 1, 0x65]).1.is_empty());
    }
}
