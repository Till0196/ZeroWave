// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What an elementary stream says about itself: where pictures begin and end, frame or field,
//! which field first, and where a decoder may start. Nothing here depends on decoding.

pub mod h264;
pub mod hevc;
pub mod mpeg2;

/// Which coding an elementary stream is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    Mpeg2,
    H264,
    Hevc,
}

impl Codec {
    pub fn of_stream_type(stream_type: u8) -> Option<Self> {
        match stream_type {
            0x01 | 0x02 => Some(Self::Mpeg2),
            0x1b => Some(Self::H264),
            0x24 => Some(Self::Hevc),
            _ => None,
        }
    }

    pub fn of_asset(kind: &str) -> Option<Self> {
        match kind {
            "hev1" | "hvc1" => Some(Self::Hevc),
            "avc1" | "avc3" => Some(Self::H264),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Structure {
    #[default]
    Frame,
    Top,
    Bottom,
}

impl Structure {
    pub fn field(self) -> bool {
        self != Self::Frame
    }
}

/// What a picture is coded as. [`Coding::Unknown`] where the stream does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coding {
    #[default]
    Unknown,
    I,
    P,
    B,
}

/// What one access unit turned out to be (the bytes are not here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Picture {
    pub coding: Coding,
    pub structure: Structure,
    pub top_field_first: bool,
    /// Whether the first field is shown again after the second (film at broadcast rates).
    pub repeat_first_field: bool,
    pub progressive: bool,
    pub random_access: bool,
    /// Decoded after a random access point but shown before it; a decoder that began there must
    /// not decode them (HEVC RASL, H.265 8.1.3).
    pub leading: bool,
    /// Whether the format was declared at or before this picture.
    pub carries_format: bool,
    /// In a field-only sequence, which field this picture is (from `pic_struct`). `structure`
    /// stays `Frame` there: each field is a whole picture, woven after decoding.
    pub lone_field: Option<Structure>,
}

/// What the stream says its pictures are, from the sequence header or SPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    /// The chroma format as the standards number it: 1 is 4:2:0, 2 is 4:2:2, 3 is 4:4:4.
    pub chroma: u8,
    pub rate: (u32, u32),
    pub progressive: bool,
    /// The display aspect of the whole picture (of the frame, in a field-only sequence).
    pub aspect: (u32, u32),
    /// Every coded picture is one field alone at half height (HEVC `field_seq_flag`). The player,
    /// not the decoder, weaves two after decoding ([`Format::woven`]).
    pub field_pictures: bool,
    /// Two field-only pictures woven into this one after decoding, to be deinterlaced as usual.
    pub woven: bool,
    pub colour: Colour,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            depth: 8,
            chroma: 1,
            rate: (0, 1),
            progressive: false,
            aspect: (0, 1),
            field_pictures: false,
            woven: false,
            colour: Colour::default(),
        }
    }
}

/// How the samples are read as colour: ITU-T H.273 code points (ARIB STD-B32 Part 1
/// Table 4-6). Two means unspecified; choosing a default is not this crate's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    /// `video_full_range_flag` (MPEG-2 has none, so false there).
    pub full_range: bool,
}

pub const UNSPECIFIED: u8 = 2;

impl Default for Colour {
    fn default() -> Self {
        Self {
            primaries: UNSPECIFIED,
            transfer: UNSPECIFIED,
            matrix: UNSPECIFIED,
            full_range: false,
        }
    }
}

impl Colour {
    /// BT.2100: `transfer_characteristics` of 16 (PQ) or 18 (HLG).
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, 16 | 18)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub picture: Picture,
    pub format: Option<Format>,
    pub range: std::ops::Range<usize>,
}

/// How the units of a stream are separated: start codes (TS) or four byte lengths (MMT).
/// A property of the transmission; it is not guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Framing {
    #[default]
    StartCodes,
    Lengths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nal {
    pub header: u8,
    /// Where the unit begins, counting the start code or length, so access units abut.
    pub at: usize,
    pub body: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Nals<'a> {
    bytes: &'a [u8],
    framing: Framing,
    at: usize,
}

