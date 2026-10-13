// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Applying the replication: from core samples to twice as many.

use super::*;

/// The gain smoothing filter (stops a border clicking); off where a sinusoid starts.
pub(super) const SMOOTH: [f32; 5] = [
    0.333_333_34,
    0.301_502_83,
    0.218_169_5,
    0.115_163_83,
    0.031_830_5,
];

pub(super) const CHIRP: [f32; 4] = [0.0, 0.75, 0.9, 0.98];

/// How much the limiter lets a band be boosted: -3, 0, +3 dB, then unlimited.
pub(super) const LIMIT: [f32; 4] = [0.707_95, 1.0, 1.412_54, 1e10];

impl Sbr {
    pub fn apply(&mut self, channels: &mut [Vec<f32>]) {
        let count = channels.len();
        if self.channels.len() != count {
            self.channels.clear();
            self.channels.resize_with(count, Channel::new);
            self.turn_off();
        }
        if self.start && !self.ready {
            // A header arrived but no frame data did.
            self.turn_off();
        }
        if self.pushed {
            self.pushed = false;
        } else {
            self.kx[0] = self.kx[1];
            self.m[0] = self.m[1];
        }
        if self.start {
            self.dequantise(count > 1);
            self.ready = false;
        }
        for (at, samples) in channels.iter_mut().enumerate() {
            let mut bands = [[[0.0f32; 2]; BANDS]; SLOTS];
            self.channel(at, samples, &mut bands);
            let mut out = vec![0.0f32; SLOTS * 64];
            self.channels[at].synthesis.frame(&bands, &mut out);
            *samples = out;
        }
        self.reset = false;
    }

    pub(super) fn channel(
        &mut self,
        at: usize,
        samples: &[f32],
        out: &mut [[Complex; BANDS]; SLOTS],
    ) {
        {
            let channel = &mut self.channels[at];
            let position = channel.w_at;
            let mut w =
                std::mem::replace(&mut channel.w[position], Box::new([[[0.0; 2]; 32]; SLOTS]));
            channel.analysis.frame(samples, &mut w);
            channel.w[position] = w;
        }
        self.low_bands(at);
        if self.start {
            self.inverse_filter();
            self.chirp(at);
            self.generate(at);
            self.map(at);
            self.estimate(at);
            self.gains(at);
            self.assemble(at);
        }
        self.channels[at].w_at ^= 1;
        self.combine(at, out);
    }

    pub(super) fn dequantise(&mut self, pair: bool) {
        let root_two = std::f32::consts::SQRT_2;
        if pair && self.coupling {
            let pan = if self.channels[0].amp_res { 12 } else { 24 };
            let envelopes = self.channels[0].envelopes;
            let noises = self.channels[0].noises;
            let amp_res = self.channels[0].amp_res;
            for index in 1..=envelopes {
                let high = self.channels[0].high_res[index];
                for band in 0..self.n[usize::from(high)].min(MOST) {
                    let level = self.channels[0].envelope_q[index][band];
                    let balance = pan - self.channels[1].envelope_q[index][band];
                    let (level, balance) = if amp_res {
                        (exponent(level + 7), exponent(balance))
                    } else {
                        (
                            exponent((level >> 1) + 7)
                                * if level & 1 == 1 { root_two } else { 1.0 },
                            exponent(balance >> 1) * if balance & 1 == 1 { root_two } else { 1.0 },
                        )
                    };
                    let gain = level / (1.0 + balance);
                    self.channels[0].envelope[index][band] = gain;
                    self.channels[1].envelope[index][band] = gain * balance;
                }
            }
            for index in 1..=noises {
                for band in 0..self.n_noise.min(5) {
                    let level = exponent(NOISE_FLOOR - self.channels[0].noise_q[index][band] + 1);
                    let balance = exponent(12 - self.channels[1].noise_q[index][band]);
                    let gain = level / (1.0 + balance);
                    self.channels[0].noise[index][band] = gain;
                    self.channels[1].noise[index][band] = gain * balance;
                }
            }
        } else {
            let low = self.n[0];
            let high = self.n[1];
            let noise_bands = self.n_noise.min(5);
            for channel in &mut self.channels {
                for index in 1..=channel.envelopes {
                    let count = if channel.high_res[index] { high } else { low }.min(MOST);
                    for band in 0..count {
                        let value = channel.envelope_q[index][band];
                        channel.envelope[index][band] = if channel.amp_res {
                            exponent(value + 6)
                        } else {
                            exponent((value >> 1) + 6) * if value & 1 == 1 { root_two } else { 1.0 }
                        };
                    }
                }
                for index in 1..=channel.noises {
                    for band in 0..noise_bands {
                        channel.noise[index][band] =
                            exponent(NOISE_FLOOR - channel.noise_q[index][band]);
                    }
                }
            }
        }
    }

