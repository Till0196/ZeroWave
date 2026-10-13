// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ゴールデンがどんな画像で出来ているか。`video.rs`（wgpu）と `present.rs`（GLES 2.0）で共有する
//! （片方の画像だけ直した物がそれ自身のテストに通らないように）。

#![allow(dead_code)]

use mediadecoder::es::{Format, Structure};
use mediadecoder::video::{Frame, Plane};
use zwcore::decode::About;
use zwcore::output::video::field::{self, Parity};
use zwcore::output::video::filter;
use zwcore::output::video::{Deinterlace, Matrix, Primaries, Settings, Transfer};

pub const W: usize = 64;
pub const H: usize = 64;

/// The synthetic broadcast at one instant: a moving bar (so weaving combs) over a three-row
/// pattern (so lost vertical detail shows).
pub fn luma_of(x: usize, y: usize, time: usize) -> u8 {
    let bar = (x + 2 * time) % 24 < 8;
    let detail = [0u32, 14, 7][y % 3];
    let base = if bar { 210 } else { 40 };
    (base + detail).min(235) as u8
}

pub fn chroma_of(x: usize, y: usize, time: usize, plane: usize) -> u8 {
    let bar = (x + time) % 12 < 4;
    let base = if bar { 200 } else { 60 };
    (base + (y % 5) * 6 + plane * 9) as u8
}

/// One frame woven from the fields at `2 * at` and `2 * at + 1`.
pub fn woven(at: usize, depth: u8, first: Parity) -> Frame {
    let bytes = if depth > 8 { 2 } else { 1 };
    let shift = depth - 8;
    // A stride wider than the picture, as decoders hand over.
    let plane = |width: usize, height: usize, of: &dyn Fn(usize, usize, usize) -> u8| {
        let stride = (width + 16) * bytes;
        let mut data = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                let early = (y % 2 == 0) == (first == Parity::Top);
                let time = 2 * at + usize::from(!early);
                let value = u32::from(of(x, y, time)) << shift;
                let at = y * stride + x * bytes;
                data[at] = value as u8;
                if bytes == 2 {
                    data[at + 1] = (value >> 8) as u8;
                }
            }
        }
        Plane {
            data,
            stride,
            width,
            height,
        }
    };
    Frame {
        tag: at as u64,
        format: Format {
            width: W as u32,
            height: H as u32,
            depth,
            chroma: 1,
            rate: (30000, 1001),
            progressive: false,
            aspect: (16, 9),
            field_pictures: false,
            woven: false,
            colour: Default::default(),
        },
        planes: [
            plane(W, H, &|x, y, t| luma_of(x, y, t)),
            plane(W / 2, H / 2, &|x, y, t| chroma_of(x, y, t, 0)),
            plane(W / 2, H / 2, &|x, y, t| chroma_of(x, y, t, 1)),
        ],
        structure: if first == Parity::Top {
            Structure::Top
        } else {
            Structure::Bottom
        },
        top_field_first: first == Parity::Top,
        repeat_first_field: false,
        progressive: false,
    }
}

/// The same picture with both fields at one instant: a still shot.
pub fn frozen(depth: u8) -> Frame {
    let mut frame = woven(0, depth, Parity::Top);
    let bytes = if depth > 8 { 2 } else { 1 };
    let shift = depth - 8;
    for plane in 0..3 {
        let (stride, width, height) = {
            let plane = &frame.planes[plane];
            (plane.stride, plane.width, plane.height)
        };
        for y in 0..height {
            for x in 0..width {
                // Bands two rows deep (yadif widens its tolerance where rows alternate).
                let base = if plane == 0 { 60 } else { 70 };
                let value = (base + ((y as u32 / 2) % 2) * 120 + (x as u32 % 7) * 3) << shift;
                let at = y * stride + x * bytes;
                frame.planes[plane].data[at] = value as u8;
                if bytes == 2 {
                    frame.planes[plane].data[at + 1] = (value >> 8) as u8;
                }
            }
        }
    }
    frame
}

