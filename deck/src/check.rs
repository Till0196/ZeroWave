// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `--check`: 同じ録画をプラットフォームとソフトウェアのデコーダーでデコードして突き合わせる。
//! MPEG-2 は逆変換が許容差でしか規定されないので一致ではなく [`MPEG2_FLOOR`] で見る。

use std::time::{Duration, Instant};

use mediadecoder::es::{Codec, Coding, Format, Framing, Picture};
use zwcore::decode::{Decode, Decoded, Prefer};
use zwcore::filters::cut::{Cutout, Cutting};

mod files;
mod report;
mod sound;

pub use files::{against, dump, dump_hardware, dump_system};
use report::*;
pub use sound::sound_check;

/// MPEG-2 でハードウェアの経路を信用してよい下限（dB）。壊れた経路は 20〜30 dB に落ちる。
const MPEG2_FLOOR: f64 = 50.0;

struct Shot {
    tag: u64,
    luma: Vec<u8>,
    pitch: usize,
}

struct Pass {
    shots: Vec<Shot>,
    spent: Duration,
    hardware: bool,
    /// The first complaint and the count (a pool hands out textures even if nothing wrote them).
    first: Option<String>,
    errors: usize,
}

fn units(path: &str) -> Option<Cutting> {
    Cutting::of_path(path, usize::MAX)
}

/// One pass with one kind of decoder; both passes get exactly the same access units.
fn pass(codec: Codec, framing: Framing, prefer: Prefer, units: &[Cutout]) -> Pass {
    let mut decoder = zwcore::decode::open(codec, framing, prefer);
    let mut out = Pass {
        shots: Vec::new(),
        spent: Duration::ZERO,
        hardware: false,
        first: None,
        errors: 0,
    };
    for Cutout {
        tag,
        picture,
        format,
        data,
    } in units
    {
        let began = Instant::now();
        let decoded = decoder.decode(*tag, picture, *format, data);
        out.spent += began.elapsed();
        if let Err(error) = decoded {
            out.first.get_or_insert_with(|| format!("{error:?}"));
            out.errors += 1;
        }
        drain(decoder.as_mut(), &mut out);
    }
    decoder.flush();
    drain(decoder.as_mut(), &mut out);
    out
}

fn drain(decoder: &mut dyn Decode, into: &mut Pass) {
    loop {
        if let Some(read) = decoder.read_back() {
            into.hardware = true;
            if let Some((bytes, pitch)) = read.luma() {
                into.shots.push(Shot {
                    tag: read.about.tag,
                    luma: bytes.to_vec(),
                    pitch,
                });
            }
            continue;
        }
        match decoder.take() {
            Some(Decoded::Planes(frame)) => {
                let plane = &frame.planes[0];
                let wide = usize::from(frame.format.depth > 8) + 1;
                let mut luma = Vec::with_capacity(plane.height * plane.width * wide);
                for y in 0..plane.height {
                    luma.extend_from_slice(&plane.row(y)[..plane.width * wide]);
                }
                into.shots.push(Shot {
                    tag: frame.tag,
                    pitch: plane.width * wide,
                    luma,
                });
            }
            Some(Decoded::Surface(_)) => {}
            None => return,
        }
    }
}

