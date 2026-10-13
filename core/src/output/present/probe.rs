// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 実機で1回にまとめて調べる。[`report`] が GLES 2.0 のレンダリングパスについて知りたいことをテキストで返す。
//! 一致は、同じデバイスの CPU で出した正解（[`crate::output::video::filter`]）と GPU の出力を比べて見る。
//! コストとアドレスは実寸で、一致は小さいフレームで測る（参照実装が遅いため）。

use mediadecoder::es::{Format, Structure};
use mediadecoder::video::{Frame, Plane};

use crate::decode::{About, Decoded};
use crate::output::video::{Deinterlace, Settings, field, filter};

use super::glsl::Taps;
use super::{Capability, Present};

/// 一致を見るフレームの大きさ。128 あれば全部の分岐を通る。
const SMALL: u32 = 128;

/// `format` は実際に受信するときのフォーマット（コストとアドレスを測る対象）。
pub fn report(format: Format) -> String {
    let mut out = String::new();
    out.push_str("\n== GLES 2.0 のレンダリングパス ==\n");

    // GPU の情報と上限を先に取る。何もビルドできない環境でも、なぜビルドできないかを残すため。
    match super::describe() {
        Ok((name, list, limits, highp)) => {
            out.push_str(&format!("{name}\n"));
            out.push_str(&format!(
                "テクスチャ単位 {}（保証 8、このパスは 6 要る）  \
                 varying {}（保証 8）  最大テクスチャ {}\n",
                limits[0], limits[1], limits[2]
            ));
            out.push_str(&format!(
                "fragment の highp  {}\n",
                if highp { "対応" } else { "**非対応**" }
            ));
            let biggest = limits[2].max(0) as u32;
            for (width, height) in [(1440u32, 1080u32), (1920, 1080), (3840, 2160)] {
                out.push_str(&format!(
                    "  {width}x{height}  {}\n",
                    if width <= biggest && height <= biggest {
                        "収まる"
                    } else {
                        "**収まらない**"
                    }
                ));
            }
            out.push_str(&extensions(&list));
        }
        Err(error) => {
            out.push_str(&format!("**コンテキストすら作れません**: {error}\n"));
            return out;
        }
    }

    let settings = Settings::of_format(format);
    let mut present = match Present::offscreen(format, settings) {
        Ok(present) => present,
        Err(error) => {
            out.push_str(&format!("**開けません**: {error}\n"));
            return out;
        }
    };

    let capability = Capability::measure_with(&mut present, format);
    out.push_str(&capability.ladder_report());
    drop(present);

    if !capability.addresses {
        out.push_str(
            "\n**実寸で texel をサンプリングできないので、ここで止まります。**\n\
             どちらのサンプリング方式でも映像がずれるので、このデバイスでは直接出力\n\
             （MediaCodec → Surface）に任せるほかありません。\n",
        );
        return out;
    }

    out.push_str("\n-- 出力が参照実装と一致するか --\n");
    out.push_str(&accuracy(format.depth, capability.taps));

    out.push_str("\n-- 1フレームをアップロードするコスト --\n");
    out.push_str(&weaving(format));
    out
}

/// 効くものを先に並べ、そのあと全部を出す。`GL_EXT_texture_rg` が無いので、プレーンを
/// `GL_LUMINANCE` と `GL_LUMINANCE_ALPHA` で持っている。
fn extensions(list: &str) -> String {
    const WANTED: [(&str, &str); 6] = [
        ("GL_EXT_texture_rg", "R8/RG8。無いので LUMINANCE で持つ"),
        (
            "GL_EXT_unpack_subimage",
            "UNPACK_ROW_LENGTH。無いと行の再パックが要る",
        ),
        (
            "GL_OES_texture_npot",
            "2の冪でないテクスチャ。コア仕様でも足りるので任意",
        ),
        ("GL_OES_rgb8_rgba8", "8ビット4成分のレンダーターゲット"),
        ("GL_EXT_sRGB", "書き込み時に sRGB へ変換するか"),
        (
            "GL_OES_EGL_image_external",
            "外部テクスチャ。RGB しか返らないので使わない",
        ),
    ];
    let have: Vec<&str> = list.split_whitespace().collect();
    let mut out = String::from("GL 拡張\n");
    for (name, why) in WANTED {
        out.push_str(&format!(
            "  {}  {name}  ({why})\n",
            if have.contains(&name) {
                "あり"
            } else {
                "なし"
            }
        ));
    }
    out.push_str(&format!("  全部（{}件）\n    {list}\n", have.len()));
    out
}

