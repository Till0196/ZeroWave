// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MPEG-2 video: where one coded picture begins, and what the headers over it say about it.
//! Without a picture coding extension (MPEG-1) frames are progressive.

use crate::bits::Reader;
use crate::es::{Access, Coding, Format, Picture, StartCodes, Structure, ratio};

const PICTURE: u8 = 0x00;
/// Hands on a picture, unless no sequence header has arrived yet.
fn give(open: Open, end: usize, out: &mut Vec<Access>) {
    if open.format.is_some() {
        out.push(open.close(end));
    }
}

const SEQUENCE: u8 = 0xb3;
const EXTENSION: u8 = 0xb5;
const GROUP: u8 = 0xb8;

const SEQUENCE_EXTENSION: u32 = 1;
const SEQUENCE_DISPLAY_EXTENSION: u32 = 2;
const PICTURE_CODING_EXTENSION: u32 = 8;

#[derive(Debug, Default)]
pub struct Splitter {
    sequence: Option<Sequence>,
    /// A sequence header since the last picture header; kept across calls (packets).
    sequence_stands: bool,
    closed_gop: Option<bool>,
}

impl Splitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A picture cut across two units is reported as far as the first one went.
    pub fn push(&mut self, unit: &[u8], out: &mut Vec<Access>) {
        let mut begins: Option<usize> = None;
        let mut open: Option<Open> = None;

        for (kind, body) in StartCodes::new(unit) {
            let code = code_at(unit, body.start.saturating_sub(1));
            match kind {
                SEQUENCE => {
                    self.read_sequence(&unit[body]);
                    self.sequence_stands = true;
                    begins.get_or_insert(code);
                }
                GROUP => {
                    self.read_group(&unit[body]);
                    begins.get_or_insert(code);
                }
                EXTENSION => {
                    let bytes = &unit[body];
                    match u32::from(bytes.first().copied().unwrap_or(0) >> 4) {
                        SEQUENCE_EXTENSION => self.read_sequence_extension(bytes),
                        SEQUENCE_DISPLAY_EXTENSION => {
                            self.read_sequence_display_extension(bytes);
                        }
                        PICTURE_CODING_EXTENSION => {
                            if let Some(open) = open.as_mut() {
                                read_picture_coding_extension(&mut open.picture, bytes);
                            }
                        }
                        _ => {}
                    }
                }
                PICTURE => {
                    let at = begins.take().unwrap_or(code);
                    if let Some(previous) = open.take() {
                        give(previous, at, out);
                    }
                    open = Some(self.open(at, &unit[body]));
                }
                _ => {}
            }
        }
        if let Some(open) = open.take() {
            give(open, unit.len(), out);
        }
    }

    pub fn format(&self) -> Option<Format> {
        self.sequence.as_ref().map(Sequence::format)
    }

    /// Whether the last GOP said it was closed.
    pub fn closed_gop(&self) -> Option<bool> {
        self.closed_gop
    }

    fn open(&mut self, at: usize, bytes: &[u8]) -> Open {
        let mut reader = Reader::new(bytes);
        reader.skip(10); // temporal_reference
        let coding = match reader.bits(3) {
            1 => Coding::I,
            2 => Coding::P,
            3 => Coding::B,
            _ => Coding::Unknown,
        };
        let carries_format = self.sequence_stands;
        self.sequence_stands = false;
        Open {
            at,
            format: self.format(),
            picture: Picture {
                coding,
                structure: Structure::Frame,
                carries_format,
                random_access: carries_format && coding == Coding::I,
                leading: false,
                ..Picture::default()
            },
        }
    }

    fn read_sequence(&mut self, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        let width = reader.bits(12);
        let height = reader.bits(12);
        let aspect = reader.bits(4) as u8;
        let rate = reader.bits(4) as u8;
        // Starting afresh keeps a stale extension from outliving its header.
        self.sequence = Some(Sequence {
            width,
            height,
            aspect,
            display: None,
            rate,
            progressive: false,
            chroma: 1,
            rate_numerator: 1,
            rate_denominator: 1,
            colour: super::Colour::default(),
        });
    }

    fn read_sequence_extension(&mut self, bytes: &[u8]) {
        let Some(sequence) = self.sequence.as_mut() else {
            return;
        };
        let mut reader = Reader::new(bytes);
        reader.skip(4 + 8); // the identifier, and profile and level
        sequence.progressive = reader.bit() == 1;
        let chroma = reader.bits(2) as u8;
        if chroma != 0 {
            sequence.chroma = chroma;
        }
        sequence.width |= reader.bits(2) << 12;
        sequence.height |= reader.bits(2) << 12;
        reader.skip(12 + 1 + 8 + 1); // bit rate, marker, buffer size, low delay
        sequence.rate_numerator = reader.bits(2) + 1;
        sequence.rate_denominator = reader.bits(5) + 1;
    }

    /// It changes what `aspect_ratio_information` means (clause 6.3.6, ARIB STD-B32 Part 1
    /// Table 5-2): a 4:3 programme in 1440x1080 sends display size 1080x1080.
    fn read_sequence_display_extension(&mut self, bytes: &[u8]) {
        let Some(sequence) = self.sequence.as_mut() else {
            return;
        };
        let mut reader = Reader::new(bytes);
        reader.skip(4 + 3); // the identifier, and video_format
        if reader.bit() == 1 {
            // STD-B32 note 5: an absent colour description is read as BT.709.
            sequence.colour.primaries = reader.bits(8) as u8;
            sequence.colour.transfer = reader.bits(8) as u8;
            sequence.colour.matrix = reader.bits(8) as u8;
        }
        let width = reader.bits(14);
        reader.skip(1); // marker_bit
        let height = reader.bits(14);
        if width != 0 && height != 0 {
            sequence.display = Some((width, height));
        }
    }

    fn read_group(&mut self, bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        reader.skip(25); // time_code
        self.closed_gop = Some(reader.bit() == 1);
    }
}

