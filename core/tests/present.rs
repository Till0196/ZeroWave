// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GLES 2.0 のレンダラーが、wgpu と同じゴールデン（Rust の参照実装の出力）に一致するか。
//! 合格線は差 0（lavapipe では wgpu も差 0）。実機で丸めが違えば mcprobe の実測を持って判断する。
//! GL の無い環境では飛ばす（Ubuntu は `libegl1`・`libgles2`・`libgl1-mesa-dri` で走る）。
//! Windows には `dlopen` が無いのでビルドされない。

#![cfg(unix)]

use zwcore::decode::{About, Decoded};
use zwcore::output::present::{Capability, Present, glsl::Taps};
use zwcore::output::video::{Deinterlace, field};

/// 入力の作り方と正解の出し方は、wgpu の側と共有する。
#[path = "pictures.rs"]
mod pictures;
use pictures::*;

/// 許す差。0 の理由は先頭のコメント。
const ALLOWED: u8 = 0;

fn open(name: &str) -> Option<Present> {
    let (_, settings, depth, first, _) = reference(name);
    let frames = frames_of(name, depth, first);
    match Present::offscreen(frames[1].format, settings) {
        Ok(present) => {
            eprintln!("{}", present.name());
            Some(present)
        }
        Err(error) => {
            eprintln!("GLES 2.0 が無いので飛ばす: {error}");
            None
        }
    }
}

fn rendered(name: &str, taps: Option<Taps>) -> Option<Vec<u8>> {
    let (_, settings, depth, first, which) = reference(name);
    let frames = frames_of(name, depth, first);
    let mut present = Present::offscreen(frames[1].format, settings).ok()?;
    if let Some(taps) = taps {
        if !taps.carries(settings.deinterlace) {
            return None;
        }
        present.force_taps(taps).ok()?;
    }
    if present.settings().deinterlace != settings.deinterlace {
        eprintln!(
            "{name}: {:?} は {:?} を持てないので飛ばす",
            present.taps(),
            settings.deinterlace
        );
        return None;
    }
    for frame in &frames {
        present
            .push(&Decoded::Planes(frame.clone()))
            .expect("スロットは3枚ぶんある");
    }
    let of_first = field::plan(&About::of_frame(&frames[0]), settings.deinterlace).len();
    for _ in 0..of_first + which {
        present.read_back().expect("出力ごとに1枚出る");
    }
    Some(present.read_back().expect("出力ごとに1枚出る").0)
}

#[test]
fn the_shaders_make_the_same_pictures_as_the_reference() {
    let Some(present) = open("bob-top") else {
        return;
    };
    let taps = present.taps();
    drop(present);
    for name in CASES {
        let (golden, ..) = reference(name);
        let Some(got) = rendered(name, None) else {
            continue;
        };
        assert_eq!(got.len(), golden.len(), "{name}");
        let differences: Vec<u8> = got
            .iter()
            .zip(&golden)
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let worst = differences.iter().copied().max().unwrap_or(0);
        let off = differences.iter().filter(|d| **d != 0).count();
        eprintln!(
            "{name}  系統 {taps:?}  最大の差 {worst}  違う成分 {off} / {}",
            golden.len()
        );
        assert_eq!(worst, ALLOWED, "{name} が参照と {worst} 違う");
    }
}

#[test]
fn the_varying_route_draws_what_the_computed_route_draws() {
    if open("bob-top").is_none() {
        return;
    }
    for name in ["weave", "bob-top"] {
        let (golden, ..) = reference(name);
        let Some(got) = rendered(name, Some(Taps::Varying)) else {
            continue;
        };
        let worst = got
            .iter()
            .zip(&golden)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        eprintln!("{name}  varying 系統  最大の差 {worst}");
        assert_eq!(worst, ALLOWED, "{name} が varying 系統で {worst} 違う");
    }
}

#[test]
fn the_last_frame_of_a_stream_is_not_lost() {
    let (_, settings, depth, first, _) = reference("bob-top");
    let frames = frames_of("bob-top", depth, first);
    let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    for frame in &frames {
        present.push(&Decoded::Planes(frame.clone())).expect("入る");
    }
    let mut drawn = 0;
    while present.ready() {
        present.read_back();
        drawn += 1;
    }
    let before = drawn;
    present.flush();
    while present.ready() {
        present.read_back();
        drawn += 1;
    }
    assert!(drawn > before, "flush で最後のフレームが出てこない");
}

#[test]
fn the_ladder_is_measured_on_this_machine() {
    let format = mediadecoder::es::Format {
        width: 1440,
        height: 1080,
        ..Default::default()
    };
    match Capability::measure(format) {
        Ok(capability) => eprintln!("{}", capability.report()),
        Err(error) => eprintln!("GLES 2.0 が無いので飛ばす: {error}"),
    }
}

