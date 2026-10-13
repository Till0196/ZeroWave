// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! NVIDIA の GPU が何に対応しているか、NVDEC のフレームが自前のソフトウェアデコーダーと合うか。
//! 実機が要るので `#[ignore]`：
//! `cargo test -p zerowave-core --features nvdec --test nvdec -- --ignored --nocapture`
//! `deck` は Windows でビルドできないのでここに置く。H.264/HEVC は ∞ dB、MPEG-2 は 65 dB 前後が正しい。

// 基準は内蔵のソフトウェアデコーダーなので、その feature も要る。
#![cfg(all(
    feature = "nvdec",
    any(target_os = "linux", windows),
    feature = "h264-decoder",
    feature = "hevc-decoder"
))]

use std::time::{Duration, Instant};

use mediadecoder::es::{Codec, Format, Framing};
use zwcore::decode::{Decode, Luma, Readback};
use zwcore::filters::cut::{Cutout, Cutting};

/// 素材は ffmpeg で作った物（`testdata/make.sh`）。
const STREAMS: [&str; 5] = [
    "h264-baseline.ts",
    "h264-high.ts",
    "hevc-main10.ts",
    "mpeg2-interlaced.ts",
    "mpeg2-telecine.ts",
];

fn testdata(name: &str) -> String {
    format!("{}/../testdata/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn frames() -> usize {
    std::env::var("ZW_NVDEC_FRAMES")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(24)
}

#[test]
#[ignore = "needs an NVIDIA machine; prints what it found"]
fn say_what_the_card_has() {
    use zwcore::decode::nvdec::cu;
    let context = match cu::Context::open() {
        Ok(context) => context,
        Err(error) => {
            println!("CUDA を開けません: {error:?}");
            return;
        }
    };
    println!("デバイス: {}", context.name);
    // エクスポートできるかは属性を取得すれば分かる（`cuMemGetHandleForAddressRange` は真のときだけ DMA-BUF
    // を渡す）。
    println!(
        "  DMA-BUF のエクスポート: {}",
        if context.exports_dma_buf() {
            "あり"
        } else {
            "ありません"
        }
    );
    for (name, codec) in [
        ("MPEG-2", cu::CUDA_VIDEO_CODEC_MPEG2),
        ("H.264", cu::CUDA_VIDEO_CODEC_H264),
        ("HEVC", cu::CUDA_VIDEO_CODEC_HEVC),
    ] {
        for depth in [8u32, 10] {
            // `cuvidGetDecoderCaps` はテーブルを参照するだけで、拒否するのは `cuvidCreateDecoder`。両方見る。
            match context.caps(codec, depth) {
                Ok(caps) if caps.supported != 0 => println!(
                    "  {name} {depth}bit: あり / 最大 {}x{} / MB {} / 最小 {}x{} \
                     / 出力の形 0x{:04x} / NVDEC {} 基",
                    caps.max_width,
                    caps.max_height,
                    caps.max_mb_count,
                    caps.min_width,
                    caps.min_height,
                    caps.output_format_mask,
                    caps.num_nvdecs,
                ),
                Ok(_) => println!("  {name} {depth}bit: ありません"),
                Err(error) => println!("  {name} {depth}bit: 取得できません（{error:?}）"),
            }
        }
    }
    let context = std::sync::Arc::new(context);
    for (name, codec, depth, width, height, format, surfaces) in [
        (
            "MPEG-2 720x480",
            cu::CUDA_VIDEO_CODEC_MPEG2,
            8u32,
            720u32,
            480u32,
            cu::CUDA_VIDEO_SURFACE_NV12,
            8,
        ),
        (
            "H.264 1920x1088",
            cu::CUDA_VIDEO_CODEC_H264,
            8,
            1920,
            1088,
            cu::CUDA_VIDEO_SURFACE_NV12,
            8,
        ),
        // わざと通らない大きさ。`INVALID_VALUE` なら構造体の並びは正しい。
        (
            "H.264 100000x1088（わざと無理な大きさ）",
            cu::CUDA_VIDEO_CODEC_H264,
            8,
            100_000,
            1088,
            cu::CUDA_VIDEO_SURFACE_NV12,
            8,
        ),
        (
            "HEVC 1920x1088",
            cu::CUDA_VIDEO_CODEC_HEVC,
            8,
            1920,
            1088,
            cu::CUDA_VIDEO_SURFACE_NV12,
            8,
        ),
        (
            "HEVC 1920x1088 サーフェス 16枚",
            cu::CUDA_VIDEO_CODEC_HEVC,
            8,
            1920,
            1088,
            cu::CUDA_VIDEO_SURFACE_NV12,
            16,
        ),
        (
            "HEVC 1920x1088 10bit",
            cu::CUDA_VIDEO_CODEC_HEVC,
            10,
            1920,
            1088,
            cu::CUDA_VIDEO_SURFACE_P016,
            8,
        ),
        (
            "HEVC 3840x2160 10bit",
            cu::CUDA_VIDEO_CODEC_HEVC,
            10,
            3840,
            2160,
            cu::CUDA_VIDEO_SURFACE_P016,
            16,
        ),
    ] {
        let info = cu::DecodeCreateInfo {
            width: cu::CuUlong::from(width),
            height: cu::CuUlong::from(height),
            num_decode_surfaces: surfaces,
            codec,
            chroma_format: cu::CUDA_VIDEO_CHROMA_420,
            creation_flags: cu::CUDA_VIDEO_CREATE_PREFER_CUVID,
            bit_depth_minus8: cu::CuUlong::from(depth - 8),
            max_width: cu::CuUlong::from(width),
            max_height: cu::CuUlong::from(height),
            output_format: format,
            deinterlace_mode: cu::CUDA_VIDEO_DEINTERLACE_WEAVE,
            target_width: cu::CuUlong::from(width),
            target_height: cu::CuUlong::from(height),
            num_output_surfaces: 1,
            ..cu::DecodeCreateInfo::default()
        };
        match context.decoder(info) {
            Ok(_) => println!("  作れます: {name}"),
            Err(error) => println!("  **作れません**: {name} — {error:?}"),
        }
    }
}

#[test]
#[ignore = "reads the streams and prints what their parameter sets ask for"]
fn say_what_the_stream_asks_for() {
    for path in STREAMS.map(testdata) {
        let Some(cutting) = read(&path, 4) else {
            continue;
        };
        let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
        if codec != Codec::Hevc {
            continue;
        }
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
        for Cutout { data, .. } in units {
            if say_hevc(&name, framing, data) {
                break;
            }
        }
    }
}

/// 取り込んで、レンダリングして、読み戻す（`read_back` が測らない Windows の二回のコピーを測る）。
/// 飛ばすときは理由を必ず出す。
#[cfg(feature = "wgpu")]
#[test]
#[ignore = "needs an NVIDIA machine and a GPU to draw on; says why it skips"]
fn a_frame_the_hardware_wrote_is_drawn_the_same_as_one_it_did_not() {
    use zwcore::output::video::Settings;
    use zwcore::output::video::gpu::{Gpu, Video};

    // 先に GPU を開く（`video::import_d3d12::adopt` にデバイスを渡してから共有する）。
    let gpu = match Gpu::open() {
        Ok(gpu) => {
            eprintln!("レンダリングするデバイス: {}", gpu.name());
            gpu
        }
        Err(error) => {
            eprintln!("**飛ばします**: レンダリングできるデバイスがありません（{error}）");
            return;
        }
    };
    let wanted = "h264-high.ts";
    let path = testdata(wanted);
    let Some(cutting) = read(&path, 32) else {
        eprintln!("**飛ばします**: {path} を切り分けられません");
        return;
    };
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let mut hardware = match zwcore::decode::nvdec::Nvdec::open(codec, framing) {
        Ok(decoder) => decoder,
        Err(error) => {
            eprintln!("**飛ばします**: NVDEC が開きません（{error:?}）");
            return;
        }
    };
    let mut software = zwcore::decode::software(codec, framing);
    let format = units[0].format;
    let settings = Settings::of_format(format);
    let through = |decoder: &mut dyn Decode, who: &str| -> Option<Vec<Vec<u8>>> {
        let mut video = Video::new(
            &gpu.device,
            &gpu.queue,
            wgpu::TextureFormat::Rgba8Unorm,
            format,
            settings,
        )
        .map_err(|error| eprintln!("{who}: レンダラーを作れません（{error:?}）"))
        .ok()?;
        let mut drawn = Vec::new();
        for Cutout {
            tag,
            picture,
            format,
            data,
        } in units
        {
            if let Err(error) = decoder.decode(*tag, picture, *format, data) {
                eprintln!("{who} {tag}: {error:?}");
            }
            while let Some(found) = decoder.take() {
                if let Err(why) = video.push(&found) {
                    eprintln!("{who}: 渡せません: {why:?}");
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
        eprintln!("{who}: フレームが {} 枚", drawn.len());
        (drawn.len() >= 3).then_some(drawn)
    };
    let Some(theirs) = through(&mut hardware, "ハード") else {
        eprintln!("**飛ばします**: ハードウェアの経路がフレームを出しませんでした");
        return;
    };
    let Some(mine) = through(software.as_mut(), "ソフト") else {
        eprintln!("**飛ばします**: ソフトウェアの経路がフレームを出しませんでした");
        return;
    };
    let spread = |a: &[u8], b: &[u8]| -> (u32, usize) {
        let worst = a
            .iter()
            .zip(b)
            .map(|(x, y)| u32::from(x.abs_diff(*y)))
            .max()
            .unwrap_or(0);
        let differing = a.iter().zip(b).filter(|(x, y)| x != y).count();
        (worst, differing)
    };
    for (at, one) in theirs.iter().enumerate().take(4) {
        let line: Vec<String> = mine
            .iter()
            .take(4)
            .map(|other| {
                let (worst, differing) = spread(one, other);
                format!("{worst}/{differing}")
            })
            .collect();
        eprintln!("  ハード {at} 対 ソフト 0..: {}", line.join("  "));
    }
    let where_ = |a: &[u8], b: &[u8]| -> String {
        let width = format.width as usize;
        let mut rows = (usize::MAX, 0usize);
        let mut columns = (usize::MAX, 0usize);
        let mut channels = [0usize; 4];
        for (at, (x, y)) in a.iter().zip(b).enumerate() {
            if x == y {
                continue;
            }
            let pixel = at / 4;
            let (row, column) = (pixel / width, pixel % width);
            rows = (rows.0.min(row), rows.1.max(row));
            columns = (columns.0.min(column), columns.1.max(column));
            channels[at % 4] += 1;
        }
        if rows.0 == usize::MAX {
            return String::from("同じ");
        }
        format!(
            "行 {}..={} / 列 {}..={} / 成分ごと {:?}",
            rows.0, rows.1, columns.0, columns.1, channels
        )
    };
    let counted = mine.len().min(theirs.len());
    for at in 0..counted {
        let (worst, differing) = spread(&theirs[at], &mine[at]);
        if differing > 0 {
            eprintln!(
                "  {at}: 最大 {worst} / {differing} バイト / {}",
                where_(&theirs[at], &mine[at])
            );
        }
    }
    // 下端の二行は別に見る（サーフェスの下のパディングをフィルターが読む）。
    // 本体は一バイトも違わないことを断言する。
    let width = format.width as usize;
    let body = (format.height as usize).saturating_sub(2) * width * 4;
    let worst_body = (0..counted)
        .map(|at| spread(&theirs[at][..body], &mine[at][..body]).0)
        .max()
        .unwrap_or(0);
    let worst_edge = (0..counted)
        .map(|at| spread(&theirs[at][body..], &mine[at][body..]).0)
        .max()
        .unwrap_or(0);
    eprintln!("{wanted}: 本体の最大の差 {worst_body} / 下端二行の最大の差 {worst_edge}");
    // 十ビットだけ最下位が 1 まで動く（正規化の違い）。`tests/vaapi.rs` に揃えて 2 まで。
    assert!(
        worst_body <= 2,
        "取り込んでレンダリングしたフレームが {worst_body} 違います（下端の二行を除いて）"
    );
}

fn say_hevc(name: &str, framing: Framing, data: &[u8]) -> bool {
    use mediadecoder::bits::{Reader, unescape};
    use mediadecoder::es::Nals;
    use mediadecoder::video::hevc::params;
    let mut sps = None;
    let mut pps = None;
    for nal in Nals::new(data, framing) {
        let kind = (nal.header >> 1) & 0x3f;
        let Some(bytes) = data.get(nal.body.start + 1..nal.body.end) else {
            continue;
        };
        match kind {
            params::SPS_NUT => sps = params::sequence(&unescape(bytes, 4096)).ok(),
            params::PPS_NUT => {
                let bytes = unescape(bytes, 4096);
                let mut reader = Reader::new(&bytes);
                reader.ue();
                reader.ue();
                pps = params::picture(&bytes, sps.as_ref()).ok();
            }
            _ => {}
        }
    }
    let (Some(sps), Some(pps)) = (sps, pps) else {
        return false;
    };
    println!("{name}:");
    println!(
        "  SPS {}x{} ビット深度 {:?} / CTB {} / CB {} / TB {}..{} / 短期 RPS {} / 長期参照 {}",
        sps.width,
        sps.height,
        sps.depth,
        1 << sps.ctb_log2,
        1 << sps.min_cb_log2,
        1 << sps.min_tb_log2,
        1 << sps.max_tb_log2,
        sps.short_term.len(),
        sps.long_term_present,
    );
    println!(
        "  スケーリングリスト 有効 {} / PPS で上書き {} ({}) / タイル {} 列 {} 行 / WPP {}",
        sps.scaling_list_enabled,
        pps.scaling_list_present,
        if pps.scaling.is_some() {
            "PPS にある"
        } else {
            "PPS には無い"
        },
        pps.column_width.len(),
        pps.row_height.len(),
        pps.entropy_coding_sync_enabled,
    );
    let scaling = pps.scaling.as_ref().unwrap_or(&sps.scaling);
    for size in 0..4 {
        let matrices = if size == 3 { 2 } else { 6 };
        let mut least = 255u8;
        let mut most = 0u8;
        let mut zeroes = 0usize;
        for matrix in 0..matrices {
            for &value in scaling.matrix(size, if size == 3 { matrix * 3 } else { matrix }) {
                least = least.min(value);
                most = most.max(value);
                zeroes += usize::from(value == 0);
            }
        }
        println!(
            "  {}x{} のスケーリングリスト: 最小 {least} 最大 {most} ゼロ {zeroes} 個 / 先頭 {:?}",
            4 << size,
            4 << size,
            &scaling.matrix(size, 0)[..8.min(scaling.matrix(size, 0).len())],
        );
    }
    true
}

#[test]
#[ignore = "needs an NVIDIA machine; prints what it measured"]
fn hold_the_hardware_against_the_software() {
    for path in &STREAMS.map(testdata) {
        match measure(path, frames()) {
            Some(line) => println!("{line}"),
            None => println!("{path}: 読めないか、映像がありません"),
        }
    }
}

fn measure(path: &str, most: usize) -> Option<String> {
    let cutting = read(path, most)?;
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let format = units.first()?.format;
    let hardware = pass(codec, framing, zwcore::decode::Prefer::Platform, units);
    let software = pass(codec, framing, zwcore::decode::Prefer::Software, units);
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if !hardware.hardware {
        return Some(format!(
            "{name}  {codec:?} {}x{}  ハードウェアの経路がありません（{}）",
            format.width,
            format.height,
            hardware
                .first
                .clone()
                .unwrap_or_else(|| String::from("理由なし")),
        ));
    }
    let common = hardware.shots.len().min(software.shots.len());
    let ordered = common > 0 && (0..common).all(|at| hardware.shots[at].0 == software.shots[at].0);
    // オープン GOP の先頭のフレームは持っていない参照から予測するので落とす。
    let entry: std::collections::BTreeSet<u64> = units
        .iter()
        .filter(|unit| unit.picture.random_access)
        .map(|unit| unit.tag)
        .collect();
    let leading = hardware
        .shots
        .iter()
        .position(|shot| entry.contains(&shot.0))
        .unwrap_or(0);
    let by_tag: std::collections::BTreeMap<u64, &Shot> =
        software.shots.iter().map(|shot| (shot.0, shot)).collect();
    let mut worst = f64::INFINITY;
    let mut total = 0.0;
    let mut finite = 0usize;
    let mut counted = 0usize;
    for shot in hardware.shots[leading..]
        .iter()
        .take(common.saturating_sub(leading))
    {
        let Some(mine) = by_tag.get(&shot.0) else {
            continue;
        };
        let found = psnr(mine, shot, format);
        worst = worst.min(found);
        if found.is_finite() {
            total += found;
            finite += 1;
        }
        counted += 1;
    }
    let rate = |spent: Duration, count: usize| {
        if spent.is_zero() {
            0.0
        } else {
            count as f64 / spent.as_secs_f64()
        }
    };
    let mut line = format!(
        "{name}  {codec:?} {}x{}{}  ハード {:.0} fps / ソフト {:.0} fps  {counted} 枚  ",
        format.width,
        format.height,
        if format.depth > 8 { " 10bit" } else { "" },
        rate(hardware.spent, hardware.shots.len()),
        rate(software.spent, software.shots.len()),
    );
    if worst.is_infinite() {
        line.push_str("完全一致");
    } else {
        line.push_str(&format!(
            "最悪 {worst:.1} / 平均 {:.1} dB",
            if finite > 0 {
                total / finite as f64
            } else {
                worst
            }
        ));
    }
    if !ordered {
        line.push_str("  順序が違います");
    }
    if hardware.errors > 0 {
        line.push_str(&format!(
            "  エラー {} 回（{}）",
            hardware.errors,
            hardware.first.clone().unwrap_or_default()
        ));
    }
    Some(line)
}

type Shot = (u64, Vec<u8>, usize);

struct Pass {
    shots: Vec<Shot>,
    spent: Duration,
    hardware: bool,
    first: Option<String>,
    errors: usize,
}

fn pass(codec: Codec, framing: Framing, prefer: zwcore::decode::Prefer, units: &[Cutout]) -> Pass {
    // どのデコーダーを測ったかを必ず出す（`decode::open` は D3D12 を先に試す）。既定は NVDEC を直接指定、
    // `ZW_NVDEC_VIA=open` で出荷する経路そのもの。
    let via_open = std::env::var("ZW_NVDEC_VIA").is_ok_and(|how| how == "open");
    let mut decoder: Box<dyn Decode> = match prefer {
        zwcore::decode::Prefer::Platform if via_open => {
            eprintln!("  ハードウェアの側: decode::open が選んだもの");
            zwcore::decode::open(codec, framing, prefer)
        }
        zwcore::decode::Prefer::Platform => {
            eprintln!("  ハードウェアの側: NVDEC（直接指定）");
            match zwcore::decode::nvdec::Nvdec::open(codec, framing) {
                Ok(decoder) => Box::new(decoder),
                Err(error) => {
                    return Pass {
                        shots: Vec::new(),
                        spent: Duration::ZERO,
                        hardware: false,
                        first: Some(format!("{error:?}")),
                        errors: 1,
                    };
                }
            }
        }
        zwcore::decode::Prefer::Software => zwcore::decode::software(codec, framing),
        zwcore::decode::Prefer::System => unreachable!("NVDEC のテストは System を渡さない"),
    };
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
    if prefer == zwcore::decode::Prefer::Platform && out.shots.is_empty() && out.first.is_none() {
        out.first = Some(String::from("フレームが一枚も出ませんでした"));
    }
    out
}

fn drain(decoder: &mut dyn Decode, into: &mut Pass) {
    loop {
        if let Some(read) = decoder.read_back() {
            into.hardware = true;
            push(into, &read);
            continue;
        }
        match decoder.take() {
            Some(zwcore::decode::Decoded::Planes(frame)) => {
                let plane = &frame.planes[0];
                let wide = usize::from(frame.format.depth > 8) + 1;
                let mut luma = Vec::with_capacity(plane.height * plane.width * wide);
                for y in 0..plane.height {
                    luma.extend_from_slice(&plane.row(y)[..plane.width * wide]);
                }
                into.shots.push((frame.tag, luma, plane.width * wide));
            }
            Some(zwcore::decode::Decoded::Surface(_)) => {}
            None => return,
        }
    }
}

fn push(into: &mut Pass, read: &Readback) {
    if let Some((bytes, pitch)) = read.luma() {
        into.shots.push((read.about.tag, bytes.to_vec(), pitch));
    }
}

/// 輝度だけの PSNR（[`zwcore::decode::psnr`]）。ハード側は十ビットを十六ビットの上に置く。
fn psnr(mine: &Shot, theirs: &Shot, format: Format) -> f64 {
    let shift = if format.depth > 8 {
        16 - u32::from(format.depth)
    } else {
        0
    };
    zwcore::decode::psnr(
        Luma {
            bytes: &mine.1,
            pitch: mine.2,
            shift: 0,
        },
        Luma {
            bytes: &theirs.1,
            pitch: theirs.2,
            shift,
        },
        format,
    )
}

fn read(path: &str, most: usize) -> Option<Cutting> {
    Cutting::of_path(path, most)
}
