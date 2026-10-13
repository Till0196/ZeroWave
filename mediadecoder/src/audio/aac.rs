// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Decoding AAC to samples: the AAC-LC core here, spectral band replication in [`sbr`].
//! Element order is not channel order; layouts over eight channels (22.2) come only from a PCE.

pub mod filterbank;
pub mod sbr;
mod tables;

use std::collections::VecDeque;

use crate::audio::config::{Config, Element, Layout, Program};
use crate::audio::{Decoder, Error, Format, Mixdown, Sound, Speaker, rate_index};
use crate::bits::Reader;
use filterbank::{Filterbank, LONG, SHORT, Sequence, Shape};
use sbr::Sbr;

mod syntax;
#[cfg(test)]
mod tests;
use syntax::{Channel, Info, Mask, intensity, mid_side};

const ZERO: u8 = 0;
const ESCAPE: u8 = 11;
const NOISE: u8 = 13;
/// The two intensity codebooks; they differ in the sign.
const INTENSITY_INVERTED: u8 = 14;
const INTENSITY: u8 = 15;

const SCALEFACTOR_OFFSET: f32 = 100.0;

/// What the standard calls unity: its gains fill a sixteen-bit range, ours is -1 to 1.
const FULL_SCALE: f32 = 32768.0;

const TNS_ORDER: usize = 20;

/// An AAC decoder (LC with HE). Keeps overlap, so [`Decoder::reset`] between streams.
#[derive(Debug)]
pub struct Aac {
    bank: Filterbank,
    format: Format,
    layout: Option<Layout>,
    /// Where a PCE read from the stream put things; overrides the channel configuration.
    program: Option<Program>,
    /// What the stream said about folding down; kept across frames (the DSE is sparse).
    mixdown: Mixdown,
    elements: Vec<ElementState>,
    ready: VecDeque<Sound>,
    /// The noise generator's state, kept across frames (restarting would be heard as a tone).
    generator: (u32, u32),
    declared_sbr: bool,
    replicated: Option<u32>,
}

impl Default for Aac {
    fn default() -> Self {
        Self::new()
    }
}

impl Aac {
    pub fn new() -> Self {
        Self {
            bank: Filterbank::new(),
            format: Format::default(),
            layout: None,
            program: None,
            mixdown: Mixdown::default(),
            elements: Vec::new(),
            ready: VecDeque::new(),
            generator: (1, 1),
            declared_sbr: false,
            replicated: None,
        }
    }

    pub fn configure(&mut self, config: &Config) {
        self.format = config.format;
        self.program = config.program.clone();
        self.layout = config.layout();
        self.elements.clear();
        self.declared_sbr = config.sbr;
        self.replicated = None;
    }

    pub fn rate(&self) -> Option<u32> {
        self.replicated
    }

    /// Whether any element announced parametric stereo, which is not decoded (comes out mono).
    pub fn parametric_stereo(&self) -> bool {
        self.elements
            .iter()
            .any(|state| state.sbr.parametric_stereo())
    }

