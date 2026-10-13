// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Whole-decoder tests: bytes that are not a stream, and a piece of a real 4K broadcast.

use crate::es::{Access, Codec, Format, Framing, Picture, Splitter, Structure};
use crate::video::{Decoder, Error};

use super::{Hevc, escapes_before};

/// An escape in the slice data would move where the platform decoder starts the arithmetic coder.
#[test]
fn only_the_escapes_before_the_data_are_counted() {
    let bytes = [0, 0, 3, 1, 2, 0, 0, 3, 1];
    assert_eq!(escapes_before(&bytes, 4), 1);
    assert_eq!(escapes_before(&bytes, 8), 2);
    assert_eq!(escapes_before(&bytes, 2), 0);
}

fn format() -> Format {
    Format {
        width: 3840,
        height: 2160,
        depth: 10,
        chroma: 1,
        ..Format::default()
    }
}

#[test]
fn a_stream_this_cannot_decode_is_refused_rather_than_guessed_at() {
    let mut decoder = Hevc::new();
    let picture = Picture::default();
    let above = Format {
        chroma: 2,
        ..format()
    };
    assert_eq!(
        decoder.decode(0, &picture, above, &[]),
        Err(Error::Unsupported("a chroma format other than 4:2:0"))
    );
    let deep = Format {
        depth: 12,
        ..format()
    };
    assert_eq!(
        decoder.decode(0, &picture, deep, &[]),
        Err(Error::Unsupported("a sample that is not eight or ten bits"))
    );
    let nothing = Format {
        width: 0,
        ..format()
    };
    assert_eq!(
        decoder.decode(0, &picture, nothing, &[]),
        Err(Error::Damaged("a parameter set with no size"))
    );
}

#[test]
fn bytes_that_are_not_a_stream_produce_no_picture_and_no_panic() {
    let mut decoder = Hevc::new();
    let picture = Picture::default();
    let mut bytes = Vec::new();
    for at in 0..4096u32 {
        bytes.extend_from_slice(&at.wrapping_mul(2654435761).to_be_bytes());
    }
    let _ = decoder.decode(0, &picture, format(), &bytes);
    assert!(decoder.take().is_none());
    decoder.flush();
    assert!(decoder.take().is_none());
}

#[test]
fn giving_up_everything_held_leaves_a_decoder_that_still_works() {
    let mut decoder = Hevc::new();
    decoder.reset();
    decoder.flush();
    assert!(decoder.take().is_none());
    let _ = decoder.decode(0, &Picture::default(), format(), &[0, 0, 0, 1, 0x40, 0x01]);
    decoder.reset();
    assert!(decoder.take().is_none());
}

/// Main 10 made by ffmpeg (`testdata/make.sh`), cut so that it begins at the second entry of an
/// open GOP: a CRA picture whose three leading pictures predict from what is not there.
#[test]
fn an_encoded_stream_decodes_to_the_pictures_it_says_it_holds() {
    let (width, height) = (416, 240);
    let bytes = crate::testdata("hevc-main10.ts");
    let mut splitter = Splitter::new(Codec::Hevc, Framing::StartCodes);
    let mut decoder = Hevc::with_framing(Framing::StartCodes);
    let mut coded = 0u64;
    let mut frames = Vec::new();
    let mut errors = Vec::new();
    for unit in crate::pes(&bytes, 0x100) {
        let mut accesses: Vec<Access> = Vec::new();
        splitter.push(&unit, &mut accesses);
        for access in accesses {
            let Some(format) = access.format else {
                continue;
            };
            assert_eq!((format.width, format.height), (width, height));
            assert_eq!(format.depth, 10);
            coded += 1;
            if let Err(error) = decoder.decode(coded, &access.picture, format, &unit[access.range])
            {
                errors.push(error);
            }
            while let Some(frame) = decoder.take() {
                frames.push(frame);
            }
        }
    }
    decoder.flush();
    while let Some(frame) = decoder.take() {
        frames.push(frame);
    }

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(coded, 27, "coded pictures");
    assert_eq!(frames.len(), 24, "frames");

    let (width, height) = (width as usize, height as usize);
    let mut shown = Vec::new();
    for frame in &frames {
        let luma = &frame.planes[0];
        assert_eq!((luma.width, luma.height), (width, height));
        assert_eq!(
            (frame.planes[1].width, frame.planes[1].height),
            (width / 2, height / 2)
        );
        assert!(luma.stride >= width * 2);
        assert_eq!(luma.data.len(), luma.stride * height);
        assert_eq!(frame.structure, Structure::Frame);
        assert!(frame.progressive);
        assert!(!frame.repeat_first_field);
        let samples: Vec<u16> = luma
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        assert!(samples.iter().any(|&value| value != 0));
        assert!(samples.iter().all(|&value| value < 1024));
        shown.push(frame.tag);
    }
    assert!(
        shown.windows(2).any(|pair| pair[0] > pair[1]),
        "nothing was reordered: {shown:?}"
    );
}
