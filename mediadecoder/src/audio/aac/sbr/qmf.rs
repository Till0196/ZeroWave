// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The two quadrature mirror filter banks of ISO/IEC 14496-3 clause 4.6.18.4: 32 complex bands in,
//! 64 out at twice the rate. Complex so that moving energy between bands does not alias.

use std::sync::OnceLock;

use super::tables::QMF_WINDOW;

/// How many time slots one 1024-sample core frame becomes.
pub const SLOTS: usize = 32;

pub const BANDS: usize = 64;

pub type Complex = [f32; 2];

/// The scale the standard's banks assume (sixteen bit). Folded into the two banks' coefficients
/// because envelope levels are absolute in that domain.
const FULL_SCALE: f32 = 32768.0;

struct Modulation {
    /// `cos` and `sin` of the analysis angle, indexed by band * 64 + tap.
    analysis: Vec<Complex>,
    /// The same for synthesis, by sample * 64 + band, with the standard's 1/64 folded in.
    synthesis: Vec<Complex>,
}

fn modulation() -> &'static Modulation {
    static ONCE: OnceLock<Modulation> = OnceLock::new();
    ONCE.get_or_init(|| {
        use std::f64::consts::PI;
        // The quarter and half phase offsets must be the standard's: a delay test cannot catch
        // them, but a copy between bands lands rotated if they are wrong.
        let mut analysis = Vec::with_capacity(32 * 64);
        for band in 0..32 {
            for tap in 0..64 {
                let angle = PI / 32.0 * (band as f64 + 0.5) * (tap as f64 - 0.25);
                let scale = 2.0 * f64::from(FULL_SCALE);
                analysis.push([(angle.cos() * scale) as f32, (angle.sin() * scale) as f32]);
            }
        }
        let mut synthesis = Vec::with_capacity(128 * 64);
        for at in 0..128 {
            for band in 0..64 {
                let angle = PI / 64.0 * (band as f64 + 0.5) * (at as f64 + 0.5);
                let scale = -1.0 / (64.0 * f64::from(FULL_SCALE));
                synthesis.push([(angle.cos() * scale) as f32, (angle.sin() * scale) as f32]);
            }
        }
        Modulation {
            analysis,
            synthesis,
        }
    })
}

#[derive(Debug)]
pub struct Analysis {
    /// The 320 samples the filter reaches back over, newest first (so new ones go in reversed).
    history: Box<[f32; 320]>,
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}

impl Analysis {
    pub fn new() -> Self {
        Self {
            history: Box::new([0.0; 320]),
        }
    }

    pub fn frame(&mut self, input: &[f32], out: &mut [[Complex; 32]; SLOTS]) {
        let modulation = &modulation().analysis;
        for (slot, bands) in out.iter_mut().enumerate() {
            self.history.copy_within(0..288, 32);
            for (at, sample) in self.history[..32].iter_mut().enumerate() {
                *sample = input.get(slot * 32 + 31 - at).copied().unwrap_or(0.0);
            }
            let mut folded = [0.0f32; 64];
            for (tap, value) in folded.iter_mut().enumerate() {
                let mut sum = 0.0;
                for step in 0..5 {
                    let at = tap + 64 * step;
                    // Every second tap: the analysis bank runs at half the rate.
                    sum += self.history[at] * QMF_WINDOW[2 * at];
                }
                *value = sum;
            }
            for (band, value) in bands.iter_mut().enumerate() {
                let (mut real, mut imaginary) = (0.0f32, 0.0f32);
                let row = &modulation[band * 64..band * 64 + 64];
                for (tap, &[cosine, sine]) in row.iter().enumerate() {
                    real += folded[tap] * cosine;
                    imaginary += folded[tap] * sine;
                }
                *value = [real, imaginary];
            }
        }
    }
}

#[derive(Debug)]
pub struct Synthesis {
    v: Box<[f32; 1280]>,
}

impl Default for Synthesis {
    fn default() -> Self {
        Self::new()
    }
}

impl Synthesis {
    pub fn new() -> Self {
        Self {
            v: Box::new([0.0; 1280]),
        }
    }

    pub fn frame(&mut self, input: &[[Complex; BANDS]; SLOTS], out: &mut [f32]) {
        let modulation = &modulation().synthesis;
        for (slot, bands) in input.iter().enumerate() {
            self.v.copy_within(0..1152, 128);
            for at in 0..128 {
                let row = &modulation[at * 64..at * 64 + 64];
                let mut sum = 0.0f32;
                for (band, &[cosine, sine]) in row.iter().enumerate() {
                    sum += bands[band][0] * cosine - bands[band][1] * sine;
                }
                self.v[at] = sum;
            }
            // The standard's `g`: five pairs of 64 at a stride of 256.
            let mut sums = [0.0f32; 64];
            for step in 0..5 {
                for at in 0..64 {
                    sums[at] += self.v[256 * step + at] * QMF_WINDOW[128 * step + at];
                    sums[at] += self.v[256 * step + 192 + at] * QMF_WINDOW[128 * step + 64 + at];
                }
            }
            out[slot * 64..slot * 64 + 64].copy_from_slice(&sums);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Analysis, BANDS, SLOTS, Synthesis};

    #[test]
    fn the_two_banks_together_only_delay_the_signal() {
        let mut analysis = Analysis::new();
        let mut synthesis = Synthesis::new();
        let tone = |at: usize| (at as f32 * 0.11).sin() * 0.4 + (at as f32 * 0.031).sin() * 0.3;
        let mut history = Vec::new();
        for frame in 0..4 {
            let input: Vec<f32> = (0..1024).map(|at| tone(frame * 1024 + at)).collect();
            let mut bands = [[[0.0f32; 2]; 32]; SLOTS];
            analysis.frame(&input, &mut bands);
            let mut wide = [[[0.0f32; 2]; BANDS]; SLOTS];
            for (slot, values) in bands.iter().enumerate() {
                wide[slot][..32].copy_from_slice(values);
            }
            let mut out = vec![0.0f32; 2048];
            synthesis.frame(&wide, &mut out);
            history.extend(out);
        }
        let delay = 578;
        let mut worst = 0.0f32;
        for (at, &got) in history.iter().enumerate().take(4096).skip(2048) {
            if (at - delay) % 2 == 0 {
                worst = worst.max((got - tone((at - delay) / 2)).abs());
            }
        }
        // Near-perfect reconstruction: the prototype's passband ripple is left over.
        assert!(worst < 2e-3, "reconstruction is off by {worst}");
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut analysis = Analysis::new();
        let mut synthesis = Synthesis::new();
        let mut bands = [[[0.0f32; 2]; 32]; SLOTS];
        analysis.frame(&[0.0; 1024], &mut bands);
        assert!(bands.iter().flatten().flatten().all(|&v| v == 0.0));
        let wide = [[[0.0f32; 2]; BANDS]; SLOTS];
        let mut out = vec![0.0f32; 2048];
        synthesis.frame(&wide, &mut out);
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
