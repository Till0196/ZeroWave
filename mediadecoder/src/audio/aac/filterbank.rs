// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The filterbank: IMDCT and overlap-add. The first frame after a start or reset lacks half its
//! aliasing cancellation; that half was never sent. The direct sum is kept to test the fast one.

use std::f64::consts::PI;

/// How the coder cut this frame (start/stop sequences bridge long and short windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sequence {
    #[default]
    OnlyLong,
    LongStart,
    EightShort,
    LongStop,
}

impl Sequence {
    pub fn of_index(index: u32) -> Self {
        match index {
            1 => Self::LongStart,
            2 => Self::EightShort,
            3 => Self::LongStop,
            _ => Self::OnlyLong,
        }
    }

    pub fn short(self) -> bool {
        self == Self::EightShort
    }
}

/// Which window shape the coder used. The previous frame's choice shapes this frame's left half.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Sine,
    /// Kaiser-Bessel derived (alpha four long, six short).
    Kaiser,
}

impl Shape {
    pub fn of_index(index: u32) -> Self {
        if index == 1 { Self::Kaiser } else { Self::Sine }
    }
}

pub const LONG: usize = 1024;
pub const SHORT: usize = LONG / 8;
const FLAT: usize = (LONG - SHORT) / 2;

#[derive(Debug)]
pub struct Filterbank {
    long: Transform,
    short: Transform,
    /// The rising half of each window by length and shape; the falling half is it read backwards.
    long_window: [Vec<f32>; 2],
    short_window: [Vec<f32>; 2],
}

impl Default for Filterbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Filterbank {
    pub fn new() -> Self {
        Self {
            long: Transform::new(LONG),
            short: Transform::new(SHORT),
            long_window: [sine(LONG), kaiser(LONG, 4.0)],
            short_window: [sine(SHORT), kaiser(SHORT, 6.0)],
        }
    }

    fn long_window(&self, shape: Shape) -> &[f32] {
        &self.long_window[shape as usize]
    }

    fn short_window(&self, shape: Shape) -> &[f32] {
        &self.short_window[shape as usize]
    }

    /// `overlap` is replaced; `previous` is the previous window shape. The previous frame's falling
    /// half was applied as it left `overlap` (matters for forbidden long-to-short jumps).
    pub fn frame(
        &self,
        spectrum: &[f32; LONG],
        sequence: Sequence,
        shape: Shape,
        previous: Shape,
        overlap: &mut [f32; LONG],
        out: &mut [f32; LONG],
    ) {
        let mut buffer = [0.0f32; 2 * LONG];
        if sequence.short() {
            for window in 0..8 {
                let at = window * SHORT;
                self.short.imdct(
                    &spectrum[at..at + SHORT],
                    &mut buffer[2 * at..2 * at + 2 * SHORT],
                );
            }
        } else {
            self.long.imdct(spectrum, &mut buffer);
        }

        let long = self.long_window(shape);
        let long_previous = self.long_window(previous);
        let short = self.short_window(shape);
        let short_previous = self.short_window(previous);

        match sequence {
            Sequence::OnlyLong | Sequence::LongStart => {
                for at in 0..LONG {
                    out[at] = overlap[at] + buffer[at] * long_previous[at];
                }
            }
            Sequence::LongStop => {
                // Rising half: silence, a short rise, then unity (the previous frame was short).
                out[..FLAT].copy_from_slice(&overlap[..FLAT]);
                for at in 0..SHORT {
                    out[FLAT + at] = overlap[FLAT + at] + buffer[FLAT + at] * short_previous[at];
                }
                for at in FLAT + SHORT..LONG {
                    out[at] = overlap[at] + buffer[at];
                }
            }
            Sequence::EightShort => {
                out[..FLAT].copy_from_slice(&overlap[..FLAT]);
                out[FLAT..].copy_from_slice(&overlap[FLAT..]);
                for at in 0..SHORT {
                    out[FLAT + at] += buffer[at] * short_previous[at];
                }
                for window in 1..8 {
                    let begin = FLAT + window * SHORT;
                    for at in 0..SHORT {
                        if begin + at >= LONG {
                            break;
                        }
                        out[begin + at] += buffer[(2 * window - 1) * SHORT + at]
                            * short[SHORT - 1 - at]
                            + buffer[2 * window * SHORT + at] * short[at];
                    }
                }
            }
        }

        match sequence {
            Sequence::OnlyLong | Sequence::LongStop => {
                for at in 0..LONG {
                    overlap[at] = buffer[LONG + at] * long[LONG - 1 - at];
                }
            }
            Sequence::LongStart => {
                // Falling half: unity, a short fall, then silence (the next frame is short).
                overlap[..FLAT].copy_from_slice(&buffer[LONG..LONG + FLAT]);
                for at in 0..SHORT {
                    overlap[FLAT + at] = buffer[LONG + FLAT + at] * short[SHORT - 1 - at];
                }
                overlap[FLAT + SHORT..].fill(0.0);
            }
            Sequence::EightShort => {
                overlap.fill(0.0);
                for window in 1..8 {
                    let begin = (FLAT + window * SHORT) as isize - LONG as isize;
                    for at in 0..SHORT {
                        let to = begin + at as isize;
                        if to < 0 {
                            continue;
                        }
                        overlap[to as usize] += buffer[(2 * window - 1) * SHORT + at]
                            * short[SHORT - 1 - at]
                            + buffer[2 * window * SHORT + at] * short[at];
                    }
                }
                let begin = FLAT + 8 * SHORT - LONG;
                for at in 0..SHORT {
                    overlap[begin + at] += buffer[15 * SHORT + at] * short[SHORT - 1 - at];
                }
            }
        }
    }
}

