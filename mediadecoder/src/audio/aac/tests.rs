// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Aac;
use crate::audio::config::{Config, Element, Layout, Program};
use crate::audio::{Decoder, Error, Format, Object, Speaker, adts};
use crate::bits::Reader;

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

    fn bytes(&mut self) -> Vec<u8> {
        while !self.bits.len().is_multiple_of(8) {
            self.bits.push(0);
        }
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

fn mono() -> Format {
    Format {
        object: Object::Lc,
        rate: 48000,
        configuration: 1,
    }
}

fn empty_block() -> Vec<u8> {
    let mut out = Write::default();
    out.put(0, 3).put(0, 4); // A single channel element, tag nought.
    out.put(0, 8); // global_gain
    out.put(0, 1).put(0, 2).put(0, 1).put(0, 6).put(0, 1); // ics_info
    out.put(0, 1).put(0, 1).put(0, 1); // no pulses, no shaping, no gain
    out.put(7, 3); // the end of the block
    out.bytes()
}

#[test]
fn a_block_that_codes_nothing_gives_silence() {
    let mut aac = Aac::new();
    let block = empty_block();
    aac.decode(mono(), &block).expect("a block");
    let sound = aac.take().expect("a frame of samples");
    assert_eq!(sound.rate, 48000);
    assert_eq!(sound.speakers, [Speaker::FrontCentre]);
    assert_eq!(sound.len(), 1024);
    assert!(sound.channels[0].iter().all(|&sample| sample == 0.0));
}

#[test]
fn two_raw_data_blocks_in_one_frame_both_come_out() {
    let mut aac = Aac::new();
    let mut bytes = empty_block();
    bytes.extend(empty_block());
    aac.decode(mono(), &bytes).expect("two blocks");
    assert_eq!(aac.take().map(|sound| sound.len()), Some(1024));
    assert_eq!(aac.take().map(|sound| sound.len()), Some(1024));
    assert!(aac.take().is_none());
}

#[test]
fn a_coding_this_decoder_does_not_have_is_refused_rather_than_guessed_at() {
    let mut aac = Aac::new();
    let format = Format {
        object: Object::Main,
        ..mono()
    };
    assert_eq!(
        aac.decode(format, &empty_block()),
        Err(Error::Unsupported("an object type other than AAC-LC"))
    );
    let format = Format { rate: 7, ..mono() };
    assert_eq!(
        aac.decode(format, &empty_block()),
        Err(Error::Damaged("a sampling rate that is not in the table"))
    );
}

#[test]
fn a_reset_gives_up_what_was_held() {
    let mut aac = Aac::new();
    aac.decode(mono(), &empty_block()).expect("a block");
    aac.reset();
    assert!(aac.take().is_none());
}

#[test]
fn damaged_data_is_refused_or_decoded_but_never_panics() {
    let mut state = 12345u32;
    let mut aac = Aac::new();
    for round in 0..200 {
        let length = 1 + (round * 7) % 400;
        let bytes: Vec<u8> = (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as u8
            })
            .collect();
        let _ = aac.decode(mono(), &bytes);
        while aac.take().is_some() {}
    }
}

#[test]
fn a_truncated_block_is_refused_rather_than_half_decoded() {
    let mut aac = Aac::new();
    let block = empty_block();
    let short = &block[..block.len() - 1];
    // The block runs out before its end marker, as a lost packet leaves it.
    let _ = aac.decode(mono(), short);
}

#[test]
fn the_layout_of_eight_kilo_sound_is_twenty_four_channels() {
    let layout = Layout::of_configuration(13).expect("a layout");
    assert_eq!(layout.slots.len(), 16);
    assert_eq!(layout.channels(), 24);
    let speakers = layout.speakers();
    assert_eq!(speakers[0], Speaker::FrontCentre);
    assert_eq!(speakers[10], Speaker::LowFrequency);
    assert_eq!(speakers[11], Speaker::LowFrequencyRight);
    assert_eq!(speakers[23], Speaker::BottomFrontRight);
    assert_eq!(layout.find(Element::Cpe, 7), Some(22));
}

#[test]
fn a_program_config_element_places_what_it_can_and_says_so_where_it_cannot() {
    let program = Program {
        front: vec![(Element::Sce, 0), (Element::Cpe, 0)],
        side: Vec::new(),
        back: vec![(Element::Cpe, 1)],
        lfe: vec![(Element::Lfe, 0)],
        ..Program::default()
    };
    let layout = Layout::of_program(&program);
    assert_eq!(
        layout.speakers(),
        [
            Speaker::FrontCentre,
            Speaker::FrontLeft,
            Speaker::FrontRight,
            Speaker::BackLeft,
            Speaker::BackRight,
            Speaker::LowFrequency,
        ]
    );
    let program = Program {
        front: vec![(Element::Sce, 0), (Element::Sce, 1), (Element::Sce, 2)],
        ..Program::default()
    };
    assert!(
        Layout::of_program(&program)
            .speakers()
            .iter()
            .all(|speaker| *speaker == Speaker::Unplaced)
    );
}