fn accuracy(depth: u8, taps: Taps) -> String {
    let mut out = String::new();
    let format = Format {
        width: SMALL,
        height: SMALL,
        depth,
        ..Default::default()
    };
    for quality in [
        Deinterlace::Weave,
        Deinterlace::Bob,
        Deinterlace::Yadif,
        Deinterlace::Bwdif,
    ] {
        if !taps.carries(quality) {
            out.push_str(&format!(
                "  {:6} このサンプリング方式では使えない\n",
                quality.name()
            ));
            continue;
        }
        let settings = Settings {
            deinterlace: quality,
            telecine: false,
            ..Settings::of_format(format)
        };
        let frames = [combed(0, format), combed(1, format), combed(2, format)];
        let Ok(mut present) = Present::offscreen(format, settings) else {
            out.push_str(&format!("  {:6} 開けません\n", quality.name()));
            continue;
        };
        if present.force_taps(taps).is_err() || present.settings().deinterlace != quality {
            out.push_str(&format!("  {:6} ビルドできません\n", quality.name()));
            continue;
        }
        let mut pushed = true;
        for frame in &frames {
            pushed &= present.push(&Decoded::Planes(frame.clone())).is_ok();
        }
        if !pushed {
            out.push_str(&format!("  {:6} 入りません\n", quality.name()));
            continue;
        }
        // 前後にフレームを持つのは2フレーム目だけで、時間方向のフィルターが効くのもそこ。
        let of_first = field::plan(&About::of_frame(&frames[0]), quality).len();
        for _ in 0..of_first {
            if present.read_back().is_none() {
                break;
            }
        }
        let Some((got, ..)) = present.read_back() else {
            out.push_str(&format!("  {:6} レンダリングできません\n", quality.name()));
            continue;
        };
        let plan = field::plan(&About::of_frame(&frames[1]), quality);
        let Some(output) = plan.first() else {
            continue;
        };
        let want = filter::picture(&frames[0], &frames[1], &frames[2], output, settings);
        let (worst, off) = compare(&got, &want);
        out.push_str(&format!(
            "  {:6} 最大の差 {worst:3}  違う成分 {off:6} / {}{}\n",
            quality.name(),
            want.len(),
            if worst == 0 { "  **一致**" } else { "" },
        ));
    }
    out
}

fn compare(got: &[u8], want: &[u8]) -> (u8, usize) {
    if got.len() != want.len() {
        return (255, want.len());
    }
    let mut worst = 0;
    let mut off = 0;
    for (a, b) in got.iter().zip(want) {
        let difference = a.abs_diff(*b);
        worst = worst.max(difference);
        off += usize::from(difference != 0);
    }
    (worst, off)
}

/// 1フレームのアップロードのコストを並べ直しと転送に分けて測る。転送が大半なら、ES 2.0 に PBO は無いので
/// このパスそのものの上限になる。
fn weaving(format: Format) -> String {
    // `Decoded` を作るとき1フレーム分のコピーが起きるので、測るループの外で作る。
    let pictures: Vec<Decoded> = (0..3)
        .map(|at| Decoded::Planes(combed(at, format)))
        .collect();
    let mut out = String::new();
    // レンダリングするか・`glGetError` を呼ぶか・逆テレシネを動かすかの三つを切り替えて、内訳で説明できない分を出す。
    for (label, draw, checks, telecine) in [
        (
            "レンダリングなし・glGetError あり・逆テレシネ切",
            false,
            true,
            false,
        ),
        (
            "**レンダリングあり**・glGetError あり・逆テレシネ切",
            true,
            true,
            false,
        ),
        (
            "レンダリングあり・**glGetError なし**・逆テレシネ切",
            true,
            false,
            false,
        ),
        (
            "レンダリングあり・glGetError あり・**逆テレシネ入**（間に合わなければ自動で切る）",
            true,
            true,
            true,
        ),
    ] {
        let settings = Settings {
            telecine,
            ..Settings::of_format(format)
        };
        let Ok(mut present) = Present::offscreen(format, settings) else {
            return String::from("  開けません\n");
        };
        present.set_error_checks(checks);
        // ウォームアップ。最初の1フレームはメモリ確保が入る。
        for picture in &pictures {
            let _ = present.push(picture);
        }
        while present.ready() {
            present.draw_ready();
        }
        present.reset();
        let _ = present.spent();

        // 平均はたまの大きな一発に引きずられるので、1回ごとに取って中央値で見る。
        let mut rounds: Vec<[f64; 6]> = Vec::new();
        for round in 0..14 {
            let picture = &pictures[round % pictures.len()];
            if present.push(picture).is_err() {
                // キューが埋まって入らないだけ。レンダリングして空けてから続ける。
                while present.ready() {
                    present.draw_ready();
                }
                present.reset();
                let _ = present.spent();
                continue;
            }
            let spent = present.spent();
            let ms = |value: std::time::Duration| value.as_secs_f64() * 1e3;
            // 内訳で説明できない分は1回ごとに出す。中央値どうしを引くと実在しない差が出る。
            let whole = ms(spent.upload);
            let named = ms(spent.pack) + ms(spent.transfer) + ms(spent.check) + ms(spent.cadence);
            rounds.push([
                whole,
                ms(spent.pack),
                ms(spent.transfer),
                ms(spent.check),
                ms(spent.cadence),
                whole - named,
            ]);
            // 投入だけを繰り返すと GPU が暇なままになる。
            if draw && present.ready() {
                present.draw_ready();
            }
        }
        if rounds.is_empty() {
            out.push_str(&format!("  {label}  1フレームも入りません\n"));
            continue;
        }
        let middle = |which: usize| {
            let mut column: Vec<f64> = rounds.iter().map(|round| round[which]).collect();
            column.sort_by(f64::total_cmp);
            column[column.len() / 2]
        };
        let (whole, pack, transfer, check, cadence, left) = (
            middle(0),
            middle(1),
            middle(2),
            middle(3),
            middle(4),
            middle(5),
        );
        out.push_str(&format!("  {label}\n"));
        out.push_str(&format!(
            "     投入 {whole:.2} ms（中央値、{} 回）\n",
            rounds.len()
        ));
        out.push_str(&format!(
            "     並べ直し {pack:.2}  転送 {transfer:.2}  glGetError {check:.2}  逆テレシネ {cadence:.2}\n",
        ));
        out.push_str(&format!("     **内訳で説明できない分 {left:.2} ms**\n"));
        if telecine {
            out.push_str(&format!(
                "     検出器が間に合うと判定したか {}  いま有効か {}\n",
                match present.telecine_verdict() {
                    Some(true) => "間に合う",
                    Some(false) => "**間に合わないので切った**",
                    None => "まだ測っていない",
                },
                present.settings().telecine
            ));
        }
    }
    out.push_str(
        "  一行目と二行目の差が「レンダリングしていること」の影響、\n\
        \x20    二行目と三行目の差が glGetError のコスト、\n\
        \x20    二行目と四行目の差が逆テレシネの検出器。\n",
    );
    out.push_str(
        "  **読み方。**一行目と二行目で大きく動くなら、それはアップロードのコストでは\n\
        \x20    なく、GPU が追いつかないことの裏返し。投入は前のフレームの\n\
        \x20    レンダリングが終わるのを待たされているだけで、そこを削っても何も\n\
        \x20    速くならない。**そのときはフィルターのコストのほうを見る。**\n\
        \x20    動かないなら、アップロードのパスそのもののコスト。\n",
    );
    out
}

