// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! LATM and LOAS, which is how MMT frames AAC. The configuration comes in band (`StreamMuxConfig`),
//! so frames before it cannot even be delimited: they are counted and dropped.
//! [`Parser::push`] reads LOAS; [`Parser::unit`] reads a bare `AudioMuxElement` (MMT).

use crate::audio::config::Config;
use crate::audio::{Error, Format};
use crate::bits::Reader;

const SYNC: u32 = 0x2b7;

pub const HEADER: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub format: Format,
    /// The raw data block, copied because a LATM payload need not start on a byte boundary.
    pub data: Vec<u8>,
}

/// Reads `AudioSyncStream`s, keeping the configuration across calls (`useSameStreamMux`).
#[derive(Debug, Clone, Default)]
pub struct Parser {
    mux: Option<Mux>,
    unconfigured: u64,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(&self) -> Option<&Config> {
        self.mux.as_ref().map(|mux| &mux.config)
    }

    pub fn unconfigured(&self) -> u64 {
        self.unconfigured
    }

    pub fn reset(&mut self) {
        self.mux = None;
        self.unconfigured = 0;
    }

    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Frame>) -> usize {
        let mut at = 0;
        while at + HEADER <= bytes.len() {
            let here = &bytes[at..];
            let word = u32::from(here[0]) << 3 | u32::from(here[1]) >> 5;
            if word != SYNC {
                at += 1;
                continue;
            }
            let length = (usize::from(here[1] & 0b1_1111) << 8) | usize::from(here[2]);
            if here.len() < HEADER + length {
                break;
            }
            self.element(&here[HEADER..HEADER + length], out);
            at += HEADER + length;
        }
        at
    }

    /// One bare `AudioMuxElement` (MMT has no LOAS sync layer).
    pub fn unit(&mut self, bytes: &[u8], out: &mut Vec<Frame>) {
        self.element(bytes, out);
    }

    fn element(&mut self, bytes: &[u8], out: &mut Vec<Frame>) {
        let mut reader = Reader::new(bytes);
        if reader.bit() == 0 {
            match Mux::read(&mut reader) {
                Ok(mux) => self.mux = Some(mux),
                // A configuration that could not be read leaves the previous one standing.
                Err(_) => return,
            }
        }
        let Some(mux) = self.mux.clone() else {
            self.unconfigured += 1;
            return;
        };
        if mux.version_a {
            // `audioMuxVersionA` is reserved; what follows has no published syntax.
            return;
        }
        mux.payloads(&mut reader, out);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mux {
    pub config: Config,
    /// `allStreamsSameTimeFraming`, which decides the shape of `PayloadLengthInfo`.
    pub same_time_framing: bool,
    pub subframes: usize,
    /// Nought: lengths counted in the element; one: a fixed length declared here.
    pub length_type: u8,
    pub length: usize,
    pub version_a: bool,
}

impl Mux {
    /// Only one programme and one layer; anything else is refused rather than half-read.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let version = reader.bit();
        let version_a = version == 1 && reader.bit() == 1;
        if version_a {
            return Ok(Self {
                config: Config::default(),
                same_time_framing: true,
                subframes: 1,
                length_type: 0,
                length: 0,
                version_a: true,
            });
        }
        if version == 1 {
            latm_value(reader);
        }
        let same_time_framing = reader.bit() == 1;
        let subframes = reader.bits(6) as usize + 1;
        let programs = reader.bits(4) as usize + 1;
        let layers = reader.bits(3) as usize + 1;
        if programs != 1 || layers != 1 {
            return Err(Error::Unsupported("LATM with several programmes or layers"));
        }
        let config = if version == 0 {
            Config::read(reader).0
        } else {
            // Read the configuration, then honour its declared length to stay at the right bit.
            let declared = latm_value(reader) as usize;
            let began = reader.at();
            let (config, took) = Config::read(reader);
            reader.skip(declared.saturating_sub(took));
            debug_assert_eq!(reader.at(), began + declared.max(took));
            config
        };
        let length_type = reader.bits(3) as u8;
        let length = match length_type {
            0 => {
                reader.skip(8);
                0
            }
            1 => reader.bits(9) as usize + 20,
            _ => return Err(Error::Unsupported("the frame length of CELP or HVXC")),
        };
        if reader.bit() == 1 {
            let bits = if version == 1 {
                latm_value(reader) as usize
            } else {
                let mut bits = 0usize;
                loop {
                    bits <<= 8;
                    let more = reader.bit() == 1;
                    bits += reader.bits(8) as usize;
                    if !more {
                        break bits;
                    }
                }
            };
            reader.skip(bits);
        }
        if reader.bit() == 1 {
            reader.skip(8);
        }
        Ok(Self {
            config,
            same_time_framing,
            subframes,
            length_type,
            length,
            version_a: false,
        })
    }

    fn payloads(&self, reader: &mut Reader<'_>, out: &mut Vec<Frame>) {
        for _ in 0..self.subframes {
            let bits = match self.length_type {
                0 => {
                    let mut bytes = 0usize;
                    loop {
                        let part = reader.bits(8) as usize;
                        bytes += part;
                        if part != 255 {
                            break;
                        }
                    }
                    // Without same time framing an end flag follows each length; it is dropped.
                    if !self.same_time_framing {
                        reader.skip(1);
                    }
                    bytes * 8
                }
                _ => self.length,
            };
            if bits > reader.left() {
                return;
            }
            let mut data = Vec::with_capacity(bits.div_ceil(8));
            for _ in 0..bits / 8 {
                data.push(reader.bits(8) as u8);
            }
            let tail = bits % 8;
            if tail > 0 {
                data.push((reader.bits(tail) << (8 - tail)) as u8);
            }
            out.push(Frame {
                format: self.config.format,
                data,
            });
        }
    }
}

