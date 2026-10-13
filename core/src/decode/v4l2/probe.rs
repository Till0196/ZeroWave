// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Finding a decoder, and [`report`] of what a board can do.

use mediadecoder::es::Codec;

use crate::decode::Error;
use crate::decode::v4l2::ctrl::{Ctl, Kind, Naming};
use crate::decode::v4l2::dev::Device;
use crate::decode::v4l2::uapi::*;
use crate::decode::v4l2::{Flavour, Mode, modifier};

#[derive(Debug)]
pub struct Found {
    pub device: Device,
    pub flavour: Flavour,
    pub compressed: u32,
}

pub fn wanted(codec: Codec) -> [(u32, Flavour); 2] {
    match codec {
        Codec::Mpeg2 => [
            (V4L2_PIX_FMT_MPEG2_SLICE, Flavour::Stateless),
            (V4L2_PIX_FMT_MPEG2, Flavour::Stateful),
        ],
        Codec::H264 => [
            (V4L2_PIX_FMT_H264_SLICE, Flavour::Stateless),
            (V4L2_PIX_FMT_H264, Flavour::Stateful),
        ],
        Codec::Hevc => [
            (V4L2_PIX_FMT_HEVC_SLICE, Flavour::Stateless),
            (V4L2_PIX_FMT_HEVC, Flavour::Stateful),
        ],
    }
}

