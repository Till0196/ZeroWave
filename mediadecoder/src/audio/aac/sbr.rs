// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Spectral band replication: ISO/IEC 14496-3 clause 4.6.18.
//! Only one-segment (HE-AAC, 24 kHz core to 48 kHz) uses it. ADTS cannot declare it, so [`Sbr`] is
//! switched on by the first payload, not by a configuration.

mod qmf;
mod tables;

use crate::audio::Error;
use crate::bits::Reader;

use qmf::{Analysis, BANDS, Complex, SLOTS, Synthesis};

mod apply;
mod build;
#[cfg(test)]
mod tests;

/// How far into `X_high` the envelope adjustment starts (the predictor reaches two slots back).
const OFFSET: usize = 2;

const LOOKBACK: usize = 8;

const WIDE: usize = LOOKBACK + SLOTS;

const MOST: usize = 48;

const ENVELOPES: usize = 5;

const NOISE_FLOOR: i32 = 6;

pub const EXTENSION: u32 = 13;
pub const EXTENSION_CRC: u32 = 14;

const PARAMETRIC_STEREO: u32 = 2;

/// The spectrum parameters of a header; a change rebuilds the band tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spectrum {
    start_freq: u8,
    stop_freq: u8,
    crossover: usize,
    freq_scale: u8,
    alter_scale: bool,
    noise_bands: u8,
}