#[test]
fn a_configuration_of_nought_takes_its_layout_from_the_program_element() {
    // Configuration nought carries a program config element instead.
    let mut out = Write::default();
    out.put(2, 5).put(3, 4).put(0, 4); // AAC-LC, 48 kHz, no configuration
    out.put(0, 1).put(0, 1).put(0, 1); // GASpecificConfig
    out.put(0, 4).put(1, 2).put(3, 4); // tag, object type, rate index
    out.put(1, 4).put(0, 4).put(0, 4).put(1, 2); // front, side, back, lfe
    out.put(0, 3).put(0, 4); // no associated data, no coupling
    out.put(0, 1).put(0, 1).put(0, 1); // no mix-down of any kind
    out.put(1, 1).put(0, 4); // the front element is a pair, tag nought
    out.put(0, 4); // the low frequency element, tag nought
    while out.bits.len() % 8 != 0 {
        out.put(0, 1);
    }
    out.put(0, 8); // no comment
    let bytes = out.bytes();
    let (config, _) = Config::read(&mut Reader::new(&bytes));
    assert_eq!(config.format.configuration, 0);
    assert_eq!(
        config.layout().expect("a layout").speakers(),
        [
            Speaker::FrontLeft,
            Speaker::FrontRight,
            Speaker::LowFrequency
        ]
    );
}

#[test]
fn a_configuration_that_declares_the_replication_doubles_the_rate_from_the_first_frame() {
    // LATM can declare HE-AAC up front: object five, extension rate, then the core object type.
    let mut out = Write::default();
    out.put(5, 5).put(6, 4).put(1, 4); // HE-AAC, 24 kHz core, mono
    out.put(3, 4); // the replication produces 48 kHz
    out.put(2, 5); // the core is AAC-LC
    out.put(0, 1).put(0, 1).put(0, 1); // GASpecificConfig
    let bytes = out.bytes();
    let (config, _) = Config::read(&mut Reader::new(&bytes));
    assert!(config.sbr);
    assert!(!config.ps);
    assert_eq!(config.format.object, Object::Lc);
    assert_eq!(config.format.rate, 24000);
    assert_eq!(config.extension_rate, Some(48000));

    let mut aac = Aac::new();
    aac.configure(&config);
    let format = Format {
        object: Object::Lc,
        rate: 24000,
        configuration: 1,
    };
    aac.decode(format, &empty_block()).expect("a block");
    let sound = aac.take().expect("a frame of samples");
    // No payload yet, but the rate doubles anyway so later frames line up.
    assert_eq!(sound.rate, 48000);
    assert_eq!(sound.len(), 2048);
    assert!(sound.channels[0].iter().all(|&sample| sample == 0.0));
}

/// ADTS that ffmpeg made (`testdata/make.sh`): two sines, one on each side, in stereo at 48 kHz.
#[test]
fn a_stream_of_adts_decodes_frame_for_frame() {
    let bytes = crate::testdata("mpeg2-interlaced.ts");
    let sound = elementary_stream(&bytes);
    let mut aac = Aac::new();
    let mut frames = 0;
    let mut decoded = 0;
    let mut loudest = 0.0f32;
    for frame in adts::Frames::new(&sound) {
        frames += 1;
        assert_eq!(frame.format.rate, 48000);
        assert_eq!(frame.format.configuration, 2);
        assert_eq!(frame.format.object, Object::Lc);
        aac.decode(frame.format, frame.data).expect("a frame");
        while let Some(sound) = aac.take() {
            decoded += 1;
            assert_eq!(sound.len(), 1024);
            assert_eq!(
                sound.speakers,
                [Speaker::FrontLeft, Speaker::FrontRight],
                "a stereo header must give a stereo layout, whatever \
                 element instance tag the stream used"
            );
            for channel in &sound.channels {
                for &sample in channel {
                    assert!(sample.is_finite());
                    loudest = loudest.max(sample.abs());
                }
            }
        }
    }
    assert!(frames > 40, "{frames} フレームしか無い");
    assert_eq!(decoded, frames);
    assert!(loudest > 0.01 && loudest < 4.0, "{loudest}");
}

/// The first audio PID's elementary stream, just enough for these tests.
fn elementary_stream(bytes: &[u8]) -> Vec<u8> {
    const PACKET: usize = 188;
    let mut wanted = None;
    let mut out = Vec::new();
    for packet in bytes.as_chunks::<PACKET>().0 {
        if packet[0] != 0x47 {
            continue;
        }
        let pid = u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2]);
        let start = packet[1] & 0x40 != 0;
        let control = packet[3] >> 4 & 0b11;
        if control & 1 == 0 {
            continue;
        }
        let mut at = 4;
        if control & 0b10 != 0 {
            at += 1 + usize::from(packet[4]);
        }
        let Some(payload) = packet.get(at..) else {
            continue;
        };
        if start {
            if payload.len() < 9 || payload[..3] != [0, 0, 1] || payload[3] & 0xe0 != 0xc0 {
                continue;
            }
            if wanted.is_some_and(|other| other != pid) {
                continue;
            }
            wanted = Some(pid);
            let header = 9 + usize::from(payload[8]);
            out.extend_from_slice(payload.get(header..).unwrap_or(&[]));
        } else if wanted == Some(pid) {
            out.extend_from_slice(payload);
        }
    }
    out
}
