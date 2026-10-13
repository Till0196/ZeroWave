// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What a stream says before decoding: the `AudioSpecificConfig` (LATM) and the program config
//! element (needed when `channelConfiguration` is nought). ADTS never uses this.

use crate::audio::{Format, Object, RATES, Speaker};
use crate::bits::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Sce,
    /// A channel pair element: two channels that may share window, mid/side and intensity.
    Cpe,
    Cce,
    /// A low frequency effects element: one channel, always a long window.
    Lfe,
    Dse,
    Pce,
    /// Fill, which also carries HE-AAC's SBR payload.
    Fil,
    End,
}

impl Element {
    pub fn of_index(index: u32) -> Self {
        match index {
            0 => Self::Sce,
            1 => Self::Cpe,
            2 => Self::Cce,
            3 => Self::Lfe,
            4 => Self::Dse,
            5 => Self::Pce,
            6 => Self::Fil,
            _ => Self::End,
        }
    }

    pub fn channels(self) -> usize {
        match self {
            Self::Sce | Self::Lfe => 1,
            Self::Cpe => 2,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub element: Element,
    /// The `element_instance_tag`; not a position, the only thing tying an element to its place.
    pub tag: u8,
    pub first: Speaker,
    pub second: Option<Speaker>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub slots: Vec<Slot>,
}

impl Layout {
    pub fn channels(&self) -> usize {
        self.slots.iter().map(|slot| slot.element.channels()).sum()
    }

    pub fn speakers(&self) -> Vec<Speaker> {
        let mut out = Vec::with_capacity(self.channels());
        for slot in &self.slots {
            out.push(slot.first);
            out.extend(slot.second);
        }
        out
    }

    /// Where the channels of this element kind and tag begin, if the layout has a place for it.
    pub fn find(&self, element: Element, tag: u8) -> Option<usize> {
        let mut at = 0;
        for slot in &self.slots {
            if slot.element == element && slot.tag == tag {
                return Some(at);
            }
            at += slot.element.channels();
        }
        None
    }

    /// The layout a `channelConfiguration` names (13 is 22.2; nought means a PCE).
    pub fn of_configuration(configuration: u8) -> Option<Self> {
        use Element::{Cpe, Lfe, Sce};
        use Speaker::{
            BackCentre, BackLeft, BackRight, BottomFrontCentre, BottomFrontLeft, BottomFrontRight,
            FrontCentre, FrontLeft, FrontLeftCentre, FrontRight, FrontRightCentre, LowFrequency,
            LowFrequencyRight, SideLeft, SideRight, TopBackCentre, TopBackLeft, TopBackRight,
            TopCentre, TopFrontCentre, TopFrontLeft, TopFrontRight, TopSideLeft, TopSideRight,
        };
        let slots: &[(Element, u8, Speaker, Option<Speaker>)] = match configuration {
            1 => &[(Sce, 0, FrontCentre, None)],
            2 => &[(Cpe, 0, FrontLeft, Some(FrontRight))],
            3 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
            ],
            4 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Sce, 1, BackCentre, None),
            ],
            5 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Cpe, 1, BackLeft, Some(BackRight)),
            ],
            6 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Cpe, 1, BackLeft, Some(BackRight)),
                (Lfe, 0, LowFrequency, None),
            ],
            7 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeftCentre, Some(FrontRightCentre)),
                (Cpe, 1, FrontLeft, Some(FrontRight)),
                (Cpe, 2, BackLeft, Some(BackRight)),
                (Lfe, 0, LowFrequency, None),
            ],
            11 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Cpe, 1, BackLeft, Some(BackRight)),
                (Sce, 1, BackCentre, None),
                (Lfe, 0, LowFrequency, None),
            ],
            12 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Cpe, 1, SideLeft, Some(SideRight)),
                (Cpe, 2, BackLeft, Some(BackRight)),
                (Lfe, 0, LowFrequency, None),
            ],
            13 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeftCentre, Some(FrontRightCentre)),
                (Cpe, 1, FrontLeft, Some(FrontRight)),
                (Cpe, 2, SideLeft, Some(SideRight)),
                (Cpe, 3, BackLeft, Some(BackRight)),
                (Sce, 1, BackCentre, None),
                (Lfe, 0, LowFrequency, None),
                (Lfe, 1, LowFrequencyRight, None),
                (Sce, 2, TopFrontCentre, None),
                (Cpe, 4, TopFrontLeft, Some(TopFrontRight)),
                (Cpe, 5, TopSideLeft, Some(TopSideRight)),
                (Sce, 3, TopCentre, None),
                (Cpe, 6, TopBackLeft, Some(TopBackRight)),
                (Sce, 4, TopBackCentre, None),
                (Sce, 5, BottomFrontCentre, None),
                (Cpe, 7, BottomFrontLeft, Some(BottomFrontRight)),
            ],
            14 => &[
                (Sce, 0, FrontCentre, None),
                (Cpe, 0, FrontLeft, Some(FrontRight)),
                (Cpe, 1, BackLeft, Some(BackRight)),
                (Lfe, 0, LowFrequency, None),
                (Cpe, 2, TopFrontLeft, Some(TopFrontRight)),
            ],
            _ => return None,
        };
        Some(Self {
            slots: slots
                .iter()
                .map(|&(element, tag, first, second)| Slot {
                    element,
                    tag,
                    first,
                    second,
                })
                .collect(),
        })
    }

    /// The layout a PCE gives. Shapes without an obvious reading stay [`Speaker::Unplaced`].
    pub fn of_program(program: &Program) -> Self {
        let mut slots = Vec::new();
        let mut place = |group: &[(Element, u8)], speakers: &mut dyn Iterator<Item = Speaker>| {
            for &(element, tag) in group {
                let first = speakers.next().unwrap_or(Speaker::Unplaced);
                let second =
                    (element == Element::Cpe).then(|| speakers.next().unwrap_or(Speaker::Unplaced));
                slots.push(Slot {
                    element,
                    tag,
                    first,
                    second,
                });
            }
        };
        place(&program.front, &mut front_speakers(&program.front));
        place(&program.side, &mut side_speakers(&program.side));
        place(&program.back, &mut back_speakers(&program.back));
        place(
            &program.lfe,
            &mut [Speaker::LowFrequency, Speaker::LowFrequencyRight]
                .into_iter()
                .chain(std::iter::repeat(Speaker::Unplaced)),
        );
        Self { slots }
    }
}