    pub(super) fn low_bands(&mut self, at: usize) {
        for band in self.x_low.iter_mut() {
            band.fill([0.0, 0.0]);
        }
        let channel = &self.channels[at];
        let current = &channel.w[channel.w_at];
        for band in 0..self.kx[1].min(32) {
            for slot in 0..SLOTS {
                self.x_low[band][LOOKBACK + slot] = current[slot][band];
            }
        }
        let previous = &channel.w[1 - channel.w_at];
        for band in 0..self.kx[0].min(32) {
            for slot in 0..LOOKBACK {
                self.x_low[band][slot] = previous[SLOTS - LOOKBACK + slot][band];
            }
        }
    }

    /// The second order predictor applied before copying up, 14496-3 clause 4.6.18.6.2. Double
    /// precision: the determinant is a difference of nearly equal numbers where it matters.
    pub(super) fn inverse_filter(&mut self) {
        for band in 0..self.k[0].min(32) {
            let x = &self.x_low[band];
            let wide = |v: [f32; 2]| [f64::from(v[0]), f64::from(v[1])];
            let (first_slot, second_slot) = (wide(x[0]), wide(x[2]));
            let mut r2 = first_slot[0] * second_slot[0] + first_slot[1] * second_slot[1];
            let mut i2 = first_slot[0] * second_slot[1] - first_slot[1] * second_slot[0];
            let (mut r1, mut i1, mut r0) = (0.0f64, 0.0f64, 0.0f64);
            for at in 1..38 {
                let (now, next, after) = (wide(x[at]), wide(x[at + 1]), wide(x[at + 2]));
                r0 += now[0] * now[0] + now[1] * now[1];
                r1 += now[0] * next[0] + now[1] * next[1];
                i1 += now[0] * next[1] - now[1] * next[0];
                r2 += now[0] * after[0] + now[1] * after[1];
                i2 += now[0] * after[1] - now[1] * after[0];
            }
            let lag2 = [r2, i2];
            let (last, past) = (wide(x[38]), wide(x[39]));
            let one = wide(x[1]);
            let energy_first = r0 + first_slot[0] * first_slot[0] + first_slot[1] * first_slot[1];
            let energy_last = r0 + last[0] * last[0] + last[1] * last[1];
            let lag1 = [
                r1 + first_slot[0] * one[0] + first_slot[1] * one[1],
                i1 + first_slot[0] * one[1] - first_slot[1] * one[0],
            ];
            let lag1_late = [
                r1 + last[0] * past[0] + last[1] * past[1],
                i1 + last[0] * past[1] - last[1] * past[0],
            ];

            // The divisor just over one is the standard's guard against near-dependent lags.
            let determinant =
                energy_first * energy_last - (lag1[0] * lag1[0] + lag1[1] * lag1[1]) / 1.000_001;
            let first = if determinant == 0.0 {
                [0.0f32, 0.0]
            } else {
                [
                    ((lag1_late[0] * lag1[0] - lag1_late[1] * lag1[1] - lag2[0] * energy_last)
                        / determinant) as f32,
                    ((lag1_late[0] * lag1[1] + lag1_late[1] * lag1[0] - lag2[1] * energy_last)
                        / determinant) as f32,
                ]
            };
            let zeroth = if energy_last == 0.0 {
                [0.0f32, 0.0]
            } else {
                let (a, b) = (f64::from(first[0]), f64::from(first[1]));
                [
                    (-(lag1_late[0] + a * lag1[0] + b * lag1[1]) / energy_last) as f32,
                    (-(lag1_late[1] + b * lag1[0] - a * lag1[1]) / energy_last) as f32,
                ]
            };
            // A predictor this strong would make the band louder than the envelope can bring down.
            let big = |c: [f32; 2]| c[0] * c[0] + c[1] * c[1] >= 16.0;
            self.alpha[band] = if big(first) || big(zeroth) {
                [[0.0, 0.0], [0.0, 0.0]]
            } else {
                [zeroth, first]
            };
        }
    }