#[test]
fn the_shader_addresses_every_texel_of_a_broadcast_sized_picture() {
    let format = mediadecoder::es::Format {
        width: 1440,
        height: 1080,
        ..Default::default()
    };
    let Ok(capability) = Capability::measure(format) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    eprintln!(
        "系統 {:?}  fragment の highp {}  実寸でサンプリングできたか {}",
        capability.taps, capability.declared_highp, capability.addresses
    );
    assert!(
        capability.addresses,
        "1440x1080 の texel を実寸でサンプリングできない。この GPU では映像がずれる"
    );
}

#[test]
fn a_route_that_cannot_carry_a_step_says_so_rather_than_drawing_something_else() {
    assert!(!Taps::Varying.carries(Deinterlace::Bwdif));
    assert!(!Taps::Varying.carries(Deinterlace::Yadif));
    assert!(Taps::Varying.carries(Deinterlace::Bob));
    assert!(Taps::Varying.carries(Deinterlace::Weave));
    for quality in [
        Deinterlace::Weave,
        Deinterlace::Bob,
        Deinterlace::Yadif,
        Deinterlace::Bwdif,
    ] {
        assert!(Taps::Computed.carries(quality));
    }
}

#[test]
fn the_probe_that_goes_to_the_device_runs_here_first() {
    let format = mediadecoder::es::Format {
        width: 1440,
        height: 1080,
        ..Default::default()
    };
    let text = zwcore::output::present::probe::report(format);
    eprintln!("{text}");
    // 「EGL が無い」と「シェーダーが間違っている」は別。文言は `probe` の言い方と揃える。
    for said in ["開けません", "GPU がありません"] {
        if text.contains(said) {
            eprintln!("GLES 2.0 をこの環境では開けないので飛ばす（{said}）");
            return;
        }
    }
    assert!(
        text.contains("texel を実寸でサンプリングできるか  できた"),
        "実寸で texel をサンプリングできない"
    );
    assert!(
        text.matches("**一致**").count() >= 2,
        "参照実装と一致した段が足りない:\n{text}"
    );
}

/// 画面へ出すパスが、映像を上下反転しない（ゴールデンはフィルターのパスまでしか見ない）。
#[test]
fn the_pass_that_reaches_the_screen_does_not_turn_the_picture_over() {
    let (_, settings, depth, first, _) = reference("bob-top");
    let mut frames = frames_of("bob-top", depth, first);
    for frame in &mut frames {
        frame.format.aspect = (1, 1);
    }
    let (width, height) = (frames[1].format.width, frames[1].format.height);
    let fed = || -> Option<Present> {
        let mut present = Present::offscreen(frames[1].format, settings).ok()?;
        for frame in &frames {
            present.push(&Decoded::Planes(frame.clone())).expect("入る");
        }
        Some(present)
    };
    let Some(mut first_way) = fed() else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    let mut second_way = fed().expect("2つ目も開ける");
    let straight = first_way.read_back().expect("1枚").0;
    let on_screen = second_way.read_screen(width, height).expect("1枚").0;
    assert_eq!(straight.len(), on_screen.len());

    // 両方を見る（片方だけだと上下対称な画像で通ってしまう）。
    let row = (width * 4) as usize;
    let upside_down: Vec<u8> = (0..height as usize)
        .rev()
        .flat_map(|y| straight[y * row..(y + 1) * row].to_vec())
        .collect();
    let same = straight == on_screen;
    let flipped = upside_down == on_screen;
    eprintln!("画面と一致 {same}  反転して一致 {flipped}");
    assert!(same, "画面へ出すパスが映像を上下反転している");
    assert!(
        !flipped || straight == upside_down,
        "上下対称な画像では判定にならない"
    );
}

#[test]
fn the_picture_is_fitted_by_the_aspect_it_declares_not_by_its_sample_count() {
    let (_, settings, depth, first, _) = reference("bob-top");
    let mut frames = frames_of("bob-top", depth, first);
    for frame in &mut frames {
        frame.format.aspect = (16, 9);
    }
    let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    let (width, height) = (frames[1].format.width, frames[1].format.height);
    for frame in &frames {
        present.push(&Decoded::Planes(frame.clone())).expect("入る");
    }
    let (screen, ..) = present.read_screen(width, height).expect("1枚");
    let lit = |y: u32| {
        (0..width).any(|x| {
            let at = ((y * width + x) * 4) as usize;
            screen[at] != 0 || screen[at + 1] != 0 || screen[at + 2] != 0
        })
    };
    let tall = (0..height).filter(|y| lit(*y)).count();
    let want = (height as f32 * 9.0 / 16.0).round() as usize;
    eprintln!("映像の高さ {tall} 行 / 画面 {height} 行（16:9 なら {want} 前後）");
    assert!(
        tall.abs_diff(want) <= 1,
        "16:9 の映像が {tall} 行を占めている。{want} 行のはず"
    );
    assert!(!lit(0), "一番上の行に映像が出ている。余白が付いていない");
}