    /// Decodes one raw data block. `index` (sampling frequency index) picks the band tables.
    fn block(&mut self, reader: &mut Reader<'_>, index: u8) -> Result<(), Error> {
        let mut produced: Vec<(Element, u8, usize, Vec<Vec<f32>>)> = Vec::new();
        loop {
            if reader.left() < 3 {
                return Err(Error::Damaged("a block that ran out with no terminator"));
            }
            let element = Element::of_index(reader.bits(3));
            match element {
                Element::Sce | Element::Lfe => {
                    let tag = reader.bits(4) as u8;
                    let mut channel = Channel::default();
                    channel.read(reader, index, None)?;
                    self.noise(&mut channel);
                    let mut spectrum = [channel.finish(index)];
                    let state = self.filter(element, tag, &mut spectrum);
                    produced.push((element, tag, state, samples(spectrum)));
                }
                Element::Cpe => {
                    let tag = reader.bits(4) as u8;
                    let (state, pair) = self.pair(reader, index, tag)?;
                    produced.push((element, tag, state, pair));
                }
                Element::Pce => {
                    let program = Program::read(reader);
                    if let Some((index, pseudo)) = program.mixdown {
                        let gain = Mixdown::matrix(index);
                        self.mixdown.centre = Some(gain);
                        self.mixdown.surround = Some(gain);
                        self.mixdown.pseudo_surround = pseudo;
                    }
                    if self.program.as_ref() != Some(&program) {
                        self.layout = Some(Layout::of_program(&program));
                        self.program = Some(program);
                    }
                }
                Element::Dse => {
                    reader.skip(4);
                    let aligned = reader.bit() == 1;
                    let mut count = reader.bits(8) as usize;
                    if count == 255 {
                        count += reader.bits(8) as usize;
                    }
                    if aligned {
                        reader.align();
                    }
                    let mut payload = Vec::with_capacity(count);
                    for _ in 0..count {
                        payload.push(reader.bits(8) as u8);
                    }
                    if let Some(sent) = super::ancillary::read(&payload) {
                        self.mixdown = sent;
                    }
                }
                Element::Fil => {
                    // Fifteen escapes to the value less one, so nought means fourteen.
                    let mut count = reader.bits(4) as usize;
                    if count == 15 {
                        count = 14 + reader.bits(8) as usize;
                    }
                    // HE-AAC's SBR rides in a fill payload (one-segment's only announcement of it).
                    // It belongs to the preceding element and fills the whole fill element.
                    let kind = if count > 0 && reader.left() >= 8 * count {
                        reader.clone().bits(4)
                    } else {
                        0
                    };
                    if matches!(kind, sbr::EXTENSION | sbr::EXTENSION_CRC)
                        && let Some(&(element, _, state, _)) = produced.last()
                    {
                        let mut payload = reader.clone();
                        payload.skip(4);
                        let core = self.format.rate;
                        let pair = element == Element::Cpe;
                        let sbr = &mut self.elements[state].sbr;
                        // An unreadable payload leaves the core; SBR falls back to upsampling.
                        let _ = sbr.payload(
                            &mut payload,
                            8 * count - 4,
                            kind == sbr::EXTENSION_CRC,
                            pair,
                            core,
                        );
                        self.replicated = Some(sbr.rate());
                    }
                    reader.skip(8 * count);
                }
                Element::Cce => {
                    // A CCE cannot be skipped without decoding it; no Japanese broadcast sends one.
                    return Err(Error::Unsupported("a coupling channel element"));
                }
                Element::End => break,
            }
        }
        reader.align();
        self.replicate(&mut produced);
        self.emit(produced);
        Ok(())
    }

    /// Every element runs through SBR, payload or not, so all channels come out at one rate.
    fn replicate(&mut self, produced: &mut [(Element, u8, usize, Vec<Vec<f32>>)]) {
        if self.replicated.is_none() && !self.declared_sbr {
            return;
        }
        let core = self.format.rate;
        for (_, _, state, out) in produced.iter_mut() {
            let sbr = &mut self.elements[*state].sbr;
            sbr.enable(core);
            sbr.apply(out);
        }
        self.replicated = Some(core.saturating_mul(2));
    }

    fn pair(
        &mut self,
        reader: &mut Reader<'_>,
        index: u8,
        tag: u8,
    ) -> Result<(usize, Vec<Vec<f32>>), Error> {
        let common = reader.bit() == 1;
        let mut shared = None;
        let mut mask = Mask::default();
        if common {
            let info = Info::read(reader, index)?;
            mask.present = reader.bits(2) as u8;
            if mask.present == 1 {
                mask.used = (0..info.groups.len() * info.max_sfb)
                    .map(|_| reader.bit() == 1)
                    .collect();
            }
            shared = Some(info);
        }
        let mut left = Channel::default();
        left.read(reader, index, shared.clone())?;
        let mut right = Channel::default();
        right.read(reader, index, shared)?;

        self.noise_pair(&mut left, &mut right, &mask);
        mid_side(&mut left, &mut right, &mask);
        intensity(&left, &mut right, &mask);
        let mut spectrum = [left.finish(index), right.finish(index)];
        let state = self.filter(Element::Cpe, tag, &mut spectrum);
        Ok((state, samples(spectrum)))
    }

