// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What the picture is supposed to look like. The golden tests compare against
//! `tests/golden` (the Rust reference, [`zwcore::output::video::filter`]); a GPU may differ
//! by two. The stream tests measure combing on interlaced streams ffmpeg made (`testdata/`).
//! Rewrite the goldens with `ZEROWAVE_WRITE_GOLDEN=1` when the filters change on purpose.

use mediadecoder::es::{Format, Structure};
use mediadecoder::video::{Frame, Plane};
use zwcore::decode::{About, Decoded, Prefer};
use zwcore::engine::decoding::Decoding;
use zwcore::output::video::field::{self, Parity};
use zwcore::output::video::filter::{self, Samples};
use zwcore::output::video::{Cadence, Deinterlace, telecine};

mod common;
/// 入力の作り方と正解の出し方は、GLES 2.0 の側と共有する。
#[path = "pictures.rs"]
mod pictures;
use pictures::*;

#[test]
fn the_reference_filters_make_the_pictures_the_goldens_hold() {
    for name in CASES {
        let (picture, ..) = reference(name);
        let path = golden_path(name);
        if std::env::var("ZEROWAVE_WRITE_GOLDEN").is_ok() {
            std::fs::write(&path, &picture).expect("the goldens are in the repository");
            continue;
        }
        let golden = std::fs::read(&path).unwrap_or_else(|_| panic!("{path} is missing"));
        assert_eq!(golden.len(), picture.len(), "{name}");
        let worst = golden
            .iter()
            .zip(&picture)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert_eq!(worst, 0, "{name} no longer matches its golden");
    }
}

#[test]
fn every_filter_takes_the_combing_out_of_a_moving_picture() {
    let woven_comb = {
        let (picture, ..) = reference("weave");
        filter::comb_rgba(&picture, W, H)
    };
    for name in ["bob-top", "yadif-top", "bwdif-top"] {
        let (picture, ..) = reference(name);
        let left = filter::comb_rgba(&picture, W, H);
        assert!(
            left * 3 < woven_comb,
            "{name} left {left} of the woven {woven_comb}"
        );
    }
}

#[test]
fn the_temporal_filters_keep_a_still_picture_and_bob_does_not() {
    let still = frozen(8);
    let truth = filter::picture(
        &still,
        &still,
        &still,
        &field::plan(&About::of_frame(&still), Deinterlace::Weave)[0],
        settings(Deinterlace::Weave),
    );
    let error_of = |quality| {
        let output = field::plan(&About::of_frame(&still), quality)[0];
        let got = filter::picture(&still, &still, &still, &output, settings(quality));
        let colour = |picture: &[u8]| -> Vec<u8> {
            picture
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|pixel| pixel[..3].to_vec())
                .collect()
        };
        let (got, truth) = (colour(&got), colour(&truth));
        got.iter()
            .zip(&truth)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>()
            / truth.len() as u64
    };
    let bob = error_of(Deinterlace::Bob);
    let yadif = error_of(Deinterlace::Yadif);
    let bwdif = error_of(Deinterlace::Bwdif);
    assert!(bob > 4, "bob lost nothing on a still picture: {bob}");
    assert!(yadif * 4 < bob, "yadif {yadif} against bob {bob}");
    assert!(bwdif * 4 < bob, "bwdif {bwdif} against bob {bob}");
}

/// Mean absolute error of each filter on a sliding slanted edge, against the progressive
/// frame it was cut from (the comb metric cannot see soft interpolation). Slopes in
/// columns per row; two and three need the nested `CHECK(2)`.
fn diagonal_error(quality: Deinterlace, slope: f32) -> f64 {
    const N: usize = 5;
    let progressive = |time: usize| -> Plane {
        let mut data = vec![0u8; W * H];
        for y in 0..H {
            for x in 0..W {
                let edge = slope * y as f32 + 2.0 * time as f32 + 8.0;
                let across = ((x as f32 - edge) * 0.7 + 0.5).clamp(0.0, 1.0);
                data[y * W + x] = (16.0 + 219.0 * across).round() as u8;
            }
        }
        Plane {
            data,
            stride: W,
            width: W,
            height: H,
        }
    };
    let interlaced = |at: usize| -> Plane {
        let (even, odd) = (progressive(2 * at), progressive(2 * at + 1));
        let mut plane = even.clone();
        for y in (1..H).step_by(2) {
            plane.data[y * W..y * W + W].copy_from_slice(&odd.data[y * W..y * W + W]);
        }
        plane
    };
    let frames: Vec<Plane> = (0..N).map(interlaced).collect();
    let mut total = 0.0;
    let mut count = 0u64;
    for at in 1..N - 1 {
        for first_row in [0u32, 1] {
            let truth = progressive(2 * at + first_row as usize);
            let filtered = filter::plane(
                quality,
                &Samples::new(&frames[at - 1], 8),
                &Samples::new(&frames[at], 8),
                &Samples::new(&frames[at + 1], 8),
                first_row,
            );
            for y in (0..H).filter(|y| *y as u32 % 2 != first_row) {
                for x in 0..W {
                    let got = filtered[y * W + x] * 255.0;
                    total += f64::from((got - f32::from(truth.data[y * W + x])).abs());
                    count += 1;
                }
            }
        }
    }
    total / count as f64
}