#[test]
fn the_varying_route_rebuilds_a_wider_sample_too() {
    use zwcore::output::video::{Settings, filter};

    let frames = frames_of("bwdif-ten-bit", 10, zwcore::output::video::Parity::Top);
    let settings = Settings {
        deinterlace: Deinterlace::Bob,
        telecine: false,
        ..Settings::of_format(frames[1].format)
    };
    let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    if present.force_taps(Taps::Varying).is_err() {
        eprintln!("varying の系統をビルドできないので飛ばす");
        return;
    }
    for frame in &frames {
        present.push(&Decoded::Planes(frame.clone())).expect("入る");
    }
    let of_first = field::plan(&About::of_frame(&frames[0]), Deinterlace::Bob).len();
    for _ in 0..of_first {
        present.read_back().expect("1枚");
    }
    let got = present.read_back().expect("1枚").0;
    let plan = field::plan(&About::of_frame(&frames[1]), Deinterlace::Bob);
    let want = filter::picture(&frames[0], &frames[1], &frames[2], &plan[0], settings);
    let worst = got
        .iter()
        .zip(&want)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    eprintln!("10ビット + varying  最大の差 {worst}");
    assert_eq!(worst, ALLOWED, "10ビットの組み立てが varying の系統で違う");
}

/// 選局しても壊れない（解放の順を間違えると GL が再割り当てした別のオブジェクトの名前を消す）。
#[test]
fn changing_channel_rebuilds_without_losing_the_picture() {
    let (_, settings, depth, first, _) = reference("bob-top");
    let frames = frames_of("bob-top", depth, first);
    let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    for round in 0..4 {
        let mut format = frames[1].format;
        format.width = if round % 2 == 0 { 32 } else { 64 };
        present.retune(format, settings).expect("ビルドし直せる");
        let sized: Vec<_> = frames
            .iter()
            .map(|frame| resized(frame, format.width))
            .collect();
        for frame in &sized {
            present.push(&Decoded::Planes(frame.clone())).expect("入る");
        }
        let (picture, width, height) = present.read_back().expect("1枚");
        assert_eq!(width, format.width, "{round} 回目の幅");
        assert_eq!(picture.len(), (width * height * 4) as usize);
        assert!(
            picture.iter().any(|value| *value != 0),
            "{round} 回目が真っ黒。ビルドし直しでサーフェスを落としている"
        );
        present.reset();
    }
}

fn resized(frame: &mediadecoder::video::Frame, width: u32) -> mediadecoder::video::Frame {
    let mut out = frame.clone();
    out.format.width = width;
    for (plane, half) in out.planes.iter_mut().zip([false, true, true]) {
        let want = if half { width.div_ceil(2) } else { width } as usize;
        let mut data = Vec::with_capacity(want * plane.height);
        for y in 0..plane.height {
            let row = plane.row(y);
            data.extend_from_slice(&row[..want.min(row.len())]);
            data.resize((y + 1) * want, 0);
        }
        plane.data = data;
        plane.stride = want;
        plane.width = want;
    }
    out
}

/// Mali-450 で差 1 が出た理由（`mediump` の精度）を、`highp` のある GPU で測る。判定ではなく測定。
#[test]
fn mediump_alone_explains_a_difference_of_one() {
    let Some(present) = open("bob-top") else {
        return;
    };
    if !present.taps().carries(Deinterlace::Bob) {
        return;
    }
    drop(present);
    for name in ["weave", "bob-top"] {
        let (golden, settings, depth, first, which) = reference(name);
        let frames = frames_of(name, depth, first);
        let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
            return;
        };
        if present.force_mediump().is_err() {
            eprintln!("mediump でビルドできないので飛ばす");
            return;
        }
        for frame in &frames {
            present.push(&Decoded::Planes(frame.clone())).expect("入る");
        }
        let of_first = field::plan(&About::of_frame(&frames[0]), settings.deinterlace).len();
        for _ in 0..of_first + which {
            present.read_back().expect("1枚");
        }
        let got = present.read_back().expect("1枚").0;
        let differences: Vec<u8> = got
            .iter()
            .zip(&golden)
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let worst = differences.iter().copied().max().unwrap_or(0);
        let off = differences.iter().filter(|d| **d != 0).count();
        eprintln!(
            "{name}  mediump  最大の差 {worst}  違う成分 {off} / {}  （{:.2}%）",
            golden.len(),
            off as f64 * 100.0 / golden.len() as f64
        );
    }
    eprintln!("実機（Mali-450、mediump）は weave 1.64%、bob 2.72% が差 1 だった");
}

