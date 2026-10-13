// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! An asynchronous sample rate converter (windowed sinc). Also stretches by a few tens of ppm
//! to take out the drift between the broadcaster's clock and the device's.

const HALF: usize = 16;

const STEPS: usize = 256;

const BETA: f64 = 9.0;

/// One channel. All channels must share one ratio, or a stereo pair drifts apart in phase.
#[derive(Debug, Clone)]
pub struct Resampler {
    taps: Vec<f32>,
    history: Vec<f32>,
    at: usize,
    fraction: f64,
    base: f64,
    drift: f64,
    ratio: f64,
}

impl Resampler {
    pub fn new() -> Self {
        Self {
            taps: taps(1.0),
            history: vec![0.0; 2 * HALF],
            at: HALF,
            fraction: 0.0,
            base: 1.0,
            drift: 1.0,
            ratio: 1.0,
        }
    }

    pub fn for_rates(from: u32, to: u32) -> Self {
        let mut resampler = Self::new();
        resampler.set_rates(from, to);
        resampler
    }

    pub fn set_rates(&mut self, from: u32, to: u32) {
        let from = f64::from(from.max(1));
        let to = f64::from(to.max(1));
        self.base = from / to;
        self.taps = taps((to / from).min(1.0));
        self.reset();
        self.retune();
    }

    fn retune(&mut self) {
        self.ratio = self.base * self.drift;
    }

    /// Clamped so a control-loop mistake cannot detune the sound.
    pub fn set_ratio(&mut self, ratio: f64) {
        self.drift = ratio.clamp(0.9, 1.1);
        self.retune();
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.history.resize(2 * HALF, 0.0);
        self.at = HALF;
        self.fraction = 0.0;
    }

    pub fn pending(&self) -> usize {
        let reach = self.history.len().saturating_sub(HALF);
        if self.at >= reach {
            return 0;
        }
        (((reach - self.at) as f64 - self.fraction) / self.ratio).ceil() as usize
    }

    /// The last `HALF` input samples wait for more, since the filter reaches forwards.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.history.extend_from_slice(input);
        let reach = self.history.len().saturating_sub(HALF);
        while self.at < reach {
            out.push(self.sample());
            self.step();
        }
        let spent = self.at.saturating_sub(HALF);
        if spent > 0 {
            self.history.drain(..spent);
            self.at -= spent;
        }
    }

    fn sample(&self) -> f32 {
        let position = self.fraction * STEPS as f64;
        let entry = position as usize;
        let blend = (position - entry as f64) as f32;
        let width = 2 * HALF;
        let low = &self.taps[entry * width..entry * width + width];
        let high = &self.taps[(entry + 1) * width..(entry + 1) * width + width];
        let begin = self.at + 1 - HALF;
        let mut sum = 0.0f32;
        for tap in 0..width {
            let coefficient = low[tap] + (high[tap] - low[tap]) * blend;
            sum += self.history[begin + tap] * coefficient;
        }
        sum
    }

    fn step(&mut self) {
        self.fraction += self.ratio;
        let whole = self.fraction.floor();
        self.at += whole as usize;
        self.fraction -= whole;
    }
}

impl Default for Resampler {
    fn default() -> Self {
        Self::new()
    }
}

fn taps(cutoff: f64) -> Vec<f32> {
    let width = 2 * HALF;
    let mut out = Vec::with_capacity((STEPS + 1) * width);
    let scale = bessel(BETA);
    for step in 0..=STEPS {
        let offset = step as f64 / STEPS as f64;
        for tap in 0..width {
            let distance = tap as f64 - (HALF as f64 - 1.0) - offset;
            let x = distance / HALF as f64;
            let taper = if x.abs() >= 1.0 {
                0.0
            } else {
                bessel(BETA * (1.0 - x * x).sqrt()) / scale
            };
            out.push((cutoff * sinc(cutoff * distance) * taper) as f32);
        }
    }
    // Normalise so a constant comes through as itself (else a whine).
    for entry in out.chunks_mut(width) {
        let total: f32 = entry.iter().sum();
        if total.abs() > 1e-6 {
            for tap in entry {
                *tap /= total;
            }
        }
    }
    out
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let x = std::f64::consts::PI * x;
        x.sin() / x
    }
}

