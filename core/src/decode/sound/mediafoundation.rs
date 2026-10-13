// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Windows の音声デコーダー（Media Foundation の MFT）。AAC は生のフレームを渡し、`MF_MT_USER_DATA` に
//! `HEAACWAVEINFO` の後ろ半分と `AudioSpecificConfig` を入れる。

use std::collections::VecDeque;

use mediadecoder::audio::{self, Format, Sound, Speaker};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVDecAudioDualMonoReproMode, IMFActivate, IMFMediaType, IMFSample, IMFTransform,
    MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_E_TRANSFORM_TYPE_NOT_SET, MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_CHANNEL_MASK,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_MT_USER_DATA, MFAudioFormat_AAC, MFAudioFormat_Float, MFAudioFormat_MPEG, MFAudioFormat_PCM,
    MFCreateMediaType, MFMediaType_Audio, MFT_CATEGORY_AUDIO_DECODER, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_COMMAND_FLUSH, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, eAVDecAudioDualMonoReproMode_STEREO,
};
use windows::core::GUID;

use super::{
    SoundCodec, System, arrange, audio_specific_config, channels_of, paired, paired_config,
};
use crate::decode::mediafoundation::{friendly, mft, start};

fn failed(what: &str, error: windows::core::Error) -> String {
    format!("{what}: {error}")
}

fn subtype(codec: SoundCodec) -> GUID {
    match codec {
        SoundCodec::Aac => MFAudioFormat_AAC,
        SoundCodec::Mpeg => MFAudioFormat_MPEG,
    }
}

fn candidates(codec: SoundCodec) -> Result<Vec<IMFActivate>, String> {
    Ok(mft::enumerate(
        MFT_CATEGORY_AUDIO_DECODER,
        MFMediaType_Audio,
        subtype(codec),
    )?)
}

/// このコーデックに使える MFT の名前。作って入力の型を渡すところまで試す。
pub(super) fn usable(codec: SoundCodec) -> Vec<String> {
    if start().is_err() {
        return Vec::new();
    }
    let Ok(found) = candidates(codec) else {
        return Vec::new();
    };
    let format = Format {
        object: audio::Object::Lc,
        rate: 48_000,
        configuration: 2,
    };
    let header = [0xFF, 0xFD, 0xC4, 0x00];
    let mut names: Vec<String> = Vec::new();
    for activate in &found {
        let name = friendly(activate);
        if names.contains(&name) {
            continue;
        }
        let usable = mft::activate(activate)
            .is_ok_and(|transform| set_input(&transform, codec, format, &header).is_ok());
        mft::shutdown(activate);
        if usable {
            names.push(name);
        }
    }
    names
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Aac {
        config: [u8; 2],
        /// 二か国語（SCE 二つ）。
        paired: bool,
    },
    Mpeg {
        rate: u32,
        channels: u32,
        layer: u8,
        mpeg1: bool,
    },
}

impl Input {
    fn of(codec: SoundCodec, format: Format, data: &[u8]) -> Option<Self> {
        match codec {
            SoundCodec::Aac => {
                let paired = paired(format, data);
                if !paired {
                    channels_of(format.configuration)?;
                }
                Some(Self::Aac {
                    config: audio_specific_config(format)?,
                    paired,
                })
            }
            SoundCodec::Mpeg => {
                let header = audio::mpeg::Header::read(data)?;
                Some(Self::Mpeg {
                    rate: header.rate,
                    channels: if header.mode == 3 { 1 } else { 2 },
                    layer: header.layer,
                    mpeg1: header.mpeg1,
                })
            }
        }
    }
}

