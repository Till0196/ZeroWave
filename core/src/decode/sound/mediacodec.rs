// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Android の音声デコーダー（MediaCodec）。AAC は `audio/mp4a-latm` に生のフレーム、MPEG-1 Audio は
//! `audio/mpeg-L2` にフレームのまま。

use std::collections::VecDeque;
use std::time::Duration;

use mediadecoder::audio::{self, Format, Sound, Speaker};
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;

use super::{
    SoundCodec, System, arrange, audio_specific_config, channels_of, paired, paired_config,
};

/// 入力バッファが空くのを待つ長さ（音声の 1 フレーム 21 ms より短く）。
const WAIT: Duration = Duration::from_millis(10);

const TRIES: usize = 20;

fn mime(codec: SoundCodec) -> &'static str {
    match codec {
        SoundCodec::Aac => "audio/mp4a-latm",
        SoundCodec::Mpeg => "audio/mpeg-L2",
    }
}

/// 一覧と記録に出す名前（`getName` は API 28 からなので問い合わせない）。
const NAME: &str = "MediaCodec";

pub(super) fn usable(codec: SoundCodec) -> Vec<String> {
    MediaCodec::from_decoder_type(mime(codec))
        .map(|_| vec![String::from(NAME)])
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Aac {
        config: [u8; 2],
        rate: u32,
        channels: u32,
        /// 二か国語（SCE 二つ）。
        paired: bool,
    },
    Mpeg {
        rate: u32,
        channels: u32,
    },
}

impl Input {
    fn of(codec: SoundCodec, format: Format, data: &[u8]) -> Option<Self> {
        match codec {
            SoundCodec::Aac => {
                let paired = paired(format, data);
                Some(Self::Aac {
                    config: audio_specific_config(format)?,
                    rate: format.rate,
                    channels: if paired {
                        2
                    } else {
                        channels_of(format.configuration)?
                    },
                    paired,
                })
            }
            SoundCodec::Mpeg => {
                let header = audio::mpeg::Header::read(data)?;
                Some(Self::Mpeg {
                    rate: header.rate,
                    channels: if header.mode == 3 { 1 } else { 2 },
                })
            }
        }
    }

    fn format(self, codec: SoundCodec) -> MediaFormat {
        let mut format = MediaFormat::new();
        format.set_str("mime", mime(codec));
        match self {
            Self::Aac {
                config,
                rate,
                channels,
                paired,
            } => {
                format.set_i32("sample-rate", rate as i32);
                format.set_i32("channel-count", channels as i32);
                format.set_i32("is-adts", 0);
                if paired {
                    format.set_buffer("csd-0", &paired_config(config));
                } else {
                    format.set_buffer("csd-0", &config);
                }
            }
            Self::Mpeg { rate, channels } => {
                format.set_i32("sample-rate", rate as i32);
                format.set_i32("channel-count", channels as i32);
            }
        }
        format
    }
}

#[derive(Debug, Clone, Copy)]
struct Shape {
    rate: u32,
    channels: usize,
    float: bool,
}

/// Android のチャンネル順（`CHANNEL_OUT_*` の既定、WAVE と同じ順）。
fn speakers_of(channels: usize) -> Vec<Speaker> {
    use Speaker::*;
    match channels {
        1 => vec![FrontCentre],
        2 => vec![FrontLeft, FrontRight],
        3 => vec![FrontLeft, FrontRight, FrontCentre],
        4 => vec![FrontLeft, FrontRight, BackLeft, BackRight],
        5 => vec![FrontLeft, FrontRight, FrontCentre, BackLeft, BackRight],
        6 => vec![
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            BackLeft,
            BackRight,
        ],
        8 => vec![
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            BackLeft,
            BackRight,
            SideLeft,
            SideRight,
        ],
        _ => vec![Unplaced; channels],
    }
}

fn shape_of(codec: &MediaCodec) -> Shape {
    let format = codec.output_format();
    Shape {
        rate: format.i32("sample-rate").unwrap_or(48_000).max(1) as u32,
        channels: format.i32("channel-count").unwrap_or(2).max(1) as usize,
        float: format.i32("pcm-encoding") == Some(4),
    }
}

pub(super) struct McSound {
    codec: MediaCodec,
    name: String,
    kind: SoundCodec,
    input: Option<Input>,
    shape: Option<Shape>,
    wanted: Vec<Speaker>,
    ready: VecDeque<Sound>,
    time: u64,
}

// SAFETY: 一つのスレッドだけが持つ。AMediaCodec はスレッドに縛られない。
unsafe impl Send for McSound {}

impl McSound {
    pub(super) fn open(kind: SoundCodec, wanted: Option<&str>) -> Result<Self, String> {
        let codec = match wanted.filter(|name| *name != NAME) {
            Some(name) => MediaCodec::from_codec_name(name),
            None => MediaCodec::from_decoder_type(mime(kind)),
        }
        .ok_or_else(|| format!("{} の MediaCodec がありません", mime(kind)))?;
        let name = wanted.unwrap_or(NAME).to_owned();
        Ok(Self {
            codec,
            name,
            kind,
            input: None,
            shape: None,
            wanted: Vec::new(),
            ready: VecDeque::new(),
            time: 0,
        })
    }