#[test]
fn the_cadence_detector_is_paid_for_only_where_it_fits() {
    let format = mediadecoder::es::Format {
        width: 1440,
        height: 1080,
        ..Default::default()
    };
    let Ok(capability) = Capability::measure(format) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    eprintln!(
        "検出器 {:.2} ms/フレーム  間に合うか {}",
        capability.telecine_ms,
        capability.can_afford_telecine()
    );
    assert!(capability.telecine_ms > 0.0, "測れていない");
    let settings = capability.settings(format);
    assert_eq!(
        settings.telecine,
        capability.can_afford_telecine(),
        "測った値とビルドする設定が一致しない"
    );
}

/// 間に合わない検出器は、どの作り方（`Present::offscreen` を直接呼んでも）でも入らない。
#[test]
fn the_verdict_on_the_detector_reaches_every_entry() {
    let (_, _, depth, first, _) = reference("bob-top");
    let frames = frames_of("bob-top", depth, first);
    let settings = zwcore::output::video::Settings::of_format(frames[1].format);
    assert!(
        settings.telecine,
        "インタレースのストリームは既定で検出器が入る"
    );
    let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    assert_eq!(
        present.telecine_verdict(),
        None,
        "push する前に判定している"
    );
    for frame in &frames {
        present.push(&Decoded::Planes(frame.clone())).expect("入る");
    }
    let verdict = present.telecine_verdict();
    eprintln!(
        "3枚で判定  間に合うか {verdict:?}  いま入っているか {}",
        present.settings().telecine
    );
    assert!(verdict.is_some(), "3枚 push しても判定していない");
    assert_eq!(
        present.settings().telecine,
        verdict == Some(true),
        "判定の結果が設定に効いていない"
    );
}

#[test]
fn a_detector_that_cannot_be_paid_for_turns_itself_off() {
    let format = mediadecoder::es::Format {
        width: 3840,
        height: 2160,
        ..Default::default()
    };
    let settings = zwcore::output::video::Settings::of_format(format);
    let Ok(mut present) = Present::offscreen(format, settings) else {
        eprintln!("GLES 2.0 が無いか 4K を持てないので飛ばす");
        return;
    };
    if !present.settings().telecine {
        eprintln!("この解像度では検出器が入らないので飛ばす");
        return;
    }
    let frame = big(format);
    for _ in 0..4 {
        if present.push(&Decoded::Planes(frame.clone())).is_err() {
            break;
        }
    }
    let verdict = present.telecine_verdict();
    eprintln!(
        "4K で判定  間に合うか {verdict:?}  いま入っているか {}",
        present.settings().telecine
    );
    let Some(afford) = verdict else {
        eprintln!("判定まで進まなかったので飛ばす");
        return;
    };
    assert_eq!(present.settings().telecine, afford, "判定が効いていない");
    if !afford {
        assert!(!present.settings().telecine, "間に合わないのに入ったまま");
    }
}

fn big(format: mediadecoder::es::Format) -> mediadecoder::video::Frame {
    use mediadecoder::video::Plane;
    let plane = |width: usize, height: usize| Plane {
        data: (0..width * height).map(|at| (at % 251) as u8).collect(),
        stride: width,
        width,
        height,
    };
    let (width, height) = (format.width as usize, format.height as usize);
    mediadecoder::video::Frame {
        tag: 0,
        format,
        planes: [
            plane(width, height),
            plane(width / 2, height / 2),
            plane(width / 2, height / 2),
        ],
        structure: mediadecoder::es::Structure::Frame,
        top_field_first: true,
        repeat_first_field: false,
        progressive: false,
    }
}

/// HDR（HLG、PQ）と BT.2020 の SDR は、SDR の画面へ wgpu の経路と同じ式
/// （`Mapping` の sRGB サーフェスへの場合）で変換される。許す差は wgpu の経路と同じ 3。
#[test]
fn a_wide_picture_is_brought_onto_the_screen_as_the_mapping_says() {
    use zwcore::output::video::colour::{Conversion, Mapping, quantise};
    use zwcore::output::video::filter::Samples;
    use zwcore::output::video::{Matrix, Parity, Primaries, Settings, Space, Transfer};

    let depth = 10;
    let frames = [
        woven(0, depth, Parity::Top),
        woven(1, depth, Parity::Top),
        woven(2, depth, Parity::Top),
    ];
    for (transfer, allowed) in [(Transfer::Hlg, 3), (Transfer::Pq, 3), (Transfer::Sdr, 1)] {
        let settings = Settings {
            deinterlace: Deinterlace::Weave,
            telecine: false,
            matrix: Matrix::Bt2020,
            limited: true,
            transfer,
            primaries: Primaries::Bt2020,
            ..Settings::default()
        };
        let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
            eprintln!("GLES 2.0 が無いので飛ばす");
            return;
        };
        for frame in &frames {
            present.push(&Decoded::Planes(frame.clone())).expect("入る");
        }
        let of_first = field::plan(&About::of_frame(&frames[0]), Deinterlace::Weave).len();
        for _ in 0..of_first {
            present.read_back().expect("1枚");
        }
        let got = present.read_back().expect("1枚").0;
        let conversion = Conversion::new(settings.matrix, depth, settings.limited);
        let mapping = Mapping::new(transfer, Primaries::Bt2020, Space::Srgb);
        let cur = &frames[1];
        let y = Samples::new(&cur.planes[0], depth);
        let cb = Samples::new(&cur.planes[1], depth);
        let cr = Samples::new(&cur.planes[2], depth);
        let mut worst = 0u8;
        for row in 0..H as i32 {
            for x in 0..W as i32 {
                let carried =
                    conversion.to_rgb(y.at(x, row), cb.at(x / 2, row / 2), cr.at(x / 2, row / 2));
                let want = mapping.apply(carried, false).map(quantise);
                let at = (row as usize * W + x as usize) * 4;
                for (c, want) in want.iter().enumerate() {
                    worst = worst.max(got[at + c].abs_diff(*want));
                }
            }
        }
        eprintln!("{transfer:?} を SDR の画面へ  最大の差 {worst}");
        assert!(worst <= allowed, "{transfer:?} の変換が参照と {worst} 違う");
    }
}