#[derive(Debug)]
struct Transform {
    n: usize,
    fft: Fft,
    /// The rotation that turns the Fourier transform into a DCT-IV.
    twiddle: Vec<(f32, f32)>,
}

impl Transform {
    fn new(n: usize) -> Self {
        let half = n / 2;
        let twiddle = (0..half)
            .map(|at| {
                let angle = -PI * (at as f64 + 0.125) / n as f64;
                (angle.cos() as f32, angle.sin() as f32)
            })
            .collect();
        Self {
            n,
            fft: Fft::new(half),
            twiddle,
        }
    }

    fn dct4(&self, input: &[f32], out: &mut [f32]) {
        let n = self.n;
        let half = n / 2;
        let mut work = vec![(0.0f32, 0.0f32); half];
        for at in 0..half {
            let (re, im) = (input[2 * at], input[n - 1 - 2 * at]);
            let (c, s) = self.twiddle[at];
            work[at] = (re * c - im * s, re * s + im * c);
        }
        self.fft.forward(&mut work);
        for at in 0..half {
            let (re, im) = work[at];
            let (c, s) = self.twiddle[at];
            out[2 * at] = re * c - im * s;
            out[n - 1 - 2 * at] = -(re * s + im * c);
        }
    }

    fn imdct(&self, input: &[f32], out: &mut [f32]) {
        let n = self.n;
        let mut y = vec![0.0f32; n];
        self.dct4(input, &mut y);
        let scale = 1.0 / n as f32;
        for at in 0..n / 2 {
            out[at] = y[at + n / 2] * scale;
        }
        for at in n / 2..3 * n / 2 {
            out[at] = -y[3 * n / 2 - 1 - at] * scale;
        }
        for at in 3 * n / 2..2 * n {
            out[at] = -y[at - 3 * n / 2] * scale;
        }
    }

    #[cfg(test)]
    fn imdct_direct(&self, input: &[f32], out: &mut [f32]) {
        let n = self.n as f64;
        for (at, sample) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (k, &value) in input.iter().enumerate() {
                let angle = PI / n * (at as f64 + 0.5 + n / 2.0) * (k as f64 + 0.5);
                sum += f64::from(value) * angle.cos();
            }
            *sample = (sum / n) as f32;
        }
    }
}

#[derive(Debug)]
struct Fft {
    n: usize,
    twiddle: Vec<(f32, f32)>,
    reversed: Vec<u32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let reversed = (0..n)
            .map(|at| (at as u32).reverse_bits() >> (32 - bits))
            .collect();
        let mut twiddle = Vec::new();
        let mut span = 2;
        while span <= n {
            for at in 0..span / 2 {
                let angle = -2.0 * PI * at as f64 / span as f64;
                twiddle.push((angle.cos() as f32, angle.sin() as f32));
            }
            span *= 2;
        }
        Self {
            n,
            twiddle,
            reversed,
        }
    }

    /// The kernel the cosine transform's derivation above assumes.
    fn forward(&self, values: &mut [(f32, f32)]) {
        for at in 0..self.n {
            let to = self.reversed[at] as usize;
            if at < to {
                values.swap(at, to);
            }
        }
        let mut span = 2;
        let mut base = 0;
        while span <= self.n {
            let half = span / 2;
            for group in (0..self.n).step_by(span) {
                for at in 0..half {
                    let (c, s) = self.twiddle[base + at];
                    let (re, im) = values[group + at + half];
                    let product = (re * c - im * s, re * s + im * c);
                    let (are, aim) = values[group + at];
                    values[group + at] = (are + product.0, aim + product.1);
                    values[group + at + half] = (are - product.0, aim - product.1);
                }
            }
            base += half;
            span *= 2;
        }
    }
}

fn sine(half: usize) -> Vec<f32> {
    (0..half)
        .map(|at| (PI / (2.0 * half as f64) * (at as f64 + 0.5)).sin() as f32)
        .collect()
}

