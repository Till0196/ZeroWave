// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! macOS のデコーダーを、ソフトウェアデコーダーと突き合わせる（`tests/vaapi.rs` と同じ形）。最後のテストは
//! `CVMetalTextureCache` で取り込んでレンダリングしたフレームを比べる。素材は ffmpeg で作った物（`testdata/make.sh`）。

// 基準は内蔵のソフトウェアデコーダーなので、その feature も要る。
#![cfg(all(
    feature = "videotoolbox",
    target_vendor = "apple",
    feature = "h264-decoder",
    feature = "hevc-decoder"
))]

use mediadecoder::es::{Codec, Format};
use zwcore::decode::{Decode, Decoded};
use zwcore::filters::cut::Cutout;

mod common;
use common::units;

fn luma(decoder: &mut dyn Decode, units: &[Cutout], most: usize) -> Vec<(u64, Vec<u8>, usize)> {
    let mut out = Vec::new();
    for Cutout {
        tag,
        picture,
        format,
        data,
    } in units
    {
        let _ = decoder.decode(*tag, picture, *format, data);
        while let Some(read) = decoder.read_back() {
            if let Some((bytes, pitch)) = read.luma() {
                out.push((read.about.tag, bytes.to_vec(), pitch));
            }
        }
        while let Some(Decoded::Planes(frame)) = decoder.take() {
            let plane = &frame.planes[0];
            let wide = usize::from(frame.format.depth > 8) + 1;
            let mut luma = Vec::new();
            for y in 0..plane.height {
                luma.extend_from_slice(&plane.row(y)[..plane.width * wide]);
            }
            out.push((frame.tag, luma, plane.width * wide));
        }
        if out.len() >= most {
            break;
        }
    }
    out
}

#[test]
fn h264_comes_out_sample_for_sample() {
    let cutting = units("h264-baseline.ts");
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    assert_eq!(codec, Codec::H264);
    let mut hardware = zwcore::decode::open(codec, framing, zwcore::decode::Prefer::Platform);
    let mut software = zwcore::decode::software(codec, framing);
    let theirs = luma(hardware.as_mut(), units, 16);
    let mine = luma(software.as_mut(), units, 16);
    assert!(
        !theirs.is_empty(),
        "ハードウェアがフレームを出しませんでした"
    );
    let common = theirs.len().min(mine.len());
    assert!(
        common >= 8,
        "比べられるフレームが {common} 枚しかありません"
    );
    for at in 0..common {
        assert_eq!(theirs[at].0, mine[at].0, "並び順が違います（{at} 枚目）");
        let rows = units[0].format.height as usize;
        let width = units[0].format.width as usize;
        for y in 0..rows {
            let a = &theirs[at].1[y * theirs[at].2..][..width];
            let b = &mine[at].1[y * mine[at].2..][..width];
            assert_eq!(a, b, "{at} 枚目の {y} 行目が違います");
        }
    }
}

/// MPEG-2 は完全一致しない（逆変換は許容差）。下限は `deck` の `MPEG2_FLOOR` と同じ 50 dB
/// （Apple は約 55 dB、壊れた経路は 20〜30 dB）。
#[test]
fn mpeg2_agrees_as_closely_as_apples_transform_gets() {
    let cutting = units("mpeg2-interlaced.ts");
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    assert_eq!(codec, Codec::Mpeg2);
    let format = units[0].format;
    let mut hardware = zwcore::decode::open(codec, framing, zwcore::decode::Prefer::Platform);
    let mut software = zwcore::decode::software(codec, framing);
    let theirs = luma(hardware.as_mut(), units, 12);
    let mine = luma(software.as_mut(), units, 12);
    // オープン GOP の先頭は落とす（どちらのデコーダーも持っていない参照から予測している）。
    let entry: std::collections::BTreeSet<u64> = units
        .iter()
        .filter(|unit| unit.picture.random_access)
        .map(|unit| unit.tag)
        .collect();
    // 番号で突き合わせる（VideoToolbox はランダムアクセスポイントより前のフレームも出すので、位置ではずれる）。
    let first = theirs
        .iter()
        .map(|shot| shot.0)
        .find(|tag| entry.contains(tag))
        .unwrap_or(0);
    let mine: std::collections::BTreeMap<u64, _> =
        mine.into_iter().map(|shot| (shot.0, shot)).collect();
    let both: Vec<_> = theirs
        .into_iter()
        .filter(|shot| shot.0 >= first)
        .filter_map(|shot| mine.get(&shot.0).map(|mine| (shot, mine)))
        .collect();
    let common = both.len();
    assert!(
        common >= 8,
        "比べられるフレームが {common} 枚しかありません"
    );
    let mut worst = f64::INFINITY;
    for (theirs, mine) in &both {
        worst = worst.min(psnr(theirs, mine, format));
    }
    eprintln!("最悪 {worst:.1} dB");
    assert!(worst > 50.0, "最悪 {worst:.1} dB");
}