/// コーミングの出るフレーム。平坦なフレームでは測ったことにならないので、斜めのエッジと細かい縦縞を入れてある。
fn combed(at: usize, format: Format) -> Frame {
    let (width, height) = (format.width as usize, format.height as usize);
    let wide = format.depth > 8;
    let bytes = if wide { 2 } else { 1 };
    let full = (1u32 << format.depth.min(16)) - 1;
    let plane = |width: usize, height: usize, seed: usize| {
        let mut data = vec![0u8; width * height * bytes];
        for y in 0..height {
            // 偶数行と奇数行は半フレーム離れた時刻のもの。
            let when = 2 * at + y % 2;
            for x in 0..width {
                let bar = (x + 2 * when + seed) % 24 < 8;
                let value = if bar { 210 } else { 40 } + [0, 14, 7][y % 3];
                let code = (value as u32 * full / 255).min(full);
                let at = (y * width + x) * bytes;
                data[at] = code as u8;
                if wide {
                    data[at + 1] = (code >> 8) as u8;
                }
            }
        }
        Plane {
            data,
            stride: width * bytes,
            width,
            height,
        }
    };
    Frame {
        tag: at as u64,
        format,
        planes: [
            plane(width, height, 0),
            plane(width.div_ceil(2), height.div_ceil(2), 5),
            plane(width.div_ceil(2), height.div_ceil(2), 9),
        ],
        structure: Structure::Frame,
        top_field_first: true,
        repeat_first_field: false,
        progressive: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// コーミングの出ないフレームで測ると、どのフィルターも同じ結果になる。
    #[test]
    fn the_picture_the_probe_measures_actually_combs() {
        let format = Format {
            width: SMALL,
            height: SMALL,
            ..Default::default()
        };
        let frame = combed(0, format);
        let comb = filter::comb(&frame.planes[0], 8);
        assert!(comb > 0, "コーミングが出ていないフレームでは測れない");
    }

    #[test]
    fn a_wider_picture_keeps_its_stride() {
        for depth in [8, 10] {
            let format = Format {
                width: SMALL,
                height: SMALL,
                depth,
                ..Default::default()
            };
            let frame = combed(0, format);
            let bytes = if depth > 8 { 2 } else { 1 };
            assert_eq!(frame.planes[0].stride, SMALL as usize * bytes);
            assert_eq!(
                frame.planes[0].data.len(),
                SMALL as usize * SMALL as usize * bytes
            );
        }
    }
}