/// The rising half of a Kaiser-Bessel derived window (square root of the running Kaiser sum).
fn kaiser(half: usize, alpha: f64) -> Vec<f32> {
    let scale = bessel(PI * alpha);
    let kernel: Vec<f64> = (0..=half)
        .map(|at| {
            let x = 2.0 * at as f64 / half as f64 - 1.0;
            bessel(PI * alpha * (1.0 - x * x).max(0.0).sqrt()) / scale
        })
        .collect();
    let total: f64 = kernel.iter().sum();
    let mut running = 0.0;
    kernel[..half]
        .iter()
        .map(|value| {
            running += value;
            (running / total).sqrt() as f32
        })
        .collect()
}

/// I0 by its series; the argument stays below about nineteen.
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
    use super::{Fft, Filterbank, LONG, SHORT, Sequence, Shape, Transform, kaiser, sine};

    fn spread(count: usize, seed: u32) -> Vec<f32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / f32::from(u16::MAX) - 128.0
            })
            .collect()
    }

    #[test]
    fn the_fast_transform_agrees_with_the_definition() {
        for n in [SHORT, LONG] {
            let transform = Transform::new(n);
            let input = spread(n, 7);
            let mut fast = vec![0.0f32; 2 * n];
            let mut slow = vec![0.0f32; 2 * n];
            transform.imdct(&input, &mut fast);
            transform.imdct_direct(&input, &mut slow);
            let worst = fast
                .iter()
                .zip(&slow)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            let size = slow.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
            assert!(worst < size * 1e-4, "{worst} against {size} at n={n}");
        }
    }

    #[test]
    fn the_transform_undoes_itself_over_a_frame() {
        for window in [
            sine(LONG),
            kaiser(LONG, 4.0),
            sine(SHORT),
            kaiser(SHORT, 6.0),
        ] {
            let half = window.len();
            for at in 0..half {
                let rise = window[at];
                let fall = window[half - 1 - at];
                assert!(
                    (rise * rise + fall * fall - 1.0).abs() < 1e-5,
                    "at {at} of {half}"
                );
            }
        }
    }

    #[test]
    fn the_fourier_transform_agrees_with_the_definition() {
        let n = 64;
        let fft = Fft::new(n);
        let input: Vec<(f32, f32)> = spread(n, 3).into_iter().zip(spread(n, 11)).collect();
        let mut fast = input.clone();
        fft.forward(&mut fast);
        for (k, &(fast_real, fast_imaginary)) in fast.iter().enumerate() {
            let mut sum = (0.0f64, 0.0f64);
            for (m, &(re, im)) in input.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * (m * k) as f64 / n as f64;
                let (c, s) = (angle.cos(), angle.sin());
                sum.0 += f64::from(re) * c - f64::from(im) * s;
                sum.1 += f64::from(re) * s + f64::from(im) * c;
            }
            assert!((f64::from(fast_real) - sum.0).abs() < 1e-2, "real at {k}");
            assert!(
                (f64::from(fast_imaginary) - sum.1).abs() < 1e-2,
                "imaginary at {k}"
            );
        }
    }

    #[test]
    fn a_run_of_long_frames_reconstructs_what_was_transformed() {
        let bank = Filterbank::new();
        let signal: Vec<f32> = (0..4 * LONG)
            .map(|at| (at as f32 * 0.013).sin() * 0.5)
            .collect();
        let window = sine(LONG);
        let mut overlap = [0.0f32; LONG];
        let mut out = [0.0f32; LONG];
        for frame in 0..3 {
            // The standard's forward transform has a factor of two; without it the result halves.
            let mut spectrum = [0.0f32; LONG];
            for (k, value) in spectrum.iter_mut().enumerate() {
                let mut sum = 0.0f64;
                for at in 0..2 * LONG {
                    let shape = if at < LONG {
                        window[at]
                    } else {
                        window[2 * LONG - 1 - at]
                    };
                    let angle = std::f64::consts::PI / LONG as f64
                        * (at as f64 + 0.5 + LONG as f64 / 2.0)
                        * (k as f64 + 0.5);
                    sum += f64::from(signal[frame * LONG + at] * shape) * angle.cos();
                }
                *value = 2.0 * sum as f32;
            }
            bank.frame(
                &spectrum,
                Sequence::OnlyLong,
                Shape::Sine,
                Shape::Sine,
                &mut overlap,
                &mut out,
            );
            if frame >= 1 {
                for at in 0..LONG {
                    let want = signal[frame * LONG + at];
                    assert!(
                        (out[at] - want).abs() < 1e-3,
                        "frame {frame} sample {at}: {} against {want}",
                        out[at]
                    );
                }
            }
        }
    }
}
