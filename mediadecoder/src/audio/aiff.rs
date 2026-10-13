// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送で届く音声ファイル（AIFF-C、STD-B24 第二分冊 (2/2) 付属 4）。PCM 16 ビットだけ。

use super::{Decoder, Error, Format, Sound, Speaker};

#[derive(Debug, Clone, Default)]
pub struct Aiff {
    ready: Option<Sound>,
}

impl Aiff {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Decoder for Aiff {
    fn decode(&mut self, _format: Format, data: &[u8]) -> Result<(), Error> {
        self.ready = Some(decode(data)?);
        Ok(())
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.take()
    }

    fn reset(&mut self) {
        self.ready = None;
    }
}

const RATES: std::ops::RangeInclusive<u32> = 4_000..=192_000;

pub fn decode(bytes: &[u8]) -> Result<Sound, Error> {
    if bytes.len() < 12 || &bytes[0..4] != b"FORM" {
        return Err(Error::Damaged("not a FORM file"));
    }
    let kind = &bytes[8..12];
    if kind != b"AIFF" && kind != b"AIFC" {
        return Err(Error::Unsupported("a FORM that is not AIFF"));
    }
    let mut at = 12;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut rate = 0u32;
    let mut frames = 0u32;
    let mut data: Option<&[u8]> = None;
    let short = Error::Damaged("a chunk cut short");
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_be_bytes(
            bytes[at + 4..at + 8]
                .try_into()
                .map_err(|_| short.clone())?,
        ) as usize;
        let body = bytes
            .get(at + 8..(at + 8 + size).min(bytes.len()))
            .ok_or_else(|| short.clone())?;
        match id {
            b"COMM" => {
                if body.len() < 18 {
                    return Err(short);
                }
                channels = u16::from_be_bytes([body[0], body[1]]);
                frames = u32::from_be_bytes(body[2..6].try_into().map_err(|_| short.clone())?);
                bits = u16::from_be_bytes([body[6], body[7]]);
                rate = extended_to_u32(&body[8..18]);
                if kind == b"AIFC" {
                    let compression = body.get(18..22).ok_or_else(|| short.clone())?;
                    if compression != b"NONE" && compression != b"sowt" {
                        return Err(Error::Unsupported("a compressed AIFF-C"));
                    }
                }
            }
            b"SSND" => {
                if body.len() < 8 {
                    return Err(short);
                }
                let offset =
                    u32::from_be_bytes(body[0..4].try_into().map_err(|_| short.clone())?) as usize;
                data = body.get(8 + offset..);
            }
            _ => {}
        }
        // チャンクは偶数の長さに揃う。
        at += 8 + size + (size & 1);
    }
    let data = data.ok_or(Error::Damaged("no SSND chunk"))?;
    if channels == 0 || rate == 0 {
        return Err(Error::Damaged("no COMM chunk"));
    }
    // 極端なレートは鳴らさない（出力のレートへ変換すると、短い音でも何 GB にもなる）。
    if !RATES.contains(&rate) {
        return Err(Error::Unsupported("a sample rate outside 4 kHz to 192 kHz"));
    }
    if bits != 16 {
        return Err(Error::Unsupported("samples that are not 16 bit"));
    }
    let per_frame = usize::from(channels) * 2;
    let count = (data.len() / per_frame).min(frames as usize);
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(count)).collect();
    for frame in data.chunks_exact(per_frame).take(count) {
        for (channel, pair) in out.iter_mut().zip(frame.as_chunks::<2>().0) {
            channel.push(f32::from(i16::from_be_bytes(*pair)) / 32768.0);
        }
    }
    let speakers = match channels {
        1 => vec![Speaker::FrontCentre],
        2 => vec![Speaker::FrontLeft, Speaker::FrontRight],
        n => vec![Speaker::Unplaced; usize::from(n)],
    };
    Ok(Sound {
        rate,
        channels: out,
        speakers,
        mixdown: Default::default(),
    })
}

/// IEEE 754 の 80 ビット拡張浮動小数（AIFF のサンプリング周波数）を整数に。
fn extended_to_u32(bytes: &[u8]) -> u32 {
    let exponent = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7fff) - 16383;
    let mantissa = u64::from_be_bytes(bytes[2..10].try_into().unwrap_or([0; 8]));
    if !(0..=62).contains(&exponent) {
        return 0;
    }
    (mantissa >> (63 - exponent)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_small_aifc() -> Vec<u8> {
        // FORM, AIFC, COMM (1ch, 2 frames, 16bit, 48000 Hz, NONE), SSND.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FORM\0\0\0\0AIFC");
        bytes.extend_from_slice(b"COMM\0\0\0\x16");
        bytes.extend_from_slice(&[0, 1, 0, 0, 0, 2, 0, 16]);
        bytes.extend_from_slice(&[0x40, 0x0e, 0xbb, 0x80, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(b"NONE");
        bytes.extend_from_slice(b"SSND\0\0\0\x0c");
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&0x4000i16.to_be_bytes());
        bytes.extend_from_slice(&(-0x4000i16).to_be_bytes());
        bytes
    }

    #[test]
    fn a_small_aifc_is_read() {
        let sound = decode(&a_small_aifc()).expect("読める");
        assert_eq!(sound.rate, 48000);
        assert_eq!(sound.channels, [[0.5, -0.5]]);
        assert_eq!(sound.speakers, [Speaker::FrontCentre]);
    }

    #[test]
    fn the_decoder_hands_one_sound_per_file() {
        let mut aiff = Aiff::new();
        aiff.decode(Format::default(), &a_small_aifc())
            .expect("読める");
        assert_eq!(aiff.take().expect("一つ").len(), 2);
        assert!(aiff.take().is_none());
    }

    #[test]
    fn a_compressed_aifc_is_refused() {
        let mut bytes = a_small_aifc();
        let at = bytes.windows(4).position(|w| w == b"NONE").unwrap();
        bytes[at..at + 4].copy_from_slice(b"ulaw");
        assert_eq!(
            decode(&bytes),
            Err(Error::Unsupported("a compressed AIFF-C"))
        );
    }
}
