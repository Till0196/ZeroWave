// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `zerowave.toml` を、コマンドラインのオプションが作るのと同じ物にする。解釈は [`Choices::parse`] に任せる。

use crate::choice::{Choices, Knobs, WheelDoes};
use crate::config::file::File;
use zwcore::sound::Passthrough;

#[derive(Debug, Clone, Default)]
pub struct Applied {
    pub choices: Choices,
    pub sound_device: String,
    pub passthrough: Passthrough,
    pub volume: Option<u32>,
    pub restore_mute: bool,
    pub plan: zwcore::decode::Plan,
    pub sound_plan: zwcore::decode::sound::SoundPlan,
}

/// コマンドラインのオプションが上書きするキー（設定ウィンドウで上書き中と出す）。オプションを足したらここにも足す。
pub fn key_of_flag(flag: &str) -> Option<&'static str> {
    Some(match flag {
        "--decoder" => "video.decoder.default",
        "--decoder-api" => "video.decoder.api",
        "--deinterlace" => "video.deinterlace.method",
        "--deinterlace-by" => "video.deinterlace.by",
        "--telecine" => "video.deinterlace.telecine",
        "--upscale" => "video.scale.upscale",
        "--downscale" => "video.scale.downscale",
        "--antiring" => "video.scale.antiring",
        "--render" => "video.render.render",
        "--backend" => "video.render.backend",
        "--gpu" => "video.render.gpu",
        "--present" => "video.render.present",
        "--hdr" => "video.render.hdr",
        "--video" => "video.frame.track",
        "--audio-latency" => "audio.latency",
        "--dual-mono" => "audio.dual_mono",
        "--mono-to-stereo" => "audio.mono_to_stereo",
        "--downmix" => "audio.downmix",
        "--downmix-22" => "audio.downmix_22",
        "--passthrough" => "audio.passthrough",
        "--sound-device" => "audio.device",
        _ => return None,
    })
}

fn said(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value != "auto").then_some(value)
}

pub fn choices(file: &File) -> Result<Applied, String> {
    let mut choices = Choices::default();
    let mut put = |flag: &str, key: &str, value: &str| -> Result<(), String> {
        Choices::parse(flag, Some(value), &mut choices).map_err(|why| format!("{key}: {why}"))
    };
    if let Some(value) = said(&file.video.decoder.default) {
        put("--decoder", "video.decoder.default", value)?;
    }
    if let Some(value) = said(&file.video.decoder.api) {
        put("--decoder-api", "video.decoder.api", value)?;
    }
    if let Some(value) = said(&file.video.deinterlace.method) {
        put("--deinterlace", "video.deinterlace.method", value)?;
    }
    if let Some(value) = said(&file.video.deinterlace.by) {
        put("--deinterlace-by", "video.deinterlace.by", value)?;
    }
    if !file.video.deinterlace.telecine {
        put("--telecine", "video.deinterlace.telecine", "off")?;
    }
    put(
        "--upscale",
        "video.scale.upscale",
        &file.video.scale.upscale,
    )?;
    put(
        "--downscale",
        "video.scale.downscale",
        &file.video.scale.downscale,
    )?;
    if !file.video.scale.antiring {
        put("--antiring", "video.scale.antiring", "off")?;
    }
    if let Some(value) = said(&file.video.render.render) {
        put("--render", "video.render.render", value)?;
    }
    if let Some(value) = said(&file.video.render.backend) {
        put("--backend", "video.render.backend", value)?;
    }
    if let Some(value) = said(&file.video.render.gpu) {
        put("--gpu", "video.render.gpu", value)?;
    }
    if let Some(value) = said(&file.video.render.present) {
        put("--present", "video.render.present", value)?;
    }
    if let Some(value) = said(&file.video.render.hdr) {
        put("--hdr", "video.render.hdr", value)?;
    }
    match file.video.frame.track.as_str() {
        "" | "main" => {}
        "backup" => put("--video", "video.frame.track", "2")?,
        other => return Err(format!("video.frame.track に {other}: main|backup")),
    }
    if file.audio.latency != 0 {
        put(
            "--audio-latency",
            "audio.latency",
            &file.audio.latency.to_string(),
        )?;
    }
    if let Some(value) = said(&file.audio.dual_mono) {
        put("--dual-mono", "audio.dual_mono", value)?;
    }
    if !file.audio.mono_to_stereo {
        put("--mono-to-stereo", "audio.mono_to_stereo", "false")?;
    }
    if let Some(value) = said(&file.audio.downmix) {
        put("--downmix", "audio.downmix", value)?;
    }
    if let Some(value) = said(&file.audio.downmix_22) {
        put("--downmix-22", "audio.downmix_22", value)?;
    }
    choices.options.mix.seven_one = matrix(&file.audio.matrix, "seven_one")?;
    choices.options.mix.downmix = matrix(&file.audio.matrix, "downmix")?;
    choices.options.mix.surround = matrix(&file.audio.matrix, "surround")?;
    let volume = match file.general.on_start_volume.as_str() {
        "" | "last" => None,
        "fixed" => Some(file.general.volume.min(zwcore::sound::volume::MAX)),
        other => return Err(format!("general.on_start_volume に {other}: last|fixed")),
    };
    choices.knobs = knobs(file)?;
    let passthrough = match file.audio.passthrough.as_str() {
        "" | "off" | "false" => Passthrough::Off,
        "on" | "true" => Passthrough::On,
        "auto" => Passthrough::Auto,
        other => return Err(format!("audio.passthrough に {other}: off|on|auto")),
    };
    Ok(Applied {
        choices,
        sound_device: file.audio.device.clone(),
        passthrough,
        volume,
        restore_mute: file.playback.restore_mute,
        plan: plan(file)?,
        sound_plan: sound_plan(file)?,
    })
}