/// HDR の画面（PQ・HLG）へも、wgpu の経路と同じ式（`Mapping` の PQ・HLG のサーフェスへの場合）で符号化される。
/// SDR の映像は BT.2020 へ広げてから符号化する。許す差は 3。
#[test]
fn a_picture_is_encoded_for_an_hdr_screen_as_the_mapping_says() {
    use zwcore::output::video::colour::{Conversion, Mapping, quantise};
    use zwcore::output::video::filter::Samples;
    use zwcore::output::video::{Matrix, Parity, Primaries, Settings, Space, Transfer};

    let depth = 10;
    let frames = [
        woven(0, depth, Parity::Top),
        woven(1, depth, Parity::Top),
        woven(2, depth, Parity::Top),
    ];
    for space in [Space::Pq, Space::Hlg] {
        for (transfer, primaries) in [
            (Transfer::Hlg, Primaries::Bt2020),
            (Transfer::Pq, Primaries::Bt2020),
            (Transfer::Sdr, Primaries::Bt709),
        ] {
            let settings = Settings {
                deinterlace: Deinterlace::Weave,
                telecine: false,
                matrix: Matrix::Bt2020,
                limited: true,
                transfer,
                primaries,
                ..Settings::default()
            };
            let Ok(mut present) = Present::offscreen(frames[1].format, settings) else {
                eprintln!("GLES 2.0 が無いので飛ばす");
                return;
            };
            present.force_space(space, false).expect("作り直せる");
            for frame in &frames {
                present.push(&Decoded::Planes(frame.clone())).expect("入る");
            }
            let of_first = field::plan(&About::of_frame(&frames[0]), Deinterlace::Weave).len();
            for _ in 0..of_first {
                present.read_back().expect("1枚");
            }
            let got = present.read_back().expect("1枚").0;
            let conversion = Conversion::new(settings.matrix, depth, settings.limited);
            let mapping = Mapping::new(transfer, primaries, space);
            let cur = &frames[1];
            let y = Samples::new(&cur.planes[0], depth);
            let cb = Samples::new(&cur.planes[1], depth);
            let cr = Samples::new(&cur.planes[2], depth);
            let mut worst = 0u8;
            for row in 0..H as i32 {
                for x in 0..W as i32 {
                    let carried = conversion.to_rgb(
                        y.at(x, row),
                        cb.at(x / 2, row / 2),
                        cr.at(x / 2, row / 2),
                    );
                    let want = mapping.apply(carried, false).map(quantise);
                    let at = (row as usize * W + x as usize) * 4;
                    for (c, want) in want.iter().enumerate() {
                        worst = worst.max(got[at + c].abs_diff(*want));
                    }
                }
            }
            eprintln!("{transfer:?} を {space:?} の画面へ  最大の差 {worst}");
            assert!(
                worst <= 3,
                "{transfer:?} → {space:?} の変換が参照と {worst} 違う"
            );
        }
    }
}

/// 画面を左右二色の半透明で塗るだけのレイヤー（GL は `glScissor` と `glClear` だけ使う）。
#[derive(Debug)]
struct Halves {
    left: [f32; 4],
    right: [f32; 4],
}