/// `LatmGetValue`: up to four bytes with a two-bit count in front.
fn latm_value(reader: &mut Reader<'_>) -> u32 {
    let bytes = reader.bits(2) + 1;
    let mut value = 0;
    for _ in 0..bytes {
        value = value << 8 | reader.bits(8);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{Frame, Parser};
    use crate::audio::{Object, Speaker};

    #[derive(Default)]
    struct Write {
        bits: Vec<u8>,
    }

    impl Write {
        fn put(&mut self, value: u32, count: usize) -> &mut Self {
            for shift in (0..count).rev() {
                self.bits.push((value >> shift & 1) as u8);
            }
            self
        }

        fn len(&self) -> usize {
            self.bits.len()
        }

        fn bytes(&self) -> Vec<u8> {
            self.bits
                .chunks(8)
                .map(|chunk| {
                    chunk
                        .iter()
                        .enumerate()
                        .fold(0u8, |byte, (at, &bit)| byte | bit << (7 - at))
                })
                .collect()
        }
    }

    fn asc(out: &mut Write) {
        out.put(2, 5).put(3, 4).put(2, 4);
        out.put(0, 1).put(0, 1).put(0, 1);
    }

    fn mux(out: &mut Write, same_time_framing: bool) {
        out.put(0, 1); // audioMuxVersion
        out.put(u32::from(same_time_framing), 1);
        out.put(0, 6); // one subframe
        out.put(0, 4); // one programme
        out.put(0, 3); // one layer
        asc(out);
        out.put(0, 3); // frameLengthType
        out.put(0xff, 8); // latmBufferFullness
        out.put(0, 1); // otherDataPresent
        out.put(0, 1); // crcCheckPresent
    }

    fn loas(element: &[u8]) -> Vec<u8> {
        let mut out = Write::default();
        out.put(0x2b7, 11).put(element.len() as u32, 13);
        let mut bytes = out.bytes();
        bytes.extend_from_slice(element);
        bytes
    }

    fn frame(same_time_framing: bool, config: bool, payload: &[u8]) -> Vec<u8> {
        let mut out = Write::default();
        out.put(u32::from(!config), 1);
        if config {
            mux(&mut out, same_time_framing);
        }
        let mut left = payload.len();
        while left >= 255 {
            out.put(255, 8);
            left -= 255;
        }
        out.put(left as u32, 8);
        if !same_time_framing {
            out.put(0, 1);
        }
        for &byte in payload {
            out.put(u32::from(byte), 8);
        }
        while out.len() % 8 != 0 {
            out.put(0, 1);
        }
        loas(&out.bytes())
    }

    #[test]
    fn a_frame_gives_back_its_payload_and_what_the_configuration_said() {
        let bytes = frame(true, true, &[1, 2, 3, 4, 5]);
        let mut parser = Parser::new();
        let mut out = Vec::new();
        assert_eq!(parser.push(&bytes, &mut out), bytes.len());
        assert_eq!(
            out,
            [Frame {
                format: crate::audio::Format {
                    object: Object::Lc,
                    rate: 48000,
                    configuration: 2,
                },
                data: vec![1, 2, 3, 4, 5],
            }]
        );
        let config = parser.config().expect("a configuration");
        assert_eq!(
            config.layout().expect("a layout").speakers(),
            [Speaker::FrontLeft, Speaker::FrontRight]
        );
    }

    #[test]
    fn a_stream_without_the_same_time_framing_reads_its_end_flag() {
        let bytes = frame(false, true, &[9, 9, 9]);
        let mut out = Vec::new();
        assert_eq!(Parser::new().push(&bytes, &mut out), bytes.len());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, [9, 9, 9]);
    }

    #[test]
    fn a_frame_before_the_configuration_is_dropped_and_counted() {
        let mut bytes = frame(true, false, &[7; 4]);
        bytes.extend(frame(true, true, &[1, 2, 3]));
        bytes.extend(frame(true, false, &[4, 5, 6]));
        let mut parser = Parser::new();
        let mut out = Vec::new();
        assert_eq!(parser.push(&bytes, &mut out), bytes.len());
        assert_eq!(parser.unconfigured(), 1);
        let payloads: Vec<_> = out.iter().map(|frame| frame.data.clone()).collect();
        assert_eq!(payloads, [vec![1, 2, 3], vec![4, 5, 6]]);
    }

    #[test]
    fn a_sync_stream_that_has_not_all_arrived_is_left_alone() {
        let whole = frame(true, true, &[1, 2, 3]);
        let mut bytes = whole.clone();
        bytes.extend(&whole[..whole.len() - 2]);
        let mut parser = Parser::new();
        let mut out = Vec::new();
        let used = parser.push(&bytes, &mut out);
        assert_eq!(used, whole.len());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_payload_longer_than_the_escape_is_read_whole() {
        let payload: Vec<u8> = (0..300u32).map(|at| at as u8).collect();
        let bytes = frame(true, true, &payload);
        let mut out = Vec::new();
        Parser::new().push(&bytes, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, payload);
    }
}
