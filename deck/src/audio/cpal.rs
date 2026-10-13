// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! cpal でデバイスの出力を開き、デコードしたサンプルを再生する（Linux 以外）。ストリームは `!Send` なので
//! イベントループに残し（[`Keep`]）、キュー（[`Pcm`]）だけがスレッドを渡る。

#[cfg(feature = "sound")]
use std::sync::atomic::Ordering;

#[cfg(feature = "sound")]
use zwcore::sound::Error;

#[cfg(feature = "sound")]
use super::queue::Parts;
#[cfg(feature = "sound")]
pub use super::queue::Pcm;

pub struct Keep {
    #[cfg(feature = "sound")]
    _stream: Option<cpal::Stream>,
    /// 開いたデバイス。既定が変わったら開き直す（WASAPI は開いたときのエンドポイントに付いたまま）。
    #[cfg(feature = "sound")]
    opened: Option<Opened>,
    #[cfg(feature = "sound")]
    checked: Option<std::time::Instant>,
}

#[cfg(feature = "sound")]
struct Opened {
    name: Option<String>,
    pinned: bool,
    parts: Parts,
}

impl Keep {
    pub fn none() -> Self {
        Self {
            #[cfg(feature = "sound")]
            _stream: None,
            #[cfg(feature = "sound")]
            opened: None,
            #[cfg(feature = "sound")]
            checked: None,
        }
    }

    #[cfg(feature = "sound")]
    pub fn follow_default(&mut self) {
        use cpal::traits::{DeviceTrait, HostTrait};
        let Some(opened) = &mut self.opened else {
            return;
        };
        // 名前を指して開いた物と Windows（WASAPI が既定に付いて行く）は開き直さない。
        if opened.pinned || cfg!(windows) {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .checked
            .is_some_and(|at| now.duration_since(at) < std::time::Duration::from_secs(2))
        {
            return;
        }
        self.checked = Some(now);
        let Some(device) = cpal::default_host().default_output_device() else {
            return;
        };
        let name = name_of(&device);
        if name == opened.name {
            return;
        }
        let rate = opened.parts.rate.load(Ordering::Relaxed);
        let stream = build_stream(&device, rate, &opened.parts).or_else(|error| {
            let other = device
                .default_output_config()
                .map(|config| config.sample_rate())
                .map_err(|_| error.clone())?;
            if other == rate {
                return Err(error);
            }
            let stream = build_stream(&device, other, &opened.parts)?;
            opened.parts.rate.store(other, Ordering::Relaxed);
            Ok(stream)
        });
        match stream {
            Ok(stream) => {
                tracing::info!(
                    "音声の出力先: {}（{} Hz）",
                    name.as_deref().unwrap_or("?"),
                    opened.parts.rate.load(Ordering::Relaxed)
                );
                self._stream = None;
                self._stream = Some(stream);
                opened.name = name;
            }
            Err(error) => {
                tracing::warn!("音声の出力先を切り替えられない: {error}");
                opened.name = name;
            }
        }
    }

    #[cfg(not(feature = "sound"))]
    pub fn follow_default(&mut self) {}
}

impl std::fmt::Debug for Keep {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Keep")
    }
}

#[cfg(feature = "sound")]
fn build_stream(device: &cpal::Device, rate: u32, parts: &Parts) -> Result<cpal::Stream, Error> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let config = cpal::StreamConfig {
        channels: parts.channels,
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let parts = parts.clone();
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let stamps = info.timestamp();
                let ahead = stamps
                    .playback
                    .checked_duration_since(stamps.callback)
                    .map(|ahead| (ahead.as_secs_f64() * f64::from(rate)) as u64);
                parts.fill(out, ahead);
            },
            |error| tracing::warn!("音声の出力: {error}"),
            None,
        )
        .map_err(|error| Error::Unavailable(error.to_string()))?;
    stream
        .play()
        .map_err(|error| Error::Unavailable(error.to_string()))?;
    Ok(stream)
}

#[cfg(feature = "sound")]
impl Pcm {
    /// デバイスを開く。`channels` が無ければデバイスのチャンネル数で（8 まで）、`named` が無ければ既定で。
    pub fn open(
        rate: u32,
        channels: Option<usize>,
        named: Option<&str>,
    ) -> Result<(Self, Keep), Error> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let wanted = named.filter(|name| !name.is_empty()).and_then(|name| {
            let found = host
                .output_devices()
                .ok()?
                .find(|device| name_of(device).is_some_and(|shown| shown == name));
            if found.is_none() {
                tracing::warn!("音声の出力先「{name}」が見つからない。既定のデバイスを使う");
            }
            found
        });
        let pinned = wanted.is_some();
        let device = match wanted {
            Some(device) => device,
            None => host
                .default_output_device()
                .ok_or_else(|| Error::Unavailable(String::from("出力デバイスが見つかりません")))?,
        };
        let channels = channels.unwrap_or_else(|| {
            device
                .default_output_config()
                .map(|config| usize::from(config.channels()))
                .unwrap_or(2)
        });
        let parts = Parts::new(rate, channels);
        // 要求したレートで開けなければデバイスのレートで（Windows の共有モードは既定のレートだけ）。
        let stream = match build_stream(&device, rate, &parts) {
            Ok(stream) => stream,
            Err(error) => {
                let other = device
                    .default_output_config()
                    .map(|config| config.sample_rate())
                    .ok()
                    .filter(|&r| r != rate)
                    .ok_or_else(|| error.clone())?;
                tracing::warn!("音声デバイスを {rate} Hz で開けない（{error}）。{other} Hz で開く");
                let stream = build_stream(&device, other, &parts)?;
                parts.rate.store(other, Ordering::Relaxed);
                stream
            }
        };
        let name = name_of(&device);
        Ok((
            parts.pcm(),
            Keep {
                _stream: Some(stream),
                opened: Some(Opened {
                    name,
                    pinned,
                    parts,
                }),
                checked: None,
            },
        ))
    }
}

#[cfg(feature = "sound")]
fn name_of(device: &cpal::Device) -> Option<String> {
    use cpal::traits::DeviceTrait;
    device
        .description()
        .ok()
        .map(|description| description.name().to_owned())
}

#[cfg(feature = "sound")]
pub fn output_names() -> Vec<String> {
    use cpal::traits::HostTrait;
    cpal::default_host()
        .output_devices()
        .map(|devices| devices.filter_map(|device| name_of(&device)).collect())
        .unwrap_or_default()
}

#[cfg(not(feature = "sound"))]
pub fn output_names() -> Vec<String> {
    Vec::new()
}

/// 音声の出力方式の名前（cpal の host）。
pub fn host_name() -> String {
    #[cfg(feature = "sound")]
    return cpal::default_host().id().name().to_owned();
    #[cfg(not(feature = "sound"))]
    String::from("なし")
}