impl zwcore::output::present::Layer for Halves {
    fn paint(&self, on: &zwcore::output::present::LayerTarget<'_>) {
        type Scissor = unsafe extern "C" fn(i32, i32, i32, i32);
        type Enable = unsafe extern "C" fn(u32);
        type ClearColor = unsafe extern "C" fn(f32, f32, f32, f32);
        type Clear = unsafe extern "C" fn(u32);
        let find = |name: &std::ffi::CStr| (on.proc_address)(name);
        // SAFETY: どれも ES 2.0 の関数で、型は `GLES2/gl2.h` のとおり。
        let (scissor, enable, disable, clear_color, clear) = unsafe {
            (
                std::mem::transmute::<*const std::ffi::c_void, Scissor>(find(c"glScissor")),
                std::mem::transmute::<*const std::ffi::c_void, Enable>(find(c"glEnable")),
                std::mem::transmute::<*const std::ffi::c_void, Enable>(find(c"glDisable")),
                std::mem::transmute::<*const std::ffi::c_void, ClearColor>(find(c"glClearColor")),
                std::mem::transmute::<*const std::ffi::c_void, Clear>(find(c"glClear")),
            )
        };
        let (width, height) = (on.size.0 as i32, on.size.1 as i32);
        // SAFETY: コンテキストは有効で、塗るのは渡された大きさの中だけ。
        unsafe {
            enable(0x0C11);
            for (x, colour) in [(0, self.left), (width / 2, self.right)] {
                scissor(x, 0, width / 2, height);
                clear_color(colour[0], colour[1], colour[2], colour[3]);
                clear(0x4000);
            }
            disable(0x0C11);
        }
    }

    fn fresh(&self) -> bool {
        true
    }
}

/// HDR の画面に画面（乗算済みのアルファ）を重ねると、下の映像と cd/m² で混ぜて符号化し直す
/// （`Mapping::lay`、`shader/over.wgsl` と同じ）。アルファが 0 の所は下の映像のまま。許す差は 2。
#[test]
fn the_screen_is_laid_over_an_hdr_picture_as_the_mapping_says() {
    use zwcore::output::present::Window;
    use zwcore::output::video::colour::{Mapping, quantise};
    use zwcore::output::video::{Primaries, Space, Transfer};

    let Ok(window) = Window::offscreen() else {
        eprintln!("GLES 2.0 が無いので飛ばす");
        return;
    };
    let size = (64u32, 16u32);
    // 下は横に明るくなる灰色に、少し色を付ける。
    let below: Vec<u8> = (0..size.1)
        .flat_map(|_| {
            (0..size.0).flat_map(|x| {
                let level = (40 + x * 3) as u8;
                [
                    level,
                    level.saturating_add(10),
                    level.saturating_sub(5),
                    255,
                ]
            })
        })
        .collect();
    // 乗算済みで渡す（egui と同じ）。
    let straight = [[0.9f32, 0.3, 0.1], [0.2, 0.4, 0.8]];
    let alphas = [0.5f32, 0.0];
    let halves = Halves {
        left: [
            straight[0][0] * alphas[0],
            straight[0][1] * alphas[0],
            straight[0][2] * alphas[0],
            alphas[0],
        ],
        right: [0.0; 4],
    };
    for space in [Space::Pq, Space::Hlg] {
        let got = window
            .lay_for_test(&halves, space, &below, size)
            .expect("重ねられる");
        let mapping = Mapping::new(Transfer::Ui, Primaries::Bt709, space);
        let mut worst = 0u8;
        for y in 0..size.1 as usize {
            for x in 0..size.0 as usize {
                let at = (y * size.0 as usize + x) * 4;
                let under = [0, 1, 2].map(|c| f32::from(below[at + c]) / 255.0);
                let half = usize::from(x >= size.0 as usize / 2);
                let want = if alphas[half] > 0.0 {
                    // レイヤーは 8 ビットに丸めて描かれるので、参照も丸めた値から戻す。
                    let eight = |value: f32| (value * 255.0).round() / 255.0;
                    let alpha = eight(alphas[half]);
                    let paper = straight[half].map(|c| eight(c * alphas[half]) / alpha);
                    mapping.lay(paper, alpha, under, false).map(quantise)
                } else {
                    [below[at], below[at + 1], below[at + 2]]
                };
                for c in 0..3 {
                    worst = worst.max(got[at + c].abs_diff(want[c]));
                }
            }
        }
        eprintln!("{space:?} の画面に重ねる  最大の差 {worst}");
        assert!(
            worst <= 2,
            "{space:?} の画面に重ねた結果が参照と {worst} 違う"
        );
    }
}

