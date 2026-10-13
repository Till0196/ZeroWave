// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The syntax elements of a raw data block: windows, sections, spectrum, tools.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct Info {
    pub(super) sequence: Sequence,
    pub(super) shape: Shape,
    /// How many bands carry anything. Everything above is silence.
    pub(super) max_sfb: usize,
    /// Windows per group; they sum to eight for a short sequence and one otherwise.
    pub(super) groups: Vec<usize>,
    pub(super) bands: &'static [u16],
    pub(super) size: usize,
}

impl Default for Info {
    fn default() -> Self {
        Self {
            sequence: Sequence::OnlyLong,
            shape: Shape::Sine,
            max_sfb: 0,
            groups: vec![1],
            bands: tables::LONG_BANDS[3],
            size: LONG,
        }
    }
}

impl Info {
    pub(super) fn read(reader: &mut Reader<'_>, index: u8) -> Result<Self, Error> {
        reader.skip(1);
        let sequence = Sequence::of_index(reader.bits(2));
        let shape = Shape::of_index(reader.bit());
        let rate = usize::from(index);
        let mut info = Self {
            sequence,
            shape,
            max_sfb: 0,
            groups: vec![1],
            bands: if sequence.short() {
                tables::SHORT_BANDS[rate]
            } else {
                tables::LONG_BANDS[rate]
            },
            size: if sequence.short() { SHORT } else { LONG },
        };
        if sequence.short() {
            info.max_sfb = reader.bits(4) as usize;
            let grouping = reader.bits(7);
            for at in 0..7 {
                if grouping >> (6 - at) & 1 == 1 {
                    *info.groups.last_mut().expect("a group") += 1;
                } else {
                    info.groups.push(1);
                }
            }
        } else {
            info.max_sfb = reader.bits(6) as usize;
            if reader.bit() == 1 {
                return Err(Error::Unsupported("a profile that predicts"));
            }
        }
        if info.max_sfb + 1 > info.bands.len() {
            return Err(Error::Damaged("more bands than the table has"));
        }
        Ok(info)
    }

    pub(super) fn windows(&self) -> usize {
        if self.sequence.short() { 8 } else { 1 }
    }

    pub(super) fn group_at(&self, group: usize) -> usize {
        self.groups[..group].iter().sum::<usize>() * self.size
    }

    pub(super) fn range(&self, group: usize, band: usize) -> impl Iterator<Item = usize> + '_ {
        let begin = self.group_at(group);
        let (low, high) = (
            usize::from(self.bands[band]),
            usize::from(self.bands[band + 1]),
        );
        (0..self.groups[group])
            .flat_map(move |window| (low..high).map(move |at| begin + window * self.size + at))
    }
}

#[derive(Debug, Default)]
pub(super) struct Mask {
    /// Nought for none, one for the flags below, two for all of them.
    pub(super) present: u8,
    pub(super) used: Vec<bool>,
}