fn set_input(
    transform: &IMFTransform,
    codec: SoundCodec,
    format: Format,
    data: &[u8],
) -> Result<Input, String> {
    let input = Input::of(codec, format, data).ok_or("Media Foundation に渡せない形式")?;
    // SAFETY: 作った型はローカル変数で、渡したら解放する。
    unsafe {
        let kind: IMFMediaType =
            MFCreateMediaType().map_err(|error| failed("MFCreateMediaType", error))?;
        let set = |result: windows::core::Result<()>, what: &str| {
            result.map_err(|error| failed(what, error))
        };
        set(
            kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio),
            "MF_MT_MAJOR_TYPE",
        )?;
        set(
            kind.SetGUID(&MF_MT_SUBTYPE, &subtype(codec)),
            "MF_MT_SUBTYPE",
        )?;
        match input {
            Input::Aac { config, paired } => {
                let channels = channels_of(format.configuration).unwrap_or(2);
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, format.rate),
                    "周波数",
                )?;
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels),
                    "チャンネル数",
                )?;
                set(
                    kind.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0),
                    "MF_MT_AAC_PAYLOAD_TYPE",
                )?;
                // `HEAACWAVEINFO` の `WAVEFORMATEX` より後ろ：生、プロファイル無指定、ASC 付き。
                let mut user = vec![0u8, 0, 0xFE, 0, 0, 0, 0, 0, 0, 0, 0, 0];
                if paired {
                    user.extend_from_slice(&paired_config(config));
                } else {
                    user.extend_from_slice(&config);
                }
                set(kind.SetBlob(&MF_MT_USER_DATA, &user), "MF_MT_USER_DATA")?;
            }
            Input::Mpeg {
                rate,
                channels,
                layer,
                mpeg1,
            } => {
                let header = audio::mpeg::Header::read(data).ok_or("MPEG Audio のヘッダー")?;
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate),
                    "周波数",
                )?;
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels),
                    "チャンネル数",
                )?;
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, header.bitrate / 8),
                    "ビットレート",
                )?;
                set(
                    kind.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1),
                    "ブロックアラインメント",
                )?;
                let head_layer: u16 = if layer == 1 { 1 } else { 2 };
                let head_mode: u16 = match header.mode {
                    0 => 1,
                    1 => 2,
                    2 => 4,
                    _ => 8,
                };
                let flags: u16 = if mpeg1 { 0x10 } else { 0 };
                let mut user = Vec::with_capacity(22);
                user.extend_from_slice(&head_layer.to_le_bytes());
                user.extend_from_slice(&header.bitrate.to_le_bytes());
                user.extend_from_slice(&head_mode.to_le_bytes());
                user.extend_from_slice(&u16::from(header.mode_extension).to_le_bytes());
                user.extend_from_slice(&1u16.to_le_bytes());
                user.extend_from_slice(&flags.to_le_bytes());
                user.extend_from_slice(&[0; 8]);
                set(kind.SetBlob(&MF_MT_USER_DATA, &user), "MF_MT_USER_DATA")?;
            }
        }
        transform
            .SetInputType(0, &kind, 0)
            .map_err(|error| failed("SetInputType", error))?;
    }
    Ok(input)
}

/// 二か国語（デュアルチャンネル）の左右を、そのまま二つのチャンネルで出させる。
/// Microsoft MPEG Audio Decoder MFT は、指定しなければ左（主）を両方に出す。ICodecAPI は持たず、
/// 属性ストアの `CODECAPI_AVDecAudioDualMonoReproMode` を読む。
fn keep_both(transform: &IMFTransform) {
    // SAFETY: 生きた MFT の属性ストアに数を一つ置く。
    let set = unsafe {
        transform.GetAttributes().and_then(|attributes| {
            attributes.SetUINT32(
                &CODECAPI_AVDecAudioDualMonoReproMode,
                eAVDecAudioDualMonoReproMode_STEREO.0 as u32,
            )
        })
    };
    if let Err(error) = set {
        tracing::debug!("二か国語の出し方を設定できない（{error}）");
    }
}

#[derive(Debug, Clone)]
struct Shape {
    rate: u32,
    channels: usize,
    float: bool,
    speakers: Vec<Speaker>,
}

fn set_output(transform: &IMFTransform) -> Result<Shape, String> {
    let mut fallback = None;
    for kind in mft::output_types(transform) {
        let subtype = mft::guid(&kind, &MF_MT_SUBTYPE).unwrap_or_default();
        if subtype == MFAudioFormat_Float {
            return choose(transform, &kind, true);
        }
        if subtype == MFAudioFormat_PCM
            && mft::uint32(&kind, &MF_MT_AUDIO_BITS_PER_SAMPLE).unwrap_or(0) == 16
            && fallback.is_none()
        {
            fallback = Some(kind);
        }
    }
    match fallback {
        Some(kind) => choose(transform, &kind, false),
        None => Err(String::from("浮動小数点も 16 ビットの PCM も出せません")),
    }
}