    fn prepare(&mut self, format: Format, data: &[u8]) -> Result<(), String> {
        let wanted = Input::of(self.kind, format, data).ok_or("MediaCodec に渡せない形式")?;
        if self.input == Some(wanted) {
            return Ok(());
        }
        if self.input.is_some() {
            let _ = self.codec.stop();
        }
        self.codec
            .configure(
                &wanted.format(self.kind),
                None,
                MediaCodecDirection::Decoder,
            )
            .map_err(|error| format!("configure: {error}"))?;
        self.codec
            .start()
            .map_err(|error| format!("start: {error}"))?;
        self.input = Some(wanted);
        self.shape = None;
        self.wanted = match (self.kind, wanted) {
            (SoundCodec::Aac, _) => audio::config::Layout::of_configuration(format.configuration)
                .map(|layout| layout.speakers())
                .unwrap_or_default(),
            (SoundCodec::Mpeg, Input::Mpeg { channels: 1, .. }) => vec![Speaker::FrontCentre],
            (SoundCodec::Mpeg, _) => vec![Speaker::FrontLeft, Speaker::FrontRight],
        };
        Ok(())
    }

    fn pull(&mut self) -> Result<(), String> {
        loop {
            match self.codec.dequeue_output_buffer(Duration::ZERO) {
                Ok(DequeuedOutputBufferInfoResult::Buffer(out)) => {
                    if self.shape.is_none() {
                        self.shape = Some(shape_of(&self.codec));
                    }
                    let sound = self.read(out.buffer());
                    let _ = self.codec.release_output_buffer(out, false);
                    if let Some(sound) = sound.filter(|sound| !sound.is_empty()) {
                        self.ready.push_back(sound);
                    }
                }
                Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                    self.shape = Some(shape_of(&self.codec));
                }
                Ok(DequeuedOutputBufferInfoResult::OutputBuffersChanged) => {}
                Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => return Ok(()),
                Err(error) => return Err(format!("dequeueOutputBuffer: {error}")),
            }
        }
    }

    fn read(&self, bytes: &[u8]) -> Option<Sound> {
        let shape = self.shape?;
        let width = if shape.float { 4 } else { 2 };
        let channels = shape.channels;
        let frames = bytes.len() / (width * channels);
        let mut out = vec![Vec::with_capacity(frames); channels];
        for frame in bytes.chunks_exact(width * channels) {
            for (channel, value) in frame.chunks_exact(width).enumerate() {
                out[channel].push(if shape.float {
                    f32::from_le_bytes([value[0], value[1], value[2], value[3]])
                } else {
                    f32::from(i16::from_le_bytes([value[0], value[1]])) / 32_768.0
                });
            }
        }
        let mut sound = Sound {
            rate: shape.rate,
            channels: out,
            speakers: speakers_of(channels),
            ..Sound::default()
        };
        arrange(&mut sound, &self.wanted);
        Some(sound)
    }

    fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        for _ in 0..TRIES {
            match self.codec.dequeue_input_buffer(WAIT) {
                Ok(DequeuedInputBufferResult::Buffer(mut input)) => {
                    // 入力バッファが NULL の端末があり、ndk が panic するので受け止める。
                    let copied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let room = input.buffer_mut();
                        if room.len() < data.len() {
                            return false;
                        }
                        // SAFETY: `room` は `data` 以上の長さで、重ならない。
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                data.as_ptr(),
                                room.as_mut_ptr().cast::<u8>(),
                                data.len(),
                            );
                        }
                        true
                    }));
                    let length = if matches!(copied, Ok(true)) {
                        data.len()
                    } else {
                        0
                    };
                    let time = self.time;
                    self.time += 1;
                    self.codec
                        .queue_input_buffer(input, 0, length, time, 0)
                        .map_err(|error| format!("queueInputBuffer: {error}"))?;
                    return match copied {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(String::from("フレームが入力バッファより大きい")),
                        Err(_) => Err(String::from("入力バッファがありません")),
                    };
                }
                Ok(DequeuedInputBufferResult::TryAgainLater) => self.pull()?,
                Err(error) => return Err(format!("dequeueInputBuffer: {error}")),
            }
        }
        Err(String::from("入力バッファが空きません"))
    }
}

impl System for McSound {
    fn name(&self) -> &str {
        &self.name
    }

    fn accepts(&self, format: Format, data: &[u8]) -> bool {
        match self.kind {
            SoundCodec::Aac => {
                (channels_of(format.configuration).is_some() || paired(format, data))
                    && audio_specific_config(format).is_some()
                    && matches!(
                        format.object,
                        audio::Object::Lc | audio::Object::Main | audio::Object::Sbr
                    )
            }
            // AOSP には Layer II のデコーダーが無く、機種ごとのデコーダーが二か国語（デュアルチャンネル）を
            // どう出すかが決まらないので任せない。
            SoundCodec::Mpeg => {
                audio::mpeg::Header::read(data).is_some_and(|header| header.mode != 2)
            }
        }
    }

    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), String> {
        self.prepare(format, data)?;
        self.feed(data)?;
        self.pull()
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        let _ = self.pull();
    }

    fn reset(&mut self) {
        self.ready.clear();
        if self.input.is_some() {
            let _ = self.codec.flush();
        }
    }
}

impl Drop for McSound {
    fn drop(&mut self) {
        if self.input.is_some() {
            let _ = self.codec.stop();
        }
    }
}