/// The linear filter ahead of the cubic one here is not a fault: a sliding edge gains nothing from the outer taps.
/// The weight is on slope two: the linear filter, which uses only the two samples along the edge, shows whether the search reaches two columns.
#[test]
fn a_moving_diagonal_is_where_the_edge_search_earns_its_place() {
    const SLOPES: [f32; 3] = [1.0, 2.0, 3.0];
    let mut measured = Vec::new();
    for quality in [Deinterlace::Bob, Deinterlace::Yadif, Deinterlace::Bwdif] {
        let errors: Vec<f64> = SLOPES
            .iter()
            .map(|slope| diagonal_error(quality, *slope))
            .collect();
        for (slope, error) in SLOPES.iter().zip(&errors) {
            eprintln!("傾き {slope}  {:5}  平均誤差 {error:.3}", quality.name());
        }
        measured.push(errors);
    }
    let (bob, yadif, bwdif) = (&measured[0], &measured[1], &measured[2]);
    for at in 0..SLOPES.len() {
        assert!(
            yadif[at] * 4.0 < bob[at],
            "傾き {}: yadif {} bob {}",
            SLOPES[at],
            yadif[at],
            bob[at]
        );
        assert!(
            bwdif[at] * 2.0 < bob[at],
            "傾き {}: bwdif {} bob {}",
            SLOPES[at],
            bwdif[at],
            bob[at]
        );
    }
    // 辺の向きを二列先まで探せているかは、辺に沿った二画素だけを使う直線の方で見る。
    assert!(
        yadif[1] < yadif[0] * 1.5,
        "yadif: 一列の傾きで {} が二列で {} に伸びた。二列先の向きを探せていない",
        yadif[0],
        yadif[1]
    );
    // 三次は外側の二画素が辺から遠く、傾きとともに伸びる。それでも bob よりずっと小さい。
    assert!(
        bwdif[1] * 4.0 < bob[1],
        "bwdif: 二列の傾きで {} が bob の {} に近い",
        bwdif[1],
        bob[1]
    );
}

// Streams ffmpeg made (`testdata/make.sh`), through the engine as a recording would go, and
// decoded in software: what is measured here are the samples.

fn stream(name: &str, want: usize) -> Vec<Frame> {
    use std::collections::VecDeque;
    use zwcore::engine::{Engine, Out};
    use zwcore::filters::Container;

    let bytes = common::testdata(name);
    let container = Container::of_path(name).expect("a name that says which");
    let mut looking = Engine::new(container, 48_000);
    looking.push(&bytes[..bytes.len().min(4 << 20)]);
    let mut engine = Engine::new(container, 48_000);
    if let Some(program) = looking.best() {
        engine.want(program);
    }
    let mut frames = Vec::new();
    let mut decoding = Decoding::new(Prefer::Software);
    let mut outs = VecDeque::new();
    for chunk in bytes.chunks(65_536) {
        engine.push(chunk);
        while let Some(out) = engine.next() {
            decoding.push(out, &mut outs);
        }
        for out in outs.drain(..) {
            if let Out::Picture { picture, .. } = out
                && let Decoded::Planes(frame) = picture
            {
                frames.push(frame);
            }
        }
        if frames.len() >= want {
            return frames;
        }
    }
    decoding.flush(&mut outs);
    for out in outs.drain(..) {
        if let Out::Picture { picture, .. } = out
            && let Decoded::Planes(frame) = picture
        {
            frames.push(frame);
        }
    }
    assert!(frames.len() >= 3, "{name}: {} frames", frames.len());
    frames
}

/// Where a picture is moving, which is where combing is (measuring the whole frame drowns it).
fn moving(prev: &Plane, cur: &Plane, depth: u8) -> Vec<bool> {
    let (prev, cur) = (Samples::new(prev, depth), Samples::new(cur, depth));
    let step = 1 << (depth - 8);
    let mut mask = vec![false; cur.width() * cur.height()];
    for y in 0..cur.height() {
        for x in 0..cur.width() {
            let difference = cur.code(x, y).abs_diff(prev.code(x, y));
            mask[y * cur.width() + x] = difference > 24 * step;
        }
    }
    mask
}

fn combing(get: impl Fn(usize, usize) -> i64, mask: &[bool], width: usize, height: usize) -> u64 {
    let mut total = 0;
    for y in 1..height - 1 {
        for x in 0..width {
            if mask[y * width + x] {
                total += (2 * get(x, y) - get(x, y - 1) - get(x, y + 1)).unsigned_abs();
            }
        }
    }
    total
}

fn worst_pair(frames: &[Frame]) -> (usize, f64) {
    let depth = frames[0].format.depth;
    let mut worst = (1, 0.0);
    for at in 1..frames.len().saturating_sub(1) {
        let plane = &frames[at].planes[0];
        let mask = moving(&frames[at - 1].planes[0], plane, depth);
        let count = mask.iter().filter(|m| **m).count();
        if count * 64 < mask.len() {
            continue;
        }
        let samples = Samples::new(plane, depth);
        let step = f64::from(1u32 << (depth - 8));
        let comb = combing(
            |x, y| i64::from(samples.code(x, y)),
            &mask,
            plane.width,
            plane.height,
        );
        let per = comb as f64 / count as f64 / step;
        if per > worst.1 {
            worst = (at, per);
        }
    }
    worst
}

fn what_the_ladder_leaves(frames: &[Frame], at: usize) -> Vec<(Deinterlace, u64)> {
    let depth = frames[0].format.depth;
    let (width, height) = (frames[at].planes[0].width, frames[at].planes[0].height);
    let mask = moving(&frames[at - 1].planes[0], &frames[at].planes[0], depth);
    let full = f64::from((1u32 << depth) - 1);
    Deinterlace::LADDER
        .iter()
        .map(|quality| {
            let output = field::plan(&About::of_frame(&frames[at]), *quality)[0];
            let before = Samples::new(&frames[at - 1].planes[0], depth);
            let cur = Samples::new(&frames[at].planes[0], depth);
            let after = Samples::new(&frames[at + 1].planes[0], depth);
            let filtered = filter::plane(
                *quality,
                &before,
                &cur,
                &after,
                output.field.map_or(0, Parity::first_row),
            );
            let left = combing(
                |x, y| (f64::from(filtered[y * width + x]) * full) as i64,
                &mask,
                width,
                height,
            );
            (*quality, left)
        })
        .collect()
}