/// An access unit being read; given up when the next one starts (the extension follows).
#[derive(Debug)]
struct Open {
    at: usize,
    picture: Picture,
    format: Option<Format>,
}

impl Open {
    fn close(self, end: usize) -> Access {
        Access {
            picture: self.picture,
            format: self.format,
            range: self.at..end.max(self.at),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Sequence {
    width: u32,
    height: u32,
    aspect: u8,
    /// The display area from the sequence display extension; the aspect is of this area.
    display: Option<(u32, u32)>,
    rate: u8,
    progressive: bool,
    chroma: u8,
    rate_numerator: u32,
    rate_denominator: u32,
    colour: super::Colour,
}

impl Sequence {
    fn format(&self) -> Format {
        let (numerator, denominator) = frame_rate(self.rate);
        Format {
            width: self.width,
            height: self.height,
            // MPEG-2 as broadcast here is Main profile, which is eight bits.
            depth: 8,
            chroma: self.chroma,
            rate: ratio(
                numerator * self.rate_numerator,
                denominator * self.rate_denominator,
            ),
            progressive: self.progressive,
            aspect: aspect(self.aspect, self.width, self.height, self.display),
            field_pictures: false,
            woven: false,
            colour: self.colour,
        }
    }
}

fn read_picture_coding_extension(picture: &mut Picture, bytes: &[u8]) {
    let mut reader = Reader::new(bytes);
    reader.skip(4 + 16 + 2); // identifier, the four f codes, intra dc precision
    picture.structure = match reader.bits(2) {
        1 => Structure::Top,
        2 => Structure::Bottom,
        _ => Structure::Frame,
    };
    let top_field_first = reader.bit() == 1;
    reader.skip(5); // frame_pred_frame_dct through alternate_scan
    picture.repeat_first_field = reader.bit() == 1;
    reader.skip(1); // chroma_420_type
    picture.progressive = reader.bit() == 1;
    // top_field_first is nought on a field picture by definition.
    picture.top_field_first = top_field_first && !picture.structure.field();
}

/// Where the start code begins, taking the whole run of zeroes so ranges abut.
fn code_at(bytes: &[u8], kind_at: usize) -> usize {
    let mut at = kind_at.saturating_sub(1);
    while at > 0 && bytes[at - 1] == 0 {
        at -= 1;
    }
    at
}

fn frame_rate(code: u8) -> (u32, u32) {
    match code {
        1 => (24000, 1001),
        2 => (24, 1),
        3 => (25, 1),
        4 => (30000, 1001),
        5 => (30, 1),
        6 => (50, 1),
        7 => (60000, 1001),
        8 => (60, 1),
        _ => (0, 1),
    }
}

/// The display aspect. A sequence display extension makes it the aspect of that area
/// (clause 6.3.6).
fn aspect(code: u8, width: u32, height: u32, display: Option<(u32, u32)>) -> (u32, u32) {
    if code == 1 {
        return ratio(width, height);
    }
    let shown = match code {
        2 => (4, 3),
        3 => (16, 9),
        4 => (221, 100),
        _ => return (0, 1),
    };
    let Some((display_width, display_height)) = display else {
        return shown;
    };
    let (sample_width, sample_height) = ratio(shown.0 * display_height, shown.1 * display_width);
    ratio(width * sample_width, height * sample_height)
}

#[cfg(test)]
mod tests {
    use super::Splitter;
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
    }

    fn header(kind: u8, write: impl FnOnce(&mut Bits)) -> Vec<u8> {
        let mut bits = Bits::default();
        write(&mut bits);
        let mut out = vec![0, 0, 1, kind];
        out.extend_from_slice(&bits.bytes);
        out
    }

    fn sequence(width: u32, height: u32, aspect: u32, rate: u32, progressive: u32) -> Vec<u8> {
        let mut out = header(0xb3, |bits| {
            bits.bits(12, width);
            bits.bits(12, height);
            bits.bits(4, aspect);
            bits.bits(4, rate);
            bits.bits(18, 0x12345); // bit_rate_value
            bits.bit(1); // marker_bit
            bits.bits(10, 112); // vbv_buffer_size_value
            bits.bit(0); // constrained_parameters_flag
            bits.bit(0); // load_intra_quantiser_matrix
            bits.bit(0); // load_non_intra_quantiser_matrix
        });
        out.extend(header(0xb5, |bits| {
            bits.bits(4, 1); // sequence extension
            bits.bits(8, 0x48); // profile_and_level_indication
            bits.bit(progressive);
            bits.bits(2, 1); // chroma_format, 4:2:0
            bits.bits(2, 0); // horizontal_size_extension
            bits.bits(2, 0); // vertical_size_extension
            bits.bits(12, 0); // bit_rate_extension
            bits.bit(1); // marker_bit
            bits.bits(8, 0); // vbv_buffer_size_extension
            bits.bit(0); // low_delay
            bits.bits(2, 0); // frame_rate_extension_n
            bits.bits(5, 0); // frame_rate_extension_d
        }));
        out
    }

    fn display(width: u32, height: u32) -> Vec<u8> {
        header(0xb5, |bits| {
            bits.bits(4, 2); // sequence display extension
            bits.bits(3, 0); // video_format, component
            bits.bit(1); // colour_description
            bits.bits(8, 1); // colour_primaries
            bits.bits(8, 1); // transfer_characteristics
            bits.bits(8, 1); // matrix_coefficients
            bits.bits(14, width);
            bits.bit(1); // marker_bit
            bits.bits(14, height);
        })
    }

    fn group(closed: u32) -> Vec<u8> {
        header(0xb8, |bits| {
            bits.bits(25, 0x0123456);
            bits.bit(closed);
            bits.bit(0); // broken_link
        })
    }

    fn picture(coding: u32, structure: u32, tff: u32, rff: u32, progressive: u32) -> Vec<u8> {
        let mut out = header(0x00, |bits| {
            bits.bits(10, 3); // temporal_reference
            bits.bits(3, coding);
            bits.bits(16, 0xffff); // vbv_delay
            bits.bit(0); // extra_bit_picture
        });
        out.extend(header(0xb5, |bits| {
            bits.bits(4, 8); // picture coding extension
            bits.bits(16, 0x4444); // the four f codes
            bits.bits(2, 0); // intra_dc_precision
            bits.bits(2, structure);
            bits.bit(tff);
            bits.bit(0); // frame_pred_frame_dct
            bits.bit(0); // concealment_motion_vectors
            bits.bit(0); // q_scale_type
            bits.bit(0); // intra_vlc_format
            bits.bit(1); // alternate_scan
            bits.bit(rff);
            bits.bit(progressive); // chroma_420_type, which must match
            bits.bit(progressive);
            bits.bit(0); // composite_display_flag
        }));
        out.extend([0, 0, 1, 0x01, 0x12, 0x34, 0x56]); // one slice
        out
    }

    fn split(unit: &[u8]) -> (Splitter, Vec<Access>) {
        let mut splitter = Splitter::new();
        let mut out = Vec::new();
        splitter.push(unit, &mut out);
        (splitter, out)
    }

    #[test]
    fn a_frame_picture_is_read_whole_and_carries_its_format() {
        let mut unit = sequence(1440, 1080, 3, 4, 0);
        unit.extend(group(0));
        unit.extend(picture(1, 3, 1, 0, 0));
        let (splitter, out) = split(&unit);

        assert_eq!(out.len(), 1);
        let access = &out[0];
        assert_eq!(access.range, 0..unit.len());
        assert_eq!(access.picture.coding, Coding::I);
        assert_eq!(access.picture.structure, Structure::Frame);
        assert!(access.picture.top_field_first);
        assert!(!access.picture.repeat_first_field);
        assert!(!access.picture.progressive);
        assert!(access.picture.carries_format);
        assert!(access.picture.random_access);
        assert_eq!(splitter.closed_gop(), Some(false));

        let format = splitter.format().expect("a sequence header was read");
        assert_eq!((format.width, format.height), (1440, 1080));
        assert_eq!(format.aspect, (16, 9));
        assert_eq!(format.rate, (30000, 1001));
        assert_eq!((format.depth, format.chroma), (8, 1));
        assert!(!format.progressive);
        assert_eq!(access.format, Some(format));
    }

    #[test]
    fn a_top_field_and_the_bottom_field_after_it_are_two_pictures() {
        let mut unit = sequence(1920, 1080, 3, 4, 0);
        unit.extend(picture(1, 1, 0, 0, 0));
        let second = unit.len();
        unit.extend(picture(2, 2, 0, 0, 0));
        let (_, out) = split(&unit);

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].picture.structure, Structure::Top);
        assert_eq!(out[0].picture.coding, Coding::I);
        assert!(out[0].picture.random_access);
        assert_eq!(out[1].picture.structure, Structure::Bottom);
        assert_eq!(out[1].picture.coding, Coding::P);
        assert!(!out[1].picture.carries_format);
        assert!(!out[1].picture.random_access);
        assert_eq!(out[0].range, 0..second);
        assert_eq!(out[1].range, second..unit.len());
    }