    /// The chirp factors, smoothed across frames (asymmetrically, as the standard says).
    pub(super) fn chirp(&mut self, at: usize) {
        let bands = self.n_noise.min(5);
        let channel = &mut self.channels[at];
        for band in 0..bands {
            let modes = (channel.invf[0][band], channel.invf[1][band]);
            let mut want = if usize::from(modes.0) + usize::from(modes.1) == 1 {
                0.6
            } else {
                CHIRP[usize::from(modes.0).min(3)]
            };
            want = if want < channel.chirp[band] {
                0.75 * want + 0.25 * channel.chirp[band]
            } else {
                0.906_25 * want + 0.093_75 * channel.chirp[band]
            };
            channel.chirp[band] = if want < 0.015_625 { 0.0 } else { want };
        }
    }

    pub(super) fn generate(&mut self, at: usize) {
        let channel = &self.channels[at];
        let begin = 2 * channel.borders[0];
        let end = 2 * channel.borders[channel.envelopes];
        let mut band = self.kx[1];
        let mut noise_band = 0i32;
        for patch in 0..self.patches {
            for offset in 0..self.patch_bands[patch] {
                if band >= BANDS {
                    break;
                }
                let from = self.patch_start[patch] + offset;
                if from >= 32 {
                    break;
                }
                while noise_band <= self.n_noise as i32
                    && band >= usize::from(self.f_noise[noise_band.min(5) as usize])
                {
                    noise_band += 1;
                }
                noise_band -= 1;
                if noise_band < 0 {
                    return;
                }
                let chirp = self.channels[at].chirp[(noise_band as usize).min(4)];
                let [zeroth, first] = self.alpha[from];
                let coefficients = [
                    first[0] * chirp * chirp,
                    first[1] * chirp * chirp,
                    zeroth[0] * chirp,
                    zeroth[1] * chirp,
                ];
                for slot in begin..end.min(WIDE - OFFSET) {
                    let at = OFFSET + slot;
                    let two = self.x_low[from][at - 2];
                    let one = self.x_low[from][at - 1];
                    let now = self.x_low[from][at];
                    self.x_high[band][at] = [
                        two[0] * coefficients[0] - two[1] * coefficients[1]
                            + one[0] * coefficients[2]
                            - one[1] * coefficients[3]
                            + now[0],
                        two[1] * coefficients[0]
                            + two[0] * coefficients[1]
                            + one[1] * coefficients[2]
                            + one[0] * coefficients[3]
                            + now[1],
                    ];
                }
                band += 1;
            }
        }
        for empty in band..(self.kx[1] + self.m[1]).min(BANDS) {
            self.x_high[empty].fill([0.0, 0.0]);
        }
    }