/// Mirakurun のソースごとのデコードと優先度を core に渡す。変わったら `true`。
pub fn mirakurun_asking(file: &File) -> bool {
    zwcore::input::source::mirakurun::set_asking(
        file.sources
            .iter()
            .filter(|source| source.kind == "mirakurun")
            .map(|source| {
                (
                    source.url.as_str(),
                    zwcore::input::source::mirakurun::Asking {
                        decode: source.decode,
                        priority: source.priority,
                    },
                )
            }),
    )
}

/// コーデックごとの並びを読む。先頭に `!` の付いた物は使わない。`zerowave` は最後に使う内蔵のデコーダーで、
/// 並びには入らない。
pub fn plan(file: &File) -> Result<zwcore::decode::Plan, String> {
    let read = |key: &str, written: &[String]| -> Result<Vec<zwcore::decode::Pick>, String> {
        written
            .iter()
            .filter(|spelled| !spelled.trim_start().starts_with('!'))
            .filter(|spelled| spelled.trim() != ZEROWAVE)
            .map(|spelled| {
                zwcore::decode::Pick::parse(spelled).ok_or_else(|| {
                    format!(
                        "video.decoder.{key} に {spelled}: auto|d3d12|d3d11|nvdec|vaapi|v4l2|videotoolbox|mediacodec|amf|qsv|system（:デバイス）"
                    )
                })
            })
            .collect()
    };
    let decoder = &file.video.decoder;
    let zerowave_off = [
        (mediadecoder::es::Codec::Mpeg2, &decoder.mpeg2),
        (mediadecoder::es::Codec::H264, &decoder.h264),
        (mediadecoder::es::Codec::Hevc, &decoder.hevc),
    ]
    .into_iter()
    .filter(|(_, written)| zerowave_off(written))
    .map(|(codec, _)| codec)
    .collect();
    Ok(zwcore::decode::Plan {
        mpeg2: read("mpeg2", &decoder.mpeg2)?,
        h264: read("h264", &decoder.h264)?,
        hevc: read("hevc", &decoder.hevc)?,
        zerowave_off,
    })
}

pub const ZEROWAVE: &str = "zerowave";

pub fn zerowave_off(written: &[String]) -> bool {
    written
        .iter()
        .any(|spelled| spelled.trim().strip_prefix('!').map(str::trim) == Some(ZEROWAVE))
}