impl Default for Spectrum {
    /// Something no header can say, so that the first header always counts as a change.
    fn default() -> Self {
        Self {
            start_freq: 0xff,
            stop_freq: 0xff,
            crossover: usize::MAX,
            freq_scale: 0xff,
            alter_scale: false,
            noise_bands: 0xff,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Class {
    #[default]
    FixFix,
    FixVar,
    VarFix,
    VarVar,
}

impl Class {
    fn of_index(index: u32) -> Self {
        match index {
            1 => Self::FixVar,
            2 => Self::VarFix,
            3 => Self::VarVar,
            _ => Self::FixFix,
        }
    }

    fn trailing(self) -> bool {
        matches!(self, Self::FixVar | Self::VarVar)
    }
}

#[derive(Debug)]
struct Channel {
    analysis: Analysis,
    synthesis: Synthesis,
    w: [Box<[[Complex; 32]; SLOTS]>; 2],
    w_at: usize,
    /// The adjusted bands of this frame and the last; `w_at` says which is current.
    y: [Box<[[Complex; BANDS]; 38]>; 2],

    envelopes: usize,
    noises: usize,
    amp_res: bool,
    /// The envelope borders, in slots; `borders[envelopes]` is the end.
    borders: [usize; 8],
    noise_borders: [usize; 3],
    /// Whether each envelope uses the high resolution table; entry nought is the last frame's last.
    high_res: [bool; 7],
    time_delta_env: [bool; ENVELOPES],
    time_delta_noise: [bool; 2],
    invf: [[u8; 5]; 2],
    add_harmonic: [bool; MOST],
    add_harmonic_flag: bool,
    border_old: usize,
    /// `l_APrev` and `l_A`: which envelope must not be smoothed because a sinusoid starts in it.
    sine_envelope: [i32; 2],
    /// The chirp factors, smoothed across frames so inverse filtering does not switch audibly.
    chirp: [f32; 5],
    envelope_q: [[i32; MOST]; 6],
    envelope: [[f32; MOST]; 6],
    noise_q: [[i32; 5]; 3],
    noise: [[f32; 5]; 3],
    sine_index: [[bool; MOST]; 8],
    gain_history: Box<[[f32; MOST]; 42]>,
    noise_history: Box<[[f32; MOST]; 42]>,
    noise_at: usize,
    sine_at: usize,
}

impl Channel {
    fn new() -> Self {
        Self {
            analysis: Analysis::new(),
            synthesis: Synthesis::new(),
            w: [
                Box::new([[[0.0; 2]; 32]; SLOTS]),
                Box::new([[[0.0; 2]; 32]; SLOTS]),
            ],
            w_at: 0,
            y: [
                Box::new([[[0.0; 2]; BANDS]; 38]),
                Box::new([[[0.0; 2]; BANDS]; 38]),
            ],
            envelopes: 0,
            noises: 0,
            amp_res: false,
            borders: [0; 8],
            noise_borders: [0; 3],
            high_res: [false; 7],
            time_delta_env: [false; ENVELOPES],
            time_delta_noise: [false; 2],
            invf: [[0; 5]; 2],
            add_harmonic: [false; MOST],
            add_harmonic_flag: false,
            border_old: 0,
            sine_envelope: [0, -1],
            chirp: [0.0; 5],
            envelope_q: [[0; MOST]; 6],
            envelope: [[0.0; MOST]; 6],
            noise_q: [[0; 5]; 3],
            noise: [[0.0; 5]; 3],
            sine_index: [[false; MOST]; 8],
            gain_history: Box::new([[0.0; MOST]; 42]),
            noise_history: Box::new([[0.0; MOST]; 42]),
            noise_at: 0,
            sine_at: 0,
        }
    }
}

/// Band tables and channel state for one element; its payload rides in the next fill element.
#[derive(Debug)]
pub struct Sbr {
    rate: u32,
    /// Whether a header has arrived. Until then the bank only upsamples.
    start: bool,
    ready: bool,
    reset: bool,
    pushed: bool,
    spectrum: Spectrum,
    amp_res_header: bool,
    limiter_bands: u8,
    limiter_gains: u8,
    interpolate: bool,
    smoothing: bool,
    coupling: bool,
    /// `k0`, `k1`, `k2`: first band, the split between warping regions, last band.
    k: [usize; 3],
    /// `kx` and its previous value: the first band the replication writes to.
    kx: [usize; 2],
    /// `M` and its previous value: how many bands the replication writes.
    m: [usize; 2],
    master: usize,
    f_master: [u16; 49],
    n: [usize; 2],
    n_noise: usize,
    n_limit: usize,
    f_low: [u16; 25],
    f_high: [u16; 49],
    f_noise: [u16; 6],
    f_limit: [u16; 30],
    patches: usize,
    patch_bands: [usize; 6],
    patch_start: [usize; 6],
    channels: Vec<Channel>,
    saw_parametric_stereo: bool,
    x_low: Box<[[Complex; WIDE]; 32]>,
    x_high: Box<[[Complex; WIDE]; BANDS]>,
    alpha: Box<[[Complex; 2]; 32]>,
    mapped: Box<Mapped>,
}

#[derive(Debug)]
struct Mapped {
    want: [[f32; MOST]; 7],
    noise: [[f32; MOST]; 7],
    sine: [[bool; MOST]; 7],
    have: [[f32; MOST]; 7],
    noise_level: [[f32; MOST]; 7],
    sine_level: [[f32; MOST]; 7],
    gain: [[f32; MOST]; 7],
}

impl Mapped {
    fn new() -> Self {
        Self {
            want: [[0.0; MOST]; 7],
            noise: [[0.0; MOST]; 7],
            sine: [[false; MOST]; 7],
            have: [[0.0; MOST]; 7],
            noise_level: [[0.0; MOST]; 7],
            sine_level: [[0.0; MOST]; 7],
            gain: [[0.0; MOST]; 7],
        }
    }
}

impl Default for Sbr {
    fn default() -> Self {
        Self::new()
    }
}

impl Sbr {
    pub fn new() -> Self {
        let mut sbr = Self {
            rate: 0,
            start: false,
            ready: false,
            reset: false,
            pushed: false,
            spectrum: Spectrum::default(),
            amp_res_header: false,
            limiter_bands: 2,
            limiter_gains: 2,
            interpolate: true,
            smoothing: false,
            coupling: false,
            k: [0; 3],
            kx: [32, 32],
            m: [0; 2],
            master: 0,
            f_master: [0; 49],
            n: [0; 2],
            n_noise: 0,
            n_limit: 0,
            f_low: [0; 25],
            f_high: [0; 49],
            f_noise: [0; 6],
            f_limit: [0; 30],
            patches: 0,
            patch_bands: [0; 6],
            patch_start: [0; 6],
            channels: Vec::new(),
            saw_parametric_stereo: false,
            x_low: Box::new([[[0.0; 2]; WIDE]; 32]),
            x_high: Box::new([[[0.0; 2]; WIDE]; BANDS]),
            alpha: Box::new([[[0.0; 2]; 2]; 32]),
            mapped: Box::new(Mapped::new()),
        };
        sbr.turn_off();
        sbr
    }

    /// Falls back to pure upsampling. The rate still doubles so later frames line up.
    fn turn_off(&mut self) {
        self.start = false;
        self.ready = false;
        // The standard says `kx'` starts at nought, but the core fills all thirty-two bands.
        self.kx[1] = 32;
        self.m[1] = 0;
        self.spectrum = Spectrum::default();
        for channel in &mut self.channels {
            channel.sine_envelope[1] = -1;
        }
    }

    pub fn parametric_stereo(&self) -> bool {
        self.saw_parametric_stereo
    }

    pub fn enable(&mut self, core: u32) {
        if self.rate == 0 {
            self.rate = core.saturating_mul(2);
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Reads one `sbr_extension_data`; `reader` is just after the extension type.
    pub fn payload(
        &mut self,
        reader: &mut Reader<'_>,
        bits: usize,
        crc: bool,
        pair: bool,
        core: u32,
    ) -> Result<(), Error> {
        self.enable(core);
        self.channels
            .resize_with(if pair { 2 } else { 1 }, Channel::new);
        self.reset = false;
        let began = reader.at();
        if crc {
            reader.skip(10);
        }
        // Keep the previous `kx` and `M`: this frame's first slots still use them.
        self.kx[0] = self.kx[1];
        self.m[0] = self.m[1];
        self.pushed = true;
        if reader.bit() == 1 {
            self.header(reader);
        }
        if self.reset && self.rebuild().is_err() {
            self.turn_off();
        }
        if self.start {
            self.data(reader, pair)?;
        }
        if reader.at() > began + bits {
            self.turn_off();
            return Err(Error::Damaged("a replication payload that overran"));
        }
        Ok(())
    }

    fn header(&mut self, reader: &mut Reader<'_>) {
        self.start = true;
        self.ready = false;
        let was = self.spectrum;
        let old_limiter = self.limiter_bands;
        self.amp_res_header = reader.bit() == 1;
        let mut spectrum = Spectrum {
            start_freq: reader.bits(4) as u8,
            stop_freq: reader.bits(4) as u8,
            crossover: reader.bits(3) as usize,
            freq_scale: 2,
            alter_scale: true,
            noise_bands: 2,
        };
        reader.skip(2);
        let extra_one = reader.bit() == 1;
        let extra_two = reader.bit() == 1;
        if extra_one {
            spectrum.freq_scale = reader.bits(2) as u8;
            spectrum.alter_scale = reader.bit() == 1;
            spectrum.noise_bands = reader.bits(2) as u8;
        }
        self.spectrum = spectrum;
        self.reset = spectrum != was;
        if extra_two {
            self.limiter_bands = reader.bits(2) as u8;
            self.limiter_gains = reader.bits(2) as u8;
            self.interpolate = reader.bit() == 1;
            // `bs_smoothing_mode` is the wrong way round: one means not smoothed.
            self.smoothing = reader.bit() == 0;
        } else {
            self.limiter_bands = 2;
            self.limiter_gains = 2;
            self.interpolate = true;
            self.smoothing = false;
        }
        if self.limiter_bands != old_limiter && !self.reset {
            self.limits();
        }
    }

    fn data(&mut self, reader: &mut Reader<'_>, pair: bool) -> Result<(), Error> {
        self.ready = true;
        if pair {
            if reader.bit() == 1 {
                reader.skip(8);
            }
            self.coupling = reader.bit() == 1;
            if self.coupling {
                self.grid(reader, 0)?;
                self.copy_grid();
                self.deltas(reader, 0);
                self.deltas(reader, 1);
                self.inverse_filtering(reader, 0);
                // A coupled pair shares the first's modes, but the second keeps its own previous
                // modes (the chirp factor is smoothed against them).
                let modes = self.channels[0].invf[0];
                let channel = &mut self.channels[1];
                channel.invf[1] = channel.invf[0];
                channel.invf[0] = modes;
                self.envelopes(reader, 0)?;
                self.noise(reader, 0)?;
                self.envelopes(reader, 1)?;
                self.noise(reader, 1)?;
            } else {
                self.grid(reader, 0)?;
                self.grid(reader, 1)?;
                self.deltas(reader, 0);
                self.deltas(reader, 1);
                self.inverse_filtering(reader, 0);
                self.inverse_filtering(reader, 1);
                self.envelopes(reader, 0)?;
                self.envelopes(reader, 1)?;
                self.noise(reader, 0)?;
                self.noise(reader, 1)?;
            }
        } else {
            if reader.bit() == 1 {
                reader.skip(4);
            }
            self.coupling = false;
            self.grid(reader, 0)?;
            self.deltas(reader, 0);
            self.inverse_filtering(reader, 0);
            self.envelopes(reader, 0)?;
            self.noise(reader, 0)?;
        }
        for at in 0..self.channels.len() {
            let count = self.n[1];
            let channel = &mut self.channels[at];
            channel.add_harmonic = [false; MOST];
            channel.add_harmonic_flag = reader.bit() == 1;
            if channel.add_harmonic_flag {
                for band in 0..count.min(MOST) {
                    channel.add_harmonic[band] = reader.bit() == 1;
                }
            }
        }
        if reader.bit() == 1 {
            let mut left = reader.bits(4) as usize;
            if left == 15 {
                left += reader.bits(8) as usize;
            }
            let mut left = left * 8;
            while left > 7 {
                left -= 2;
                if reader.bits(2) == PARAMETRIC_STEREO {
                    self.saw_parametric_stereo = true;
                }
                // Extensions (parametric stereo included) are padding to this decoder.
                reader.skip(left);
                left = 0;
            }
            reader.skip(left);
        }
        Ok(())
    }

    fn grid(&mut self, reader: &mut Reader<'_>, at: usize) -> Result<(), Error> {
        let mut trail = 16usize;
        let mut pointer = 0usize;
        let channel = &mut self.channels[at];
        let old = channel.envelopes;
        channel.high_res[0] = channel.high_res[old];
        channel.border_old = channel.borders[old];
        channel.amp_res = self.amp_res_header;
        let class = Class::of_index(reader.bits(2));
        match class {
            Class::FixFix => {
                let envelopes = 1usize << reader.bits(2);
                if envelopes > 4 {
                    return Err(Error::Damaged("too many replication envelopes"));
                }
                channel.envelopes = envelopes;
                if envelopes == 1 {
                    // One envelope over the whole frame is always at the coarser resolution.
                    channel.amp_res = false;
                }
                channel.borders[0] = 0;
                channel.borders[envelopes] = trail;
                let step = (trail + envelopes / 2) / envelopes;
                for at in 0..envelopes - 1 {
                    channel.borders[at + 1] = channel.borders[at] + step;
                }
                let high = reader.bit() == 1;
                for at in 1..=envelopes {
                    channel.high_res[at] = high;
                }
            }
            Class::FixVar => {
                trail += reader.bits(2) as usize;
                let relative = reader.bits(2) as usize;
                channel.envelopes = relative + 1;
                channel.borders[0] = 0;
                channel.borders[channel.envelopes] = trail;
                for at in 0..relative {
                    let last = channel.borders[channel.envelopes - at];
                    let step = 2 * reader.bits(2) as usize + 2;
                    if step > last {
                        return Err(Error::Damaged("a replication border before the frame"));
                    }
                    channel.borders[channel.envelopes - 1 - at] = last - step;
                }
                pointer = reader.bits(pointer_bits(channel.envelopes)) as usize;
                for at in 0..channel.envelopes {
                    channel.high_res[channel.envelopes - at] = reader.bit() == 1;
                }
            }
            Class::VarFix => {
                channel.borders[0] = reader.bits(2) as usize;
                let relative = reader.bits(2) as usize;
                channel.envelopes = relative + 1;
                channel.borders[channel.envelopes] = trail;
                for at in 0..relative {
                    channel.borders[at + 1] = channel.borders[at] + 2 * reader.bits(2) as usize + 2;
                }
                pointer = reader.bits(pointer_bits(channel.envelopes)) as usize;
                for at in 1..=channel.envelopes {
                    channel.high_res[at] = reader.bit() == 1;
                }
            }
            Class::VarVar => {
                channel.borders[0] = reader.bits(2) as usize;
                trail += reader.bits(2) as usize;
                let lead = reader.bits(2) as usize;
                let relative = reader.bits(2) as usize;
                let envelopes = lead + relative + 1;
                if envelopes > ENVELOPES {
                    return Err(Error::Damaged("too many replication envelopes"));
                }
                channel.envelopes = envelopes;
                channel.borders[envelopes] = trail;
                for at in 0..lead {
                    channel.borders[at + 1] = channel.borders[at] + 2 * reader.bits(2) as usize + 2;
                }
                for at in 0..relative {
                    let last = channel.borders[envelopes - at];
                    let step = 2 * reader.bits(2) as usize + 2;
                    if step > last {
                        return Err(Error::Damaged("a replication border before the frame"));
                    }
                    channel.borders[envelopes - 1 - at] = last - step;
                }
                pointer = reader.bits(pointer_bits(envelopes)) as usize;
                for at in 1..=envelopes {
                    channel.high_res[at] = reader.bit() == 1;
                }
            }
        }
        let envelopes = channel.envelopes;
        if pointer > envelopes + 1 {
            return Err(Error::Damaged("a noise border outside the frame"));
        }
        for at in 1..=envelopes {
            if channel.borders[at - 1] >= channel.borders[at] {
                return Err(Error::Damaged("replication borders that do not advance"));
            }
        }
        if channel.borders[envelopes] > 32 {
            return Err(Error::Damaged("a replication border past the frame"));
        }

        channel.noises = usize::from(envelopes > 1) + 1;
        channel.noise_borders[0] = channel.borders[0];
        channel.noise_borders[channel.noises] = channel.borders[envelopes];
        if channel.noises > 1 {
            let index = match class {
                Class::FixFix => envelopes / 2,
                // Sum before difference: `envelopes - pointer` alone underflows.
                Class::FixVar | Class::VarVar => (envelopes + 1) - pointer.max(2),
                Class::VarFix => match pointer {
                    0 => 1,
                    1 => envelopes - 1,
                    other => other - 1,
                },
            };
            channel.noise_borders[1] = channel.borders[index.min(envelopes)];
        }

        channel.sine_envelope[0] = if channel.sine_envelope[1] == old as i32 {
            0
        } else {
            -1
        };
        channel.sine_envelope[1] = -1;
        if class.trailing() && pointer > 0 {
            channel.sine_envelope[1] = envelopes as i32 + 1 - pointer as i32;
        } else if class == Class::VarFix && pointer > 1 {
            channel.sine_envelope[1] = pointer as i32 - 1;
        }
        Ok(())
    }

    /// A coupled pair shares one grid, except the parts that are previous-frame state.
    fn copy_grid(&mut self) {
        let (from, to) = self.channels.split_at_mut(1);
        let (from, to) = (&from[0], &mut to[0]);
        to.high_res[0] = to.high_res[to.envelopes];
        to.border_old = to.borders[to.envelopes];
        to.sine_envelope[0] = if to.sine_envelope[1] == to.envelopes as i32 {
            0
        } else {
            -1
        };
        to.high_res[1..].copy_from_slice(&from.high_res[1..]);
        to.borders = from.borders;
        to.noise_borders = from.noise_borders;
        to.envelopes = from.envelopes;
        to.amp_res = from.amp_res;
        to.noises = from.noises;
        to.sine_envelope[1] = from.sine_envelope[1];
    }

    fn deltas(&mut self, reader: &mut Reader<'_>, at: usize) {
        let channel = &mut self.channels[at];
        for index in 0..channel.envelopes.min(ENVELOPES) {
            channel.time_delta_env[index] = reader.bit() == 1;
        }
        for index in 0..channel.noises.min(2) {
            channel.time_delta_noise[index] = reader.bit() == 1;
        }
    }

    fn inverse_filtering(&mut self, reader: &mut Reader<'_>, at: usize) {
        let bands = self.n_noise;
        let channel = &mut self.channels[at];
        channel.invf[1] = channel.invf[0];
        for band in 0..bands.min(5) {
            channel.invf[0][band] = reader.bits(2) as u8;
        }
    }

    fn envelopes(&mut self, reader: &mut Reader<'_>, at: usize) -> Result<(), Error> {
        // A coupled pair's second channel carries a balance, with steps twice as coarse.
        let coupled = self.coupling && at == 1;
        let step = if coupled { 2 } else { 1 };
        let odd = self.n[1] & 1;
        let n = self.n;
        let channel = &mut self.channels[at];
        let (time, freq, width) = match (coupled, channel.amp_res) {
            (true, true) => (&tables::T_ENV_BALANCE_3_0, &tables::F_ENV_BALANCE_3_0, 5),
            (true, false) => (&tables::T_ENV_BALANCE_1_5, &tables::F_ENV_BALANCE_1_5, 6),
            (false, true) => (&tables::T_ENV_3_0, &tables::F_ENV_3_0, 6),
            (false, false) => (&tables::T_ENV_1_5, &tables::F_ENV_1_5, 7),
        };
        let time_lav = match (coupled, channel.amp_res) {
            (true, true) => tables::T_ENV_BALANCE_3_0_LAV,
            (true, false) => tables::T_ENV_BALANCE_1_5_LAV,
            (false, true) => tables::T_ENV_3_0_LAV,
            (false, false) => tables::T_ENV_1_5_LAV,
        };
        let freq_lav = match (coupled, channel.amp_res) {
            (true, true) => tables::F_ENV_BALANCE_3_0_LAV,
            (true, false) => tables::F_ENV_BALANCE_1_5_LAV,
            (false, true) => tables::F_ENV_3_0_LAV,
            (false, false) => tables::F_ENV_1_5_LAV,
        };
        for index in 0..channel.envelopes {
            let high = channel.high_res[index + 1];
            let count = n[usize::from(high)].min(MOST);
            if channel.time_delta_env[index] {
                for band in 0..count {
                    // Previous envelope may differ in resolution: delta from the covering band.
                    let from = if high == channel.high_res[index] {
                        band
                    } else if high {
                        (band + odd) >> 1
                    } else if band == 0 {
                        0
                    } else {
                        2 * band - odd
                    };
                    let delta = i32::from(time.read(reader)) - time_lav;
                    let value = channel.envelope_q[index][from.min(MOST - 1)] + step * delta;
                    if !(0..=127).contains(&value) {
                        return Err(Error::Damaged("an envelope scalefactor out of range"));
                    }
                    channel.envelope_q[index + 1][band] = value;
                }
            } else {
                let mut value = step * reader.bits(width) as i32;
                channel.envelope_q[index + 1][0] = value;
                for band in 1..count {
                    let delta = i32::from(freq.read(reader)) - freq_lav;
                    value += step * delta;
                    if !(0..=127).contains(&value) {
                        return Err(Error::Damaged("an envelope scalefactor out of range"));
                    }
                    channel.envelope_q[index + 1][band] = value;
                }
            }
        }
        // The last envelope becomes the nought-th for the next frame's time delta.
        channel.envelope_q[0] = channel.envelope_q[channel.envelopes];
        Ok(())
    }

    fn noise(&mut self, reader: &mut Reader<'_>, at: usize) -> Result<(), Error> {
        let coupled = self.coupling && at == 1;
        let step = if coupled { 2 } else { 1 };
        let bands = self.n_noise.min(5);
        let channel = &mut self.channels[at];
        // Noise frequency deltas borrow the envelope's 3 dB books.
        let (time, time_lav) = if coupled {
            (
                &tables::T_NOISE_BALANCE_3_0,
                tables::T_NOISE_BALANCE_3_0_LAV,
            )
        } else {
            (&tables::T_NOISE_3_0, tables::T_NOISE_3_0_LAV)
        };
        let (freq, freq_lav) = if coupled {
            (&tables::F_ENV_BALANCE_3_0, tables::F_ENV_BALANCE_3_0_LAV)
        } else {
            (&tables::F_ENV_3_0, tables::F_ENV_3_0_LAV)
        };
        for index in 0..channel.noises {
            if channel.time_delta_noise[index] {
                for band in 0..bands {
                    let delta = i32::from(time.read(reader)) - time_lav;
                    let value = channel.noise_q[index][band] + step * delta;
                    if !(0..=30).contains(&value) {
                        return Err(Error::Damaged("a noise floor scalefactor out of range"));
                    }
                    channel.noise_q[index + 1][band] = value;
                }
            } else {
                let mut value = step * reader.bits(5) as i32;
                channel.noise_q[index + 1][0] = value;
                for band in 1..bands {
                    let delta = i32::from(freq.read(reader)) - freq_lav;
                    value += step * delta;
                    if !(0..=30).contains(&value) {
                        return Err(Error::Damaged("a noise floor scalefactor out of range"));
                    }
                    channel.noise_q[index + 1][band] = value;
                }
            }
        }
        channel.noise_q[0] = channel.noise_q[channel.noises];
        Ok(())
    }
}

fn pointer_bits(envelopes: usize) -> usize {
    match envelopes {
        0 | 1 => envelopes,
        2 | 3 => 2,
        _ => 3,
    }
}

/// Geometric division of `start` to `stop` into `count` bands, as widths. Single precision on
/// purpose: every decoder must land on the same borders.
fn make_bands(out: &mut [i32], start: usize, stop: usize, count: usize) {
    if count == 0 {
        return;
    }
    let base = (stop as f32 / start as f32).powf(1.0 / count as f32);
    let mut product = start as f32;
    let mut previous = start as i32;
    for slot in out.iter_mut().take(count - 1) {
        product *= base;
        let present = product.round_ties_even() as i32;
        *slot = present - previous;
        previous = present;
    }
    out[count - 1] = stop as i32 - previous;
}

/// Two to a whole power, saturating (a damaged stream can ask for 2^134).
fn exponent(power: i32) -> f32 {
    if power > 120 {
        return 1.0;
    }
    (power as f32).exp2()
}
