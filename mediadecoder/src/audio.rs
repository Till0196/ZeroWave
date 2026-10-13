// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Sound: its framings (ADTS in TS, LATM/LOAS in MMT) and decoding it. One-segment is HE-AAC,
//! the one case where [`Sound::rate`] is not [`Format::rate`].

pub mod aac;
pub mod adts;
pub mod aiff;
pub mod ancillary;
pub mod config;
pub mod latm;
pub mod mpeg;

/// Which coding tool set (MPEG-4 audio object type) a stream says it uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Object {
    Main,
    #[default]
    Lc,
    Ssr,
    Ltp,
    /// HE-AAC: AAC-LC with spectral band replication ([`aac::sbr`]).
    Sbr,
    /// HE-AAC v2: parametric stereo is not decoded.
    Ps,
    /// MPEG-1/2 audio layer I and II (object types 32 and 33).
    Layer1,
    Layer2,
    Other(u8),
}

impl Object {
    pub fn of_index(index: u8) -> Self {
        match index {
            1 => Self::Main,
            2 => Self::Lc,
            3 => Self::Ssr,
            4 => Self::Ltp,
            5 => Self::Sbr,
            29 => Self::Ps,
            32 => Self::Layer1,
            33 => Self::Layer2,
            other => Self::Other(other),
        }
    }

    pub fn index(self) -> u8 {
        match self {
            Self::Main => 1,
            Self::Lc => 2,
            Self::Ssr => 3,
            Self::Ltp => 4,
            Self::Sbr => 5,
            Self::Ps => 29,
            Self::Layer1 => 32,
            Self::Layer2 => 33,
            Self::Other(other) => other,
        }
    }

    /// Whether the core coder is AAC-LC. PS is not decoded, so such a stream is mono.
    pub fn has_lc_core(self) -> bool {
        matches!(self, Self::Lc | Self::Sbr | Self::Ps)
    }
}

/// The sampling frequencies a `samplingFrequencyIndex` can name (13, 14 reserved; 15 explicit).
pub const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

pub fn rate_index(rate: u32) -> Option<u8> {
    RATES.iter().position(|&r| r == rate).map(|at| at as u8)
}

/// What the framing declared about the sound it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Format {
    pub object: Object,
    /// Samples per second of the core coder. For HE-AAC this is half of [`Sound::rate`].
    pub rate: u32,
    /// The `channelConfiguration` field; nought means a program config element gives it (22.2).
    pub configuration: u8,
}

/// Where a channel is meant to be heard. Names are ITU-R BS.2051's, as ARIB STD-B32 uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Speaker {
    FrontCentre,
    FrontLeft,
    FrontRight,
    FrontLeftCentre,
    FrontRightCentre,
    SideLeft,
    SideRight,
    BackCentre,
    BackLeft,
    BackRight,
    TopCentre,
    TopFrontCentre,
    TopFrontLeft,
    TopFrontRight,
    TopSideLeft,
    TopSideRight,
    TopBackCentre,
    TopBackLeft,
    TopBackRight,
    BottomFrontCentre,
    BottomFrontLeft,
    BottomFrontRight,
    /// The low frequency effects channels (22.2 has two).
    LowFrequency,
    LowFrequencyRight,
    Unplaced,
}

/// Downmix coefficients the stream sent, as gains (PCE `matrix_mixdown_idx`, ISO/IEC 14496-3
/// 4.5.1.2.2, and `MPEG4_ancillary_data`, ARIB STD-B32 part 2 6.2.3). `None` means use the default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mixdown {
    /// The centre channel's gain into left and right, folding 5.1 to two.
    pub centre: Option<f32>,
    pub surround: Option<f32>,
    /// The LFE channel's gain into the fold. Nought means dropped.
    pub lfe: Option<f32>,
    pub pseudo_surround: bool,
    /// g1 to g6 of the 22.2 to 5.1 fold.
    pub twenty_two: Option<[f32; 6]>,
}

impl Mixdown {
    /// ISO/IEC 14496-3 AMD4 table AMD4.8: three bits, 1.5 dB a step, seven is silence.
    pub fn level(index: u8) -> f32 {
        if index >= 7 {
            return 0.0;
        }
        db(-1.5 * f32::from(index))
    }

    /// dmix_lfe_idx, dmix_l_idx: table AMD4.9, four bits.
    pub fn lfe_level(index: u8) -> f32 {
        const TABLE: [f32; 16] = [
            10.0,
            6.0,
            4.5,
            3.0,
            1.5,
            0.0,
            -1.5,
            -3.0,
            -4.5,
            -6.0,
            -10.0,
            -15.0,
            -20.0,
            -30.0,
            -40.0,
            f32::NEG_INFINITY,
        ];
        db(TABLE[usize::from(index & 15)])
    }

    /// `matrix_mixdown_idx` of a program config element: two bits.
    pub fn matrix(index: u8) -> f32 {
        match index & 3 {
            0 => std::f32::consts::FRAC_1_SQRT_2,
            1 => 0.5,
            2 => 0.5 * std::f32::consts::FRAC_1_SQRT_2,
            _ => 0.0,
        }
    }
}

fn db(decibels: f32) -> f32 {
    if decibels == f32::NEG_INFINITY {
        0.0
    } else {
        10f32.powf(decibels / 20.0)
    }
}

/// A decoded run of samples, one frame's worth, channels kept apart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sound {
    /// Samples per second of this run. For HE-AAC it is twice [`Format::rate`], and is only known
    /// once a frame is decoded, so a caller must follow this field rather than cache it.
    pub rate: u32,
    /// One vector per channel, nominally -1 to 1 but not clamped.
    pub channels: Vec<Vec<f32>>,
    pub speakers: Vec<Speaker>,
    pub mixdown: Mixdown,
}

impl Sound {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub use crate::Error;

/// What every sound decoder answers to. One raw frame in (framing removed); zero or more runs out.
pub trait Decoder {
    fn decode(&mut self, format: Format, data: &[u8]) -> Result<(), Error>;

    fn take(&mut self) -> Option<Sound>;

    /// Says the stream has ended. Nothing comes out: half a window of overlap is not sound.
    fn flush(&mut self) {}

    /// Gives up everything held, including the filterbank overlap (a change of channel).
    fn reset(&mut self);
}
