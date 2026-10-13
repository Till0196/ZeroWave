// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! macOS の音声デコーダー（AudioToolbox の `AudioConverter`）。
//! 24 kHz 以下の AAC（ワンセグ）は暗黙の SBR なので、HE-AAC と明示した設定にして渡す。

use std::collections::VecDeque;
use std::ffi::c_void;

use mediadecoder::audio::{self, Format, Sound, Speaker};

use super::{SoundCodec, System, audio_specific_config, channels_of, paired, paired_config};

type OsStatus = i32;
type AudioConverterRef = *mut c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct AudioStreamPacketDescription {
    start_offset: i64,
    variable_frames_in_packet: u32,
    data_byte_size: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

type InputProc = unsafe extern "C" fn(
    converter: AudioConverterRef,
    packets: *mut u32,
    data: *mut AudioBufferList,
    description: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> OsStatus;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioConverterNew(
        source: *const AudioStreamBasicDescription,
        destination: *const AudioStreamBasicDescription,
        converter: *mut AudioConverterRef,
    ) -> OsStatus;
    fn AudioConverterSetProperty(
        converter: AudioConverterRef,
        property: u32,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
    fn AudioConverterFillComplexBuffer(
        converter: AudioConverterRef,
        input: InputProc,
        user: *mut c_void,
        packets: *mut u32,
        output: *mut AudioBufferList,
        description: *mut AudioStreamPacketDescription,
    ) -> OsStatus;
    fn AudioConverterReset(converter: AudioConverterRef) -> OsStatus;
    fn AudioConverterDispose(converter: AudioConverterRef) -> OsStatus;
}

const fn four(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const FORMAT_AAC: u32 = four(b"aac ");
const FORMAT_AAC_HE: u32 = four(b"aach");
const FORMAT_MP2: u32 = four(b".mp2");
const FORMAT_MP1: u32 = four(b".mp1");
const FORMAT_PCM: u32 = four(b"lpcm");
const FLAG_FLOAT: u32 = 1;
const FLAG_PACKED: u32 = 8;
const MAGIC_COOKIE: u32 = four(b"dmgc");
/// 入力を渡し終えたことを示す、こちらで決めた値（0 でなければ何でもよい）。
const NO_MORE: OsStatus = 1;
const OUT_FRAMES: u32 = 4096;

/// 作ったコンバーター。ドロップ時に一度だけ解放する。
struct Converter(AudioConverterRef);

// SAFETY: AudioConverter はスレッドに縛られず、ハンドルは一つのスレッドだけが持つ。
unsafe impl Send for Converter {}

impl Converter {
    fn raw(&self) -> AudioConverterRef {
        self.0
    }

    fn set_property(&self, property: u32, data: &[u8]) -> OsStatus {
        // SAFETY: 生きたコンバーターに、呼び出しの間生きたバイト列を渡す。
        unsafe {
            AudioConverterSetProperty(self.0, property, data.len() as u32, data.as_ptr().cast())
        }
    }

    /// 内部に残っているデータを捨てる。
    fn reset(&self) {
        // SAFETY: 生きたコンバーター。
        unsafe { AudioConverterReset(self.0) };
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: 作ったコンバーターを一度だけ解放する。
        unsafe { AudioConverterDispose(self.0) };
    }
}

struct Feed {
    data: *const u8,
    length: u32,
    channels: u32,
    given: bool,
    description: AudioStreamPacketDescription,
}

/// `AudioConverterFillComplexBuffer` の入力コールバック。1 フレームを一度だけ渡す。
/// # Safety
///
/// `user` は [`AtSound::convert`] が渡した生きた [`Feed`] のアドレス。
unsafe extern "C" fn feed(
    _converter: AudioConverterRef,
    packets: *mut u32,
    data: *mut AudioBufferList,
    description: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> OsStatus {
    // SAFETY: 上の前提のとおり。
    let feed = unsafe { &mut *user.cast::<Feed>() };
    if feed.given {
        // SAFETY: コンバーターが渡した生きた出力先。
        unsafe { *packets = 0 };
        return NO_MORE;
    }
    feed.given = true;
    feed.description = AudioStreamPacketDescription {
        start_offset: 0,
        variable_frames_in_packet: 0,
        data_byte_size: feed.length,
    };
    // SAFETY: 生きた出力先に書く。フレームは呼び出し側が呼び出しの間持っている。
    unsafe {
        *packets = 1;
        (*data).number_buffers = 1;
        (*data).buffers[0] = AudioBuffer {
            number_channels: feed.channels,
            data_byte_size: feed.length,
            data: feed.data.cast_mut().cast(),
        };
        if !description.is_null() {
            *description = &raw mut feed.description;
        }
    }
    0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Aac {
        config: [u8; 2],
        rate: u32,
        channels: u32,
        /// SBR で周波数が倍になる（HE-AAC）。
        sbr: bool,
        /// 二か国語（SCE 二つ）。
        paired: bool,
    },
    Mpeg {
        rate: u32,
        channels: u32,
        layer: u8,
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
                    sbr: sbr(format) && !paired,
                    paired,
                })
            }
            SoundCodec::Mpeg => {
                let header = audio::mpeg::Header::read(data)?;
                Some(Self::Mpeg {
                    rate: header.rate,
                    channels: if header.mode == 3 { 1 } else { 2 },
                    layer: header.layer,
                })
            }
        }
    }

    /// 出てくる周波数。
    fn rate(self) -> u32 {
        match self {
            Self::Aac {
                rate, sbr: true, ..
            } => rate * 2,
            Self::Aac { rate, .. } | Self::Mpeg { rate, .. } => rate,
        }
    }

    fn channels(self) -> u32 {
        match self {
            Self::Aac { channels, .. } | Self::Mpeg { channels, .. } => channels,
        }
    }
}

