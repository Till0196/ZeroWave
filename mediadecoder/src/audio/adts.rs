// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ADTS, which is how a transport stream frames AAC. Every header repeats the format, so a receiver
//! can join anywhere. A PES is frame-aligned; the syncword search is only for recovery after loss.

use crate::audio::{Format, Object, RATES};

/// The syncword: twelve ones, which matches coded data often, so the length is checked too.
const SYNC: u16 = 0xfff;

pub const HEADER: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    /// The whole frame including this header.
    pub length: usize,
    pub prefix: usize,
    pub blocks: usize,
}

impl Header {
    /// Reads a header at the start of `bytes`. A frame whose payload is short still gives a header.
    pub fn read(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER {
            return None;
        }
        let word = u16::from(bytes[0]) << 4 | u16::from(bytes[1]) >> 4;
        if word != SYNC {
            return None;
        }
        // The layer is always nought for AAC; otherwise it is an MPEG audio frame.
        if bytes[1] >> 1 & 0b11 != 0 {
            return None;
        }
        let protected = bytes[1] & 1 == 0;
        let object = Object::of_index((bytes[2] >> 6) + 1);
        let rate = *RATES.get(usize::from(bytes[2] >> 2 & 0x0f))?;
        let configuration = (bytes[2] << 2 | bytes[3] >> 6) & 0b111;
        let length = (usize::from(bytes[3] & 0b11) << 11)
            | (usize::from(bytes[4]) << 3)
            | (usize::from(bytes[5]) >> 5);
        let blocks = usize::from(bytes[6] & 0b11) + 1;
        if length < HEADER {
            return None;
        }
        let prefix = if protected {
            HEADER + 2 * blocks
        } else {
            HEADER
        };
        if length < prefix {
            return None;
        }
        Some(Self {
            format: Format {
                object,
                rate,
                configuration,
            },
            length,
            prefix,
            blocks,
        })
    }
}

/// Seven header bytes (no CRC) over `payload` bytes, for passthrough (IEC 61937 needs ADTS).
/// `None` for configuration nought (22.2), a rate outside [`RATES`] or an unwritable size.
pub fn write_header(format: Format, blocks: usize, payload: usize) -> Option<[u8; HEADER]> {
    if !(1..=7).contains(&format.configuration) || !(1..=4).contains(&blocks) {
        return None;
    }
    let profile = if format.object.has_lc_core() {
        1
    } else {
        format.object.index().checked_sub(1).filter(|&at| at < 4)?
    };
    let rate = crate::audio::rate_index(format.rate)?;
    let length = HEADER.checked_add(payload)?;
    if length >= 1 << 13 {
        return None;
    }
    let configuration = format.configuration;
    Some([
        0xff,
        // ID nought (MPEG-4), layer nought, CRC absent.
        0xf1,
        profile << 6 | rate << 2 | configuration >> 2,
        (configuration & 0b11) << 6 | ((length >> 11) & 0b11) as u8,
        (length >> 3) as u8,
        ((length & 0b111) << 5) as u8 | 0b1_1111,
        // Buffer fullness all ones (variable rate), then blocks less one.
        0b1111_1100 | (blocks - 1) as u8,
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub format: Format,
    pub blocks: usize,
    pub data: &'a [u8],
}

/// Cuts bytes into frames. An incomplete frame stops iteration; [`Frames::rest`] returns it.
#[derive(Debug, Clone)]
pub struct Frames<'a> {
    bytes: &'a [u8],
    at: usize,
    skipped: usize,
}

impl<'a> Frames<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            skipped: 0,
        }
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at.min(self.bytes.len())..]
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }
}

impl<'a> Iterator for Frames<'a> {
    type Item = Frame<'a>;