    pub(super) fn map(&mut self, at: usize) {
        let kx = self.kx[1];
        for row in self.channels[at].sine_index[1..].iter_mut() {
            row.fill(false);
        }
        let envelopes = self.channels[at].envelopes;
        for envelope in 0..envelopes {
            let high = self.channels[at].high_res[envelope + 1];
            let count = self.n[usize::from(high)].min(MOST);
            let table: &[u16] = if high { &self.f_high } else { &self.f_low };
            for group in 0..count {
                let value = self.channels[at].envelope[envelope + 1][group];
                for band in usize::from(table[group])..usize::from(table[group + 1]) {
                    if let Some(slot) = self.mapped.want[envelope].get_mut(band - kx) {
                        *slot = value;
                    }
                }
            }
            let which = usize::from(
                self.channels[at].noises > 1
                    && self.channels[at].borders[envelope] >= self.channels[at].noise_borders[1],
            );
            for group in 0..self.n_noise.min(5) {
                let value = self.channels[at].noise[which + 1][group];
                for band in usize::from(self.f_noise[group])..usize::from(self.f_noise[group + 1]) {
                    if let Some(slot) = self.mapped.noise[envelope].get_mut(band - kx) {
                        *slot = value;
                    }
                }
            }

            for group in 0..self.n[1].min(MOST) {
                if !self.channels[at].add_harmonic_flag {
                    continue;
                }
                let middle =
                    (usize::from(self.f_high[group]) + usize::from(self.f_high[group + 1])) >> 1;
                if middle < kx || middle - kx >= MOST {
                    continue;
                }
                let started = envelope as i32 >= self.channels[at].sine_envelope[1]
                    || self.channels[at].sine_index[0][middle - kx];
                self.channels[at].sine_index[envelope + 1][middle - kx] =
                    self.channels[at].add_harmonic[group] && started;
            }

            for group in 0..count {
                let (low, high) = (
                    usize::from(table[group]).saturating_sub(kx),
                    usize::from(table[group + 1]).saturating_sub(kx),
                );
                let present = (low..high.min(MOST))
                    .any(|band| self.channels[at].sine_index[envelope + 1][band]);
                for band in low..high.min(MOST) {
                    self.mapped.sine[envelope][band] = present;
                }
            }
        }
        let last = self.channels[at].sine_index[envelopes];
        self.channels[at].sine_index[0] = last;
    }

    pub(super) fn estimate(&mut self, at: usize) {
        let kx = self.kx[1];
        let channel = &self.channels[at];
        for envelope in 0..channel.envelopes {
            let low = channel.borders[envelope] * 2 + OFFSET;
            let high = channel.borders[envelope + 1] * 2 + OFFSET;
            if high <= low || high > WIDE {
                continue;
            }
            if self.interpolate {
                let scale =
                    0.5 / (channel.borders[envelope + 1] - channel.borders[envelope]) as f32;
                for band in 0..self.m[1].min(MOST) {
                    let row = &self.x_high[(kx + band).min(BANDS - 1)];
                    let sum: f32 = row[low..high]
                        .iter()
                        .map(|v| v[0] * v[0] + v[1] * v[1])
                        .sum();
                    self.mapped.have[envelope][band] = sum * scale;
                }
            } else {
                let high_res = channel.high_res[envelope + 1];
                let table: &[u16] = if high_res { &self.f_high } else { &self.f_low };
                let width = 2 * (channel.borders[envelope + 1] - channel.borders[envelope]);
                for group in 0..self.n[usize::from(high_res)].min(MOST) {
                    let (first, last) = (usize::from(table[group]), usize::from(table[group + 1]));
                    if last <= first {
                        continue;
                    }
                    let mut sum = 0.0f32;
                    for band in first..last.min(BANDS) {
                        sum += self.x_high[band][low..high]
                            .iter()
                            .map(|v| v[0] * v[0] + v[1] * v[1])
                            .sum::<f32>();
                    }
                    sum /= (width * (last - first)) as f32;
                    for band in first..last {
                        if let Some(slot) = self.mapped.have[envelope].get_mut(band - kx) {
                            *slot = sum;
                        }
                    }
                }
            }
        }
    }