fn converter(input: Input) -> Result<Converter, String> {
    let (format_id, frames) = match input {
        Input::Aac { sbr: true, .. } => (FORMAT_AAC_HE, 2048),
        Input::Aac { .. } => (FORMAT_AAC, 1024),
        Input::Mpeg { layer: 1, .. } => (FORMAT_MP1, 384),
        Input::Mpeg { .. } => (FORMAT_MP2, 1152),
    };
    let channels = input.channels();
    let source = AudioStreamBasicDescription {
        sample_rate: f64::from(input.rate()),
        format_id,
        frames_per_packet: frames,
        channels_per_frame: channels,
        ..AudioStreamBasicDescription::default()
    };
    let destination = AudioStreamBasicDescription {
        sample_rate: f64::from(input.rate()),
        format_id: FORMAT_PCM,
        format_flags: FLAG_FLOAT | FLAG_PACKED,
        bytes_per_packet: 4 * channels,
        frames_per_packet: 1,
        bytes_per_frame: 4 * channels,
        channels_per_frame: channels,
        bits_per_channel: 32,
        reserved: 0,
    };
    let mut made: AudioConverterRef = std::ptr::null_mut();
    // SAFETY: 二つの記述はローカル変数で、出力は生きたローカル変数。
    let status = unsafe { AudioConverterNew(&source, &destination, &mut made) };
    if status != 0 || made.is_null() {
        return Err(format!("AudioConverterNew: {status}"));
    }
    let made = Converter(made);
    if let Input::Aac {
        config,
        sbr,
        paired,
        ..
    } = input
    {
        let config = if paired {
            paired_config(config)
        } else if sbr {
            explicit_sbr(config).to_vec()
        } else {
            config.to_vec()
        };
        // 生の AudioSpecificConfig で拒否されたら `esds` の形で渡し直す（それしか受け付けない実装がある）。
        let tried = [config.clone(), esds(&config)];
        let mut last = 0;
        let mut taken = false;
        for cookie in &tried {
            last = made.set_property(MAGIC_COOKIE, cookie);
            if last == 0 {
                taken = true;
                break;
            }
        }
        if !taken {
            return Err(format!("magic cookie: {last}"));
        }
    }
    Ok(made)
}

/// SBR を使う AAC か。24 kHz 以下の AAC（ワンセグ）は暗黙の SBR で、HE-AAC として扱う。
fn sbr(format: Format) -> bool {
    format.object == audio::Object::Sbr || format.rate <= 24_000
}

/// LC の設定を、HE-AAC と明示した設定（ISO/IEC 14496-3 1.6.2.1 の階層的な書き方）にする。
/// 物の種類 5、周波数、チャンネル構成、倍の周波数、物の種類 2、`GASpecificConfig` の 3 ビット。
fn explicit_sbr(config: [u8; 2]) -> [u8; 4] {
    let lc = u16::from_be_bytes(config);
    let index = u32::from((lc >> 7) & 15);
    let configuration = u32::from((lc >> 3) & 15);
    // 周波数の表は倍になるごとに三つ前へ進む（24 kHz の 6 → 48 kHz の 3）。
    let doubled = index.saturating_sub(3);
    let bits = (5 << 27) | (index << 23) | (configuration << 19) | (doubled << 15) | (2 << 10);
    bits.to_be_bytes()
}

/// `esds` の中身（ISO/IEC 14496-1 の ES_Descriptor）で包んだ設定。
fn esds(config: &[u8]) -> Vec<u8> {
    let specific = [&[0x05, config.len() as u8][..], config].concat();
    let decoder = [
        &[0x04, (13 + specific.len()) as u8, 0x40, 0x15][..],
        &[0; 11],
        &specific,
    ]
    .concat();
    let sl = [0x06, 0x01, 0x02];
    [
        &[0x03, (3 + decoder.len() + sl.len()) as u8, 0, 0, 0][..],
        &decoder,
        &sl,
    ]
    .concat()
}