/// 10 ビットの DMA-BUF（P010）をコピーせずに外部テクスチャで取り込み、HDR を SDR に変換してレンダリングする。
/// 許す差は 8（末尾のコメント）。
/// DRM の dumb バッファに画像を書いて渡す。表示の DRM デバイス（`/dev/dri/card0`）と、P010 を外部
/// テクスチャで取り込める EGL（Mesa の panfrost など）が無ければ飛ばす。色差は一枚で一色
/// にする（ドライバーは色差を補間してサンプリングするので、色の境目ではこちらの参照と比べられない）。
#[cfg(target_os = "linux")]
#[test]
fn a_ten_bit_dma_buf_is_drawn_and_brought_down_as_the_mapping_says() {
    use zwcore::decode::{DmaBuf, DmaBufPlane, Held, Surface};
    use zwcore::output::video::colour::{Conversion, Mapping, quantise};
    use zwcore::output::video::{Matrix, Primaries, Settings, Space, Transfer};

    let Some(card) = dumb::Card::open() else {
        eprintln!("/dev/dri/card0 で dumb バッファを作れないので飛ばす");
        return;
    };
    let depth = 10;
    let mut format = woven(0, depth, zwcore::output::video::Parity::Top).format;
    format.progressive = true;
    // 画面と同じアスペクト比に（黒帯を付けずに一画素ずつ比べる）。
    format.aspect = (1, 1);
    // 輝度は横に 64 から 940 まで、行ごとに少しずらす。色差は一色。
    let luma = |x: usize, y: usize| (64 + (x * 876 + y * 5) / (W - 1)).min(940) as u16;
    let mut worsts = Vec::new();
    for (transfer, (cb, cr)) in [
        (Transfer::Hlg, (512u16, 512u16)),
        (Transfer::Hlg, (400, 700)),
        (Transfer::Pq, (600, 450)),
    ] {
        let settings = Settings {
            deinterlace: Deinterlace::Weave,
            deinterlace_chosen: true,
            telecine: false,
            matrix: Matrix::Bt2020,
            limited: true,
            transfer,
            primaries: Primaries::Bt2020,
        };
        let Ok(mut present) = Present::offscreen(format, settings) else {
            eprintln!("GLES 2.0 が無いので飛ばす");
            return;
        };
        // P010：一つのバッファに輝度 H 行、続けて色差 H/2 行。サンプルは 16 ビットの上位 10 ビット。
        let Some(mut buffer) = card.buffer(W as u32, (H + H / 2) as u32) else {
            eprintln!("dumb バッファを作れないので飛ばす");
            return;
        };
        let pitch = buffer.pitch as usize;
        let words = buffer.bytes();
        for y in 0..H {
            for x in 0..W {
                let at = y * pitch + x * 2;
                words[at..at + 2].copy_from_slice(&(luma(x, y) << 6).to_le_bytes());
            }
        }
        for y in 0..H / 2 {
            for x in 0..W / 2 {
                let at = (H + y) * pitch + x * 4;
                words[at..at + 2].copy_from_slice(&(cb << 6).to_le_bytes());
                words[at + 2..at + 4].copy_from_slice(&(cr << 6).to_le_bytes());
            }
        }
        let fd = |buffer: &dumb::Buffer| buffer.export();
        let (Some(luma_fd), Some(chroma_fd)) = (fd(&buffer), fd(&buffer)) else {
            eprintln!("dumb バッファを DMA-BUF にできないので飛ばす");
            return;
        };
        let frame = woven(0, depth, zwcore::output::video::Parity::Top);
        let mut about = About::of_frame(&frame);
        about.format = format;
        about.progressive = true;
        let surface = Surface {
            about,
            held: Held::DmaBuf(DmaBuf {
                width: W as u32,
                height: H as u32,
                modifier: 0,
                planes: vec![
                    DmaBufPlane {
                        fd: luma_fd,
                        offset: 0,
                        stride: pitch as u32,
                        format: u32::from_le_bytes(*b"R16 "),
                    },
                    DmaBufPlane {
                        fd: chroma_fd,
                        offset: (pitch * H) as u32,
                        stride: pitch as u32,
                        format: u32::from_le_bytes(*b"GR32"),
                    },
                ],
                lease: None,
            }),
        };
        if let Err(error) = present.push(&Decoded::Surface(surface)) {
            eprintln!("P010 の DMA-BUF を取り込めないので飛ばす: {error:?}");
            return;
        }
        // 一枚だけなので、後ろのフレームを待たずに出させる。
        present.flush();
        let Some((got, ..)) = present.read_screen(W as u32, H as u32) else {
            eprintln!("レンダリングできなかったので飛ばす");
            return;
        };
        let conversion = Conversion::new(settings.matrix, depth, settings.limited);
        let mapping = Mapping::new(transfer, Primaries::Bt2020, Space::Srgb);
        let full = 1023.0;
        let mut worst = 0u8;
        let mut first_off = None;
        for y in 0..H {
            for x in 0..W {
                let carried = conversion.to_rgb(
                    f32::from(luma(x, y)) / full,
                    f32::from(cb) / full,
                    f32::from(cr) / full,
                );
                let want = mapping.apply(carried, false).map(quantise);
                let at = (y * W + x) * 4;
                for (c, one) in want.iter().enumerate() {
                    let off = got[at + c].abs_diff(*one);
                    if off > 4 && first_off.is_none() {
                        first_off = Some((x, y, got[at..at + 3].to_vec(), want));
                    }
                    worst = worst.max(off);
                }
            }
        }
        eprintln!("{transfer:?} の P010（Cb {cb} Cr {cr}）を SDR の画面へ  最大の差 {worst}");
        if let Some((x, y, got, want)) = first_off {
            eprintln!("  最初に外れた画素 ({x}, {y})  出力 {got:?}  参照 {want:?}");
        }
        worsts.push(worst);
    }
    // YCbCr から RGB へはドライバーが直す。Mesa（panfrost、2026-10 の 26.0）は 10 ビットの値にも
    // 8 ビットの黒と中央（16/255、128/255）を当てるらしく、灰色で G だけが上がる向きに 0.2% ほど
    // ずれ、HLG の明るい所の急な曲線で 5〜6 に広がった（実測）。色の行列は合っている。
    let worst = worsts.iter().copied().max().unwrap_or(0);
    assert!(
        worst <= 8,
        "P010 を外部テクスチャでレンダリングした結果が参照と {worst} 違う"
    );
}