    fn noise(&mut self, channel: &mut Channel) {
        for group in 0..channel.info.groups.len() {
            for band in 0..channel.info.max_sfb {
                self.noise_band(channel, group, band);
            }
        }
    }

    /// A band that is noise in both with mid/side on gets the same noise in both (stereo image).
    fn noise_pair(&mut self, left: &mut Channel, right: &mut Channel, mask: &Mask) {
        self.noise(left);
        if mask.present == 0 {
            self.noise(right);
            return;
        }
        for group in 0..right.info.groups.len() {
            for band in 0..right.info.max_sfb {
                let at = group * right.info.max_sfb + band;
                let both = left.codebooks.get(at) == Some(&NOISE)
                    && right.codebooks[at] == NOISE
                    && mask.at(at);
                if both {
                    let from = left.info.range(group, band);
                    let to: Vec<usize> = right.info.range(group, band).collect();
                    for (source, target) in from.zip(to) {
                        right.spectrum[target] = left.spectrum[source];
                    }
                } else {
                    self.noise_band(right, group, band);
                }
            }
        }
    }

    fn noise_band(&mut self, channel: &mut Channel, group: usize, band: usize) {
        let at = group * channel.info.max_sfb + band;
        if channel.codebooks.get(at) != Some(&NOISE) {
            return;
        }
        let scale = 2f32.powf(0.25 * f32::from(channel.scalefactors[at])) / FULL_SCALE;
        let indices: Vec<usize> = channel.info.range(group, band).collect();
        let mut energy = 0.0f32;
        for &to in &indices {
            let value = self.random();
            channel.spectrum[to] = value;
            energy += value * value;
        }
        if energy > 0.0 {
            let gain = scale / energy.sqrt();
            for &to in &indices {
                channel.spectrum[to] *= gain;
            }
        }
    }

    /// A random value between -1 and 1. Noise bands never match another decoder sample for sample.
    fn random(&mut self) -> f32 {
        self.generator.0 = self
            .generator
            .0
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.generator.1 = self
            .generator
            .1
            .wrapping_mul(1_103_515_245)
            .wrapping_add(12345);
        (self.generator.0 ^ self.generator.1) as i32 as f32 / 2_147_483_648.0
    }

    fn filter(&mut self, element: Element, tag: u8, spectrum: &mut [Frame]) -> usize {
        let at = match self
            .elements
            .iter()
            .position(|state| state.element == element && state.tag == tag)
        {
            Some(at) => at,
            None => {
                self.elements.push(ElementState {
                    element,
                    tag,
                    channels: Vec::new(),
                    sbr: Sbr::new(),
                });
                self.elements.len() - 1
            }
        };
        let state = &mut self.elements[at];
        state
            .channels
            .resize_with(spectrum.len(), ChannelState::new);
        for (frame, channel) in spectrum.iter_mut().zip(&mut state.channels) {
            let mut out = [0.0f32; LONG];
            self.bank.frame(
                &frame.spectrum,
                frame.sequence,
                frame.shape,
                channel.previous,
                &mut channel.overlap,
                &mut out,
            );
            channel.previous = frame.shape;
            frame.spectrum = out;
        }
        at
    }