pub(super) fn usable(codec: SoundCodec) -> Vec<String> {
    let input = match codec {
        SoundCodec::Aac => Input::Aac {
            config: [0x11, 0x90],
            rate: 48_000,
            channels: 2,
            sbr: false,
            paired: false,
        },
        SoundCodec::Mpeg => Input::Mpeg {
            rate: 48_000,
            channels: 2,
            layer: 2,
        },
    };
    match converter(input) {
        Ok(made) => {
            drop(made);
            vec![String::from(NAME)]
        }
        Err(why) => {
            if super::SAY.on() {
                tracing::warn!("AudioToolbox {}: {why}", codec.name());
            }
            Vec::new()
        }
    }
}

const NAME: &str = "AudioToolbox";

pub(super) struct AtSound {
    codec: SoundCodec,
    converter: Option<Converter>,
    input: Option<Input>,
    wanted: Vec<Speaker>,
    ready: VecDeque<Sound>,
}

impl AtSound {
    pub(super) fn open(codec: SoundCodec, _wanted: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            codec,
            converter: None,
            input: None,
            wanted: Vec::new(),
            ready: VecDeque::new(),
        })
    }

    fn prepare(&mut self, format: Format, data: &[u8]) -> Result<AudioConverterRef, String> {
        let wanted = Input::of(self.codec, format, data).ok_or("AudioToolbox に渡せない形式")?;
        if self.input == Some(wanted)
            && let Some(made) = &self.converter
        {
            return Ok(made.raw());
        }
        drop(self.converter.take());
        let made = converter(wanted)?;
        let raw = made.raw();
        self.converter = Some(made);
        self.input = Some(wanted);
        self.wanted = match (self.codec, wanted) {
            (SoundCodec::Aac, _) => audio::config::Layout::of_configuration(format.configuration)
                .map(|layout| layout.speakers())
                .unwrap_or_default(),
            (SoundCodec::Mpeg, Input::Mpeg { channels: 1, .. }) => vec![Speaker::FrontCentre],
            (SoundCodec::Mpeg, _) => vec![Speaker::FrontLeft, Speaker::FrontRight],
        };
        Ok(raw)
    }

    fn convert(&mut self, made: AudioConverterRef, data: &[u8]) -> Result<(), String> {
        let input = self.input.ok_or("入力の形式が決まっていません")?;
        let channels = input.channels() as usize;
        let mut feed_data = Feed {
            data: data.as_ptr(),
            length: data.len() as u32,
            channels: input.channels(),
            given: false,
            description: AudioStreamPacketDescription {
                start_offset: 0,
                variable_frames_in_packet: 0,
                data_byte_size: 0,
            },
        };
        let mut out = vec![0f32; OUT_FRAMES as usize * channels];
        let mut list = AudioBufferList {
            number_buffers: 1,
            buffers: [AudioBuffer {
                number_channels: input.channels(),
                data_byte_size: (out.len() * 4) as u32,
                data: out.as_mut_ptr().cast(),
            }],
        };
        let mut frames = OUT_FRAMES;
        // SAFETY: コンバーターとローカル変数は呼び出しの間生きている。
        let status = unsafe {
            AudioConverterFillComplexBuffer(
                made,
                feed,
                (&raw mut feed_data).cast(),
                &mut frames,
                &mut list,
                std::ptr::null_mut(),
            )
        };
        if status != 0 && status != NO_MORE {
            return Err(format!("AudioConverterFillComplexBuffer: {status}"));
        }
        let frames = (frames as usize).min(OUT_FRAMES as usize);
        if frames == 0 {
            return Ok(());
        }
        let mut split = vec![Vec::with_capacity(frames); channels];
        for frame in out[..frames * channels].chunks_exact(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                split[channel].push(*sample);
            }
        }
        let speakers = if self.wanted.len() == channels {
            self.wanted.clone()
        } else {
            vec![Speaker::Unplaced; channels]
        };
        self.ready.push_back(Sound {
            rate: input.rate(),
            channels: split,
            speakers,
            ..Sound::default()
        });
        Ok(())
    }
}

impl System for AtSound {
    fn name(&self) -> &str {
        NAME
    }

    fn accepts(&self, format: Format, data: &[u8]) -> bool {
        match self.codec {
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
        let made = self.prepare(format, data)?;
        self.convert(made, data)
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {}

    fn reset(&mut self) {
        self.ready.clear();
        if let Some(made) = &self.converter {
            made.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_at_24k_is_spelled_as_he_aac_at_48k() {
        assert_eq!(explicit_sbr([0x13, 0x10]), [0x2B, 0x11, 0x88, 0x00]);
    }
}