    /// Gains, noise and sinusoid levels with the limiter, 14496-3 clause 4.6.18.7.5.
    pub(super) fn gains(&mut self, at: usize) {
        let kx = self.kx[1];
        let smallest = f32::MIN_POSITIVE;
        let epsilon = f32::EPSILON;
        let envelopes = self.channels[at].envelopes;
        let sine_envelope = self.channels[at].sine_envelope;
        for envelope in 0..envelopes {
            // No noise in the envelope a sinusoid starts or stops in.
            let with_noise =
                !(envelope as i32 == sine_envelope[1] || envelope as i32 == sine_envelope[0]);
            for limit in 0..self.n_limit {
                let low = usize::from(self.f_limit[limit]).saturating_sub(kx);
                let high = usize::from(self.f_limit[limit + 1]).saturating_sub(kx);
                let high = high.min(self.m[1]).min(MOST);
                if high <= low {
                    continue;
                }
                for band in low..high {
                    let want = self.mapped.want[envelope][band];
                    let noise = self.mapped.noise[envelope][band];
                    let share = want / (1.0 + noise);
                    self.mapped.noise_level[envelope][band] = (share * noise).sqrt();
                    self.mapped.sine_level[envelope][band] =
                        if self.channels[at].sine_index[envelope + 1][band] {
                            share.sqrt()
                        } else {
                            0.0
                        };
                    let have = self.mapped.have[envelope][band];
                    self.mapped.gain[envelope][band] = if self.mapped.sine[envelope][band] {
                        (want * noise / ((1.0 + have) * (1.0 + noise))).sqrt()
                    } else {
                        (want / ((1.0 + have) * (1.0 + noise * f32::from(with_noise)))).sqrt()
                    } + smallest;
                }
                let wanted: f32 = self.mapped.want[envelope][low..high].iter().sum();
                let had: f32 = self.mapped.have[envelope][low..high].iter().sum();
                let ceiling = (LIMIT[usize::from(self.limiter_gains).min(3)]
                    * ((epsilon + wanted) / (epsilon + had)).sqrt())
                .min(100_000.0);
                for band in low..high {
                    let most = self.mapped.noise_level[envelope][band] * ceiling
                        / self.mapped.gain[envelope][band];
                    self.mapped.noise_level[envelope][band] =
                        self.mapped.noise_level[envelope][band].min(most);
                    self.mapped.gain[envelope][band] =
                        self.mapped.gain[envelope][band].min(ceiling);
                }
                let mut had = 0.0f32;
                for band in low..high {
                    let gain = self.mapped.gain[envelope][band];
                    let sine = self.mapped.sine_level[envelope][band];
                    let noise = self.mapped.noise_level[envelope][band];
                    had += self.mapped.have[envelope][band] * gain * gain
                        + sine * sine
                        + if with_noise && sine == 0.0 {
                            noise * noise
                        } else {
                            0.0
                        };
                }
                let boost = ((epsilon + wanted) / (epsilon + had))
                    .sqrt()
                    .min(1.584_893_2);
                for band in low..high {
                    self.mapped.gain[envelope][band] *= boost;
                    self.mapped.noise_level[envelope][band] *= boost;
                    self.mapped.sine_level[envelope][band] *= boost;
                }
            }
        }
    }