impl Mask {
    pub(super) fn at(&self, index: usize) -> bool {
        match self.present {
            1 => self.used.get(index).copied().unwrap_or(false),
            2 => true,
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Channel {
    pub(super) info: Info,
    pub(super) codebooks: Vec<u8>,
    /// One per group and band: a gain, intensity position or noise energy by codebook.
    pub(super) scalefactors: Vec<i16>,
    pub(super) spectrum: Vec<f32>,
    pub(super) tns: Tns,
}

impl Channel {
    pub(super) fn read(
        &mut self,
        reader: &mut Reader<'_>,
        index: u8,
        shared: Option<Info>,
    ) -> Result<(), Error> {
        let global = reader.bits(8) as i32;
        self.info = match shared {
            Some(info) => info,
            None => Info::read(reader, index)?,
        };
        let cells = self.info.groups.len() * self.info.max_sfb;
        self.codebooks = vec![ZERO; cells];
        self.scalefactors = vec![0; cells];
        self.spectrum = vec![0.0; LONG];
        self.sections(reader);
        self.scalefactors(reader, global);
        let pulse = reader.bit() == 1;
        let pulses = pulse.then(|| Pulse::read(reader));
        if reader.bit() == 1 {
            self.tns = Tns::read(reader, &self.info);
        } else {
            self.tns = Tns::default();
        }
        if reader.bit() == 1 {
            return Err(Error::Unsupported("gain control data"));
        }
        self.spectrum(reader, pulses.as_ref())?;
        Ok(())
    }

    pub(super) fn sections(&mut self, reader: &mut Reader<'_>) {
        let bits = if self.info.sequence.short() { 3 } else { 5 };
        let escape = (1u32 << bits) - 1;
        for group in 0..self.info.groups.len() {
            let mut band = 0;
            while band < self.info.max_sfb {
                let codebook = reader.bits(4) as u8;
                let mut run = 0usize;
                loop {
                    let step = reader.bits(bits);
                    run += step as usize;
                    if step != escape {
                        break;
                    }
                    if reader.empty() {
                        break;
                    }
                }
                let end = (band + run).min(self.info.max_sfb);
                for at in band..end {
                    self.codebooks[group * self.info.max_sfb + at] = codebook;
                }
                if run == 0 {
                    break;
                }
                band += run;
            }
        }
    }

    /// `scale_factor_data`: three kinds of delta, each from the last of its kind, so read in order.
    pub(super) fn scalefactors(&mut self, reader: &mut Reader<'_>, global: i32) {
        let mut gain = global;
        let mut position = 0i32;
        let mut energy = global - 90;
        let mut first_noise = true;
        for at in 0..self.codebooks.len() {
            let value = match self.codebooks[at] {
                ZERO => continue,
                INTENSITY | INTENSITY_INVERTED => {
                    position += difference(reader);
                    position
                }
                NOISE => {
                    if first_noise {
                        first_noise = false;
                        energy += reader.bits(9) as i32 - 256;
                    } else {
                        energy += difference(reader);
                    }
                    energy
                }
                _ => {
                    gain += difference(reader);
                    gain
                }
            };
            self.scalefactors[at] = value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
    }

    pub(super) fn spectrum(
        &mut self,
        reader: &mut Reader<'_>,
        pulses: Option<&Pulse>,
    ) -> Result<(), Error> {
        let mut quantised = vec![0i32; LONG];
        for group in 0..self.info.groups.len() {
            let begin = self.info.group_at(group);
            for band in 0..self.info.max_sfb {
                let codebook = self.codebooks[group * self.info.max_sfb + band];
                if codebook == ZERO || codebook >= NOISE {
                    continue;
                }
                let Some(book) = tables::SPECTRUM.get(usize::from(codebook) - 1) else {
                    return Err(Error::Damaged("a codebook that is not in the table"));
                };
                let (low, high) = (
                    usize::from(self.info.bands[band]),
                    usize::from(self.info.bands[band + 1]),
                );
                let pair = codebook >= 5;
                let unsigned = matches!(codebook, 3 | 4 | 7 | 8 | 9 | 10 | 11);
                let step = if pair { 2 } else { 4 };
                for window in 0..self.info.groups[group] {
                    let at = begin + window * self.info.size + low;
                    let mut offset = 0;
                    while offset < high - low {
                        let index = book.read(reader);
                        let mut values = tuple(codebook, index);
                        if unsigned {
                            for value in values.iter_mut().take(step) {
                                if *value != 0 && reader.bit() == 1 {
                                    *value = -*value;
                                }
                            }
                        }
                        if codebook == ESCAPE {
                            for value in values.iter_mut().take(step) {
                                *value = escape(reader, *value);
                            }
                        }
                        for (slot, value) in values.iter().take(step).enumerate() {
                            if let Some(cell) = quantised.get_mut(at + offset + slot) {
                                *cell = *value;
                            }
                        }
                        offset += step;
                    }
                }
            }
        }
        if let Some(pulses) = pulses {
            pulses.apply(&self.info, &mut quantised);
        }
        for group in 0..self.info.groups.len() {
            for band in 0..self.info.max_sfb {
                let at = group * self.info.max_sfb + band;
                if matches!(
                    self.codebooks[at],
                    ZERO | NOISE | INTENSITY | INTENSITY_INVERTED
                ) {
                    continue;
                }
                let gain = 2f32
                    .powf(0.25 * (f32::from(self.scalefactors[at]) - SCALEFACTOR_OFFSET))
                    / FULL_SCALE;
                for to in self.info.range(group, band) {
                    let value = quantised[to];
                    let magnitude = (value.unsigned_abs() as f32).powf(4.0 / 3.0);
                    self.spectrum[to] = magnitude.copysign(value as f32) * gain;
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self, index: u8) -> Frame {
        self.tns.apply(&self.info, index, &mut self.spectrum);
        let mut spectrum = [0.0f32; LONG];
        spectrum.copy_from_slice(&self.spectrum);
        Frame {
            spectrum,
            sequence: self.info.sequence,
            shape: self.info.shape,
        }
    }
}

pub(super) fn difference(reader: &mut Reader<'_>) -> i32 {
    i32::from(tables::SCALEFACTOR.read(reader)) - 60
}

/// The escape of codebook eleven: a count of leading ones, then that many plus four bits.
pub(super) fn escape(reader: &mut Reader<'_>, value: i32) -> i32 {
    if value.abs() != 16 {
        return value;
    }
    let mut bits = 4;
    while reader.bit() == 1 && bits < 24 {
        bits += 1;
    }
    let magnitude = (1i32 << bits) + reader.bits(bits) as i32;
    if value < 0 { -magnitude } else { magnitude }
}

/// The values a codebook index stands for (the index is the tuple in a fixed base).
pub(super) fn tuple(codebook: u8, index: u16) -> [i32; 4] {
    let (dimension, base, offset) = match codebook {
        1 | 2 => (4, 3, 1),
        3 | 4 => (4, 3, 0),
        5 | 6 => (2, 9, 4),
        7 | 8 => (2, 8, 0),
        9 | 10 => (2, 13, 0),
        _ => (2, 17, 0),
    };
    let mut values = [0i32; 4];
    let mut left = i32::from(index);
    for slot in (0..dimension).rev() {
        values[slot] = left % base - offset;
        left /= base;
    }
    values
}

#[derive(Debug)]
pub(super) struct Pulse {
    pub(super) band: usize,
    pub(super) pulses: Vec<(usize, i32)>,
}

impl Pulse {
    pub(super) fn read(reader: &mut Reader<'_>) -> Self {
        let count = reader.bits(2) as usize + 1;
        let band = reader.bits(6) as usize;
        let pulses = (0..count)
            .map(|_| (reader.bits(5) as usize, reader.bits(4) as i32))
            .collect();
        Self { band, pulses }
    }

    pub(super) fn apply(&self, info: &Info, quantised: &mut [i32]) {
        if info.sequence.short() || self.band + 1 >= info.bands.len() {
            return;
        }
        let mut at = usize::from(info.bands[self.band]);
        for &(offset, amplitude) in &self.pulses {
            at += offset;
            let Some(value) = quantised.get_mut(at) else {
                return;
            };
            *value += if *value >= 0 { amplitude } else { -amplitude };
        }
    }
}

/// Temporal noise shaping: an LPC filter run along the spectrum.
#[derive(Debug, Default)]
pub(super) struct Tns {
    pub(super) present: bool,
    /// The filters of each window, from the top band downwards.
    pub(super) windows: Vec<Vec<Filter>>,
}

#[derive(Debug)]
pub(super) struct Filter {
    pub(super) bands: usize,
    pub(super) direction: bool,
    pub(super) coefficients: Vec<f32>,
}

impl Tns {
    pub(super) fn read(reader: &mut Reader<'_>, info: &Info) -> Self {
        let short = info.sequence.short();
        let (count_bits, length_bits, order_bits) = if short { (1, 4, 3) } else { (2, 6, 5) };
        let mut windows = Vec::with_capacity(info.windows());
        for _ in 0..info.windows() {
            let count = reader.bits(count_bits) as usize;
            let resolution = if count > 0 { reader.bit() } else { 0 };
            let mut filters = Vec::with_capacity(count);
            for _ in 0..count {
                let bands = reader.bits(length_bits) as usize;
                let order = (reader.bits(order_bits) as usize).min(TNS_ORDER);
                if order == 0 {
                    filters.push(Filter {
                        bands,
                        direction: false,
                        coefficients: Vec::new(),
                    });
                    continue;
                }
                let direction = reader.bit() == 1;
                let compress = reader.bit();
                // Compression drops a bit but the quantiser step stays the resolution's.
                let resolution_bits = resolution + 3;
                let width = (resolution_bits - compress) as usize;
                let coefficients = (0..order)
                    .map(|_| reflection(reader.bits(width), width, resolution_bits as usize))
                    .collect();
                filters.push(Filter {
                    bands,
                    direction,
                    coefficients,
                });
            }
            windows.push(filters);
        }
        Self {
            present: true,
            windows,
        }
    }

    pub(super) fn apply(&self, info: &Info, index: u8, spectrum: &mut [f32]) {
        if !self.present {
            return;
        }
        let reach = usize::from(if info.sequence.short() {
            tables::TNS_SHORT_BANDS[usize::from(index)]
        } else {
            tables::TNS_LONG_BANDS[usize::from(index)]
        });
        let top = info.bands.len() - 1;
        for (window, filters) in self.windows.iter().enumerate() {
            let mut bottom = top;
            for filter in filters {
                let high = bottom;
                bottom = high.saturating_sub(filter.bands);
                if filter.coefficients.is_empty() {
                    continue;
                }
                let band = |at: usize| usize::from(info.bands[at.min(reach).min(info.max_sfb)]);
                let (begin, end) = (band(bottom), band(high));
                if end <= begin {
                    continue;
                }
                let lpc = predictor(&filter.coefficients);
                let at = window * info.size;
                let slice = &mut spectrum[at + begin..at + end];
                all_pole(slice, &lpc, filter.direction);
            }
        }
    }
}

/// One reflection coefficient, inverse quantised; the step is set by the resolution, not the width.
pub(super) fn reflection(raw: u32, width: usize, resolution: usize) -> f32 {
    let sign = 1i32 << (width - 1);
    let mut value = raw as i32;
    if value & sign != 0 {
        value -= sign << 1;
    }
    let half = f64::from(1i32 << (resolution - 1));
    let step = if value >= 0 { half - 0.5 } else { half + 0.5 };
    ((f64::from(value) / (step / (std::f64::consts::PI / 2.0))).sin()) as f32
}

/// Reflection coefficients to filter coefficients (Levinson recursion).
pub(super) fn predictor(reflections: &[f32]) -> Vec<f32> {
    let mut a = vec![0.0f32; reflections.len() + 1];
    a[0] = 1.0;
    let mut work = a.clone();
    for (step, &reflection) in reflections.iter().enumerate() {
        let order = step + 1;
        work[..order].copy_from_slice(&a[..order]);
        for at in 1..order {
            work[at] = a[at] + reflection * a[order - at];
        }
        work[order] = reflection;
        a[..=order].copy_from_slice(&work[..=order]);
    }
    a
}

pub(super) fn all_pole(values: &mut [f32], lpc: &[f32], backwards: bool) {
    let order = lpc.len() - 1;
    let mut state = vec![0.0f32; order];
    let count = values.len();
    for step in 0..count {
        let at = if backwards { count - 1 - step } else { step };
        let mut y = values[at];
        for (delay, coefficient) in lpc[1..].iter().enumerate() {
            y -= state[delay] * coefficient;
        }
        for delay in (1..order).rev() {
            state[delay] = state[delay - 1];
        }
        if order > 0 {
            state[0] = y;
        }
        values[at] = y;
    }
}

pub(super) fn mid_side(left: &mut Channel, right: &mut Channel, mask: &Mask) {
    if mask.present == 0 {
        return;
    }
    for group in 0..left.info.groups.len() {
        for band in 0..left.info.max_sfb {
            let at = group * left.info.max_sfb + band;
            if !mask.at(at) {
                continue;
            }
            // An intensity or noise band was never mid/side, whatever the mask says.
            if matches!(
                right.codebooks.get(at),
                Some(&INTENSITY) | Some(&INTENSITY_INVERTED)
            ) || left.codebooks[at] == NOISE
            {
                continue;
            }
            for to in left.info.range(group, band) {
                let (middle, side) = (left.spectrum[to], right.spectrum[to]);
                left.spectrum[to] = middle + side;
                right.spectrum[to] = middle - side;
            }
        }
    }
}

pub(super) fn intensity(left: &Channel, right: &mut Channel, mask: &Mask) {
    for group in 0..right.info.groups.len() {
        for band in 0..right.info.max_sfb {
            let at = group * right.info.max_sfb + band;
            let direction = match right.codebooks[at] {
                INTENSITY => 1.0f32,
                INTENSITY_INVERTED => -1.0,
                _ => continue,
            };
            // In an intensity band the mid/side flag inverts the sign of the scaling.
            let inverted = if mask.present == 1 && mask.at(at) {
                -1.0
            } else {
                1.0
            };
            let scale = 0.5f32.powf(0.25 * f32::from(right.scalefactors[at]));
            for to in right.info.range(group, band) {
                right.spectrum[to] = left.spectrum[to] * scale * direction * inverted;
            }
        }
    }
}