    #[test]
    fn top_field_first_is_not_read_on_a_field_picture() {
        let mut unit = sequence(1920, 1080, 3, 4, 0);
        unit.extend(picture(3, 1, 1, 0, 0));
        let (_, out) = split(&unit);
        assert_eq!(out[0].picture.structure, Structure::Top);
        assert!(!out[0].picture.top_field_first);
    }

    #[test]
    fn repeat_first_field_and_progressive_frame_come_out_as_written() {
        let mut unit = sequence(720, 480, 2, 4, 1);
        unit.extend(picture(1, 3, 1, 1, 1));
        let (splitter, out) = split(&unit);
        assert!(out[0].picture.repeat_first_field);
        assert!(out[0].picture.progressive);
        let format = splitter.format().expect("a sequence header was read");
        assert!(format.progressive);
        assert_eq!(format.aspect, (4, 3));
    }

    #[test]
    fn square_samples_make_the_aspect_the_shape_of_the_picture() {
        let mut unit = sequence(720, 480, 1, 5, 0);
        unit.extend(picture(1, 3, 1, 0, 0));
        let (splitter, _) = split(&unit);
        let format = splitter.format().expect("a sequence header was read");
        assert_eq!(format.aspect, (3, 2));
        assert_eq!(format.rate, (30, 1));
    }