/// Front channels: one single is centre, a pair is L/R, centre + two pairs is the wide front.
fn front_speakers(front: &[(Element, u8)]) -> Box<dyn Iterator<Item = Speaker>> {
    use Speaker::{FrontCentre, FrontLeft, FrontLeftCentre, FrontRight, FrontRightCentre};
    let pairs = front
        .iter()
        .filter(|(element, _)| *element == Element::Cpe)
        .count();
    let singles = front.len() - pairs;
    match (singles, pairs) {
        (1, 0) | (1, 1) => Box::new(
            [FrontCentre, FrontLeft, FrontRight]
                .into_iter()
                .chain(unknown()),
        ),
        (0, 1) => Box::new([FrontLeft, FrontRight].into_iter().chain(unknown())),
        (1, 2) => Box::new(
            [
                FrontCentre,
                FrontLeftCentre,
                FrontRightCentre,
                FrontLeft,
                FrontRight,
            ]
            .into_iter()
            .chain(unknown()),
        ),
        _ => Box::new(unknown()),
    }
}

fn side_speakers(side: &[(Element, u8)]) -> Box<dyn Iterator<Item = Speaker>> {
    if side.len() == 1 && side[0].0 == Element::Cpe {
        Box::new(
            [Speaker::SideLeft, Speaker::SideRight]
                .into_iter()
                .chain(unknown()),
        )
    } else {
        Box::new(unknown())
    }
}

fn back_speakers(back: &[(Element, u8)]) -> Box<dyn Iterator<Item = Speaker>> {
    use Speaker::{BackCentre, BackLeft, BackRight};
    let pairs = back
        .iter()
        .filter(|(element, _)| *element == Element::Cpe)
        .count();
    match (back.len() - pairs, pairs) {
        (0, 1) => Box::new([BackLeft, BackRight].into_iter().chain(unknown())),
        (1, 0) => Box::new([BackCentre].into_iter().chain(unknown())),
        // The pair before the centre, as in the standard's five-element layouts.
        (1, 1) => Box::new(
            [BackLeft, BackRight, BackCentre]
                .into_iter()
                .chain(unknown()),
        ),
        _ => Box::new(unknown()),
    }
}

fn unknown() -> std::iter::Repeat<Speaker> {
    std::iter::repeat(Speaker::Unplaced)
}

/// A program config element, as far as it says where sound goes (only the matrix mixdown is kept).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Program {
    pub tag: u8,
    pub object: Object,
    pub rate: u32,
    pub front: Vec<(Element, u8)>,
    pub side: Vec<(Element, u8)>,
    pub back: Vec<(Element, u8)>,
    pub lfe: Vec<(Element, u8)>,
    pub mixdown: Option<(u8, bool)>,
}