fn choose(transform: &IMFTransform, kind: &IMFMediaType, float: bool) -> Result<Shape, String> {
    // SAFETY: 生きた型を渡し、読む。
    unsafe {
        transform
            .SetOutputType(0, kind, 0)
            .map_err(|error| failed("SetOutputType", error))?;
        let rate = kind
            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .map_err(|error| failed("出力の周波数", error))?;
        let channels = kind
            .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
            .map_err(|error| failed("出力のチャンネル数", error))? as usize;
        let mask = kind.GetUINT32(&MF_MT_AUDIO_CHANNEL_MASK).unwrap_or(0);
        Ok(Shape {
            rate,
            channels,
            float,
            speakers: speakers_of(mask, channels),
        })
    }
}

/// WAVE のチャンネルマスクをビットの低い順に配置へ。マスクが無ければ数から。
fn speakers_of(mask: u32, channels: usize) -> Vec<Speaker> {
    use Speaker::*;
    const BITS: [Speaker; 18] = [
        FrontLeft,
        FrontRight,
        FrontCentre,
        LowFrequency,
        BackLeft,
        BackRight,
        FrontLeftCentre,
        FrontRightCentre,
        BackCentre,
        SideLeft,
        SideRight,
        TopCentre,
        TopFrontLeft,
        TopFrontCentre,
        TopFrontRight,
        TopBackLeft,
        TopBackCentre,
        TopBackRight,
    ];
    let mut out: Vec<Speaker> = BITS
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, speaker)| *speaker)
        .take(channels)
        .collect();
    if out.len() != channels {
        out = match channels {
            1 => vec![FrontCentre],
            2 => vec![FrontLeft, FrontRight],
            _ => vec![Unplaced; channels],
        };
    }
    out
}

pub(super) struct MfSound {
    transform: IMFTransform,
    activate: IMFActivate,
    name: String,
    codec: SoundCodec,
    input: Option<Input>,
    shape: Option<Shape>,
    /// ZeroWave がこの形式で出すチャンネル順。出力をこれに並べ替える。
    wanted: Vec<Speaker>,
    ready: VecDeque<Sound>,
    time: i64,
}

// SAFETY: 一つのスレッドだけが持つ。MFT は自由スレッド（MTA）で作る。
unsafe impl Send for MfSound {}

impl MfSound {
    pub(super) fn open(codec: SoundCodec, wanted: Option<&str>) -> Result<Self, String> {
        start().map_err(|error| error.to_string())?;
        let found = candidates(codec)?;
        for activate in found {
            let name = friendly(&activate);
            if wanted.is_some_and(|wanted| wanted != name) {
                continue;
            }
            let transform = match mft::activate(&activate) {
                Ok(transform) => transform,
                Err(_) => continue,
            };
            return Ok(Self {
                transform,
                activate,
                name,
                codec,
                input: None,
                shape: None,
                wanted: Vec::new(),
                ready: VecDeque::new(),
                time: 0,
            });
        }
        Err(match wanted {
            Some(wanted) => format!("{wanted} がありません"),
            None => format!("{} の MFT がありません", codec.name()),
        })
    }

    fn prepare(&mut self, format: Format, data: &[u8]) -> Result<(), String> {
        let wanted =
            Input::of(self.codec, format, data).ok_or("Media Foundation に渡せない形式")?;
        if self.input == Some(wanted) {
            return Ok(());
        }
        if self.input.is_some() {
            // 内部に残っているデータを捨ててから型を設定し直す。
            mft::message(&self.transform, MFT_MESSAGE_COMMAND_FLUSH);
        }
        self.input = Some(set_input(&self.transform, self.codec, format, data)?);
        if self.codec == SoundCodec::Mpeg {
            keep_both(&self.transform);
        }
        self.shape = Some(set_output(&self.transform)?);
        self.wanted = match self.codec {
            SoundCodec::Aac => audio::config::Layout::of_configuration(format.configuration)
                .map(|layout| layout.speakers())
                .unwrap_or_default(),
            SoundCodec::Mpeg => match wanted {
                Input::Mpeg { channels: 1, .. } => vec![Speaker::FrontCentre],
                _ => vec![Speaker::FrontLeft, Speaker::FrontRight],
            },
        };
        mft::begin(&self.transform);
        Ok(())
    }