#[test]
fn an_interlaced_stream_comes_out_far_less_combed() {
    let frames = stream("mpeg2-interlaced.ts", 40);
    let (at, per) = worst_pair(&frames);
    let depth = frames[0].format.depth;
    let plane = &frames[at].planes[0];
    let mask = moving(&frames[at - 1].planes[0], plane, depth);
    let samples = Samples::new(plane, depth);
    let woven_comb = combing(
        |x, y| i64::from(samples.code(x, y)),
        &mask,
        plane.width,
        plane.height,
    );
    eprintln!(
        "mpeg2-interlaced.ts 第{at}枚  動く画素あたりのコーミング {per:.1}  合計 {woven_comb}"
    );
    assert!(per > 25.0, "this stream is not combed enough to test with");
    for (quality, left) in what_the_ladder_leaves(&frames, at) {
        eprintln!("  {}  残り {left}", quality.name());
        assert!(
            left * 3 < woven_comb,
            "{} left {left} of {woven_comb}",
            quality.name()
        );
    }
}

#[test]
fn interlaced_frame_pictures_are_shown_as_two_fields() {
    let frames = stream("mpeg2-interlaced.ts", 40);
    assert!(
        frames
            .iter()
            .all(|frame| frame.structure == Structure::Frame)
    );
    let (at, per) = worst_pair(&frames);
    eprintln!("mpeg2-interlaced.ts 第{at}枚  動く画素あたりのコーミング {per:.1}");
    let plan = field::plan(&About::of_frame(&frames[at]), Deinterlace::Bwdif);
    assert_eq!(plan.len(), 2);
    assert_eq!(
        plan[0].field,
        Some(field::first_field(&About::of_frame(&frames[at])))
    );
    assert_ne!(plan[1].field, plan[0].field);
    for (quality, left) in what_the_ladder_leaves(&frames, at) {
        eprintln!("  {}  残り {left}", quality.name());
    }
    let picture = filter::picture(
        &frames[at - 1],
        &frames[at],
        &frames[at + 1],
        &plan[0],
        settings(Deinterlace::Bwdif),
    );
    assert_eq!(picture.len(), 352 * 240 * 4);
}

// Inverse telecine. No flag says film is film, so it is tested against ffmpeg's `telecine`
// filter output (luma only, `testdata/make.sh`).

fn y4m(name: &str) -> Option<Vec<Frame>> {
    let bytes = common::testdata(name);
    let end = bytes.iter().position(|b| *b == b'\n')?;
    let header = std::str::from_utf8(&bytes[..end]).ok()?;
    let field = |tag: char| -> Option<usize> {
        header
            .split(' ')
            .find(|word| word.starts_with(tag))
            .and_then(|word| word[1..].parse().ok())
    };
    let (width, height) = (field('W')?, field('H')?);
    let luma = width * height;
    // `Cmono` is luma alone; anything else here is 4:2:0, whose chroma is not looked at.
    let mono = header.split(' ').any(|word| word == "Cmono");
    let whole = if mono { luma } else { luma * 3 / 2 };
    let mut frames = Vec::new();
    let mut at = end + 1;
    while at < bytes.len() {
        let head = bytes[at..].iter().position(|b| *b == b'\n')? + 1;
        at += head;
        if at + whole > bytes.len() {
            break;
        }
        let plane = |width: usize, height: usize, data: Vec<u8>| Plane {
            data,
            stride: width,
            width,
            height,
        };
        frames.push(Frame {
            tag: frames.len() as u64,
            format: Format {
                width: width as u32,
                height: height as u32,
                depth: 8,
                chroma: 1,
                rate: (30000, 1001),
                progressive: false,
                aspect: (1, 1),
                field_pictures: false,
                woven: false,
                colour: Default::default(),
            },
            planes: [
                plane(width, height, bytes[at..at + luma].to_vec()),
                plane(width / 2, height / 2, vec![128; luma / 4]),
                plane(width / 2, height / 2, vec![128; luma / 4]),
            ],
            structure: Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
        });
        at += whole;
    }
    Some(frames)
}

/// A 2:3 pulldown made by ffmpeg has to come back exactly (weaving has no error).
#[test]
fn a_pulldown_made_by_ffmpeg_comes_back_as_the_frames_it_was_made_from() {
    let (Some(source), Some(telecined)) = (y4m("telecine-source.y4m"), y4m("telecine-23.y4m"))
    else {
        panic!("the Y4M files do not read");
    };
    assert!(telecined.len() > source.len(), "テレシネされていない");
    let mut detector = telecine::Detector::new();
    let mut recovered = 0;
    let mut deinterlaced = 0;
    for at in 0..telecined.len() {
        let cadence = detector.push(&telecined[at]);
        let plan = field::plan_of(
            &About::of_frame(&telecined[at]),
            Deinterlace::Bwdif,
            cadence,
        );
        if cadence == Cadence::None {
            deinterlaced += plan.len();
            continue;
        }
        let Some(output) = plan.first() else { continue };
        let before = &telecined[at.saturating_sub(1)];
        let cur = &telecined[at];
        let mut got = cur.planes[0].data.clone();
        if let Some(parity) = output.weave {
            let width = cur.planes[0].width;
            for y in (parity.first_row() as usize..cur.planes[0].height).step_by(2) {
                got[y * width..y * width + width]
                    .copy_from_slice(&before.planes[0].data[y * width..y * width + width]);
            }
        }
        assert!(
            source.iter().any(|frame| frame.planes[0].data == got),
            "第{at}枚が元のどのフレームとも一致しない"
        );
        recovered += 1;
    }
    eprintln!(
        "{}枚のテレシネのうち {recovered} 枚を復元し、{deinterlaced} 枚をデインタレースした",
        telecined.len()
    );
    assert!(
        recovered * 5 > telecined.len() * 3,
        "{recovered} of {} recovered",
        telecined.len()
    );
}

#[test]
fn interlaced_video_is_not_mistaken_for_film() {
    let name = "mpeg2-interlaced.ts";
    let frames = stream(name, 40);
    let mut detector = telecine::Detector::new();
    let fired = frames
        .iter()
        .filter(|frame| detector.push(frame) != Cadence::None)
        .count();
    eprintln!("{name}  {}枚のうち誤検出 {fired}枚", frames.len());
    assert_eq!(fired, 0, "{name} をテレシネと誤検出した");
}