pub fn sound_plan(file: &File) -> Result<zwcore::decode::sound::SoundPlan, String> {
    use zwcore::decode::sound::SoundPick;
    let read = |key: &str, written: &[String]| -> Result<Vec<SoundPick>, String> {
        written
            .iter()
            .filter(|spelled| !spelled.trim_start().starts_with('!'))
            .map(|spelled| {
                SoundPick::parse(spelled).ok_or_else(|| {
                    format!("audio.decoder.{key} に {spelled}: system（:名前）|zerowave")
                })
            })
            .collect()
    };
    Ok(zwcore::decode::sound::SoundPlan {
        aac: read("aac", &file.audio.decoder.aac)?,
        mpeg: read("mpeg", &file.audio.decoder.mpeg)?,
    })
}

fn matrix<const I: usize, const N: usize>(
    written: &std::collections::BTreeMap<String, Vec<Vec<f32>>>,
    key: &str,
) -> Result<Option<[[zwcore::sound::mix::Milli; I]; N]>, String> {
    let Some(rows) = written.get(key) else {
        return Ok(None);
    };
    let wrong = || format!("audio.matrix.{key} は {N} 行 {I} 列");
    if rows.len() != N {
        return Err(wrong());
    }
    let mut out = [[0; I]; N];
    for (into, row) in out.iter_mut().zip(rows) {
        if row.len() != I {
            return Err(wrong());
        }
        for (cell, value) in into.iter_mut().zip(row) {
            if !value.is_finite() || value.abs() > 10.0 {
                return Err(format!("audio.matrix.{key} に {value}: -10〜10"));
            }
            *cell = (value * 1000.0).round() as zwcore::sound::mix::Milli;
        }
    }
    Ok(Some(out))
}

pub fn networks(file: &File, profile: &str) -> Vec<zwcore::guide::Network> {
    let mut out = Vec::new();
    for network in &file.networks {
        if !network.profile.is_empty() && network.profile != profile {
            continue;
        }
        let mut ranges = Vec::new();
        for value in &network.nids {
            match crate::config::key::pattern_of(value) {
                Some(crate::config::key::Pattern::One(one)) => ranges.push((one, one)),
                Some(crate::config::key::Pattern::Range(from, to)) => ranges.push((from, to)),
                Some(crate::config::key::Pattern::Any) => ranges.push((0, u16::MAX)),
                None => tracing::warn!(
                    "[[network]] {} の nid を解釈できない: {value}",
                    network.name
                ),
            }
        }
        out.push(zwcore::guide::Network {
            name: network.name.clone(),
            ranges,
        });
    }
    out
}

/// 番組詳細を読むネットワークの範囲。`None` は全部。解釈できない値が混ざっても全部読む。
pub fn extended(file: &File) -> Option<Vec<(u16, u16)>> {
    let mut ranges = Vec::new();
    for value in &file.guide.extended {
        match crate::config::key::pattern_of(value) {
            Some(crate::config::key::Pattern::One(one)) => ranges.push((one, one)),
            Some(crate::config::key::Pattern::Range(from, to)) => ranges.push((from, to)),
            Some(crate::config::key::Pattern::Any) => return None,
            None => {
                tracing::warn!(
                    "guide.extended の {value} を解釈できない（数値・\"a-b\"・\"*\" のいずれか）。\
                     全ネットワークの番組詳細を取得する"
                );
                return None;
            }
        }
    }
    Some(ranges)
}

pub fn prefs(file: &File) -> zwcore::guide::Prefs {
    let mut prefs = zwcore::guide::Prefs::default();
    for (text, name) in &file.channel.name {
        match crate::config::key::key(text) {
            Some(key) => {
                prefs.names.insert(key, name.clone());
            }
            None => tracing::warn!("[channel.name] のキーを解釈できない: {text}"),
        }
    }
    for (text, number) in &file.channel.key {
        match (crate::config::key::key(text), u8::try_from(*number)) {
            (Some(key), Ok(number)) => {
                prefs.keys.insert(key, number);
            }
            _ => tracing::warn!("[channel.key] の行を解釈できない: {text} = {number}"),
        }
    }
    prefs
}