fn bessel(x: f64) -> f64 {
    let mut term = 1.0;
    let mut sum = 1.0;
    for step in 1..40 {
        term *= (x / 2.0) / f64::from(step);
        sum += term * term;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::Resampler;

    fn tone(signal: &[f32], frequency: f64) -> (f64, f64) {
        let count = signal.len() as f64;
        let mut cosine = 0.0;
        let mut sine = 0.0;
        for (at, &sample) in signal.iter().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * frequency * at as f64;
            cosine += f64::from(sample) * angle.cos();
            sine += f64::from(sample) * angle.sin();
        }
        cosine *= 2.0 / count;
        sine *= 2.0 / count;
        let mut rest = 0.0;
        for (at, &sample) in signal.iter().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * frequency * at as f64;
            let fitted = cosine * angle.cos() + sine * angle.sin();
            let error = f64::from(sample) - fitted;
            rest += error * error;
        }
        (
            (cosine * cosine + sine * sine).sqrt(),
            (rest / count).sqrt(),
        )
    }

    fn sine(count: usize, frequency: f64) -> Vec<f32> {
        (0..count)
            .map(|at| (2.0 * std::f64::consts::PI * frequency * at as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn a_tone_comes_through_at_the_ratio_it_was_stretched_by() {
        for ratio in [1.0, 1.000_05, 0.999_9, 1.01] {
            let mut resampler = Resampler::new();
            resampler.set_ratio(ratio);
            let input = sine(20000, 0.05);
            let mut out = Vec::new();
            resampler.push(&input, &mut out);
            let wanted = input.len() as f64 / ratio;
            assert!(
                (out.len() as f64 - wanted).abs() < 40.0,
                "{ratio}: {} against {wanted}",
                out.len()
            );
            // A whole number of cycles, or the analysis window leaks the tone.
            let frequency = 0.05 * ratio;
            let available = out.len() - 400;
            let cycles = (available as f64 * frequency).floor();
            let length = (cycles / frequency).round() as usize;
            let (amplitude, rest) = tone(&out[200..200 + length], frequency);
            assert!((amplitude - 1.0).abs() < 0.01, "{ratio}: {amplitude}");
            assert!(rest < 0.001, "{ratio}: {rest}");
        }
    }

    #[test]
    fn a_ratio_that_moves_leaves_no_step_in_the_output() {
        let mut resampler = Resampler::new();
        let mut out = Vec::new();
        let mut ratio = 0.999_5;
        for block in 0..200 {
            ratio += 0.000_005;
            resampler.set_ratio(ratio);
            let input: Vec<f32> = (0..1024)
                .map(|at| {
                    let n = (block * 1024 + at) as f64;
                    (2.0 * std::f64::consts::PI * 0.01 * n).sin() as f32
                })
                .collect();
            resampler.push(&input, &mut out);
        }
        let worst = out[256..]
            .windows(3)
            .map(|three| (three[0] - 2.0 * three[1] + three[2]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn feeding_it_in_pieces_gives_the_same_answer_as_all_at_once() {
        let input = sine(8192, 0.037);
        let mut whole = Resampler::new();
        whole.set_ratio(1.000_02);
        let mut want = Vec::new();
        whole.push(&input, &mut want);

        let mut piecemeal = Resampler::new();
        piecemeal.set_ratio(1.000_02);
        let mut got = Vec::new();
        for piece in input.chunks(37) {
            piecemeal.push(piece, &mut got);
        }
        assert_eq!(got.len(), want.len());
        let worst = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{worst}");
    }

    #[test]
    fn a_stream_at_half_the_device_rate_comes_out_at_the_same_pitch() {
        let mut resampler = Resampler::for_rates(24_000, 48_000);
        let input = sine(4_000, 1_000.0 / 24_000.0);
        let mut out = Vec::new();
        resampler.push(&input, &mut out);
        let expected = 2 * input.len();
        assert!(
            out.len().abs_diff(expected) < 64,
            "{} ではなく {}",
            expected,
            out.len()
        );
        let settled = &out[256..out.len() - 256];
        let (amplitude, noise) = tone(settled, 1_000.0 / 48_000.0);
        assert!((amplitude - 1.0).abs() < 0.02, "{amplitude}");
        assert!(noise < 0.02, "{noise}");
    }

    #[test]
    fn going_down_in_rate_does_not_fold_the_top_back_in() {
        let mut resampler = Resampler::for_rates(48_000, 24_000);
        let input = sine(8_000, 15_000.0 / 48_000.0);
        let mut out = Vec::new();
        resampler.push(&input, &mut out);
        let settled = &out[256..out.len() - 256];
        let (folded, _) = tone(settled, 9_000.0 / 24_000.0);
        assert!(folded < 0.05, "折り返しが {folded}");
    }

    #[test]
    fn the_drift_correction_multiplies_the_rate_conversion() {
        let mut resampler = Resampler::for_rates(24_000, 48_000);
        assert!((resampler.ratio() - 0.5).abs() < 1e-9);
        resampler.set_ratio(1.001);
        assert!(
            (resampler.ratio() - 0.5005).abs() < 1e-9,
            "{}",
            resampler.ratio()
        );
        resampler.set_ratio(10.0);
        assert!(
            (resampler.ratio() - 0.55).abs() < 1e-9,
            "{}",
            resampler.ratio()
        );
    }

    #[test]
    fn silence_stays_silent_and_a_constant_stays_constant() {
        let mut resampler = Resampler::new();
        resampler.set_ratio(1.000_1);
        let mut out = Vec::new();
        resampler.push(&vec![0.5f32; 4096], &mut out);
        for (at, &sample) in out.iter().enumerate().skip(64) {
            assert!((sample - 0.5).abs() < 1e-4, "at {at}: {sample}");
        }
    }
}