impl Program {
    pub fn read(reader: &mut Reader<'_>) -> Self {
        let tag = reader.bits(4) as u8;
        // Two bits: only the four original object types.
        let object = Object::of_index(reader.bits(2) as u8 + 1);
        let rate = rate_of_index(reader.bits(4) as u8).unwrap_or(0);
        let fronts = reader.bits(4) as usize;
        let sides = reader.bits(4) as usize;
        let backs = reader.bits(4) as usize;
        let lfes = reader.bits(2) as usize;
        let associated = reader.bits(3) as usize;
        let coupling = reader.bits(4) as usize;
        for _ in 0..2 {
            if reader.bit() == 1 {
                reader.skip(4);
            }
        }
        let mixdown = (reader.bit() == 1).then(|| {
            let index = reader.bits(2) as u8;
            (index, reader.bit() == 1)
        });
        let mut group = |count: usize, paired: bool| {
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                let element = if paired && reader.bit() == 1 {
                    Element::Cpe
                } else {
                    Element::Sce
                };
                out.push((element, reader.bits(4) as u8));
            }
            out
        };
        let front = group(fronts, true);
        let side = group(sides, true);
        let back = group(backs, true);
        let lfe = group(lfes, false)
            .into_iter()
            .map(|(_, tag)| (Element::Lfe, tag))
            .collect();
        reader.skip(4 * associated);
        reader.skip(5 * coupling);
        reader.align();
        let comment = reader.bits(8) as usize;
        reader.skip(8 * comment);
        Self {
            tag,
            object,
            rate,
            front,
            side,
            back,
            lfe,
            mixdown,
        }
    }
}

pub fn rate_of_index(index: u8) -> Option<u32> {
    RATES.get(usize::from(index)).copied()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub format: Format,
    /// Whether SBR was declared (doubles the output rate).
    pub sbr: bool,
    pub ps: bool,
    pub extension_rate: Option<u32>,
    /// 960-sample frames rather than 1024 (no Japanese broadcast sets it).
    pub short_frames: bool,
    pub program: Option<Program>,
}

impl Config {
    /// Reads one and says how many bits it took (checked against LATM's length field).
    pub fn read(reader: &mut Reader<'_>) -> (Self, usize) {
        let began = reader.at();
        let mut config = Self::default();
        let mut object = read_object(reader);
        config.format.rate = read_rate(reader);
        config.format.configuration = reader.bits(4) as u8;
        // HE object types say so first and then name the core object type.
        if object == 5 || object == 29 {
            config.sbr = true;
            config.ps = object == 29;
            config.extension_rate = Some(read_rate(reader));
            object = read_object(reader);
            if object == 22 {
                reader.skip(4);
            }
        }
        config.format.object = Object::of_index(object);
        if matches!(object, 1..=4 | 6 | 7 | 17 | 19..=23) {
            config.read_general(reader, object);
        }
        (config, reader.at() - began)
    }

    fn read_general(&mut self, reader: &mut Reader<'_>, object: u8) {
        self.short_frames = reader.bit() == 1;
        if reader.bit() == 1 {
            // A delay against a core coder this decoder does not have.
            reader.skip(14);
        }
        let extension = reader.bit() == 1;
        if self.format.configuration == 0 {
            self.program = Some(Program::read(reader));
        }
        if matches!(object, 6 | 20) {
            reader.skip(3);
        }
        if extension {
            if object == 22 {
                reader.skip(5 + 11);
            }
            if matches!(object, 17 | 19 | 20 | 23) {
                reader.skip(3);
            }
            reader.skip(1);
        }
    }

    pub fn layout(&self) -> Option<Layout> {
        match &self.program {
            Some(program) => Some(Layout::of_program(program)),
            None => Layout::of_configuration(self.format.configuration),
        }
    }
}

/// An `audioObjectType`, which escapes to a wider field past five bits.
fn read_object(reader: &mut Reader<'_>) -> u8 {
    let object = reader.bits(5) as u8;
    if object == 31 {
        32 + reader.bits(6) as u8
    } else {
        object
    }
}

/// A `samplingFrequencyIndex`, which escapes to the rate itself.
fn read_rate(reader: &mut Reader<'_>) -> u32 {
    let index = reader.bits(4) as u8;
    if index == 0x0f {
        reader.bits(24)
    } else {
        rate_of_index(index).unwrap_or(0)
    }
}