/// Film telecined by ffmpeg and coded as interlaced MPEG-2: taken once and held.
#[test]
fn the_detector_does_not_take_and_drop_a_cadence_over_and_over() {
    let frames = stream("mpeg2-telecine.ts", usize::MAX);
    let mut detector = telecine::Detector::new();
    let mut locked = false;
    let mut changes = 0;
    let mut run = String::new();
    for frame in &frames {
        let cadence = detector.push(frame);
        if detector.locked() != locked {
            locked = !locked;
            changes += 1;
        }
        run.push(match cadence {
            Cadence::None => '.',
            Cadence::Whole => 'W',
            Cadence::Spanning => 'S',
            Cadence::Drop => 'D',
        });
    }
    eprintln!("{}枚、検出・解除が {changes} 回\n{run}", frames.len());
    assert!(
        frames.len() > 100,
        "{}枚しかデコードできていない",
        frames.len()
    );
    assert!(changes >= 1, "テレシネを検出しなかった");
    assert!(changes <= 2, "検出・解除が {changes} 回");
}

#[test]
fn the_padding_a_decoder_leaves_never_reaches_the_picture() {
    let padded = woven(1, 8, Parity::Top);
    assert!(
        padded.planes[0].stride > padded.planes[0].width,
        "the case is not being tested"
    );
    let mut tight = padded.clone();
    let mut noisy = padded.clone();
    for plane in 0..3 {
        let (stride, width, height) = {
            let plane = &padded.planes[plane];
            (plane.stride, plane.width, plane.height)
        };
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            data.extend_from_slice(&padded.planes[plane].data[y * stride..y * stride + width]);
        }
        tight.planes[plane].data = data;
        tight.planes[plane].stride = width;
        for y in 0..height {
            for x in width..stride {
                noisy.planes[plane].data[y * stride + x] = 255;
            }
        }
    }
    let output = field::plan(&About::of_frame(&padded), Deinterlace::Bwdif)[0];
    let from =
        |frame: &Frame| filter::picture(frame, frame, frame, &output, settings(Deinterlace::Bwdif));
    assert_eq!(from(&tight), from(&padded));
    assert_eq!(from(&noisy), from(&padded));
}

#[test]
fn a_decoded_stream_arrives_padded() {
    let frames = stream("mpeg2-interlaced.ts", 4);
    for plane in &frames[0].planes {
        assert!(plane.stride >= plane.width, "{plane:?}");
    }
    eprintln!(
        "mpeg2-interlaced.ts  {}x{}  行の長さ {} / {} / {}",
        frames[0].planes[0].width,
        frames[0].planes[0].height,
        frames[0].planes[0].stride,
        frames[0].planes[1].stride,
        frames[0].planes[2].stride,
    );
}

// The GPU. Without one these say so and pass.

#[cfg(feature = "wgpu")]
mod on_a_gpu {
    use super::*;
    use zwcore::output::video::colour::{Conversion, Mapping, quantise};
    use zwcore::output::video::gpu::{Capability, Gpu, Onto, Upload, Video};
    use zwcore::output::video::{Primaries, Settings, Space, Transfer};

    fn open() -> Option<Gpu> {
        match Gpu::open() {
            Ok(gpu) => {
                eprintln!("{}", gpu.name());
                Some(gpu)
            }
            Err(error) => {
                eprintln!("GPUが無いので飛ばす: {error}");
                None
            }
        }
    }

