// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたフレームをファイルへ書き出す、外部のフレームと突き合わせる。

use super::*;

pub(super) fn each_frame(
    codec: Codec,
    framing: Framing,
    units: &[Cutout],
    most: usize,
    mut each: impl FnMut(&mediadecoder::video::Frame),
) -> usize {
    let mut decoder = zwcore::decode::open(codec, framing, Prefer::Software);
    let mut made = 0usize;
    for Cutout {
        tag,
        picture,
        format,
        data,
    } in units
    {
        if let Err(error) = decoder.decode(*tag, picture, *format, data) {
            tracing::warn!("  {tag}: {error:?}");
        }
        while let Some(Decoded::Planes(frame)) = decoder.take() {
            each(&frame);
            made += 1;
        }
        if made >= most {
            return made;
        }
    }
    decoder.flush();
    while let Some(Decoded::Planes(frame)) = decoder.take() {
        each(&frame);
        made += 1;
    }
    made
}

pub(super) fn write_frame(
    into: &mut impl std::io::Write,
    frame: &mediadecoder::video::Frame,
) -> std::io::Result<()> {
    let wide = usize::from(frame.format.depth > 8) + 1;
    for plane in 0..3 {
        let (width, height) = if plane == 0 {
            (frame.format.width as usize, frame.format.height as usize)
        } else {
            (
                (frame.format.width as usize).div_ceil(2),
                (frame.format.height as usize).div_ceil(2),
            )
        };
        for y in 0..height {
            into.write_all(&frame.planes[plane].row(y)[..width * wide])?;
        }
    }
    Ok(())
}

/// プラットフォームのデコーダーのフレームを [`dump`] と同じ生の並びで書き出す（二プレーンは三プレーンに）。
pub fn dump_hardware(path: &str, into: &str, most: usize) -> bool {
    dump_other(path, into, most, false)
}

/// OS のソフトウェアデコーダーのフレームを書き出す。開けなくても内蔵のデコーダーに切り替えない（比べる相手が自分になる）。
pub fn dump_system(path: &str, into: &str, most: usize) -> bool {
    dump_other(path, into, most, true)
}