    #[test]
    fn a_display_area_makes_the_aspect_the_aspect_of_that_area() {
        // A 4:3 programme with side panels (ARIB STD-B32 Part 1 Table 5-2, row two).
        let mut unit = sequence(1440, 1080, 2, 4, 0);
        unit.extend(display(1080, 1080));
        unit.extend(picture(1, 3, 1, 0, 0));
        let (splitter, out) = split(&unit);
        let format = splitter.format().expect("a sequence header was read");
        assert_eq!((format.width, format.height), (1440, 1080));
        assert_eq!(format.aspect, (16, 9));
        assert_eq!(out[0].format, Some(format));
        assert_eq!(
            (
                format.colour.primaries,
                format.colour.transfer,
                format.colour.matrix
            ),
            (1, 1, 1)
        );
        assert!(!format.colour.full_range);
    }

    #[test]
    fn a_display_area_the_size_of_the_frame_leaves_the_aspect_alone() {
        let mut unit = sequence(1440, 1080, 3, 4, 0);
        unit.extend(display(1440, 1080));
        unit.extend(picture(1, 3, 1, 0, 0));
        let (splitter, _) = split(&unit);
        assert_eq!(splitter.format().expect("read").aspect, (16, 9));
    }

    #[test]
    fn a_display_area_does_not_outlive_the_sequence_header_that_carried_it() {
        let mut unit = sequence(1440, 1080, 2, 4, 0);
        unit.extend(display(1080, 1080));
        unit.extend(picture(1, 3, 1, 0, 0));
        let (mut splitter, mut out) = split(&unit);
        assert_eq!(splitter.format().expect("read").aspect, (16, 9));

        let mut next = sequence(1440, 1080, 2, 4, 0);
        next.extend(picture(1, 3, 1, 0, 0));
        splitter.push(&next, &mut out);
        assert_eq!(splitter.format().expect("read").aspect, (4, 3));
    }