pub fn nodes() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = name.strip_prefix("video")?;
            number.parse::<u32>().ok()?;
            Some(format!("/dev/{name}"))
        })
        .collect();
    out.sort_by_key(|path| {
        path.trim_start_matches("/dev/video")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    out
}

/// The stateless one is preferred (our own reordering, like the software decoder).
pub fn find(codec: Codec) -> Result<Found, Error> {
    let mut tried = 0;
    let mut best: Option<Found> = None;
    for path in nodes() {
        let Ok(device) = Device::open(&path) else {
            continue;
        };
        tried += 1;
        let formats = device.formats(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
        for (compressed, flavour) in wanted(codec) {
            if !formats.contains(&compressed) {
                continue;
            }
            if flavour == Flavour::Stateless {
                return Ok(Found {
                    device,
                    flavour,
                    compressed,
                });
            }
            if best.is_none() {
                best = Some(Found {
                    device,
                    flavour,
                    compressed,
                });
            }
            break;
        }
    }
    best.ok_or_else(|| {
        Error::Unavailable(format!(
            "V4L2 のデコーダー {tried} 台に {codec:?} がありません",
        ))
    })
}

pub fn mode_of(device: &Device, codec: Codec, naming: Naming) -> Result<(Mode, u32), Error> {
    let (mode_kind, code_kind) = match codec {
        Codec::Mpeg2 => {
            return Ok((Mode::Frame, 0));
        }
        Codec::H264 => (Kind::H264DecodeMode, Kind::H264StartCode),
        Codec::Hevc => (Kind::HevcDecodeMode, Kind::HevcStartCode),
    };
    let (mode_id, code_id) = (mode_kind.id_for(naming), code_kind.id_for(naming));
    let mode = match device.control_range(mode_id) {
        Some((min, max, _)) if min <= 0 && max >= 0 => Mode::Slice,
        Some(_) => Mode::Frame,
        None => Mode::Frame,
    };
    // Annex B where the driver takes it; most drivers insist on it.
    let prefix = match device.control_range(code_id) {
        Some((min, max, _)) if min <= 1 && max >= 1 => 3,
        Some(_) => 0,
        None => 3,
    };
    // Said back to the driver: one that offers both would otherwise keep its default.
    let mut chosen = Vec::new();
    if device.control_range(mode_id).is_some() {
        chosen.push(Ctl::integer(
            mode_kind,
            if mode == Mode::Slice { 0 } else { 1 },
        ));
    }
    if device.control_range(code_id).is_some() {
        chosen.push(Ctl::integer(code_kind, i32::from(prefix != 0)));
    }
    device.set_controls(naming, &mut chosen, None)?;
    Ok((mode, prefix))
}

pub fn fourcc_name(code: u32) -> String {
    let bytes = code.to_le_bytes();
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(&bytes).trim_end().to_owned()
    } else {
        format!("{code:#010x}")
    }
}

pub fn report() -> String {
    let mut out = String::new();
    let say = |out: &mut String, line: String| {
        out.push_str(&line);
        out.push('\n');
    };

    say(&mut out, String::from("# 1. /dev/video* と、その出力形式"));
    say(
        &mut out,
        String::from("#    OUT に *_SLICE があれば stateless、素の H264/HEVC/MPG2 なら stateful。"),
    );
    say(
        &mut out,
        String::from("#    CAP に NC12/NC30 が出たら Raspberry Pi の SAND です。"),
    );
    let paths = nodes();
    if paths.is_empty() {
        say(&mut out, String::from("(なし)"));
    }
    for path in &paths {
        let device = match Device::open(path) {
            Ok(device) => device,
            Err(error) => {
                say(
                    &mut out,
                    format!("{path}\t開けません／M2M ではありません（{error}）"),
                );
                continue;
            }
        };
        let outputs = device.formats(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
        let captures = device.formats(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
        say(
            &mut out,
            format!(
                "{path}\tdriver={} card={}\n\tOUT {}\n\tCAP {}",
                device.driver,
                device.card,
                outputs
                    .iter()
                    .map(|code| fourcc_name(*code))
                    .collect::<Vec<_>>()
                    .join(" "),
                captures
                    .iter()
                    .map(|code| fourcc_name(*code))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        );

        for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
            for (compressed, flavour) in wanted(codec) {
                if !outputs.contains(&compressed) {
                    continue;
                }
                say(
                    &mut out,
                    format!("\t{codec:?}: {flavour:?}（{}）", fourcc_name(compressed)),
                );
                if flavour == Flavour::Stateful {
                    continue;
                }
                say(&mut out, controls_line(&device, codec));
            }
        }

        // These caps are only reported once buffers are requested, so ask for one.
        if let Ok((_, caps)) = device.request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 1) {
            say(
                &mut out,
                format!(
                    "\tOUT queue: REQUESTS={} HOLD_CAPTURE_BUF={}\n\
                     \t  # HOLD がないと、1 枚を複数スライスに分けたフレームを出せません",
                    yes(caps & V4L2_BUF_CAP_SUPPORTS_REQUESTS != 0),
                    yes(caps & V4L2_BUF_CAP_SUPPORTS_M2M_HOLD_CAPTURE_BUF != 0),
                ),
            );
            let _ = device.request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 0);
        }
        match crate::decode::v4l2::dev::media_for(&device) {
            Ok(_) => say(
                &mut out,
                String::from("\tmedia: あり（Request API が使えます）"),
            ),
            Err(error) => say(&mut out, format!("\tmedia: {error:?}")),
        }
    }

    say(&mut out, String::new());
    say(
        &mut out,
        String::from(
            "# 2. Vulkan が列挙する修飾子。SAND128 が 8 ビットと 10 ビットの両方で出るか。\n\
             #    R8/GR88 に SAND128 が出れば Pi の 8 ビットは取り込めます。\n\
             #    R16/GR1616 に出ても 10 ビットは通りません（NC30 は P030 で、\n\
             #    3 サンプルを 30 ビットに詰めた形なのでプレーンごとの Vulkan 形式がありません）。",
        ),
    );
    for wide in [false, true] {
        match modifier::all(wide) {
            Ok(found) => {
                for (drm, list) in found {
                    let mut line = format!("{}\t", modifier::fourcc_text(drm));
                    if list.is_empty() {
                        line.push_str("(なし)");
                    }
                    for taken in list {
                        line.push_str(&format!(
                            "{}{:#018x}(planes={}{}) ",
                            if taken.value >> 56 == 0x07 {
                                "BROADCOM/SAND "
                            } else {
                                ""
                            },
                            taken.value,
                            taken.planes,
                            if taken.sampled { "" } else { ",not sampled" }
                        ));
                    }
                    say(&mut out, line);
                }
            }
            Err(why) => say(
                &mut out,
                format!("{}ビット: {why}", if wide { 10 } else { 8 }),
            ),
        }
    }

    say(&mut out, String::new());
    say(
        &mut out,
        String::from(
            "# 3. このビルドが書く制御のサイズ。上の各デバイスの行と違えば、\n\
             #    そのバージョンのカーネルとは構造体が違うということです。",
        ),
    );
    for kind in [
        Kind::Mpeg2Sequence,
        Kind::Mpeg2Picture,
        Kind::Mpeg2Quantisation,
        Kind::H264Sps,
        Kind::H264Pps,
        Kind::H264ScalingMatrix,
        Kind::H264PredWeights,
        Kind::H264SliceParams,
        Kind::H264DecodeParams,
        Kind::HevcSps,
        Kind::HevcPps,
        Kind::HevcSliceParams,
        Kind::HevcScalingMatrix,
        Kind::HevcDecodeParams,
    ] {
        say(
            &mut out,
            format!(
                "{}\t{} バイト\t{:#010x}",
                kind.name(),
                kind.size(),
                kind.id()
            ),
        );
    }
    out
}

fn controls_line(device: &Device, codec: Codec) -> String {
    let naming = match codec {
        Codec::Hevc => match device.hevc_naming() {
            Ok(naming) => naming,
            Err(error) => return format!("\t  制御: {error:?}"),
        },
        _ => Naming::Stateless,
    };
    let mut line = format!("\t  制御({naming:?}): ");
    for kind in crate::decode::v4l2::wanted_controls(codec) {
        let id = kind.id_for(naming);
        match device.control_size(id) {
            None => line.push_str(&format!("{}=なし ", kind.name())),
            Some(size) if size as usize == kind.size() => {
                line.push_str(&format!("{}=ok ", kind.name()));
            }
            Some(size) => line.push_str(&format!(
                "{}=カーネル {size}／このビルド {} ",
                kind.name(),
                kind.size()
            )),
        }
    }
    match mode_of(device, codec, naming) {
        Ok((mode, prefix)) => {
            line.push_str(&format!("\n\t  mode={mode:?} start_code={} バイト", prefix))
        }
        Err(error) => line.push_str(&format!("\n\t  mode: {error:?}")),
    }
    line
}

fn yes(value: bool) -> &'static str {
    if value { "あり" } else { "なし" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stateless_format_is_looked_for_first() {
        for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
            let [(_, first), (_, second)] = wanted(codec);
            assert_eq!(first, Flavour::Stateless);
            assert_eq!(second, Flavour::Stateful);
        }
        assert_eq!(wanted(Codec::H264)[0].0, V4L2_PIX_FMT_H264_SLICE);
        assert_eq!(wanted(Codec::H264)[1].0, V4L2_PIX_FMT_H264);
    }

    #[test]
    fn a_format_reads_as_the_letters_it_spells() {
        assert_eq!(fourcc_name(V4L2_PIX_FMT_H264_SLICE), "S264");
        assert_eq!(fourcc_name(V4L2_PIX_FMT_NV12_COL128), "NC12");
        assert_eq!(fourcc_name(V4L2_PIX_FMT_NV12_10_COL128), "NC30");
        assert_eq!(fourcc_name(0x0001_0203), "0x00010203");
    }

    #[test]
    fn the_report_comes_out_on_a_machine_with_no_decoder() {
        let text = report();
        assert!(text.contains("/dev/video*"));
        assert!(text.contains("HEVC_SLICE_PARAMS"));
    }
}