fn dump_other(path: &str, into: &str, most: usize, system: bool) -> bool {
    let Some(cutting) = units(path) else {
        println!("{path}: 読めないか、映像がありません");
        return false;
    };
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let format = units[0].format;
    let file = match std::fs::File::create(into) {
        Ok(file) => file,
        Err(error) => {
            println!("{into}: 書けません（{error}）");
            return false;
        }
    };
    let mut out = std::io::BufWriter::new(file);
    zwcore::decode::set_memory_only(true);
    let mut decoder = if system {
        match zwcore::decode::system(codec, framing) {
            Ok(decoder) => decoder,
            Err(error) => {
                println!("{into}: OS のソフトウェアデコーダーが開けません（{error}）");
                return false;
            }
        }
    } else {
        zwcore::decode::open(codec, framing, Prefer::Platform)
    };
    let mut made = 0usize;
    let mut hardware = false;
    let deinterlaces = decoder.deinterlaces();
    let in_memory = system || deinterlaces || decoder.hardware();
    // 深さは読み戻したフレームの物（10 ビットのストリームを 8 ビットで出すデコーダーがある）。
    let mut depth = format.depth;
    let mut write = |read: &zwcore::decode::Readback, out: &mut dyn std::io::Write| {
        depth = read.about.format.depth;
        let wide = usize::from(depth > 8) + 1;
        let width = format.width as usize;
        let height = format.height as usize;
        let Some((luma, pitch, _)) = read.planes.first() else {
            return;
        };
        for y in 0..height {
            let at = y * pitch;
            let row = luma.get(at..at + width * wide).unwrap_or(&[]);
            let _ = out.write_all(row);
        }
        let Some((chroma, pitch, _)) = read.planes.get(1) else {
            return;
        };
        for side in 0..2 {
            for y in 0..height.div_ceil(2) {
                for x in 0..width.div_ceil(2) {
                    let at = y * pitch + (x * 2 + side) * wide;
                    let sample = chroma.get(at..at + wide).unwrap_or(&[0, 0][..wide]);
                    let _ = out.write_all(sample);
                }
            }
        }
    };
    for Cutout {
        tag,
        picture,
        format,
        data,
    } in units
    {
        if let Err(error) = decoder.decode(*tag, picture, *format, data) {
            tracing::warn!("  {tag}: {error:?}");
        }
        // デインターレースするデコーダーは再生と同じ取り出しで書く（読み戻しは後処理を通さない）。
        while !deinterlaces && let Some(read) = decoder.read_back() {
            hardware = true;
            write(&read, &mut out);
            made += 1;
        }
        while in_memory && let Some(Decoded::Planes(frame)) = decoder.take() {
            hardware = true;
            let _ = write_frame(&mut out, &frame);
            made += 1;
        }
        if made >= most {
            break;
        }
    }
    if made < most {
        decoder.flush();
        while !deinterlaces && let Some(read) = decoder.read_back() {
            hardware = true;
            write(&read, &mut out);
            made += 1;
        }
        while in_memory && let Some(Decoded::Planes(frame)) = decoder.take() {
            hardware = true;
            let _ = write_frame(&mut out, &frame);
            made += 1;
        }
    }
    if !hardware {
        println!(
            "{into}: {}がフレームを出しませんでした",
            if system {
                "OS のソフトウェアデコーダー"
            } else {
                "プラットフォームのデコーダー"
            }
        );
        return false;
    }
    let format = Format { depth, ..format };
    println!(
        "{into}: {codec:?} {}x{} を {made} 枚、{} バイト/枚（yuv420p{}）",
        format.width,
        format.height,
        frame_bytes(format),
        if format.depth > 8 { "10le" } else { "" }
    );
    true
}

pub(super) fn frame_bytes(format: Format) -> usize {
    let wide = usize::from(format.depth > 8) + 1;
    let luma = (format.width as usize) * (format.height as usize);
    let chroma = (format.width as usize).div_ceil(2) * (format.height as usize).div_ceil(2);
    (luma + 2 * chroma) * wide
}

pub fn dump(path: &str, into: &str, most: usize) -> bool {
    let Some(cutting) = units(path) else {
        println!("{path}: 読めないか、映像がありません");
        return false;
    };
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let format = units[0].format;
    let file = match std::fs::File::create(into) {
        Ok(file) => file,
        Err(error) => {
            println!("{into}: 書けません（{error}）");
            return false;
        }
    };
    let mut out = std::io::BufWriter::new(file);
    let mut failed = None;
    let made = each_frame(codec, framing, units, most, |frame| {
        if failed.is_none()
            && let Err(error) = write_frame(&mut out, frame)
        {
            failed = Some(error);
        }
    });
    if let Some(error) = failed {
        println!("{into}: 書けません（{error}）");
        return false;
    }
    println!(
        "{into}: {codec:?} {}x{} を {made} 枚、{} バイト/枚（yuv420p{}）",
        format.width,
        format.height,
        frame_bytes(format),
        if format.depth > 8 { "10le" } else { "" }
    );
    // `-fps_mode passthrough` is required, or ffmpeg duplicates or drops pictures.
    println!(
        "  ffmpeg -i {path} -an -pix_fmt yuv420p{} -fps_mode passthrough -frames:v {} -f rawvideo ffmpeg.yuv",
        if format.depth > 8 { "10le" } else { "" },
        made + 8
    );
    true
}

