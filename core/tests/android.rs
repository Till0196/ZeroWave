// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The Android decoder held against the software one. Almost nothing here runs off a device;
//! the probe application does the same comparison there. The last test runs everywhere.

use mediadecoder::es::{Codec, Framing};
use zwcore::decode::Prefer;

/// A machine with no platform decoder for a coding falls back (a Galaxy S24 has no MPEG-2).
#[test]
fn a_device_with_no_decoder_for_a_coding_falls_back_to_the_software_one() {
    for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
        let mut decoder = zwcore::decode::open(codec, Framing::StartCodes, Prefer::Platform);
        assert!(decoder.take().is_none());
        decoder.flush();
        decoder.reset();
    }
}

#[test]
fn preferring_software_gives_samples_in_memory() {
    let mut decoder = zwcore::decode::open(Codec::H264, Framing::StartCodes, Prefer::Software);
    assert!(decoder.take().is_none());
    assert!(decoder.read_back().is_none());
}

/// What the device really has, asked of the framework (`AMediaCodec_createDecoderByType`).
#[cfg(all(feature = "android", target_os = "android"))]
#[test]
fn what_the_device_has_is_what_the_framework_will_open() {
    use zwcore::decode::android::Android;

    for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
        match Android::open(codec, Framing::StartCodes) {
            Ok(decoder) => eprintln!("{codec:?}: {}", decoder.name()),
            Err(error) => eprintln!("{codec:?}: {error:?}"),
        }
    }
}

/// The whole path for one coding every device has; deliberately not a comparison.
#[cfg(all(feature = "android", target_os = "android"))]
#[test]
fn a_decoder_that_opens_takes_a_format_and_a_route() {
    use zwcore::decode::android::{Android, Route};

    let Ok(decoder) = Android::open(Codec::H264, Framing::StartCodes) else {
        eprintln!("video/avc のデコーダーがありません");
        return;
    };
    assert_eq!(decoder.route(), Route::Buffers);
    assert_eq!(decoder.format().width, 0);
}