impl<'a> Nals<'a> {
    pub fn new(bytes: &'a [u8], framing: Framing) -> Self {
        Self {
            bytes,
            framing,
            at: 0,
        }
    }
}

impl Iterator for Nals<'_> {
    type Item = Nal;

    fn next(&mut self) -> Option<Self::Item> {
        match self.framing {
            Framing::StartCodes => {
                let header = find_start_code(self.bytes, self.at)?;
                let end = find_start_code(self.bytes, header + 1)
                    .map_or(self.bytes.len(), |next| trim_zeroes(self.bytes, next));
                self.at = header + 1;
                Some(Nal {
                    header: *self.bytes.get(header)?,
                    at: trim_zeroes(self.bytes, header),
                    body: header + 1..end.max(header + 1),
                })
            }
            Framing::Lengths => {
                let at = self.at;
                let start = at + 4;
                if start > self.bytes.len() {
                    return None;
                }
                let length = u32::from_be_bytes(self.bytes[at..start].try_into().ok()?) as usize;
                let end = start.saturating_add(length).min(self.bytes.len());
                if end <= start {
                    return None;
                }
                self.at = end;
                Some(Nal {
                    header: self.bytes[start],
                    at,
                    body: start + 1..end,
                })
            }
        }
    }
}

/// Cuts an elementary stream into access units and reads what each one is. Gives ranges
/// into what was fed, so pictures reach a decoder without a copy.
#[derive(Debug)]
pub enum Splitter {
    Mpeg2(mpeg2::Splitter),
    H264(h264::Splitter),
    Hevc(hevc::Splitter),
}

impl Splitter {
    /// MPEG-2 always uses start codes; `framing` is ignored for it.
    pub fn new(codec: Codec, framing: Framing) -> Self {
        match codec {
            Codec::Mpeg2 => Self::Mpeg2(mpeg2::Splitter::new()),
            Codec::H264 => Self::H264(h264::Splitter::new(framing)),
            Codec::Hevc => Self::Hevc(hevc::Splitter::new(framing)),
        }
    }

    /// Feeds one PES payload or MMT access unit and appends the access units it held.
    pub fn push(&mut self, unit: &[u8], out: &mut Vec<Access>) {
        match self {
            Self::Mpeg2(splitter) => splitter.push(unit, out),
            Self::H264(splitter) => splitter.push(unit, out),
            Self::Hevc(splitter) => splitter.push(unit, out),
        }
    }

    pub fn format(&self) -> Option<Format> {
        match self {
            Self::Mpeg2(splitter) => splitter.format(),
            Self::H264(splitter) => splitter.format(),
            Self::Hevc(splitter) => splitter.format(),
        }
    }
}

/// Walks the start codes of a byte stream: `(the byte after the code, what follows)`.
#[derive(Debug, Clone)]
pub struct StartCodes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> StartCodes<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
}

impl Iterator for StartCodes<'_> {
    type Item = (u8, std::ops::Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        let start = find_start_code(self.bytes, self.at)?;
        let kind = *self.bytes.get(start)?;
        let end = find_start_code(self.bytes, start + 1)
            .map_or(self.bytes.len(), |next| trim_zeroes(self.bytes, next));
        self.at = start + 1;
        Some((kind, start + 1..end.max(start + 1)))
    }
}

fn find_start_code(bytes: &[u8], at: usize) -> Option<usize> {
    let mut zeroes = 0;
    for (offset, &byte) in bytes.iter().enumerate().skip(at) {
        match byte {
            0 => zeroes += 1,
            1 if zeroes >= 2 => return Some(offset + 1),
            _ => zeroes = 0,
        }
    }
    None
}