pub fn knobs(file: &File) -> Result<Knobs, String> {
    if file.audio.volume_step == 0 || file.audio.volume_step > zwcore::sound::volume::MAX {
        return Err(format!(
            "audio.volume_step に {}: 1〜{}",
            file.audio.volume_step,
            zwcore::sound::volume::MAX
        ));
    }
    Ok(Knobs {
        volume_step: file.audio.volume_step,
        wheel: WheelDoes::parse("operation.wheel", &file.operation.wheel)?,
        wheel_shift: WheelDoes::parse("operation.wheel_shift", &file.operation.wheel_shift)?,
        wheel_ctrl: WheelDoes::parse("operation.wheel_ctrl", &file.operation.wheel_ctrl)?,
        oneseg: crate::choice::OneSeg::parse(&file.playback.oneseg)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decoder_order_is_read() {
        let mut file = File::default();
        file.video.decoder.mpeg2 = vec![
            String::from("d3d11:AMD Radeon(TM) Graphics"),
            String::from("!system"),
            String::from("auto"),
        ];
        let read = plan(&file).unwrap();
        let spelled: Vec<String> = read
            .mpeg2
            .iter()
            .map(zwcore::decode::Pick::spelling)
            .collect();
        assert_eq!(spelled, ["d3d11:AMD Radeon(TM) Graphics", "auto"]);
        assert!(
            read.h264.is_empty(),
            "書いていないコーデックは空（決まった順）"
        );
        file.video.decoder.hevc = vec![String::from("dxva2:何か")];
        let why = plan(&file).unwrap_err();
        assert!(why.contains("video.decoder.hevc"), "{why}");
    }

    use zwcore::decode::{DeinterlaceBy, Prefer};
    use zwcore::engine::DualMono;
    use zwcore::output::video::Deinterlace;

    #[test]
    fn the_defaults_change_nothing() {
        let applied = choices(&File::default()).unwrap();
        assert_eq!(applied.choices.options.decoder, None);
        assert_eq!(applied.choices.options.deinterlace, None);
        assert_eq!(applied.choices.options.deinterlace_by, DeinterlaceBy::Auto);
        assert_eq!(applied.choices.render, crate::choice::Render::Auto);
        assert_eq!(applied.passthrough, Passthrough::Off);
        assert!(applied.sound_device.is_empty());
        assert_eq!(applied.volume, None);
        assert_eq!(applied.choices.knobs, Knobs::default());
        assert!(!applied.restore_mute);
        assert_eq!(
            applied.choices.options.mix,
            zwcore::sound::mix::Mix::default()
        );
    }

    #[test]
    fn by_default_every_network_reads_its_details() {
        assert_eq!(extended(&File::default()), None);
    }

    #[test]
    fn details_read_the_networks_written_as_nids() {
        let of = |written: Vec<toml::Value>| {
            let mut file = File::default();
            file.guide.extended = written;
            extended(&file)
        };
        assert_eq!(of(vec![toml::Value::Integer(4)]), Some(vec![(4, 4)]));
        assert_eq!(
            of(vec![toml::Value::String(String::from("0x0006-0x0007"))]),
            Some(vec![(6, 7)])
        );
        assert_eq!(of(Vec::new()), Some(Vec::new()));
    }

    #[test]
    fn a_spelling_that_cannot_be_read_falls_back_to_every_network() {
        let mut file = File::default();
        file.guide.extended = vec![
            toml::Value::String(String::from("bs")),
            toml::Value::String(String::from("cs")),
        ];
        assert_eq!(extended(&file), None);
    }

    #[test]
    fn every_flag_the_file_puts_has_a_key() {
        for flag in [
            "--decoder",
            "--deinterlace",
            "--deinterlace-by",
            "--render",
            "--backend",
            "--gpu",
            "--present",
            "--hdr",
            "--video",
            "--audio-latency",
            "--dual-mono",
            "--mono-to-stereo",
            "--downmix",
            "--downmix-22",
        ] {
            assert!(key_of_flag(flag).is_some(), "{flag}");
        }
        assert_eq!(key_of_flag("--fit-window"), None);
    }

    #[test]
    fn the_five_one_matrices_reach_the_mix() {
        let file = File::parse(
            r#"
[audio.matrix]
downmix = [[1.0, 0.0, 0.707, 0.0, 0.707, 0.0], [0.0, 1.0, 0.707, 0.0, 0.0, 0.707]]
"#,
        )
        .unwrap();
        let mix = choices(&file).unwrap().choices.options.mix;
        assert_eq!(
            mix.downmix,
            Some([[1000, 0, 707, 0, 707, 0], [0, 1000, 707, 0, 0, 707]])
        );
        assert_eq!(mix.surround, None);
        let mut file = File::default();
        file.audio
            .matrix
            .insert(String::from("surround"), vec![vec![1.0; 6]; 5]);
        assert!(
            choices(&file)
                .unwrap_err()
                .contains("audio.matrix.surround")
        );
    }

    #[test]
    fn the_seven_one_matrix_reaches_the_mix() {
        use zwcore::sound::mix::TwentyTwo;
        let mut file = File::default();
        file.audio.downmix_22 = String::from("71");
        let mut rows = vec![vec![0.0; 24]; 8];
        rows[4][9] = 1.0;
        file.audio.matrix.insert(String::from("seven_one"), rows);
        let mix = choices(&file).unwrap().choices.options.mix;
        assert_eq!(mix.twenty_two, TwentyTwo::SevenOne);
        let table = mix.seven_one.expect("入る");
        assert_eq!(table[4][9], 1000);
        file.audio
            .matrix
            .insert(String::from("seven_one"), vec![vec![0.0; 6]; 8]);
        assert!(
            choices(&file)
                .unwrap_err()
                .contains("audio.matrix.seven_one")
        );
    }

    #[test]
    fn the_sound_keys_reach_the_mix_and_the_volume() {
        use zwcore::sound::mix::{FiveOne, TwentyTwo};
        let file = File::parse(
            r#"
[general]
on_start_volume = "fixed"
volume = 40
[audio]
mono_to_stereo = false
downmix = "stereo"
downmix_22 = "51"
volume_step = 10
[playback]
restore_mute = true
"#,
        )
        .unwrap();
        let applied = choices(&file).unwrap();
        let mix = applied.choices.options.mix;
        assert!(!mix.mono_to_stereo);
        assert_eq!(mix.five_one, FiveOne::Stereo);
        assert_eq!(mix.twenty_two, TwentyTwo::FiveOne);
        assert_eq!(applied.volume, Some(40));
        assert_eq!(applied.choices.knobs.volume_step, 10);
        assert!(applied.restore_mute);
        let mut file = File::default();
        file.audio.downmix = String::from("mono");
        assert!(choices(&file).unwrap_err().contains("audio.downmix"));
        let mut file = File::default();
        file.audio.volume_step = 0;
        assert!(choices(&file).is_err());
        let mut file = File::default();
        file.operation.wheel_shift = String::from("zoom");
        assert!(
            choices(&file)
                .unwrap_err()
                .contains("operation.wheel_shift")
        );
    }

    #[test]
    fn the_file_says_what_the_flags_say() {
        let file = File::parse(
            r#"
[video.decoder]
default = "software"
[video.deinterlace]
method = "bwdif"
by = "hardware"
[video.render]
render = "gles"
hdr = "on"
[video.frame]
track = "backup"
[audio]
latency = -40
dual_mono = "both"
passthrough = "on"
device = "hdmi"
"#,
        )
        .unwrap();
        let applied = choices(&file).unwrap();
        let options = applied.choices.options;
        assert_eq!(options.decoder, Some(Prefer::Software));
        assert_eq!(options.deinterlace, Some(Deinterlace::Bwdif));
        assert_eq!(options.deinterlace_by, DeinterlaceBy::Hardware);
        assert_eq!(options.video, Some(2));
        assert_eq!(options.audio_latency, -40);
        assert_eq!(options.dual_mono, DualMono::Both);
        assert_eq!(applied.choices.render, crate::choice::Render::Gles);
        assert_eq!(applied.choices.hdr, crate::choice::Hdr::On);
        assert_eq!(applied.passthrough, Passthrough::On);
        assert_eq!(applied.sound_device, "hdmi");
        let mut file = File::default();
        file.audio.passthrough = String::from("auto");
        assert_eq!(choices(&file).unwrap().passthrough, Passthrough::Auto);
    }

    #[test]
    fn an_unknown_word_names_the_key() {
        let mut file = File::default();
        file.video.deinterlace.method = String::from("magic");
        let error = choices(&file).unwrap_err();
        assert!(error.contains("video.deinterlace.method"), "{error}");
        assert!(error.contains("bwdif"), "使える名前を並べる: {error}");
        let mut file = File::default();
        file.video.frame.track = String::from("third");
        assert!(choices(&file).is_err());
    }
}