fn psnr(mine: &(u64, Vec<u8>, usize), theirs: &(u64, Vec<u8>, usize), format: Format) -> f64 {
    fn luma(shot: &(u64, Vec<u8>, usize)) -> zwcore::decode::Luma<'_> {
        zwcore::decode::Luma {
            bytes: &shot.1,
            pitch: shot.2,
            shift: 0,
        }
    }
    zwcore::decode::psnr(luma(mine), luma(theirs), format)
}

/// 経路の全部：デコード、取り込み、二プレーンからレンダリングして三プレーンのものと比べる。許容は 2。
#[cfg(feature = "wgpu")]
#[test]
fn a_frame_the_hardware_wrote_is_drawn_the_same_as_one_it_did_not() {
    // 8 ビットと 10 ビットの両方（取り込みのプレーンの形式が違う）。
    drawn_the_same("h264-baseline.ts");
    drawn_the_same("hevc-main10.ts");
}

#[cfg(feature = "wgpu")]
fn drawn_the_same(name: &str) {
    use zwcore::output::video::gpu::{Gpu, Video};
    use zwcore::output::video::{Settings, shader::Shape};

    let cutting = units(name);
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let gpu = match Gpu::open() {
        Ok(gpu) => {
            eprintln!("{}", gpu.name());
            gpu
        }
        Err(error) => {
            eprintln!("GPUが無いので飛ばす: {error}");
            return;
        }
    };
    let format = units[0].format;
    let settings = Settings::of_format(format);
    let through = |decoder: &mut dyn Decode| -> Option<Vec<u8>> {
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            format,
            settings,
        )
        .ok()?;
        let mut drawn = Vec::new();
        for Cutout {
            tag,
            picture,
            format,
            data,
        } in units
        {
            let _ = decoder.decode(*tag, picture, *format, data);
            while let Some(found) = decoder.take() {
                if video.push(&found).is_err() {
                    break;
                }
                while video.ready() {
                    if let Some((picture, ..)) = video.read_back() {
                        drawn.push(picture);
                    }
                }
            }
            if drawn.len() >= 4 {
                break;
            }
        }
        (drawn.len() >= 3).then(|| drawn.swap_remove(2))
    };
    let mut hardware = zwcore::decode::open(codec, framing, zwcore::decode::Prefer::Platform);
    let Some(theirs) = through(hardware.as_mut()) else {
        panic!("{name}: ハードウェアの経路がフレームを出しませんでした");
    };
    let mut software = zwcore::decode::software(codec, framing);
    let Some(mine) = through(software.as_mut()) else {
        eprintln!("ソフトウェアの経路がフレームを出しませんでした");
        return;
    };
    assert_eq!(mine.len(), theirs.len());
    let worst = mine
        .iter()
        .zip(&theirs)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    eprintln!("{name}: 最大の差 {worst}");
    assert!(worst <= 2, "the two paths differ by {worst}");
    assert_eq!(Shape::Biplanar.textures(), 2);
}
