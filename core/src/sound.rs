// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコード後の音声の経路。[`Device`] の二つの出力は同じ単位（IEC 60958 のフレーム、
//! 毎秒 48000）で数えるので、クロックは一つで済む。

pub mod chime;
pub mod iec61937;
pub mod mix;
pub mod resample;
pub mod sounding;
pub mod stretch;
pub mod volume;

use mediadecoder::audio::Sound;

pub use crate::output::sound::Error;

/// デコードしたサンプルを再生するもの。数は一チャンネル分で、無音を出した分も数える
/// （数えなければクロックが止まる）。
pub trait Pcm: Send {
    fn rate(&self) -> u32;
    fn played(&self) -> u64;
    fn missed(&self) -> u64;
    fn latency(&self) -> u64;
    fn queued(&self) -> usize;
    fn flush(&self) -> u64;
    fn set_paused(&self, paused: bool);
    fn chime(&self, samples: &[f32]);
    fn write(&self, sound: &Sound) -> usize;
    fn channels(&self) -> usize;
    fn set_gain(&self, gain: f32);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Passthrough {
    #[default]
    Off,
    On,
    Auto,
}

impl Passthrough {
    /// ADTS に載せられる配置で 2 チャンネルより多いものだけ。22.2 は ADTS で表せない。
    pub fn passes(self, format: &mediadecoder::audio::Format) -> bool {
        match self {
            Self::Off => false,
            Self::On => true,
            Self::Auto => (3..=7).contains(&format.configuration),
        }
    }
}

pub enum Device {
    Pcm(Box<dyn Pcm>),
    Burst(Burst),
    /// Both, and the sound decides. The sound thread starts the clock over when it changes sides.
    Either {
        pcm: Box<dyn Pcm>,
        burst: Burst,
        on_burst: bool,
    },
}

impl std::fmt::Debug for Device {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.name())
    }
}