/// Edges at three shallow angles sliding sideways, so the edge search and the nested
/// `CHECK(2)` are covered ([`woven`] never exercises them).
pub fn slanted(at: usize, depth: u8, first: Parity) -> Frame {
    let mut frame = woven(at, depth, first);
    let bytes = if depth > 8 { 2 } else { 1 };
    let shift = depth - 8;
    for plane in 0..3 {
        let (stride, width, height) = {
            let plane = &frame.planes[plane];
            (plane.stride, plane.width, plane.height)
        };
        for y in 0..height {
            for x in 0..width {
                let early = (y % 2 == 0) == (first == Parity::Top);
                let time = (2 * at + usize::from(!early)) as f32;
                let slope = [1.0f32, 2.0, 3.0][(y * 3 / height).min(2)];
                let edge = slope * y as f32 + 1.5 * time + 4.0 + (plane as f32) * 5.0;
                let across = ((x as f32 - edge) * 0.6 + 0.5).clamp(0.0, 1.0);
                let value = ((16.0 + 219.0 * across).round() as u32) << shift;
                let at = y * stride + x * bytes;
                frame.planes[plane].data[at] = value as u8;
                if bytes == 2 {
                    frame.planes[plane].data[at + 1] = (value >> 8) as u8;
                }
            }
        }
    }
    frame
}

pub fn settings(deinterlace: Deinterlace) -> Settings {
    Settings {
        deinterlace,
        // The goldens are of the filters only; the cadence detector is out of the way.
        telecine: false,
        matrix: Matrix::Bt709,
        limited: true,
        transfer: Transfer::Sdr,
        primaries: Primaries::Bt709,
        deinterlace_chosen: false,
    }
}

pub fn reference(name: &str) -> (Vec<u8>, Settings, u8, Parity, usize) {
    let settings = settings(case(name).0);
    let (quality, first, which, depth) = case(name);
    let frames = frames_of(name, depth, first);
    let plan = field::plan(&About::of_frame(&frames[1]), quality);
    let output = plan[which.min(plan.len() - 1)];
    let picture = filter::picture(&frames[0], &frames[1], &frames[2], &output, settings);
    (picture, settings, depth, first, which)
}

pub fn case(name: &str) -> (Deinterlace, Parity, usize, u8) {
    match name {
        "weave" => (Deinterlace::Weave, Parity::Top, 0, 8),
        "bob-top" => (Deinterlace::Bob, Parity::Top, 0, 8),
        "yadif-top" => (Deinterlace::Yadif, Parity::Top, 0, 8),
        "bwdif-top" => (Deinterlace::Bwdif, Parity::Top, 0, 8),
        "bwdif-bottom-second" => (Deinterlace::Bwdif, Parity::Bottom, 1, 8),
        "bwdif-ten-bit" => (Deinterlace::Bwdif, Parity::Top, 1, 10),
        "yadif-diagonal" => (Deinterlace::Yadif, Parity::Top, 0, 8),
        "bwdif-diagonal" => (Deinterlace::Bwdif, Parity::Top, 0, 8),
        other => panic!("no such case: {other}"),
    }
}

pub fn frames_of(name: &str, depth: u8, first: Parity) -> [Frame; 3] {
    let build = if name.ends_with("-diagonal") {
        slanted
    } else {
        woven
    };
    [
        build(0, depth, first),
        build(1, depth, first),
        build(2, depth, first),
    ]
}

pub const CASES: [&str; 8] = [
    "weave",
    "bob-top",
    "yadif-top",
    "bwdif-top",
    "bwdif-bottom-second",
    "bwdif-ten-bit",
    "yadif-diagonal",
    "bwdif-diagonal",
];

pub fn golden_path(name: &str) -> String {
    format!("{}/tests/golden/{name}.rgba", env!("CARGO_MANIFEST_DIR"))
}