    /// Puts channels in layout order. Japanese broadcasts do not use the tags implicit layouts
    /// assume (BS stereo sends tag two), so elements match by order first, then by tag.
    fn emit(&mut self, produced: Vec<(Element, u8, usize, Vec<Vec<f32>>)>) {
        if produced.is_empty() {
            return;
        }
        let layout = self.layout.clone().or_else(|| {
            let total: usize = produced.iter().map(|(_, _, _, out)| out.len()).sum();
            // A configuration layout is only believed if the block agrees on the channel count.
            Layout::of_configuration(self.format.configuration)
                .filter(|layout| layout.channels() == total)
        });
        let ordered = layout.as_ref().is_some_and(|layout| {
            layout.slots.len() == produced.len()
                && layout
                    .slots
                    .iter()
                    .zip(&produced)
                    .all(|(slot, (element, _, _, _))| slot.element == *element)
        });
        let speakers = layout.as_ref().map(Layout::speakers).unwrap_or_default();

        let mut places = Vec::with_capacity(produced.len());
        let mut end = layout.as_ref().map_or(0, Layout::channels);
        let mut ordered_at = 0;
        for (element, tag, _, out) in &produced {
            let found = if ordered {
                let place = ordered_at;
                ordered_at += out.len();
                Some(place)
            } else {
                layout
                    .as_ref()
                    .and_then(|layout| layout.find(*element, *tag))
            };
            let place = found.unwrap_or(end);
            end = end.max(place + out.len());
            places.push(place);
        }

        // A layout may name channels the block did not send; the gaps are taken out.
        let mut slots: Vec<Option<Vec<f32>>> = vec![None; end];
        for ((_, _, _, out), place) in produced.into_iter().zip(places) {
            for (offset, samples) in out.into_iter().enumerate() {
                slots[place + offset] = Some(samples);
            }
        }
        let mut channels = Vec::with_capacity(slots.len());
        let mut placed = Vec::with_capacity(slots.len());
        for (at, samples) in slots.into_iter().enumerate() {
            if let Some(samples) = samples {
                channels.push(samples);
                placed.push(speakers.get(at).copied().unwrap_or(Speaker::Unplaced));
            }
        }
        self.ready.push_back(Sound {
            rate: self.replicated.unwrap_or(self.format.rate),
            channels,
            speakers: placed,
            mixdown: self.mixdown,
        });
    }
}

impl Decoder for Aac {
    /// Not implemented: parametric stereo (mono), Main/LTP prediction and CCEs, 960-sample frames.
    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), Error> {
        if !format.object.has_lc_core() {
            return Err(Error::Unsupported("an object type other than AAC-LC"));
        }
        let Some(index) = rate_index(format.rate) else {
            return Err(Error::Damaged("a sampling rate that is not in the table"));
        };
        if self.format.rate != format.rate || self.format.configuration != format.configuration {
            // A change of rate or channel count is a new stream; the overlap does not belong.
            self.elements.clear();
            self.replicated = None;
            self.mixdown = Mixdown::default();
            if format.configuration != 0 {
                self.layout = Layout::of_configuration(format.configuration);
                self.program = None;
            }
        }
        self.format = format;
        let mut reader = Reader::new(data);
        while reader.left() >= 8 {
            self.block(&mut reader, index)?;
        }
        Ok(())
    }

    fn take(&mut self) -> Option<Sound> {
        self.ready.pop_front()
    }

    fn reset(&mut self) {
        self.elements.clear();
        self.ready.clear();
        self.generator = (1, 1);
        self.replicated = None;
    }
}

#[derive(Debug)]
struct ElementState {
    element: Element,
    tag: u8,
    channels: Vec<ChannelState>,
    sbr: Sbr,
}

#[derive(Debug)]
struct ChannelState {
    overlap: Box<[f32; LONG]>,
    previous: Shape,
}

impl ChannelState {
    fn new() -> Self {
        Self {
            overlap: Box::new([0.0; LONG]),
            previous: Shape::Sine,
        }
    }
}

fn samples<const N: usize>(frames: [Frame; N]) -> Vec<Vec<f32>> {
    frames
        .into_iter()
        .map(|frame| frame.spectrum.to_vec())
        .collect()
}

#[derive(Debug)]
struct Frame {
    spectrum: [f32; LONG],
    sequence: Sequence,
    shape: Shape,
}
