// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The Windows hardware decoder (D3D12) held against the software one. Needs Windows, a D3D12
//! video queue; each is checked and skipped. The streams are ffmpeg's (`testdata/make.sh`).

// 基準は内蔵のソフトウェアデコーダーなので、その feature も要る。
#![cfg(all(
    feature = "d3d12",
    windows,
    feature = "h264-decoder",
    feature = "hevc-decoder"
))]

use mediadecoder::es::{Codec, Framing};
use zwcore::decode::Decoded;
use zwcore::decode::d3d12::D3d12;
use zwcore::filters::cut::Cutout;

mod common;
use common::units;

fn hardware(codec: Codec, framing: Framing) -> Option<D3d12> {
    let decoder = common::hardware("D3D12 のデコーダー", || D3d12::open(codec, framing))?;
    eprintln!("{codec:?}: {}", decoder.name());
    Some(decoder)
}

fn compared(name: &str, most: usize) -> Option<(f64, usize)> {
    common::compared(name, most, hardware, |_| false)
}

/// H.264 and HEVC must agree sample for sample.
#[test]
fn the_exact_codings_come_out_sample_for_sample() {
    for name in ["h264-high.ts", "hevc-main10.ts", "h264-baseline.ts"] {
        let Some((worst, counted)) = compared(name, 8) else {
            continue;
        };
        eprintln!("{name}: {counted} 枚、最悪 {worst} dB");
        assert!(
            worst.is_infinite(),
            "{name} differs from the software decoder ({worst:.2} dB over {counted} frames)"
        );
    }
}

/// MPEG-2 is held to agreement, not equality. `decode::d3d12::profiles` tries
/// `MPEG1_AND_MPEG2` first (a GeForce decodes only under that name).
#[test]
fn mpeg2_agrees_as_closely_as_the_standard_allows() {
    for name in ["mpeg2-interlaced.ts", "mpeg2-telecine.ts"] {
        let Some((worst, counted)) = compared(name, 8) else {
            continue;
        };
        eprintln!("{name}: {counted} 枚、最悪 {worst:.2} dB");
        assert!(
            worst > 55.0,
            "{name} is {worst:.2} dB from the software decoder over {counted} frames"
        );
    }
}

#[test]
fn preferring_the_platform_reaches_it_and_preferring_software_does_not() {
    use zwcore::decode::Prefer;

    let cutting = units("h264-baseline.ts");
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    if D3d12::open(codec, framing).is_err() {
        eprintln!("D3D12 のデコーダーが無いので飛ばす");
        return;
    }
    for (prefer, hardware) in [(Prefer::Platform, true), (Prefer::Software, false)] {
        let mut decoder = zwcore::decode::open(codec, framing, prefer);
        let mut seen = 0;
        for Cutout {
            tag,
            picture,
            format,
            data,
        } in units
        {
            let _ = decoder.decode(*tag, picture, *format, data);
            while let Some(found) = decoder.take() {
                assert_eq!(
                    matches!(found, Decoded::Surface(_)),
                    hardware,
                    "{prefer:?} gave the wrong kind of picture"
                );
                seen += 1;
            }
            if seen >= 4 {
                break;
            }
        }
        assert!(seen > 0, "{prefer:?} decoded nothing");
    }
}

/// A machine with no D3D12 video queue says so and the caller falls back.
#[test]
fn a_machine_with_no_device_falls_back_to_the_software_decoder() {
    let mut decoder = zwcore::decode::open(
        Codec::Mpeg2,
        Framing::StartCodes,
        zwcore::decode::Prefer::Platform,
    );
    assert!(decoder.take().is_none());
    decoder.reset();
}

/// The whole path. The device is opened before the decoder, which writes into wgpu's own
/// `ID3D12Device`.
#[cfg(feature = "wgpu")]
#[test]
fn a_frame_the_hardware_wrote_is_drawn_the_same_as_one_it_did_not() {
    use zwcore::decode::Decode;
    use zwcore::output::video::gpu::{Gpu, Video};
    use zwcore::output::video::{Settings, shader::Shape};

    let cutting = units("h264-high.ts");
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
    let Some(mut hardware) = hardware(codec, framing) else {
        return;
    };
    let mut software = zwcore::decode::software(codec, framing);
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
    let Some(theirs) = through(&mut hardware) else {
        eprintln!("ハードウェアの経路がフレームを出しませんでした");
        return;
    };
    let Some(mine) = through(software.as_mut()) else {
        return;
    };
    assert_eq!(mine.len(), theirs.len());
    let worst = mine
        .iter()
        .zip(&theirs)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    eprintln!("最大の差 {worst}");
    assert!(worst <= 2, "the two paths differ by {worst}");
    assert_eq!(Shape::Biplanar.textures(), 2);
}