fn one(path: &str, most: usize) -> bool {
    let Some(cutting) = units(path) else {
        println!("{path}: 読めないか、映像がありません");
        return false;
    };
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let format = units[0].format;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    let head = format!(
        "{name}  {codec:?} {}x{}{}",
        format.width,
        format.height,
        if format.depth > 8 { " 10bit" } else { "" }
    );

    let feed = &units[..units.len().min(most)];
    let hardware = pass(codec, framing, Prefer::Platform, feed);
    let software = pass(codec, framing, Prefer::Software, feed);
    let rate = |pass: &Pass| {
        if pass.spent.is_zero() {
            0.0
        } else {
            pass.shots.len() as f64 / pass.spent.as_secs_f64()
        }
    };
    if !hardware.hardware && !hardware.shots.is_empty() {
        println!(
            "{head}  プラットフォームのデコーダーがありません（ソフトウェアのみ {:.0} fps）",
            rate(&software)
        );
        return true;
    }

    let best = compare(&hardware, &software, units, format);
    let Verdict {
        worst,
        mean,
        counted,
        ordered,
        ..
    } = best;

    if hardware.shots.is_empty() {
        println!(
            "{head}  ← 映像が出ませんでした{}",
            hardware
                .first
                .as_deref()
                .map_or(String::new(), |error| format!("（{error}）"))
        );
        if hardware.errors > 0 {
            println!(
                "    ハードウェアが拒否した access unit: {} 件",
                hardware.errors
            );
        }
        return false;
    }

    let exact = codec != Codec::Mpeg2;
    let agreed = counted > 0
        && ordered
        && if exact {
            worst.is_infinite()
        } else {
            worst > MPEG2_FLOOR
        };
    let quality = if worst.is_infinite() {
        String::from("完全一致")
    } else {
        format!("最悪 {worst:.1} / 平均 {mean:.1} dB")
    };
    println!(
        "{head}  ハード {:.0} fps / ソフト {:.0} fps  {counted} 枚  {quality}{}{}",
        rate(&hardware),
        rate(&software),
        if ordered {
            ""
        } else {
            "  順序が違います"
        },
        if agreed { "" } else { "  ← 合いません" },
    );
    if best.held_back > 0 {
        println!(
            "    ハードウェアは先頭の {} 枚を出しませんでした（IDR でない所から入ったときの回復）",
            best.held_back
        );
    }
    if let Some(listed) = poorest(&best, units) {
        println!("    不一致のフレーム: {listed}");
        println!("    種類ごと: {}", spread(&best, units));
        if let Some((tag, _)) = best.poorest.first() {
            let ours = software.shots.iter().find(|shot| shot.tag == *tag);
            let theirs = hardware.shots.iter().find(|shot| shot.tag == *tag);
            if let (Some(ours), Some(theirs)) = (ours, theirs) {
                println!(
                    "    tag {tag} の中身: {}",
                    where_wrong(ours, theirs, format)
                );
                let kinds: std::collections::BTreeMap<u64, Coding> = units
                    .iter()
                    .map(|unit| (unit.tag, unit.picture.coding))
                    .collect();
                let reference = |shot: &&Shot| {
                    kinds
                        .get(&shot.tag)
                        .is_some_and(|coding| *coding != Coding::B)
                };
                if let Some(at) = software.shots.iter().position(|shot| shot.tag == *tag) {
                    let before = software.shots[..at].iter().rev().find(reference);
                    let after = software.shots[at + 1..].iter().find(reference);
                    println!("    {}", copied(theirs, before, after, format));
                }
            }
        }
    }
    println!(
        "    この環境の {codec:?}: {}",
        if agreed {
            String::from("ハードウェアを使えます")
        } else {
            format!(
                "**ハードウェアは信用できません**（{}）",
                if exact {
                    String::from("完全一致でなければなりません")
                } else {
                    format!("最悪 {worst:.1} dB、{MPEG2_FLOOR:.0} dB を下回ります")
                }
            )
        }
    );
    if hardware.errors > 0 {
        println!(
            "    ハードウェアが拒否した access unit: {} 件（{}）",
            hardware.errors,
            hardware.first.as_deref().unwrap_or("?")
        );
    }
    if software.errors > 0 {
        println!(
            "    ソフトウェアが拒否した access unit: {} 件（{}）",
            software.errors,
            software.first.as_deref().unwrap_or("?")
        );
    }
    if !ordered {
        divergence(&hardware, &software);
    }
    agreed
}

pub fn run(paths: &[String], most: usize) -> bool {
    #[cfg(windows)]
    if let Ok(decoder) = zwcore::decode::d3d12::D3d12::open(
        mediadecoder::es::Codec::H264,
        mediadecoder::es::Framing::StartCodes,
    ) {
        println!("{}", decoder.name());
    }
    #[cfg(any(feature = "vaapi", target_os = "linux"))]
    if let Ok(decoder) = zwcore::decode::vaapi::Vaapi::open(
        mediadecoder::es::Codec::Mpeg2,
        mediadecoder::es::Framing::StartCodes,
    ) {
        println!("VA-API ({})", decoder.vendor());
    }
    #[cfg(target_os = "linux")]
    if let Ok(decoder) = zwcore::decode::v4l2::open(
        mediadecoder::es::Codec::Mpeg2,
        mediadecoder::es::Framing::StartCodes,
    ) {
        println!(
            "V4L2 ({}, {:?}, {:?})",
            decoder.driver(),
            decoder.flavour(),
            decoder.mode()
        );
    }
    let mut all = true;
    for path in paths {
        all &= one(path, most);
    }
    all
}