fn trim_zeroes(bytes: &[u8], end: usize) -> usize {
    let mut at = end.saturating_sub(2);
    while at > 0 && bytes[at - 1] == 0 {
        at -= 1;
    }
    at
}

/// A ratio in lowest terms (60000/2002 equals 30000/1001). HEVC's `reduce` is separate.
pub(crate) fn ratio(numerator: u32, denominator: u32) -> (u32, u32) {
    match gcd(numerator, denominator) {
        0 => (0, 1),
        divisor => (numerator / divisor, denominator / divisor),
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::{Codec, Framing, Nal, Nals, StartCodes};

    #[test]
    fn a_stream_type_says_which_coding() {
        assert_eq!(Codec::of_stream_type(0x02), Some(Codec::Mpeg2));
        assert_eq!(Codec::of_stream_type(0x1b), Some(Codec::H264));
        assert_eq!(Codec::of_stream_type(0x24), Some(Codec::Hevc));
        assert_eq!(Codec::of_stream_type(0x0f), None);
    }

    #[test]
    fn an_asset_type_says_which_coding() {
        assert_eq!(Codec::of_asset("hev1"), Some(Codec::Hevc));
        assert_eq!(Codec::of_asset("mp4a"), None);
    }

    #[test]
    fn start_codes_give_back_what_lies_between_them() {
        let bytes = [0, 0, 1, 0xb3, 1, 2, 0, 0, 1, 0x00, 3, 0, 0, 0, 1, 0xb5, 4];
        let found: Vec<_> = StartCodes::new(&bytes).collect();
        assert_eq!(found[0], (0xb3, 4..6));
        assert_eq!(found[1], (0x00, 10..11));
        assert_eq!(found[2], (0xb5, 16..17));
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn a_stream_with_no_start_code_gives_nothing() {
        assert_eq!(StartCodes::new(&[1, 2, 3]).count(), 0);
    }

    #[test]
    fn units_behind_their_lengths_come_out_whole_and_touching() {
        let bytes = [0, 0, 0, 3, 0x46, 1, 2, 0, 0, 0, 2, 0x26, 9];
        let found: Vec<_> = Nals::new(&bytes, Framing::Lengths).collect();
        assert_eq!(
            found,
            [
                Nal {
                    header: 0x46,
                    at: 0,
                    body: 5..7
                },
                Nal {
                    header: 0x26,
                    at: 7,
                    body: 12..13
                },
            ]
        );
    }

    #[test]
    fn a_length_that_runs_past_the_end_gives_what_is_there() {
        let bytes = [0, 0, 0, 40, 0x46, 1, 2];
        let found: Vec<_> = Nals::new(&bytes, Framing::Lengths).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body, 5..7);
    }

    #[test]
    fn a_length_of_nothing_ends_the_walk() {
        let bytes = [0, 0, 0, 1, 0x46, 0, 0, 0, 0];
        assert_eq!(Nals::new(&bytes, Framing::Lengths).count(), 1);
    }

    #[test]
    fn start_codes_and_lengths_cut_the_same_units() {
        let coded = [0x46, 1, 2];
        let other = [0x26, 9];
        let start_codes = [0, 0, 1, 0x46, 1, 2, 0, 0, 1, 0x26, 9];
        let lengths = [0, 0, 0, 3, 0x46, 1, 2, 0, 0, 0, 2, 0x26, 9];
        let one: Vec<_> = Nals::new(&start_codes, Framing::StartCodes)
            .map(|nal| (nal.header, start_codes[nal.body].to_vec()))
            .collect();
        let two: Vec<_> = Nals::new(&lengths, Framing::Lengths)
            .map(|nal| (nal.header, lengths[nal.body].to_vec()))
            .collect();
        assert_eq!(one, two);
        assert_eq!(one[0], (coded[0], coded[1..].to_vec()));
        assert_eq!(one[1], (other[0], other[1..].to_vec()));
    }
}