    fn next(&mut self) -> Option<Frame<'a>> {
        loop {
            let here = self.bytes.get(self.at..)?;
            if here.len() < HEADER {
                return None;
            }
            let Some(header) = Header::read(here) else {
                self.at += 1;
                self.skipped += 1;
                continue;
            };
            if here.len() < header.length {
                return None;
            }
            let frame = Frame {
                format: header.format,
                blocks: header.blocks,
                data: &here[header.prefix..header.length],
            };
            self.at += header.length;
            return Some(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Frames, Header};
    use crate::audio::{Format, Object};

    fn header(protected: bool, blocks: usize, payload: usize) -> Vec<u8> {
        let prefix = if protected { 7 + 2 * blocks } else { 7 };
        let length = prefix + payload;
        let mut out = vec![0u8; prefix];
        out[0] = 0xff;
        out[1] = 0xf0 | if protected { 0 } else { 1 };
        // LC, 48 kHz (index three), channel configuration two.
        out[2] = 1 << 6 | 3 << 2;
        out[3] = 2 << 6 | ((length >> 11) & 0b11) as u8;
        out[4] = (length >> 3) as u8;
        out[5] = ((length & 0b111) << 5) as u8 | 0b1_1111;
        out[6] = 0b1111_1100 | (blocks - 1) as u8;
        out.extend(std::iter::repeat_n(0xa5, payload));
        out
    }

    #[test]
    fn a_header_says_the_format_and_the_length() {
        let bytes = header(false, 1, 100);
        let read = Header::read(&bytes).expect("a header");
        assert_eq!(
            read.format,
            Format {
                object: Object::Lc,
                rate: 48000,
                configuration: 2,
            }
        );
        assert_eq!(read.length, 107);
        assert_eq!(read.prefix, 7);
        assert_eq!(read.blocks, 1);
    }

    #[test]
    fn the_error_check_lengthens_the_prefix_but_not_the_frame() {
        let bytes = header(true, 1, 100);
        let read = Header::read(&bytes).expect("a header");
        assert_eq!(read.length, 109);
        assert_eq!(read.prefix, 9);
        let frame = Frames::new(&bytes).next().expect("a frame");
        assert_eq!(frame.data.len(), 100);
    }

    #[test]
    fn several_frames_in_one_payload_all_come_out() {
        let mut bytes = Vec::new();
        for payload in [10, 20, 30] {
            bytes.extend(header(false, 1, payload));
        }
        let mut frames = Frames::new(&bytes);
        let lengths: Vec<_> = frames.by_ref().map(|frame| frame.data.len()).collect();
        assert_eq!(lengths, [10, 20, 30]);
        assert!(frames.rest().is_empty());
        assert_eq!(frames.skipped(), 0);
    }

    #[test]
    fn a_truncated_frame_is_left_for_the_next_call() {
        let mut bytes = header(false, 1, 10);
        bytes.extend(header(false, 1, 40));
        bytes.truncate(bytes.len() - 5);
        let mut frames = Frames::new(&bytes);
        assert_eq!(frames.by_ref().count(), 1);
        assert_eq!(frames.rest().len(), 7 + 40 - 5);
        assert_eq!(frames.rest()[0], 0xff);
    }

    #[test]
    fn a_frame_after_rubbish_is_found_again() {
        let mut bytes = vec![0x00, 0xff, 0xff, 0x11];
        bytes.extend(header(false, 1, 10));
        let mut frames = Frames::new(&bytes);
        assert_eq!(frames.by_ref().next().expect("a frame").data.len(), 10);
        assert_eq!(frames.skipped(), 4);
    }

    #[test]
    fn several_raw_data_blocks_are_reported() {
        let bytes = header(false, 3, 60);
        let frame = Frames::new(&bytes).next().expect("a frame");
        assert_eq!(frame.blocks, 3);
        assert_eq!(frame.data.len(), 60);
    }

    #[test]
    fn a_written_header_reads_back_as_what_was_asked_for() {
        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 6,
        };
        let written = super::write_header(format, 1, 400).expect("a header");
        let read = Header::read(&written).expect("a header");
        assert_eq!(read.format, format);
        assert_eq!(read.length, 407);
        assert_eq!(read.prefix, 7);
        assert_eq!(read.blocks, 1);
    }

    #[test]
    fn high_efficiency_is_written_as_low_complexity() {
        let format = Format {
            object: Object::Sbr,
            rate: 24000,
            configuration: 2,
        };
        let written = super::write_header(format, 1, 100).expect("a header");
        let read = Header::read(&written).expect("a header");
        assert_eq!(read.format.object, Object::Lc);
        assert_eq!(read.format.rate, 24000);
    }

    #[test]
    fn several_blocks_survive_the_round_trip() {
        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 2,
        };
        for blocks in 1..=4 {
            let written = super::write_header(format, blocks, 100).expect("a header");
            assert_eq!(Header::read(&written).expect("a header").blocks, blocks);
        }
    }

    #[test]
    fn a_layout_adts_cannot_name_is_refused() {
        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 0,
        };
        assert!(super::write_header(format, 1, 100).is_none());
    }

    #[test]
    fn a_frame_too_long_for_the_length_field_is_refused() {
        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 2,
        };
        assert!(super::write_header(format, 1, 8185).is_none());
        assert!(super::write_header(format, 1, 8186).is_none());
    }

    #[test]
    fn a_rate_that_is_not_one_of_the_thirteen_is_refused() {
        let format = Format {
            object: Object::Lc,
            rate: 47999,
            configuration: 2,
        };
        assert!(super::write_header(format, 1, 100).is_none());
    }

    #[test]
    fn what_is_not_a_header_is_not_read_as_one() {
        let mut bytes = header(false, 1, 10);
        bytes[1] |= 0b0000_0110;
        assert!(Header::read(&bytes).is_none());
        let mut bytes = header(false, 1, 10);
        bytes[3] &= 0b1111_1100;
        bytes[4] = 0;
        bytes[5] = 0b0100_0000;
        assert!(Header::read(&bytes).is_none());
    }
}
