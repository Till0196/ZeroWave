// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたサンプルのキュー。再生スレッドが [`Pcm`] で積み、出力（cpal のコールバックや ALSA の書き込みの
//! スレッド）が [`Parts::fill`] で取り出す。取り出した数がプレイヤーの時計になる。

#[cfg(feature = "sound")]
use std::collections::VecDeque;
#[cfg(feature = "sound")]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[cfg(feature = "sound")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "sound")]
use mediadecoder::audio::Sound;
use mediadecoder::audio::Speaker;

#[cfg(feature = "sound")]
#[derive(Clone)]
pub(super) struct Parts {
    shared: Arc<Mutex<Shared>>,
    played: Arc<AtomicU64>,
    latency: Arc<AtomicU64>,
    pub(super) rate: Arc<AtomicU32>,
    pub(super) channels: u16,
    /// サンプルに掛ける倍率（f32 のビット）。出力が読むのでキューを待たずに効く。
    gain: Arc<AtomicU32>,
}

#[cfg(feature = "sound")]
#[derive(Default)]
struct Shared {
    queue: VecDeque<f32>,
    /// Samples per channel the device asked for and the queue could not give (silence went out).
    missed: u64,
    chime: VecDeque<f32>,
    /// 一時停止。無音を出し、キューも数も動かさない。内蔵音だけは重ねる。
    paused: bool,
}

#[cfg(feature = "sound")]
impl Parts {
    pub(super) fn new(rate: u32, channels: usize) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            played: Arc::new(AtomicU64::new(0)),
            latency: Arc::new(AtomicU64::new(0)),
            rate: Arc::new(AtomicU32::new(rate)),
            channels: channels.clamp(1, 8) as u16,
            gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }

    pub(super) fn pcm(&self) -> Pcm {
        Pcm {
            shared: self.shared.clone(),
            played: self.played.clone(),
            latency: self.latency.clone(),
            rate: self.rate.clone(),
            channels: usize::from(self.channels),
            gain: self.gain.clone(),
        }
    }

    /// デバイスが求めた `out`（チャンネルを交互に並べたもの）を埋める。`ahead` はこの塊の最初のサンプルが
    /// 鳴るまでのサンプル数（チャンネルあたり）。
    pub(super) fn fill(&self, out: &mut [f32], ahead: Option<u64>) {
        if let Some(ahead) = ahead {
            self.latency.store(ahead, Ordering::Relaxed);
        }
        let mut shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        let channels = usize::from(self.channels).max(1);
        let mut written = 0;
        let paused = shared.paused;
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        for (at, slot) in out.iter_mut().enumerate() {
            match if paused {
                None
            } else {
                shared.queue.pop_front()
            } {
                Some(sample) => {
                    *slot = sample * gain;
                    written += 1;
                }
                // Silence rather than the last block again; repeating is heard as a click.
                None => *slot = 0.0,
            }
            if !shared.chime.is_empty() {
                let mixed = if at % channels == channels - 1 {
                    shared.chime.pop_front()
                } else {
                    shared.chime.front().copied()
                };
                if let Some(mixed) = mixed {
                    *slot = (*slot + mixed * gain).clamp(-1.0, 1.0);
                }
            }
        }
        // Count what the device consumed, not what it was given: this count is the clock.
        if !paused {
            let asked = out.len() / channels;
            let got = written / channels;
            shared.missed += (asked - got) as u64;
            // Counted under the queue's lock so a flush reads a count that agrees.
            self.played.fetch_add(asked as u64, Ordering::Relaxed);
        }
    }
}

/// Which of the stream's channels goes to each device slot (WAVE order for the count).
pub fn slots(device_channels: usize, speakers: &[Speaker]) -> Vec<Option<usize>> {
    use Speaker::*;
    let layout: &[Speaker] = match device_channels {
        1 => &[FrontCentre],
        2 => &[FrontLeft, FrontRight],
        6 => &[
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            BackLeft,
            BackRight,
        ],
        8 => &[
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            BackLeft,
            BackRight,
            SideLeft,
            SideRight,
        ],
        _ => &[],
    };
    let by_role = !layout.is_empty() && speakers.iter().any(|placed| layout.contains(placed));
    if !by_role {
        return (0..device_channels)
            .map(|at| (at < speakers.len().max(1)).then_some(at))
            .collect();
    }
    layout
        .iter()
        .map(|role| speakers.iter().position(|placed| placed == role))
        .collect()
}

#[cfg(feature = "sound")]
pub struct Pcm {
    shared: Arc<Mutex<Shared>>,
    played: Arc<AtomicU64>,
    latency: Arc<AtomicU64>,
    rate: Arc<AtomicU32>,
    channels: usize,
    gain: Arc<AtomicU32>,
}

#[cfg(feature = "sound")]
impl zwcore::sound::Pcm for Pcm {
    fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    fn played(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }

    /// Throws away what is waiting and says how many the device had taken.
    fn flush(&self) -> u64 {
        let mut shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        shared.queue.clear();
        self.played.load(Ordering::Relaxed)
    }

    fn queued(&self) -> usize {
        let shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        shared.queue.len() / self.channels.max(1)
    }

    fn latency(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }

    fn missed(&self) -> u64 {
        let shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        shared.missed
    }

    fn set_paused(&self, paused: bool) {
        let mut shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        shared.paused = paused;
    }

    fn chime(&self, samples: &[f32]) {
        let mut shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        shared.chime.clear();
        shared.chime.extend(samples.iter().copied());
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() { gain.max(0.0) } else { 1.0 };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Hands over a run of sound, interleaved in the device's WAVE order ([`slots`]).
    fn write(&self, sound: &Sound) -> usize {
        let samples = sound.len();
        let order = slots(self.channels, &sound.speakers);
        let mut shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        for at in 0..samples {
            for from in &order {
                let sample = from
                    .and_then(|channel| sound.channels.get(channel))
                    .and_then(|channel| channel.get(at))
                    .copied()
                    .unwrap_or(0.0);
                shared.queue.push_back(sample.clamp(-1.0, 1.0));
            }
        }
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_one_is_placed_by_role_on_a_six_channel_device() {
        use Speaker::*;
        let speakers = [
            FrontCentre,
            FrontLeft,
            FrontRight,
            BackLeft,
            BackRight,
            LowFrequency,
        ];
        let order = slots(6, &speakers);
        assert_eq!(
            order,
            [Some(1), Some(2), Some(0), Some(5), Some(3), Some(4)]
        );
        assert_eq!(slots(2, &speakers), [Some(1), Some(2)]);
    }

    #[test]
    fn unplaced_pairs_keep_their_order() {
        use Speaker::*;
        assert_eq!(slots(2, &[Unplaced, Unplaced]), [Some(0), Some(1)]);
        assert_eq!(slots(2, &[Unplaced]), [Some(0), None]);
        assert_eq!(
            slots(4, &[FrontLeft, FrontRight]),
            [Some(0), Some(1), None, None]
        );
    }
}