    fn rendered(gpu: &Gpu, name: &str) -> Vec<u8> {
        let (_, settings, depth, first, which) = reference(name);
        let frames = frames_of(name, depth, first);
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            frames[1].format,
            settings,
        )
        .expect("a format the pool can hold");
        for frame in &frames {
            video
                .push(&Decoded::Planes(frame.clone()))
                .expect("six slots for three frames");
        }
        let of_first = field::plan(&About::of_frame(&frames[0]), settings.deinterlace).len();
        for _ in 0..of_first + which {
            video.read_back().expect("an output for every field");
        }
        video.read_back().expect("an output for every field").0
    }

    /// 表示のサーフェスへ出したものを符号値のまま読み戻す（`read_back` と違い blit とガンマを通る）。
    fn shown_on(gpu: &Gpu, name: &str, target: wgpu::TextureFormat) -> Vec<u8> {
        let (_, settings, depth, first, which) = reference(name);
        let frames = frames_of(name, depth, first);
        shown_on_with(gpu, frames, settings, which, target, Space::Srgb)
    }

    fn shown_on_with(
        gpu: &Gpu,
        frames: [Frame; 3],
        settings: Settings,
        which: usize,
        target: wgpu::TextureFormat,
        space: Space,
    ) -> Vec<u8> {
        shown_on_placed(gpu, frames, settings, which, target, space, None)
    }

    fn shown_on_placed(
        gpu: &Gpu,
        mut frames: [Frame; 3],
        settings: Settings,
        which: usize,
        target: wgpu::TextureFormat,
        space: Space,
        placed: Option<(f32, f32)>,
    ) -> Vec<u8> {
        // アスペクト比は正方にし直す（16:9 のままだと blit が黒を足してサンプルが混ざる）。
        for frame in &mut frames {
            frame.format.aspect = (1, 1);
        }
        let format = frames[1].format;
        let mut video = Video::new(&gpu.device, &gpu.queue, target, format, settings)
            .expect("a format the pool can hold");
        video.set_space(space);
        if let Some((white, peak)) = placed {
            video.set_sdr_white(white);
            video.set_hlg_peak(peak);
        }
        for frame in &frames {
            video
                .push(&Decoded::Planes(frame.clone()))
                .expect("six slots for three frames");
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display"),
            size: wgpu::Extent3d {
                width: format.width,
                height: format.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: target,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let onto = Onto::all((format.width, format.height));
        let of_first = field::plan(&About::of_frame(&frames[0]), settings.deinterlace).len();
        for _ in 0..of_first + which {
            video
                .draw_onto(&view, onto)
                .expect("an output for every field");
        }
        video
            .draw_onto(&view, onto)
            .expect("an output for every field");
        read_texture(gpu, &texture, format.width, format.height)
    }

    fn read_texture(gpu: &Gpu, texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> {
        let pixel = texture.format().block_copy_size(None).unwrap_or(4);
        let stride = (width * pixel).div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the queue drains");
        let mapped = buffer.get_mapped_range(..).expect("the buffer maps");
        let mut out = Vec::with_capacity((width * height * pixel) as usize);
        for y in 0..height as usize {
            let at = y * stride as usize;
            out.extend_from_slice(&mapped[at..at + (width * pixel) as usize]);
        }
        drop(mapped);
        buffer.unmap();
        out
    }

    /// 表示のサーフェスまで通したものが、正解と同じ符号値であること（sRGB のサーフェスへ
    /// リニアを書いて暗部が潰れていた所）。
    #[test]
    fn what_reaches_the_display_is_what_the_reference_says() {
        let Some(gpu) = open() else {
            return;
        };
        for name in ["bwdif-top", "bob-top"] {
            let (want, ..) = reference(name);
            for target in [
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ] {
                let shown = shown_on(&gpu, name, target);
                assert_eq!(shown.len(), want.len(), "{name} {target:?}");
                let worst = want
                    .iter()
                    .zip(&shown)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0);
                assert_eq!(worst, 0, "{name} は {target:?} のサーフェスで正解と違う");
            }
        }
    }

    #[test]
    fn an_hlg_picture_is_mapped_onto_each_kind_of_surface_as_the_reference_says() {
        let Some(gpu) = open() else {
            return;
        };
        let depth = 10;
        let frames = [
            woven(0, depth, Parity::Top),
            woven(1, depth, Parity::Top),
            woven(2, depth, Parity::Top),
        ];
        let settings = Settings {
            deinterlace: Deinterlace::Weave,
            deinterlace_chosen: false,
            telecine: false,
            matrix: zwcore::output::video::Matrix::Bt2020,
            limited: true,
            transfer: Transfer::Hlg,
            primaries: Primaries::Bt2020,
        };
        let conversion = Conversion::new(settings.matrix, depth, settings.limited);
        let cur = &frames[1];
        let y = Samples::new(&cur.planes[0], depth);
        let cb = Samples::new(&cur.planes[1], depth);
        let cr = Samples::new(&cur.planes[2], depth);
        let carried = |x: i32, row: i32| -> [f32; 3] {
            conversion.to_rgb(y.at(x, row), cb.at(x / 2, row / 2), cr.at(x / 2, row / 2))
        };
        struct Case {
            target: wgpu::TextureFormat,
            space: Space,
            allowed: u32,
            placed: Option<(f32, f32)>,
        }
        for case in [
            Case {
                target: wgpu::TextureFormat::Rgba8Unorm,
                space: Space::Srgb,
                allowed: 3,
                placed: None,
            },
            Case {
                target: wgpu::TextureFormat::Rgba8UnormSrgb,
                space: Space::Srgb,
                allowed: 3,
                placed: None,
            },
            Case {
                target: wgpu::TextureFormat::Rgb10a2Unorm,
                space: Space::Pq,
                allowed: 12,
                placed: None,
            },
            Case {
                target: wgpu::TextureFormat::Rgb10a2Unorm,
                space: Space::Hlg,
                allowed: 12,
                placed: None,
            },
            Case {
                target: wgpu::TextureFormat::Rgba16Float,
                space: Space::ScRgb,
                allowed: 12,
                placed: None,
            },
            Case {
                target: wgpu::TextureFormat::Rgba16Float,
                space: Space::ScRgb,
                allowed: 12,
                placed: Some((80.0, 400.0)),
            },
            Case {
                target: wgpu::TextureFormat::Rgb10a2Unorm,
                space: Space::Pq,
                allowed: 12,
                placed: Some((203.0, 400.0)),
            },
        ] {
            let mut mapping = Mapping::new(settings.transfer, settings.primaries, case.space);
            if let Some((white, peak)) = case.placed {
                mapping = mapping.with_white(white).with_peak(peak);
            }
            let shown = shown_on_placed(
                &gpu,
                frames.clone(),
                settings,
                0,
                case.target,
                case.space,
                case.placed,
            );
            let mut worst = 0u32;
            let mut sum = 0u64;
            for row in 0..H as i32 {
                for x in 0..W as i32 {
                    let want = mapping.apply(carried(x, row), false);
                    let at = (row as usize * W + x as usize)
                        * if case.target == wgpu::TextureFormat::Rgba16Float {
                            8
                        } else {
                            4
                        };
                    let (got, want): ([u32; 3], [u32; 3]) = match case.target {
                        wgpu::TextureFormat::Rgb10a2Unorm => {
                            let word = u32::from_le_bytes([
                                shown[at],
                                shown[at + 1],
                                shown[at + 2],
                                shown[at + 3],
                            ]);
                            (
                                [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff],
                                want.map(|c| (c * 1023.0 + 0.5) as u32),
                            )
                        }
                        wgpu::TextureFormat::Rgba16Float => {
                            let half = |lo: u8, hi: u8| -> f32 {
                                let bits = u16::from_le_bytes([lo, hi]);
                                let sign = if bits >> 15 == 0 { 1.0 } else { -1.0 };
                                let exponent = i32::from((bits >> 10) & 0x1f);
                                let mantissa = f32::from(bits & 0x3ff);
                                if exponent == 0 {
                                    sign * mantissa * 2f32.powi(-24)
                                } else {
                                    sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15)
                                }
                            };
                            (
                                [
                                    (half(shown[at], shown[at + 1]) * 1000.0) as u32,
                                    (half(shown[at + 2], shown[at + 3]) * 1000.0) as u32,
                                    (half(shown[at + 4], shown[at + 5]) * 1000.0) as u32,
                                ],
                                want.map(|c| (c * 1000.0) as u32),
                            )
                        }
                        _ => (
                            [
                                u32::from(shown[at]),
                                u32::from(shown[at + 1]),
                                u32::from(shown[at + 2]),
                            ],
                            want.map(|c| u32::from(quantise(c))),
                        ),
                    };
                    for (g, w) in got.iter().zip(&want) {
                        let slack = if case.target == wgpu::TextureFormat::Rgba16Float {
                            w / 200
                        } else {
                            0
                        };
                        let off = g.abs_diff(*w).saturating_sub(slack);
                        sum += u64::from(off);
                        worst = worst.max(off);
                    }
                }
            }
            eprintln!(
                "{:?} のサーフェスに {:?} {:?}: 最大の差 {worst}、平均 {:.3}",
                case.space,
                case.target,
                case.placed,
                sum as f64 / (W * H * 3) as f64
            );
            assert!(
                worst <= case.allowed,
                "{:?} のサーフェス {:?} で正解と {worst} 違う（許すのは {}）",
                case.space,
                case.placed,
                case.allowed
            );
        }
    }

    #[test]
    fn a_sheet_of_controls_lands_on_an_hdr_surface_at_sdr_white() {
        use zwcore::output::video::gpu::Sheet;
        let Some(gpu) = open() else {
            return;
        };
        let size = (8u32, 8u32);
        let surface = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgb10a2Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let onto = surface.create_view(&wgpu::TextureViewDescriptor::default());
        let mapping = Mapping::new(Transfer::Ui, Primaries::Bt709, Space::Pq);
        let grey = zwcore::output::video::colour::nits_to_pq(100.0) as f64;
        for (alpha, under) in [(1.0f64, 0.0f64), (0.5, 0.0), (0.5, grey)] {
            let mut sheet = Sheet::new(
                &gpu.device,
                wgpu::TextureFormat::Rgb10a2Unorm,
                Space::Pq,
                zwcore::output::video::colour::SDR_WHITE,
                size,
            );
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            let clear = |encoder: &mut wgpu::CommandEncoder, view, colour| {
                let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(colour),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            };
            clear(
                &mut encoder,
                sheet.view(),
                wgpu::Color {
                    r: 1.0 * alpha,
                    g: 0.5 * alpha,
                    b: 0.25 * alpha,
                    a: alpha,
                },
            );
            clear(
                &mut encoder,
                &onto,
                wgpu::Color {
                    r: under,
                    g: under,
                    b: under,
                    a: 1.0,
                },
            );
            sheet.lay(&gpu.device, &gpu.queue, &mut encoder, &surface, None);
            gpu.queue.submit([encoder.finish()]);
            let shown = read_texture(&gpu, &surface, size.0, size.1);
            let word = u32::from_le_bytes([shown[0], shown[1], shown[2], shown[3]]);
            let got = [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff];
            let below = ((under * 1023.0 + 0.5).floor() / 1023.0) as f32;
            let want = mapping
                .lay([1.0, 0.5, 0.25], alpha as f32, [below; 3], false)
                .map(|c| (c * 1023.0 + 0.5) as u32);
            for (g, w) in got.iter().zip(&want) {
                assert!(
                    g.abs_diff(*w) <= 4,
                    "アルファ {alpha}、下 {under:.3}: サーフェスに {got:?}、正解は {want:?}"
                );
            }
        }
        let white = mapping.apply([1.0; 3], false);
        for c in white {
            assert!((c - 0.5807).abs() < 0.003, "{white:?}");
        }
    }

    fn rendered_biplanar(gpu: &Gpu, name: &str) -> Option<Vec<u8>> {
        rendered_biplanar_padded(gpu, name, 0)
    }

    /// The same, in textures `pad` luma rows taller (as hardware decoders write). The padding
    /// is white so any tap reaching it shows.
    fn rendered_biplanar_padded(gpu: &Gpu, name: &str, pad: usize) -> Option<Vec<u8>> {
        use zwcore::output::video::colour::Conversion;
        use zwcore::output::video::shader::{Params, Shape};

        let (_, settings, depth, first, which) = reference(name);
        let wide = depth > 8;
        if wide
            && !gpu
                .device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
        {
            eprintln!("十六ビットの正規化が無いので飛ばす");
            return None;
        }
        let frames = frames_of(name, depth, first);
        let bytes = if wide { 2usize } else { 1 };
        let (luma_format, chroma_format) = if wide {
            (
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm,
            )
        } else {
            (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
        };

        let shift = if wide { 16 - u32::from(depth) } else { 0 };
        let store = |value: u32, into: &mut Vec<u8>| {
            let value = value << shift;
            into.push(value as u8);
            if wide {
                into.push((value >> 8) as u8);
            }
        };
        let read = |plane: &mediadecoder::video::Plane, x: usize, y: usize| -> u32 {
            let row = plane.row(y);
            if wide {
                u32::from(row[x * 2]) | (u32::from(row[x * 2 + 1]) << 8)
            } else {
                u32::from(row[x])
            }
        };

        let texture = |format: wgpu::TextureFormat, width: u32, height: u32, data: &[u8]| {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("plane"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let row = data.len() as u32 / height;
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        };

        let plan = field::plan(&About::of_frame(&frames[1]), settings.deinterlace);
        let output = plan[which.min(plan.len() - 1)];
        let picked = |source: zwcore::output::video::Source| match source {
            zwcore::output::video::Source::Previous => &frames[0],
            zwcore::output::video::Source::Current => &frames[1],
            zwcore::output::video::Source::Next => &frames[2],
        };
        let window = [picked(output.before), &frames[1], picked(output.after)];

        let mut views = Vec::new();
        for frame in window {
            let plane = &frame.planes[0];
            let mut data = Vec::with_capacity(plane.width * (plane.height + pad) * bytes);
            for y in 0..plane.height {
                for x in 0..plane.width {
                    store(read(plane, x, y), &mut data);
                }
            }
            data.resize(plane.width * (plane.height + pad) * bytes, 0xff);
            views.push(texture(
                luma_format,
                plane.width as u32,
                (plane.height + pad) as u32,
                &data,
            ));
        }
        for frame in window {
            let (cb, cr) = (&frame.planes[1], &frame.planes[2]);
            let rows = cb.height + pad / 2;
            let mut data = Vec::with_capacity(cb.width * rows * 2 * bytes);
            for y in 0..cb.height {
                for x in 0..cb.width {
                    store(read(cb, x, y), &mut data);
                    store(read(cr, x, y), &mut data);
                }
            }
            data.resize(cb.width * rows * 2 * bytes, 0xff);
            views.push(texture(chroma_format, cb.width as u32, rows as u32, &data));
        }

        let rows = filter::Rows::of(&output);
        let conversion = Conversion::new(settings.matrix, depth, settings.limited);
        let params = Params {
            first_row: rows.first,
            mate_row: rows.mate,
            luma_offset: conversion.luma_offset,
            luma_gain: conversion.luma_gain,
            chroma_offset: conversion.chroma_offset,
            chroma_gain: conversion.chroma_gain,
            kr: conversion.kr,
            kb: conversion.kb,
            width: frames[1].planes[0].width as u32,
            height: frames[1].planes[0].height as u32,
            comb: 0.0,
            motion: 0.0,
        };
        let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("conversion"),
            size: Params::SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&uniform, 0, &params.bytes());

        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..Shape::Biplanar.bindings())
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: Shape::Biplanar.bindings(),
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("planes"),
                entries: &entries,
            });
        let mut bindings: Vec<wgpu::BindGroupEntry> = views
            .iter()
            .enumerate()
            .map(|(at, view)| wgpu::BindGroupEntry {
                binding: at as u32,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        bindings.push(wgpu::BindGroupEntry {
            binding: Shape::Biplanar.bindings(),
            resource: uniform.as_entire_binding(),
        });
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planes"),
            layout: &layout,
            entries: &bindings,
        });

        let module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("deinterlace"),
                source: wgpu::ShaderSource::Wgsl(
                    zwcore::output::video::shader::deinterlace(
                        settings.deinterlace,
                        depth,
                        Shape::Biplanar,
                    )
                    .into(),
                ),
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("deinterlace"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("deinterlace"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let picture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picture"),
            size: wgpu::Extent3d {
                width: W as u32,
                height: H as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = picture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deinterlace"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &picture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((W * 4) as u32),
                    rows_per_image: Some(H as u32),
                },
            },
            wgpu::Extent3d {
                width: W as u32,
                height: H as u32,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let out = readback.get_mapped_range(..).expect("the picture").to_vec();
        readback.unmap();
        Some(out)
    }

    #[test]
    fn two_planes_draw_the_picture_three_planes_do() {
        let Some(gpu) = open() else {
            return;
        };
        for name in CASES {
            let Some(picture) = rendered_biplanar(&gpu, name) else {
                continue;
            };
            let golden = std::fs::read(golden_path(name)).expect("the golden");
            assert_eq!(golden.len(), picture.len(), "{name}");
            let differences: Vec<u8> = golden
                .iter()
                .zip(&picture)
                .map(|(a, b)| a.abs_diff(*b))
                .collect();
            let worst = differences.iter().copied().max().unwrap_or(0);
            let decided = differences.iter().filter(|d| **d > 2).count();
            eprintln!("{name}  最大の差 {worst}  丸めを超えるもの {decided}");
            if !name.starts_with("bwdif") {
                assert!(worst <= 2, "{name} differs by {worst}");
                continue;
            }
            assert!(
                decided * 100 < golden.len(),
                "{name}: {decided} 成分が丸めでは説明がつかない"
            );
            assert!(worst <= 24, "{name} differs by {worst}");
        }
    }

    /// A picture drawn out of a taller texture is the same picture: catches the D3D12 bottom
    /// edge (`textureDimensions` returned 1088 instead of 1080).
    #[test]
    fn a_picture_in_a_taller_texture_is_the_same_picture() {
        let Some(gpu) = open() else {
            return;
        };
        for name in CASES {
            let Some(plain) = rendered_biplanar_padded(&gpu, name, 0) else {
                continue;
            };
            let padded = rendered_biplanar_padded(&gpu, name, 8).expect("the same case again");
            let worst = plain
                .iter()
                .zip(&padded)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            eprintln!("{name}  パディングの分の差 {worst}");
            assert_eq!(worst, 0, "{name}: サーフェスの高さが映像に漏れている");
        }
    }

    /// The shaders and the Rust reference agree within two; the cubic filter is allowed a few more samples
    /// because the synthetic picture is full of exact ties in its comparison.
    #[test]
    fn the_shaders_make_the_same_pictures_as_the_reference() {
        let Some(gpu) = open() else { return };
        for name in CASES {
            let (golden, ..) = reference(name);
            let got = rendered(&gpu, name);
            assert_eq!(got.len(), golden.len(), "{name}");
            let differences: Vec<u8> = got
                .iter()
                .zip(&golden)
                .map(|(a, b)| a.abs_diff(*b))
                .collect();
            let worst = differences.iter().copied().max().unwrap_or(0);
            let off = differences.iter().filter(|d| **d != 0).count();
            let decided = differences.iter().filter(|d| **d > 2).count();
            eprintln!(
                "{name}  最大の差 {worst}  違う成分 {off} / {}  うち丸めを超えるもの {decided}",
                golden.len()
            );
            if !name.starts_with("bwdif") {
                assert!(worst <= 2, "{name} differs by {worst}");
                continue;
            }
            assert!(
                decided * 100 < golden.len(),
                "{name}: {decided} 成分が丸めでは説明がつかない"
            );
            assert!(worst <= 24, "{name} differs by {worst}");
        }
    }

    #[test]
    fn both_ways_of_uploading_give_the_same_picture() {
        let Some(gpu) = open() else { return };
        let frames = [
            woven(0, 8, Parity::Top),
            woven(1, 8, Parity::Top),
            woven(2, 8, Parity::Top),
        ];
        let mut pictures = Vec::new();
        for upload in [Upload::Staged, Upload::Mapped] {
            let mut video = Video::new(
                &gpu.device,
                &gpu.queue,
                wgpu::TextureFormat::Rgba8Unorm,
                frames[1].format,
                settings(Deinterlace::Bwdif),
            )
            .expect("a format the pool can hold");
            video.set_upload(upload);
            for frame in &frames {
                video
                    .push(&Decoded::Planes(frame.clone()))
                    .expect("six slots for three frames");
            }
            video.read_back();
            video.read_back();
            pictures.push(video.read_back().expect("an output").0);
        }
        assert_eq!(pictures[0], pictures[1]);
    }

    #[test]
    fn the_ladder_is_measured_on_this_machine() {
        let Some(gpu) = open() else { return };
        let format = Format {
            width: 1920,
            height: 1080,
            depth: 8,
            chroma: 1,
            rate: (30000, 1001),
            progressive: false,
            aspect: (16, 9),
            field_pictures: false,
            woven: false,
            colour: Default::default(),
        };
        let capability = Capability::measure(&gpu, format);
        eprintln!(
            "アップロード  write_texture {:.3} ms / マップしたバッファ {:.3} ms → {:?}",
            capability.staged_ms, capability.mapped_ms, capability.upload
        );
        for (quality, cost) in &capability.costs {
            eprintln!(
                "{:5}  {cost:.2} ms/枚  予算の {:.1}%",
                quality.name(),
                cost / zwcore::output::video::FIELD_BUDGET_MS * 100.0
            );
        }
        eprintln!("既定は {}", capability.affordable().name());
        assert_eq!(capability.costs.len(), 4);
        assert!(capability.costs.iter().all(|(_, cost)| *cost > 0.0));
    }

    /// What `push` says a frame became has to be what the schedule holds, including the
    /// inverse telecine (the field matching decides on the GPU, so a caller counting for
    /// itself drifts once it locks).
    #[test]
    fn what_push_says_a_frame_became_is_what_the_schedule_holds() {
        let Some(gpu) = open() else { return };
        // The field matching settles on what it has seen of a few seconds, and counts combing
        // a picture the size of the Y4M ones gives too little of.
        let telecined = stream("mpeg2-telecine.ts", 100);
        let mut settings = settings(Deinterlace::Bwdif);
        settings.telecine = true;
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            telecined[0].format,
            settings,
        )
        .expect("a format the pool can hold");
        let mut said_by_push = 0usize;
        let mut pushed = 0usize;
        let mut held = 0usize;
        let mut said_by_plan_alone = 0usize;
        for frame in &telecined {
            let about = About::of_frame(frame);
            let before = video.pending();
            let Ok(outputs) = video.push(&Decoded::Planes(frame.clone())) else {
                break;
            };
            said_by_push += outputs;
            pushed += 1;
            held += video.pending() - before;
            // 進めないとスロットが埋まって push が失敗する。
            while video.skip() {}
            // コーミングの集計は待たずに読み戻す。再生の速さなら間に合うが、ここでは GPU を待つ。
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
            said_by_plan_alone += field::plan(&about, Deinterlace::Bwdif).len();
        }
        // 出力の予定は後ろのフレームを待って入るので、最後のフレームの分は流し終えてから入る。
        let before = video.pending();
        video.flush();
        held += video.pending() - before;
        assert_eq!(pushed, telecined.len(), "途中で push が失敗した");
        assert_eq!(
            said_by_push, held,
            "push が返した枚数が予定に入った数と違う"
        );
        assert_ne!(
            said_by_push, said_by_plan_alone,
            "この素材では逆テレシネが噛み合っていないので、テストになっていない"
        );
    }

    #[test]
    fn a_redrawn_surface_shows_the_last_picture_drawn() {
        let Some(gpu) = open() else { return };
        let frames = [
            woven(0, 8, Parity::Top),
            woven(1, 8, Parity::Top),
            woven(2, 8, Parity::Top),
        ];
        let format = frames[0].format;
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            format,
            settings(Deinterlace::Weave),
        )
        .expect("a format the pool can hold");
        let texture = |label: &str| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: format.width,
                    height: format.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let onto = Onto::all((format.width, format.height));
        let drawn = texture("drawn");
        let again = texture("again");
        let mut last: Option<Vec<u8>> = None;
        for frame in &frames {
            video.push(&Decoded::Planes(frame.clone())).expect("a slot");
            if !video.ready() {
                continue;
            }
            let view = drawn.create_view(&wgpu::TextureViewDescriptor::default());
            video.draw_onto(&view, onto).expect("an output");
            let shown = read_texture(&gpu, &drawn, format.width, format.height);
            assert!(
                last.as_ref() != Some(&shown),
                "the next frame drew the same picture"
            );
            for _ in 0..3 {
                let view = again.create_view(&wgpu::TextureViewDescriptor::default());
                video.redraw_onto(&view, onto);
                let redrawn = read_texture(&gpu, &again, format.width, format.height);
                assert_eq!(redrawn, shown, "the redraw showed a different picture");
            }
            last = Some(shown);
        }
        assert!(last.is_some(), "nothing was drawn");
    }

    #[test]
    fn the_last_frame_of_a_stream_is_not_lost() {
        let Some(gpu) = open() else { return };
        let frame = woven(0, 8, Parity::Top);
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            frame.format,
            settings(Deinterlace::Yadif),
        )
        .expect("a format the pool can hold");
        video.push(&Decoded::Planes(frame.clone())).expect("a slot");
        assert!(!video.ready(), "shown before the frame after it arrived");
        video.flush();
        assert_eq!(video.pending(), 2, "a frame is two fields");
    }
}