    /// Applying gains, noise and sinusoids, 14496-3 clause 4.6.18.7.6.
    pub(super) fn assemble(&mut self, at: usize) {
        let kx = self.kx[1];
        let width = self.m[1].min(MOST);
        let reach = if self.smoothing { 4 } else { 0 };
        let reset = self.reset;
        let position = self.channels[at].w_at ^ 1;
        let envelopes = self.channels[at].envelopes;
        let sine_envelope = self.channels[at].sine_envelope;

        {
            let gains = self.mapped.gain[0];
            let noises = self.mapped.noise_level[0];
            let channel = &mut self.channels[at];
            let first = 2 * channel.borders[0];
            if reset {
                // After a table rebuild, smoothing starts from this frame's own gains.
                for slot in 0..reach {
                    if first + slot < 42 {
                        channel.gain_history[first + slot][..width]
                            .copy_from_slice(&gains[..width]);
                        channel.noise_history[first + slot][..width]
                            .copy_from_slice(&noises[..width]);
                    }
                }
            } else if reach > 0 {
                // The filter reaches back to where the last frame's last envelope ended.
                let old = 2 * channel.border_old;
                for slot in 0..4 {
                    if old + slot < 42 && first + slot < 42 {
                        channel.gain_history[first + slot] = channel.gain_history[old + slot];
                        channel.noise_history[first + slot] = channel.noise_history[old + slot];
                    }
                }
            }
        }
        for envelope in 0..envelopes {
            let channel = &mut self.channels[at];
            let (from, to) = (
                2 * channel.borders[envelope],
                2 * channel.borders[envelope + 1],
            );
            for slot in from..to.min(42 - reach) {
                channel.gain_history[reach + slot][..width]
                    .copy_from_slice(&self.mapped.gain[envelope][..width]);
                channel.noise_history[reach + slot][..width]
                    .copy_from_slice(&self.mapped.noise_level[envelope][..width]);
            }
        }

        let mut noise_at = self.channels[at].noise_at;
        let mut sine_at = self.channels[at].sine_at;
        for envelope in 0..envelopes {
            let (from, to) = (
                2 * self.channels[at].borders[envelope],
                2 * self.channels[at].borders[envelope + 1],
            );
            let plain = envelope as i32 == sine_envelope[0] || envelope as i32 == sine_envelope[1];
            for slot in from..to.min(38) {
                let mut gain = [0.0f32; MOST];
                let mut noise = [0.0f32; MOST];
                if reach > 0 && !plain {
                    let channel = &self.channels[at];
                    for band in 0..width {
                        let mut g = 0.0;
                        let mut q = 0.0;
                        for (step, weight) in SMOOTH.iter().enumerate() {
                            g += channel.gain_history[slot + reach - step][band] * weight;
                            q += channel.noise_history[slot + reach - step][band] * weight;
                        }
                        gain[band] = g;
                        noise[band] = q;
                    }
                } else {
                    let channel = &self.channels[at];
                    gain[..width].copy_from_slice(&channel.gain_history[slot + reach][..width]);
                    noise[..width].copy_from_slice(&channel.noise_history[slot][..width]);
                }

                // The sinusoid's phase cycles in four steps, alternating between neighbours.
                let component = sine_at & 1;
                let sign = 1.0 - ((sine_at + (kx & 1)) & 2) as f32;
                let y = &mut self.channels[at].y[position][slot];
                for band in 0..width {
                    let value = self.x_high[kx + band][slot + OFFSET];
                    y[kx + band] = [value[0] * gain[band], value[1] * gain[band]];
                }
                for band in 0..width {
                    noise_at = (noise_at + 1) & 0x1ff;
                    let sine = self.mapped.sine_level[envelope][band];
                    if sine != 0.0 {
                        let sign = if component == 1 && band & 1 == 1 {
                            -sign
                        } else {
                            sign
                        };
                        y[kx + band][component] += sine * sign;
                    } else if !plain {
                        let table = tables::NOISE[noise_at];
                        y[kx + band][0] += noise[band] * table[0];
                        y[kx + band][1] += noise[band] * table[1];
                    }
                }
                sine_at = (sine_at + 1) & 3;
            }
        }
        self.channels[at].noise_at = noise_at;
        self.channels[at].sine_at = sine_at;
    }

    /// A header in this frame takes effect from where the last frame's last envelope ended.
    pub(super) fn combine(&mut self, at: usize, out: &mut [[Complex; BANDS]; SLOTS]) {
        for slot in out.iter_mut() {
            slot.fill([0.0, 0.0]);
        }
        let channel = &self.channels[at];
        let previous = &channel.y[1 - channel.w_at];
        let current = &channel.y[channel.w_at];
        let split = (2 * channel.border_old).saturating_sub(SLOTS).min(SLOTS);
        for (slot, bands) in out.iter_mut().enumerate().take(split) {
            for (band, value) in bands.iter_mut().enumerate().take(self.kx[0].min(32)) {
                *value = self.x_low[band][slot + OFFSET];
            }
            let high = (self.kx[0] + self.m[0]).min(BANDS);
            bands[self.kx[0]..high].copy_from_slice(&previous[slot + SLOTS][self.kx[0]..high]);
        }
        for (slot, bands) in out.iter_mut().enumerate().skip(split) {
            for (band, value) in bands.iter_mut().enumerate().take(self.kx[1].min(32)) {
                *value = self.x_low[band][slot + OFFSET];
            }
            let high = (self.kx[1] + self.m[1]).min(BANDS);
            bands[self.kx[1]..high].copy_from_slice(&current[slot][self.kx[1]..high]);
        }
    }
}