enum Side<'a> {
    Pcm(&'a dyn Pcm),
    Burst(&'a Burst),
}

impl Device {
    pub fn passthrough(rate: u32, on: Option<&str>) -> Result<Self, Error> {
        let link = crate::output::sound::open_on(rate, on)?;
        Ok(Self::Burst(Burst {
            link,
            packed: Vec::new(),
        }))
    }

    /// 最初はデコードする側（選局直後はたいていステレオ）。
    pub fn either(pcm: Box<dyn Pcm>, link: Self) -> Self {
        match link {
            Self::Burst(burst) => Self::Either {
                pcm,
                burst,
                on_burst: false,
            },
            other => other,
        }
    }

    fn side(&self) -> Side<'_> {
        match self {
            Self::Pcm(device) => Side::Pcm(device.as_ref()),
            Self::Burst(burst) => Side::Burst(burst),
            Self::Either {
                pcm,
                burst,
                on_burst,
            } => {
                if *on_burst {
                    Side::Burst(burst)
                } else {
                    Side::Pcm(pcm.as_ref())
                }
            }
        }
    }

    pub fn passthrough_mode(&self) -> Passthrough {
        match self {
            Self::Pcm(_) => Passthrough::Off,
            Self::Burst(_) => Passthrough::On,
            Self::Either { .. } => Passthrough::Auto,
        }
    }

    pub fn on_burst(&self) -> bool {
        matches!(self.side(), Side::Burst(_))
    }

    /// [`Device::Either`] だけ。切り替わったら真。
    pub fn use_burst(&mut self, burst: bool) -> bool {
        match self {
            Self::Either { on_burst, .. } if *on_burst != burst => {
                *on_burst = burst;
                true
            }
            _ => false,
        }
    }

    pub fn name(&self) -> &'static str {
        match self.side() {
            Side::Pcm(_) => "PCM",
            Side::Burst(burst) => burst.link.name(),
        }
    }

    pub fn rate(&self) -> u32 {
        match self.side() {
            Side::Pcm(device) => device.rate(),
            Side::Burst(burst) => burst.link.rate(),
        }
    }

    pub fn played(&self) -> u64 {
        match self.side() {
            Side::Pcm(device) => device.played(),
            Side::Burst(burst) => burst.link.played(),
        }
    }

    pub fn missed(&self) -> u64 {
        match self.side() {
            Side::Pcm(device) => device.missed(),
            Side::Burst(_) => 0,
        }
    }

    /// Without subtracting it every picture shows early. Nought for a passthrough link.
    pub fn latency(&self) -> u64 {
        match self.side() {
            Side::Pcm(device) => device.latency(),
            Side::Burst(_) => 0,
        }
    }

    /// The clock cannot move until sound reaches the device.
    pub fn queued(&self) -> usize {
        match self.side() {
            Side::Pcm(device) => device.queued(),
            Side::Burst(burst) => burst.link.queued(),
        }
    }

    pub fn flush(&self) -> u64 {
        match self.side() {
            Side::Pcm(device) => device.flush(),
            Side::Burst(burst) => burst.link.played(),
        }
    }

    /// AV アンプへパススルーする経路では止められない（無音を作る場所が無い）。
    pub fn set_paused(&self, paused: bool) {
        match self.side() {
            Side::Pcm(device) => device.set_paused(paused),
            Side::Burst(_) => {}
        }
    }

    pub fn channels(&self) -> usize {
        match self.side() {
            Side::Pcm(device) => device.channels(),
            Side::Burst(_) => usize::MAX,
        }
    }

    pub fn set_gain(&self, gain: f32) {
        match self {
            Self::Pcm(device) | Self::Either { pcm: device, .. } => device.set_gain(gain),
            Self::Burst(_) => {}
        }
    }

    pub fn chime(&self, samples: &[f32]) {
        match self.side() {
            Side::Pcm(device) => device.chime(samples),
            Side::Burst(_) => {}
        }
    }

    pub fn write(&self, sound: &Sound) -> usize {
        match self.side() {
            Side::Pcm(device) => device.write(sound),
            Side::Burst(_) => 0,
        }
    }

    pub fn write_burst(&mut self, frame: &[u8]) -> Result<usize, Error> {
        let rate = self.rate();
        let burst = match self {
            Self::Pcm(_) => return Ok(0),
            Self::Burst(burst) => burst,
            Self::Either {
                burst, on_burst, ..
            } => {
                if !*on_burst {
                    return Ok(0);
                }
                burst
            }
        };
        // A burst runs at the link's rate, so a frame at another rate (ワンセグ's 24 kHz core)
        // would play at the wrong speed.
        let coded = mediadecoder::audio::adts::Header::read(frame)
            .ok_or(Error::Unsupported("ADTS ヘッダーが読めない"))?;
        if coded.format.rate != rate {
            return Err(Error::Unavailable(format!(
                "{} Hz の音声は {rate} Hz のリンクに載せられない",
                coded.format.rate
            )));
        }
        burst.packed.clear();
        let samples = iec61937::pack(frame, &mut burst.packed).map_err(|error| match error {
            mediadecoder::Error::Unsupported(what) | mediadecoder::Error::Damaged(what) => {
                Error::Unsupported(what)
            }
        })?;
        burst.link.write(&burst.packed)?;
        Ok(samples)
    }
}

pub struct Burst {
    link: Box<dyn crate::output::sound::Link>,
    packed: Vec<u8>,
}

impl std::fmt::Debug for Burst {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.link.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_passes_only_what_adts_can_carry_and_is_more_than_a_pair() {
        let format = |configuration: u8| mediadecoder::audio::Format {
            configuration,
            ..Default::default()
        };
        assert!(!Passthrough::Auto.passes(&format(1)));
        assert!(!Passthrough::Auto.passes(&format(2)));
        assert!(Passthrough::Auto.passes(&format(6)));
        assert!(Passthrough::Auto.passes(&format(7)));
        assert!(!Passthrough::Auto.passes(&format(13)));
        assert!(!Passthrough::Auto.passes(&format(0)));
        assert!(Passthrough::On.passes(&format(2)));
        assert!(!Passthrough::Off.passes(&format(6)));
    }
}