    fn sample_of(&mut self, data: &[u8]) -> Result<IMFSample, String> {
        let length =
            u32::try_from(data.len()).map_err(|_| String::from("フレームが大きすぎます"))?;
        let time = self.time;
        self.time += 1;
        Ok(mft::filled(length.max(1), data, time, false)?)
    }

    fn pull(&mut self) -> Result<(), String> {
        loop {
            let info = mft::output_info(&self.transform)?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0;
            let given = if provides {
                None
            } else {
                Some(mft::empty(info.cbSize.max(4096))?)
            };
            let (result, _, sample) = mft::output(&self.transform, given);
            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        let sound = self.read(&sample)?;
                        if !sound.is_empty() {
                            self.ready.push_back(sound);
                        }
                    }
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                // HE-AAC の SBR を見つけて周波数が倍になった、など。選び直す。
                Err(error)
                    if error.code() == MF_E_TRANSFORM_STREAM_CHANGE
                        || error.code() == MF_E_TRANSFORM_TYPE_NOT_SET =>
                {
                    self.shape = Some(set_output(&self.transform)?);
                }
                Err(error) => return Err(failed("ProcessOutput", error)),
            }
        }
    }

    fn read(&self, sample: &IMFSample) -> Result<Sound, String> {
        let shape = self.shape.as_ref().ok_or("出力の形式が決まっていません")?;
        let buffer = mft::contiguous(sample)?;
        let locked = mft::Locked::plain(&buffer)?;
        let bytes = locked
            .bytes(locked.length())
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        drop(locked);
        let channels = shape.channels.max(1);
        let width = if shape.float { 4 } else { 2 };
        let frames = bytes.len() / (width * channels);
        let mut out = vec![Vec::with_capacity(frames); channels];
        for frame in bytes.chunks_exact(width * channels) {
            for (channel, value) in frame.chunks_exact(width).enumerate() {
                let sample = if shape.float {
                    f32::from_le_bytes([value[0], value[1], value[2], value[3]])
                } else {
                    f32::from(i16::from_le_bytes([value[0], value[1]])) / 32_768.0
                };
                out[channel].push(sample);
            }
        }
        let mut sound = Sound {
            rate: shape.rate,
            channels: out,
            speakers: shape.speakers.clone(),
            ..Sound::default()
        };
        arrange(&mut sound, &self.wanted);
        Ok(sound)
    }
}

impl System for MfSound {
    fn name(&self) -> &str {
        &self.name
    }

    fn accepts(&self, format: Format, data: &[u8]) -> bool {
        match self.codec {
            // チャンネル構成 0（PCE で配置を指定する 22.2ch など）と、テーブルに無い周波数は任せない。
            SoundCodec::Aac => {
                (channels_of(format.configuration).is_some() || paired(format, data))
                    && audio_specific_config(format).is_some()
                    && matches!(
                        format.object,
                        audio::Object::Lc | audio::Object::Main | audio::Object::Sbr
                    )
            }
            SoundCodec::Mpeg => audio::mpeg::Header::read(data).is_some(),
        }
    }

    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), String> {
        self.prepare(format, data)?;
        let sample = self.sample_of(data)?;
        match mft::input(&self.transform, &sample) {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                self.pull()?;
                mft::input(&self.transform, &sample)
                    .map_err(|error| failed("ProcessInput", error))?;
            }
            Err(error) => return Err(failed("ProcessInput", error)),
        }
        self.pull()
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        if self.input.is_none() {
            return;
        }
        mft::message(&self.transform, MFT_MESSAGE_COMMAND_DRAIN);
        let _ = self.pull();
    }

    fn reset(&mut self) {
        self.ready.clear();
        if self.input.is_some() {
            mft::message(&self.transform, MFT_MESSAGE_COMMAND_FLUSH);
        }
    }
}

impl Drop for MfSound {
    fn drop(&mut self) {
        mft::shutdown(&self.activate);
    }
}
