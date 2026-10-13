// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Time-stretching at the same pitch (WSOLA).

use mediadecoder::audio::Sound;

const SEQUENCE_MS: f64 = 40.0;
const OVERLAP_MS: f64 = 8.0;
const SEEK_MS: f64 = 15.0;

#[derive(Debug, Clone)]
pub struct Stretcher {
    rate: u32,
    channels: usize,
    pace: f64,
    sequence: usize,
    overlap: usize,
    seek: usize,
    input: Vec<Vec<f32>>,
    pos: f64,
    tail: Vec<Vec<f32>>,
    /// Input samples dropped from the front, ever; the caller derives output time from it.
    consumed: u64,
}

impl Stretcher {
    pub fn new(rate: u32, channels: usize, pace: f64) -> Self {
        let per_ms = f64::from(rate.max(1)) / 1000.0;
        let overlap = ((OVERLAP_MS * per_ms) as usize).max(8);
        Self {
            rate: rate.max(1),
            channels: channels.max(1),
            pace: crate::engine::clock::sane_pace(pace),
            sequence: ((SEQUENCE_MS * per_ms) as usize).max(overlap * 3),
            overlap,
            seek: ((SEEK_MS * per_ms) as usize).max(1),
            input: vec![Vec::new(); channels.max(1)],
            pos: 0.0,
            tail: Vec::new(),
            consumed: 0,
        }
    }

    pub fn pace(&self) -> f64 {
        self.pace
    }

    pub fn position(&self) -> u64 {
        self.consumed + self.pos as u64
    }

    pub fn push(&mut self, sound: &Sound) -> Sound {
        let mut out = Sound {
            rate: self.rate,
            channels: vec![Vec::new(); self.channels],
            speakers: sound.speakers.clone(),
            mixdown: sound.mixdown,
        };
        if sound.rate != self.rate || sound.channels.len() != self.channels {
            return out;
        }
        for (held, run) in self.input.iter_mut().zip(&sound.channels) {
            held.extend_from_slice(run);
        }
        let need = self.sequence + self.seek;
        while (self.pos as usize) + need <= self.input[0].len() {
            let from = self.pos as usize;
            let offset = self.best_seam(from);
            let start = from + offset;
            let (overlap, sequence) = (self.overlap, self.sequence);
            for ch in 0..self.channels {
                let input = &self.input[ch];
                if let Some(tail) = self.tail.get(ch) {
                    for i in 0..overlap {
                        let t = i as f32 / overlap as f32;
                        out.channels[ch].push(tail[i] * (1.0 - t) + input[start + i] * t);
                    }
                } else {
                    out.channels[ch].extend_from_slice(&input[start..start + overlap]);
                }
                out.channels[ch]
                    .extend_from_slice(&input[start + overlap..start + sequence - overlap]);
            }
            if self.tail.len() != self.channels {
                self.tail = vec![vec![0.0; overlap]; self.channels];
            }
            for ch in 0..self.channels {
                let input = &self.input[ch];
                self.tail[ch].copy_from_slice(&input[start + sequence - overlap..start + sequence]);
            }
            self.pos += (sequence - overlap) as f64 * self.pace;
        }
        let keep_from = (self.pos as usize).min(self.input[0].len());
        if keep_from > 0 {
            for channel in &mut self.input {
                channel.drain(..keep_from);
            }
            self.pos -= keep_from as f64;
            self.consumed += keep_from as u64;
        }
        out
    }

    /// Normalised cross-correlation against the tail. Nought before there is a tail.
    fn best_seam(&self, from: usize) -> usize {
        if self.tail.len() != self.channels {
            return 0;
        }
        let mono = |ch_sum: &dyn Fn(usize, usize) -> f32, i: usize| -> f32 {
            (0..self.channels).map(|ch| ch_sum(ch, i)).sum::<f32>()
        };
        let tail_at = |ch: usize, i: usize| self.tail[ch][i];
        let input_at = |ch: usize, i: usize| self.input[ch][i];
        let mut best = 0usize;
        let mut best_score = f32::MIN;
        for offset in 0..self.seek {
            let mut cross = 0.0f32;
            let mut energy = 0.0f32;
            for i in 0..self.overlap {
                let a = mono(&tail_at, i);
                let b = mono(&input_at, from + offset + i);
                cross += a * b;
                energy += b * b;
            }
            let score = if energy > 0.0 {
                cross / energy.sqrt()
            } else {
                0.0
            };
            if score > best_score {
                best_score = score;
                best = offset;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::audio::Speaker;

    fn tone(rate: u32, seconds: f32, hz: f32) -> Sound {
        let n = (rate as f32 * seconds) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin())
            .collect();
        Sound {
            rate,
            channels: vec![samples.clone(), samples],
            speakers: vec![Speaker::FrontLeft, Speaker::FrontRight],
            mixdown: Default::default(),
        }
    }

    #[test]
    fn twice_the_pace_is_half_the_length_at_the_same_pitch() {
        let rate = 48_000;
        let input = tone(rate, 2.0, 440.0);
        let mut stretcher = Stretcher::new(rate, 2, 2.0);
        let out = stretcher.push(&input);
        let expected = input.len() / 2;
        assert!(
            out.len().abs_diff(expected) < stretcher.sequence * 2,
            "{} for {expected}",
            out.len()
        );
        let crossings = |samples: &[f32]| {
            samples
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        let per_second_in = crossings(&input.channels[0]) as f32 / 2.0;
        let per_second_out = crossings(&out.channels[0]) as f32 / (out.len() as f32 / rate as f32);
        assert!(
            (per_second_out - per_second_in).abs() / per_second_in < 0.03,
            "{per_second_out} vs {per_second_in}"
        );
        assert!(out.channels[0].iter().all(|s| s.abs() <= 1.01));
    }

    #[test]
    fn slower_is_longer_and_one_is_the_same() {
        let rate = 48_000;
        let input = tone(rate, 1.0, 300.0);
        let mut slow = Stretcher::new(rate, 2, 0.5);
        let out = slow.push(&input);
        assert!(
            out.len().abs_diff(input.len() * 2) < slow.sequence * 3,
            "{}",
            out.len()
        );
        let mut same = Stretcher::new(rate, 2, 1.0);
        let out = same.push(&input);
        assert!(
            out.len().abs_diff(input.len()) < same.sequence * 2,
            "{}",
            out.len()
        );
        assert!(same.position() as usize <= input.len());
    }
}