    #[test]
    fn a_picture_with_no_coding_extension_is_a_frame() {
        let mut unit = sequence(720, 480, 2, 4, 0);
        unit.extend([0, 0, 1, 0x00, 0x00, 0xc8, 0xff, 0xff, 0x80]);
        let (_, out) = split(&unit);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].picture.structure, Structure::Frame);
        assert_eq!(out[0].picture.coding, Coding::I);
    }

    #[test]
    fn a_sequence_header_in_one_unit_still_stands_over_the_next() {
        let mut splitter = Splitter::new();
        let mut out = Vec::new();
        splitter.push(&sequence(1440, 1080, 3, 4, 0), &mut out);
        assert!(out.is_empty());
        splitter.push(&picture(1, 3, 1, 0, 0), &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].picture.carries_format);
        assert!(out[0].picture.random_access);
        assert_eq!(out[0].format.map(|format| format.width), Some(1440));
    }

    #[test]
    fn a_picture_that_is_not_intra_is_not_an_entry_point() {
        let mut unit = sequence(1440, 1080, 3, 4, 0);
        unit.extend(picture(2, 3, 1, 0, 0));
        let (_, out) = split(&unit);
        assert!(out[0].picture.carries_format);
        assert!(!out[0].picture.random_access);
    }

    #[test]
    fn a_unit_cut_short_anywhere_gives_back_what_it_held() {
        let mut unit = sequence(1440, 1080, 3, 4, 0);
        unit.extend(group(1));
        unit.extend(picture(1, 3, 1, 0, 0));
        unit.extend(picture(3, 3, 1, 0, 0));
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
        assert!(split(&[0, 0, 1, 0xb3]).1.is_empty());
    }
}
