// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What the tests of the encoded streams share. The streams are small ones ffmpeg makes into the
//! workspace's `testdata/` (`sh testdata/make.sh`).

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use mediadecoder::es::{Codec, Framing};
use mediadecoder::video::Frame;
use zwcore::decode::{Decode, Decoded, Luma, Readback, psnr};
use zwcore::filters::cut::{Cutout, Cutting};

/// 素材のパス。ワークスペースの `testdata/` の中。
pub fn testdata_path(name: &str) -> String {
    format!("{}/../testdata/{name}", env!("CARGO_MANIFEST_DIR"))
}

pub fn testdata(name: &str) -> Vec<u8> {
    let path = testdata_path(name);
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{path}: {error}（sh testdata/make.sh で作る）"))
}

/// A stream cut into the access units of its one video stream ([`zwcore::filters::cut`]).
pub fn units(name: &str) -> Cutting {
    Cutting::of_path(&testdata_path(name), usize::MAX)
        .unwrap_or_else(|| panic!("{name} has no video to cut"))
}

pub fn hardware<D: Decode>(
    what: &str,
    open: impl FnOnce() -> Result<D, zwcore::decode::Error>,
) -> Option<D> {
    match open() {
        Ok(decoder) => Some(decoder),
        Err(error) => {
            eprintln!("{what} が無いので飛ばす: {error:?}");
            None
        }
    }
}

/// The worst luma PSNR over the frames both decoders produced, and how many. Nothing where
/// the decoder is missing, or cannot read back (`refused`).
pub fn compared<D: Decode>(
    name: &str,
    most: usize,
    open: impl FnOnce(Codec, Framing) -> Option<D>,
    refused: impl Fn(&D) -> bool,
) -> Option<(f64, usize)> {
    let cutting = units(name);
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let mut hardware = open(codec, framing)?;
    let mut software = zwcore::decode::software(codec, framing);

    let mut soft: Vec<Frame> = Vec::new();
    let mut hard: Vec<Readback> = Vec::new();
    for Cutout {
        tag,
        picture,
        format,
        data,
    } in units
    {
        if let Err(error) = software.decode(*tag, picture, *format, data) {
            eprintln!("ソフト {tag}: {error:?}");
        }
        while let Some(Decoded::Planes(frame)) = software.take() {
            soft.push(frame);
        }
        if let Err(error) = hardware.decode(*tag, picture, *format, data) {
            eprintln!("ハード {tag}: {error:?}");
        }
        while let Some(found) = hardware.read_back() {
            hard.push(found);
        }
        if soft.len() >= most && hard.len() >= most {
            break;
        }
    }

    // The output order has to match too.
    let mine: Vec<u64> = soft.iter().map(|frame| frame.tag).collect();
    let theirs: Vec<u64> = hard.iter().map(|read| read.about.tag).collect();
    let common = mine.len().min(theirs.len()).min(most);
    // 読み戻せないドライバーがある（「フレームが出なかった」と読むと壊れたデコードと区別が付かない）。
    if common == 0 && refused(&hardware) {
        eprintln!("{name}: このドライバーはサンプルを CPU へ返しません。飛ばします");
        return None;
    }
    assert!(common > 0, "{name}: neither decoder produced anything");
    assert_eq!(
        mine[..common],
        theirs[..common],
        "{name}: the two decoders show the pictures in different orders"
    );

    // Leading pictures of an open group predict from a reference neither decoder has; dropped.
    let entry: BTreeSet<u64> = units
        .iter()
        .filter(|unit| unit.picture.random_access)
        .map(|unit| unit.tag)
        .collect();
    let leading = hard
        .iter()
        .position(|read| entry.contains(&read.about.tag))
        .unwrap_or(0);

    let by_tag: BTreeMap<u64, &Frame> = soft.iter().map(|frame| (frame.tag, frame)).collect();
    let mut worst = f64::INFINITY;
    let mut counted = 0;
    for read in hard[leading..].iter().take(common - leading) {
        let Some(frame) = by_tag.get(&read.about.tag) else {
            continue;
        };
        let Some((theirs, pitch)) = read.luma() else {
            continue;
        };
        // `P010` keeps its ten bits at the top of its sixteen.
        let shift = if frame.format.depth > 8 {
            16 - u32::from(frame.format.depth)
        } else {
            0
        };
        let found = psnr(
            Luma::of_frame(frame),
            Luma {
                bytes: theirs,
                pitch,
                shift,
            },
            frame.format,
        );
        worst = worst.min(found);
        counted += 1;
    }
    Some((worst, counted))
}
