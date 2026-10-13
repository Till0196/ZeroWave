// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! IEC 61937: wrapping ADTS AAC in data bursts for a link built for stereo PCM.
//! Little endian 16-bit, the payload byte swapped in pairs.

use mediadecoder::audio::{Error, adts};

pub const PA: u16 = 0xf872;

pub const PB: u16 = 0x4e1f;

pub const PREAMBLE: usize = 8;

/// For HE-AAC the burst is sized by the core, which is what the link sees.
pub const BLOCK: usize = 1024;

/// IEC 61937-6. Three blocks has no number, so such a frame cannot be passed through.
pub fn data_type(samples: usize) -> Option<u16> {
    match samples {
        1024 => Some(0x07),
        2048 => Some(0x13),
        4096 => Some(0x13 | 0x20),
        _ => None,
    }
}

pub fn period(samples: usize) -> usize {
    samples * 4
}

/// Returns the number of IEC 60958 frames written (sample slots, not bytes).
pub fn pack(frame: &[u8], out: &mut Vec<u8>) -> Result<usize, Error> {
    let header = adts::Header::read(frame).ok_or(Error::Damaged("not an ADTS frame"))?;
    if frame.len() < header.length {
        return Err(Error::Damaged("the ADTS frame is short of its own length"));
    }
    let frame = &frame[..header.length];
    let samples = header.blocks * BLOCK;
    let data_type = data_type(samples).ok_or(Error::Unsupported(
        "IEC 61937 has no data type for this many raw data blocks",
    ))?;
    let written = frame.len().next_multiple_of(2);
    let period = period(samples);
    if PREAMBLE + written > period {
        return Err(Error::Unsupported(
            "the coded frame is too long for the link",
        ));
    }

    out.reserve(period);
    out.extend_from_slice(&PA.to_le_bytes());
    out.extend_from_slice(&PB.to_le_bytes());
    out.extend_from_slice(&data_type.to_le_bytes());
    // In bits, of what was written, so an odd frame declares its padding.
    out.extend_from_slice(&((written * 8) as u16).to_le_bytes());
    for pair in frame.chunks(2) {
        match pair {
            [first, second] => out.extend_from_slice(&[*second, *first]),
            [only] => out.extend_from_slice(&[0, *only]),
            _ => unreachable!("chunks(2) gives one or two"),
        }
    }
    out.resize(out.len() + period - PREAMBLE - written, 0);
    Ok(samples)
}

pub fn null(samples: usize, out: &mut Vec<u8>) {
    let period = period(samples);
    out.reserve(period);
    out.extend_from_slice(&PA.to_le_bytes());
    out.extend_from_slice(&PB.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.resize(out.len() + period - PREAMBLE, 0);
}

#[cfg(test)]
mod tests {
    use super::{PREAMBLE, null, pack, period};
    use mediadecoder::audio::{Error, Format, Object, adts};

    fn frame(blocks: usize, payload: usize) -> Vec<u8> {
        let format = Format {
            object: Object::Lc,
            rate: 48000,
            configuration: 6,
        };
        let mut out = adts::write_header(format, blocks, payload)
            .expect("a header")
            .to_vec();
        out.extend((0..payload).map(|at| (at % 251) as u8));
        out
    }

    #[test]
    fn the_preamble_says_what_the_burst_holds() {
        let frame = frame(1, 400);
        let mut out = Vec::new();
        assert_eq!(pack(&frame, &mut out).expect("a burst"), 1024);
        assert_eq!(
            out[..PREAMBLE],
            [
                0x72,
                0xf8, // Pa
                0x1f,
                0x4e, // Pb
                0x07,
                0x00, // Pc: MPEG-2 AAC
                ((408 * 8) & 0xff) as u8,
                ((408 * 8) >> 8) as u8,
            ]
        );
    }

    #[test]
    fn the_burst_is_the_whole_period_and_the_tail_is_nought() {
        let frame = frame(1, 400);
        let mut out = Vec::new();
        pack(&frame, &mut out).expect("a burst");
        assert_eq!(out.len(), period(1024));
        assert_eq!(out.len(), 4096);
        assert!(out[PREAMBLE + 408..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_payload_comes_back_when_the_pairs_are_swapped_again() {
        let frame = frame(1, 400);
        let mut out = Vec::new();
        pack(&frame, &mut out).expect("a burst");
        let mut back = Vec::new();
        for pair in out[PREAMBLE..PREAMBLE + 408].chunks(2) {
            back.push(pair[1]);
            back.push(pair[0]);
        }
        assert_eq!(&back[..frame.len()], &frame[..]);
        assert_eq!(back[frame.len()], 0);
    }

    #[test]
    fn several_blocks_lengthen_the_period() {
        for (blocks, samples, data_type) in [(1, 1024, 0x07), (2, 2048, 0x13), (4, 4096, 0x33)] {
            let frame = frame(blocks, 400);
            let mut out = Vec::new();
            assert_eq!(pack(&frame, &mut out).expect("a burst"), samples);
            assert_eq!(out.len(), samples * 4);
            assert_eq!(out[4], data_type);
        }
    }

    #[test]
    fn a_block_count_the_standard_does_not_name_is_refused() {
        let frame = frame(3, 400);
        let mut out = Vec::new();
        assert!(matches!(pack(&frame, &mut out), Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_frame_too_long_for_the_link_is_refused() {
        let fits = frame(1, 4096 - PREAMBLE - 7);
        let mut out = Vec::new();
        assert_eq!(pack(&fits, &mut out).expect("a burst"), 1024);
        assert_eq!(out.len(), 4096);

        let frame = frame(1, 4096 - PREAMBLE - 7 + 1);
        let mut out = Vec::new();
        assert!(matches!(pack(&frame, &mut out), Err(Error::Unsupported(_))));
    }

    #[test]
    fn what_is_not_a_frame_is_not_wrapped() {
        let mut out = Vec::new();
        assert!(matches!(pack(&[0; 7], &mut out), Err(Error::Damaged(_))));
        let mut frame = frame(1, 400);
        frame.truncate(100);
        assert!(matches!(pack(&frame, &mut out), Err(Error::Damaged(_))));
    }

    #[test]
    fn a_null_burst_is_a_whole_period_of_nothing() {
        let mut out = Vec::new();
        null(1024, &mut out);
        assert_eq!(out.len(), 4096);
        assert_eq!(out[..PREAMBLE], [0x72, 0xf8, 0x1f, 0x4e, 0, 0, 0, 0]);
        assert!(out[PREAMBLE..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn bursts_follow_one_another_in_the_same_buffer() {
        let frame = frame(1, 400);
        let mut out = Vec::new();
        pack(&frame, &mut out).expect("a burst");
        null(1024, &mut out);
        pack(&frame, &mut out).expect("a burst");
        assert_eq!(out.len(), 3 * 4096);
        assert_eq!(out[4096 + 4], 0);
        assert_eq!(out[2 * 4096 + 4], 0x07);
    }
}