/// DRM の dumb バッファ（表示デバイスが持つ、CPU から書ける DMA-BUF）。依存を増やさないよう、
/// ioctl は手で出す（`drm.h`、`drm_mode.h`）。
#[cfg(target_os = "linux")]
mod dumb {
    use std::ffi::c_void;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    unsafe extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
        fn mmap(
            at: *mut c_void,
            length: usize,
            protect: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(at: *mut c_void, length: usize) -> i32;
    }

    const CREATE_DUMB: std::ffi::c_ulong = 0xC020_64B2;
    const MAP_DUMB: std::ffi::c_ulong = 0xC010_64B3;
    const PRIME_HANDLE_TO_FD: std::ffi::c_ulong = 0xC00C_642D;
    const O_RDWR: u32 = 2;
    const O_CLOEXEC: u32 = 0o2_000_000;

    #[repr(C)]
    #[derive(Default)]
    struct CreateDumb {
        height: u32,
        width: u32,
        bpp: u32,
        flags: u32,
        handle: u32,
        pitch: u32,
        size: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MapDumb {
        handle: u32,
        pad: u32,
        offset: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct PrimeHandle {
        handle: u32,
        flags: u32,
        fd: i32,
    }

    pub struct Card {
        file: std::fs::File,
    }

    pub struct Buffer<'a> {
        card: &'a Card,
        handle: u32,
        pub pitch: u32,
        map: *mut u8,
        size: usize,
    }

    impl Card {
        pub fn open() -> Option<Self> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/dri/card0")
                .ok()?;
            Some(Self { file })
        }

        /// 16 ビットの画素で `width` × `rows`。
        pub fn buffer(&self, width: u32, rows: u32) -> Option<Buffer<'_>> {
            let fd = self.file.as_raw_fd();
            let mut create = CreateDumb {
                height: rows,
                width,
                bpp: 16,
                ..Default::default()
            };
            // SAFETY: 構造体は `drm_mode_create_dumb` と同じ並び。
            if unsafe { ioctl(fd, CREATE_DUMB, &mut create) } != 0 {
                return None;
            }
            let mut map = MapDumb {
                handle: create.handle,
                ..Default::default()
            };
            // SAFETY: 同上（`drm_mode_map_dumb`）。
            if unsafe { ioctl(fd, MAP_DUMB, &mut map) } != 0 {
                return None;
            }
            let size = create.size as usize;
            // SAFETY: デバイスが返したオフセットと大きさでマップする。
            let at = unsafe { mmap(std::ptr::null_mut(), size, 3, 1, fd, map.offset as i64) };
            if at as isize == -1 {
                return None;
            }
            Some(Buffer {
                card: self,
                handle: create.handle,
                pitch: create.pitch,
                map: at.cast(),
                size,
            })
        }
    }

    impl Buffer<'_> {
        pub fn bytes(&mut self) -> &mut [u8] {
            // SAFETY: マップした範囲で、このテストのほかに書き込む所はない。
            unsafe { std::slice::from_raw_parts_mut(self.map, self.size) }
        }

        pub fn export(&self) -> Option<OwnedFd> {
            let mut prime = PrimeHandle {
                handle: self.handle,
                flags: O_RDWR | O_CLOEXEC,
                fd: -1,
            };
            // SAFETY: 構造体は `drm_prime_handle` と同じ並び。
            if unsafe { ioctl(self.card.file.as_raw_fd(), PRIME_HANDLE_TO_FD, &mut prime) } != 0
                || prime.fd < 0
            {
                return None;
            }
            // SAFETY: デバイスが返したばかりのファイル記述子で、所有しているのはここだけ。
            Some(unsafe { OwnedFd::from_raw_fd(prime.fd) })
        }
    }

    impl Drop for Buffer<'_> {
        fn drop(&mut self) {
            // SAFETY: `buffer` でマップした範囲。バッファ自体はデバイスを閉じると消える。
            unsafe { munmap(self.map.cast(), self.size) };
        }
    }
}