pub fn against(path: &str, raw: &str, most: usize) -> bool {
    let Some(cutting) = units(path) else {
        println!("{path}: 読めないか、映像がありません");
        return false;
    };
    let (codec, framing, units) = (cutting.codec, cutting.framing, &cutting.units);
    let format = units[0].format;
    let bytes = frame_bytes(format);
    let file = match std::fs::File::open(raw) {
        Ok(file) => file,
        Err(error) => {
            println!("{raw}: 読めません（{error}）");
            return false;
        }
    };
    let held = file.metadata().map(|found| found.len()).unwrap_or(0) as usize;
    if held < bytes {
        println!("{raw}: {held} バイトしかありません（1枚 {bytes} バイト）");
        return false;
    }
    let mut file = std::io::BufReader::new(file);

    let mut mine: Vec<Shot> = Vec::new();
    let mut kinds: Vec<u64> = Vec::new();
    each_frame(codec, framing, units, most, |frame| {
        let plane = &frame.planes[0];
        let wide = usize::from(frame.format.depth > 8) + 1;
        let mut luma = Vec::with_capacity(plane.height * plane.width * wide);
        for y in 0..frame.format.height as usize {
            luma.extend_from_slice(&plane.row(y)[..frame.format.width as usize * wide]);
        }
        mine.push(Shot {
            tag: frame.tag,
            pitch: frame.format.width as usize * wide,
            luma,
        });
        kinds.push(frame.tag);
    });
    if mine.is_empty() {
        println!("{path}: 映像が出ませんでした");
        return false;
    }

    // The two may not begin at the same picture and the shift may drift: measure at three places.
    let luma =
        (format.width as usize) * (format.height as usize) * (usize::from(format.depth > 8) + 1);
    let read = |file: &mut std::io::BufReader<std::fs::File>, at: usize| -> Option<Shot> {
        use std::io::{Read, Seek};
        file.seek(std::io::SeekFrom::Start((at * bytes) as u64))
            .ok()?;
        let mut into = vec![0u8; luma];
        file.read_exact(&mut into).ok()?;
        // 十ビットの配置をデータから決める。どの値も 1023 以下なら下位ビット寄せと見る。
        if format.depth > 8 {
            let bottom = into
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| u16::from_le_bytes(*pair) <= 1023);
            if bottom {
                for pair in into.as_chunks_mut::<2>().0 {
                    let value = u16::from_le_bytes(*pair) << (16 - u32::from(format.depth));
                    *pair = value.to_le_bytes();
                }
            }
        }
        Some(Shot {
            tag: 0,
            pitch: format.width as usize * (usize::from(format.depth > 8) + 1),
            luma: into,
        })
    };
    let count = held / bytes;
    println!(
        "    {raw}: {held} バイト、1枚 {bytes} バイト、{count} 枚。こちらは {} 枚",
        mine.len()
    );

    let wide = usize::from(format.depth > 8) + 1;
    let pitch = format.width as usize * wide;
    let tall = format.height as usize;
    let from = tall / 2 - 32.min(tall / 2);
    let rows = 64.min(tall);
    let mut stripe = |at: usize| -> Option<Vec<u8>> {
        use std::io::{Read, Seek};
        file.seek(std::io::SeekFrom::Start((at * bytes + from * pitch) as u64))
            .ok()?;
        let mut into = vec![0u8; rows * pitch];
        file.read_exact(&mut into).ok()?;
        Some(into)
    };
    pub(super) fn ours(shot: &Shot, from: usize, rows: usize, pitch: usize) -> &[u8] {
        &shot.luma[from * pitch..(from + rows) * pitch]
    }
    let alike = |a: &[u8], b: &[u8]| -> f64 {
        let total: f64 = a
            .iter()
            .zip(b)
            .map(|(a, b)| {
                let difference = f64::from(*a) - f64::from(*b);
                difference * difference
            })
            .sum();
        if total == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0 * 255.0 * a.len() as f64 / total).log10()
        }
    };
    let mut found: Vec<(usize, i64)> = Vec::new();
    for share in [4usize, 2, 4] {
        let at = if share == 2 {
            mine.len() / 2
        } else if found.is_empty() {
            mine.len() / 4
        } else {
            mine.len() * 3 / 4
        };
        // A still picture matches everywhere: among ties with the best, pick the nearest zero.
        let mut best = f64::NEG_INFINITY;
        let mut ties: Vec<i64> = Vec::new();
        for step in -64i64..=64 {
            let want = at as i64 + step;
            if want < 0 || want as usize >= count {
                continue;
            }
            if let Some(theirs) = stripe(want as usize) {
                let score = alike(ours(&mine[at], from, rows, pitch), &theirs);
                if score > best + 0.1 {
                    best = score;
                    ties.clear();
                    ties.push(step);
                } else if score > best - 0.1 {
                    best = best.max(score);
                    ties.push(step);
                }
            }
        }
        let step = ties
            .iter()
            .copied()
            .min_by_key(|step| (step.abs(), *step))
            .unwrap_or(0);
        if ties.len() > 8 {
            println!(
                "    {at}枚目は {} か所で同じくらい合います（この辺りは動きが無く、位置を決められません）",
                ties.len()
            );
        }
        found.push((at, step));
    }
    let shift = found[1].1;
    let steady = found.iter().all(|(_, step)| *step == shift);
    println!(
        "    ずらし {}{}",
        found
            .iter()
            .map(|(at, step)| format!("{at}枚目で{step:+}"))
            .collect::<Vec<_>>()
            .join("、"),
        if steady {
            "（一定）"
        } else {
            "（**ずれていきます。どちらかが余分に出しているか落としています**）"
        }
    );

    let mut theirs: Vec<Shot> = Vec::new();
    let mut ours: Vec<Shot> = Vec::new();
    for (at, shot) in mine.into_iter().enumerate() {
        let want = at as i64 + shift;
        if want < 0 || want as usize >= count {
            continue;
        }
        let Some(mut found) = read(&mut file, want as usize) else {
            continue;
        };
        found.tag = shot.tag;
        theirs.push(found);
        ours.push(shot);
    }
    let theirs = Pass {
        shots: theirs,
        spent: std::time::Duration::from_secs(1),
        hardware: true,
        first: None,
        errors: 0,
    };
    let mine = Pass {
        shots: ours,
        spent: std::time::Duration::from_secs(1),
        hardware: false,
        first: None,
        errors: 0,
    };
    let best = compare(&theirs, &mine, units, format);
    let Verdict {
        worst,
        mean,
        counted,
        ..
    } = best;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    // The last picture of a file cut by hand is cut through.
    let tail = best.every.len().saturating_sub(3);
    let poor: Vec<usize> = best
        .every
        .iter()
        .enumerate()
        .filter(|(_, (_, db))| *db <= 55.0)
        .map(|(at, _)| at)
        .collect();
    let only_the_end = !poor.is_empty() && poor.iter().all(|at| *at >= tail);
    let agreed = counted > 0 && (worst > 55.0 || only_the_end);
    println!(
        "{name}  {codec:?} {}x{}  ソフト対 {raw}  {counted} 枚  最悪 {worst:.1} / 平均 {mean:.1} dB{}",
        format.width,
        format.height,
        if !agreed {
            "  ← 合いません"
        } else if only_the_end {
            "  ← 合致（悪いのは最後の1枚だけで、そこは切り出しの端です）"
        } else {
            "  ← 合致"
        },
    );

    if let Some(listed) = poorest(&best, units) {
        println!("    不一致のフレーム: {listed}");
        println!("    種類ごと: {}", spread(&best, units));
        if let Some((tag, _)) = best.poorest.first() {
            let a = mine.shots.iter().find(|shot| shot.tag == *tag);
            let b = theirs.shots.iter().find(|shot| shot.tag == *tag);
            if let (Some(a), Some(b)) = (a, b) {
                println!("    tag {tag} の中身: {}", where_wrong(a, b, format));
            }
        }
    }
    agreed
}
