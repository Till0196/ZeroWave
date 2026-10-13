// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! マルチチャンネルの音声をデバイスのチャンネル数にダウンミックスする（しないと `Pcm::write` が無いチャンネルを捨てる）。
//! - 5.1 → 2：ARIB STD-B21 6.2.1(7)。放送の係数が優先（STD-B32 第 2 部 5.2.4(2)・6.2.3(3)）。
//! - 22.2 → 5.1：STD-B32 第 2 部 6.2.3(3)(d)。22.2 → 7.1 は規格に式が無く 5.1 の式から導く。
//! - ユーザーが設定した割り当ては式と放送の係数より優先。

use std::borrow::Cow;

use mediadecoder::audio::{Mixdown, Sound, Speaker};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FiveOne {
    #[default]
    Auto,
    Stereo,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TwentyTwo {
    /// 2ch なら 5.1 を経てステレオへ、6ch なら 5.1、8ch なら 7.1。
    #[default]
    Auto,
    Stereo,
    FiveOne,
    SevenOne,
    Off,
}

/// 22.2 の割り当ての入力チャンネルの順序：中層、上層、下層、LFE。
pub const TWENTY_TWO: [Speaker; 24] = [
    Speaker::FrontLeft,
    Speaker::FrontRight,
    Speaker::FrontCentre,
    Speaker::FrontLeftCentre,
    Speaker::FrontRightCentre,
    Speaker::SideLeft,
    Speaker::SideRight,
    Speaker::BackLeft,
    Speaker::BackRight,
    Speaker::BackCentre,
    Speaker::TopFrontLeft,
    Speaker::TopFrontRight,
    Speaker::TopFrontCentre,
    Speaker::TopCentre,
    Speaker::TopSideLeft,
    Speaker::TopSideRight,
    Speaker::TopBackLeft,
    Speaker::TopBackRight,
    Speaker::TopBackCentre,
    Speaker::BottomFrontLeft,
    Speaker::BottomFrontRight,
    Speaker::BottomFrontCentre,
    Speaker::LowFrequency,
    Speaker::LowFrequencyRight,
];

pub const SEVEN_ONE: [Speaker; 8] = [
    Speaker::FrontLeft,
    Speaker::FrontRight,
    Speaker::FrontCentre,
    Speaker::LowFrequency,
    Speaker::SideLeft,
    Speaker::SideRight,
    Speaker::BackLeft,
    Speaker::BackRight,
];

pub const FIVE_ONE: [Speaker; 6] = [
    Speaker::FrontLeft,
    Speaker::FrontRight,
    Speaker::FrontCentre,
    Speaker::LowFrequency,
    Speaker::BackLeft,
    Speaker::BackRight,
];

/// 1/1000 単位（設定の型に `Eq` が要るので浮動小数点数にしない）。
pub type Milli = i32;

fn milli(value: Milli) -> f32 {
    value as f32 / 1000.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mix {
    pub mono_to_stereo: bool,
    pub five_one: FiveOne,
    pub twenty_two: TwentyTwo,
    /// 22.2 → 7.1 の割り当て（出力 [`SEVEN_ONE`] × 入力 [`TWENTY_TWO`]）。
    pub seven_one: Option<[[Milli; 24]; 8]>,
    pub downmix: Option<[[Milli; 6]; 2]>,
    pub surround: Option<[[Milli; 6]; 6]>,
}

impl Default for Mix {
    fn default() -> Self {
        Self {
            mono_to_stereo: true,
            five_one: FiveOne::Auto,
            twenty_two: TwentyTwo::Auto,
            seven_one: None,
            downmix: None,
            surround: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    pub centre: f32,
    pub surround: f32,
    pub lfe: f32,
    /// PCE の `pseudo_surround_enable`。
    pub pseudo_surround: bool,
    /// 22.2 → 5.1。STD-B32 第 2 部の g1〜g6。
    pub g1: f32,
    pub g2: f32,
    pub g3: f32,
    pub g4: f32,
    pub g5: f32,
    pub g6: f32,
    pub side: f32,
}

const DB_0: f32 = 1.0;
const DB_MINUS_3: f32 = std::f32::consts::FRAC_1_SQRT_2;
const DB_MINUS_4_5: f32 = 0.595_662_1;
const DB_MINUS_6: f32 = 0.501_187_2;

impl Coefficients {
    pub const DEFAULT: Self = Self {
        centre: DB_MINUS_3,
        surround: DB_MINUS_3,
        lfe: 0.0,
        pseudo_surround: false,
        g1: DB_MINUS_4_5,
        g2: DB_MINUS_4_5,
        g3: DB_0,
        g4: DB_MINUS_6,
        g5: DB_MINUS_3,
        g6: DB_MINUS_3,
        side: DB_0,
    };

    pub fn with(sent: &Mixdown) -> Self {
        let g = sent.twenty_two.unwrap_or([
            Self::DEFAULT.g1,
            Self::DEFAULT.g2,
            Self::DEFAULT.g3,
            Self::DEFAULT.g4,
            Self::DEFAULT.g5,
            Self::DEFAULT.g6,
        ]);
        Self {
            centre: sent.centre.unwrap_or(Self::DEFAULT.centre),
            surround: sent.surround.unwrap_or(Self::DEFAULT.surround),
            lfe: sent.lfe.unwrap_or(0.0),
            pseudo_surround: sent.pseudo_surround,
            g1: g[0],
            g2: g[1],
            g3: g[2],
            g4: g[3],
            g5: g[4],
            g6: g[5],
            side: Self::DEFAULT.side,
        }
    }
}

pub fn seven_one_default() -> [[Milli; 24]; 8] {
    let mut out = [[0; 24]; 8];
    for (input, speaker) in TWENTY_TWO.iter().enumerate() {
        let one = Sound {
            rate: 48_000,
            channels: TWENTY_TWO
                .iter()
                .map(|placed| vec![if placed == speaker { 1.0 } else { 0.0 }])
                .collect(),
            speakers: TWENTY_TWO.to_vec(),
            mixdown: Mixdown::default(),
        };
        let folded = seven_one_of_twenty_two(&one, &Coefficients::DEFAULT);
        for (output, wanted) in SEVEN_ONE.iter().enumerate() {
            if let Some(at) = folded.speakers.iter().position(|placed| placed == wanted) {
                out[output][input] = (folded.channels[at][0] * 1000.0).round() as Milli;
            }
        }
    }
    out
}

pub fn mix<'a>(sound: &'a Sound, device_channels: usize, mix: &Mix) -> Cow<'a, Sound> {
    let coefficients = Coefficients::with(&sound.mixdown);
    match sound.channels.len() {
        1 if mix.mono_to_stereo && device_channels >= 2 => Cow::Owned(stereo_of_mono(sound)),
        6 => {
            let to_stereo = match mix.five_one {
                FiveOne::Auto => device_channels < 6,
                FiveOne::Stereo => true,
                FiveOne::Off => false,
            };
            if !is_five_one(sound) {
                Cow::Borrowed(sound)
            } else if to_stereo {
                Cow::Owned(match &mix.downmix {
                    Some(matrix) => by_matrix(sound, matrix, &FIVE_ONE, &FIVE_ONE[..2]),
                    None => stereo_of_five_one(sound, &coefficients),
                })
            } else if let Some(matrix) = &mix.surround {
                Cow::Owned(by_matrix(sound, matrix, &FIVE_ONE, &FIVE_ONE))
            } else {
                Cow::Borrowed(sound)
            }
        }
        24 => {
            let want = match mix.twenty_two {
                TwentyTwo::Auto if device_channels >= 24 => 24,
                TwentyTwo::Auto if device_channels >= 8 => 8,
                TwentyTwo::Auto if device_channels >= 6 => 6,
                TwentyTwo::Auto | TwentyTwo::Stereo => 2,
                TwentyTwo::FiveOne => 6,
                TwentyTwo::SevenOne => 8,
                TwentyTwo::Off => 24,
            };
            if want == 24 || !is_twenty_two(sound) {
                return Cow::Borrowed(sound);
            }
            if want == 8 {
                return Cow::Owned(match &mix.seven_one {
                    Some(matrix) => by_matrix(sound, matrix, &TWENTY_TWO, &SEVEN_ONE),
                    None => seven_one_of_twenty_two(sound, &coefficients),
                });
            }
            let five = five_one_of_twenty_two(sound, &coefficients);
            if want == 6 {
                Cow::Owned(five)
            } else {
                Cow::Owned(stereo_of_five_one(&five, &coefficients))
            }
        }
        _ => Cow::Borrowed(sound),
    }
}

fn of(sound: &Sound, speaker: Speaker) -> Option<&[f32]> {
    sound
        .speakers
        .iter()
        .position(|placed| *placed == speaker)
        .and_then(|at| sound.channels.get(at))
        .map(Vec::as_slice)
}

fn is_five_one(sound: &Sound) -> bool {
    use Speaker::*;
    [FrontCentre, FrontLeft, FrontRight, BackLeft, BackRight]
        .iter()
        .all(|speaker| of(sound, *speaker).is_some())
}

fn is_twenty_two(sound: &Sound) -> bool {
    use Speaker::*;
    [
        FrontCentre,
        FrontLeft,
        FrontRight,
        TopFrontCentre,
        BottomFrontCentre,
    ]
    .iter()
    .all(|speaker| of(sound, *speaker).is_some())
}

fn stereo_of_mono(sound: &Sound) -> Sound {
    Sound {
        rate: sound.rate,
        channels: vec![sound.channels[0].clone(), sound.channels[0].clone()],
        speakers: vec![Speaker::FrontLeft, Speaker::FrontRight],
        mixdown: sound.mixdown,
    }
}

fn sum(sound: &Sound, terms: &[(Speaker, f32)]) -> Vec<f32> {
    let length = sound.len();
    let mut out = vec![0.0f32; length];
    for (speaker, gain) in terms {
        if *gain == 0.0 {
            continue;
        }
        if let Some(channel) = of(sound, *speaker) {
            for (o, sample) in out.iter_mut().zip(channel) {
                *o += gain * sample;
            }
        }
    }
    out
}

/// 行が出力、列が入力。
fn by_matrix<const I: usize, const N: usize>(
    sound: &Sound,
    matrix: &[[Milli; I]; N],
    inputs: &[Speaker; I],
    outputs: &[Speaker],
) -> Sound {
    Sound {
        rate: sound.rate,
        channels: matrix
            .iter()
            .map(|row| {
                let terms: Vec<(Speaker, f32)> = inputs
                    .iter()
                    .zip(row)
                    .map(|(speaker, value)| (*speaker, milli(*value)))
                    .collect();
                sum(sound, &terms)
            })
            .collect(),
        speakers: outputs.to_vec(),
        mixdown: sound.mixdown,
    }
}

/// 5.1 → 2。擬似サラウンドは ISO/IEC 14496-3 4.5.1.2.2。
fn stereo_of_five_one(sound: &Sound, c: &Coefficients) -> Sound {
    use Speaker::*;
    let (left, right) = if c.pseudo_surround {
        (
            vec![
                (FrontLeft, 1.0),
                (FrontCentre, c.centre),
                (BackLeft, -c.surround),
                (BackRight, -c.surround),
                (LowFrequency, c.lfe),
            ],
            vec![
                (FrontRight, 1.0),
                (FrontCentre, c.centre),
                (BackLeft, c.surround),
                (BackRight, c.surround),
                (LowFrequency, c.lfe),
            ],
        )
    } else {
        (
            vec![
                (FrontLeft, 1.0),
                (FrontCentre, c.centre),
                (BackLeft, c.surround),
                (LowFrequency, c.lfe),
            ],
            vec![
                (FrontRight, 1.0),
                (FrontCentre, c.centre),
                (BackRight, c.surround),
                (LowFrequency, c.lfe),
            ],
        )
    };
    Sound {
        rate: sound.rate,
        channels: vec![sum(sound, &left), sum(sound, &right)],
        speakers: vec![FrontLeft, FrontRight],
        mixdown: sound.mixdown,
    }
}

/// 22.2 → 5.1。STD-B32 第 2 部 6.2.3(3)(d) の式そのまま。
fn five_one_of_twenty_two(sound: &Sound, c: &Coefficients) -> Sound {
    use Speaker::*;
    let (g1, g2, g3, g4, g5, g6) = (c.g1, c.g2, c.g3, c.g4, c.g5, c.g6);
    Sound {
        rate: sound.rate,
        channels: vec![
            sum(
                sound,
                &[
                    (FrontCentre, 1.0),
                    (FrontLeftCentre, g1),
                    (FrontRightCentre, g1),
                    (TopFrontCentre, g3),
                    (TopCentre, g3 * g4),
                    (BottomFrontCentre, g3),
                ],
            ),
            sum(
                sound,
                &[
                    (FrontLeft, 1.0),
                    (FrontLeftCentre, g1),
                    (SideLeft, g2),
                    (TopFrontLeft, g3),
                    (TopSideLeft, g3 * g2),
                    (BottomFrontLeft, g3),
                ],
            ),
            sum(
                sound,
                &[
                    (FrontRight, 1.0),
                    (FrontRightCentre, g1),
                    (SideRight, g2),
                    (TopFrontRight, g3),
                    (TopSideRight, g3 * g2),
                    (BottomFrontRight, g3),
                ],
            ),
            sum(
                sound,
                &[
                    (BackLeft, 1.0),
                    (BackCentre, g5),
                    (SideLeft, g2),
                    (TopBackLeft, g3),
                    (TopBackCentre, g3 * g5),
                    (TopSideLeft, g3 * g2),
                    (TopCentre, g3 * g4),
                ],
            ),
            sum(
                sound,
                &[
                    (BackRight, 1.0),
                    (BackCentre, g5),
                    (SideRight, g2),
                    (TopBackRight, g3),
                    (TopBackCentre, g3 * g5),
                    (TopSideRight, g3 * g2),
                    (TopCentre, g3 * g4),
                ],
            ),
            sum(sound, &[(LowFrequency, g6), (LowFrequencyRight, g6)]),
        ],
        speakers: vec![
            FrontCentre,
            FrontLeft,
            FrontRight,
            BackLeft,
            BackRight,
            LowFrequency,
        ],
        mixdown: sound.mixdown,
    }
}

/// 22.2 → 7.1。5.1 の式で Ls/Rs に寄せていたものを側方と後方に分け、g2 は使わない。
fn seven_one_of_twenty_two(sound: &Sound, c: &Coefficients) -> Sound {
    use Speaker::*;
    let (g1, g3, g4, g5, g6, side) = (c.g1, c.g3, c.g4, c.g5, c.g6, c.side);
    let back = |outer: Speaker, top: Speaker| {
        vec![
            (outer, 1.0),
            (BackCentre, g5),
            (top, g3),
            (TopBackCentre, g3 * g5),
            (TopCentre, g3 * g4),
        ]
    };
    Sound {
        rate: sound.rate,
        channels: vec![
            sum(
                sound,
                &[
                    (FrontLeft, 1.0),
                    (FrontLeftCentre, g1),
                    (TopFrontLeft, g3),
                    (BottomFrontLeft, g3),
                ],
            ),
            sum(
                sound,
                &[
                    (FrontRight, 1.0),
                    (FrontRightCentre, g1),
                    (TopFrontRight, g3),
                    (BottomFrontRight, g3),
                ],
            ),
            sum(
                sound,
                &[
                    (FrontCentre, 1.0),
                    (FrontLeftCentre, g1),
                    (FrontRightCentre, g1),
                    (TopFrontCentre, g3),
                    (TopCentre, g3 * g4),
                    (BottomFrontCentre, g3),
                ],
            ),
            sum(sound, &[(LowFrequency, g6), (LowFrequencyRight, g6)]),
            sum(sound, &back(BackLeft, TopBackLeft)),
            sum(sound, &back(BackRight, TopBackRight)),
            sum(sound, &[(SideLeft, side), (TopSideLeft, g3)]),
            sum(sound, &[(SideRight, side), (TopSideRight, g3)]),
        ],
        speakers: vec![
            FrontLeft,
            FrontRight,
            FrontCentre,
            LowFrequency,
            BackLeft,
            BackRight,
            SideLeft,
            SideRight,
        ],
        mixdown: sound.mixdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::audio::config::Layout;

    fn only(configuration: u8, loud: &[Speaker]) -> Sound {
        let speakers = Layout::of_configuration(configuration)
            .map(|layout| layout.speakers())
            .unwrap_or_default();
        let channels = speakers
            .iter()
            .map(|speaker| vec![if loud.contains(speaker) { 1.0 } else { 0.0 }; 8])
            .collect();
        Sound {
            rate: 48_000,
            channels,
            speakers,
            mixdown: Mixdown::default(),
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a_centre_voice_comes_out_of_both_sides_three_decibels_down() {
        let sound = only(6, &[Speaker::FrontCentre]);
        let out = mix(&sound, 2, &Mix::default());
        assert_eq!(out.channels.len(), 2);
        assert_eq!(out.speakers, [Speaker::FrontLeft, Speaker::FrontRight]);
        assert!(
            near(out.channels[0][0], DB_MINUS_3),
            "{}",
            out.channels[0][0]
        );
        assert!(near(out.channels[1][0], DB_MINUS_3));
    }

    #[test]
    fn the_left_surround_goes_left_only_and_the_lfe_is_dropped() {
        let sound = only(6, &[Speaker::BackLeft, Speaker::LowFrequency]);
        let out = mix(&sound, 2, &Mix::default());
        assert!(near(out.channels[0][0], DB_MINUS_3));
        assert!(near(out.channels[1][0], 0.0));
    }

    #[test]
    fn the_coefficients_the_broadcast_sent_win() {
        let mut sound = only(6, &[Speaker::FrontCentre, Speaker::LowFrequency]);
        sound.mixdown = Mixdown {
            centre: Some(0.5),
            surround: Some(0.0),
            lfe: Some(0.25),
            ..Mixdown::default()
        };
        let out = mix(&sound, 2, &Mix::default());
        assert!(near(out.channels[0][0], 0.75), "{}", out.channels[0][0]);
        let mut sound = only(6, &[Speaker::BackLeft]);
        sound.mixdown.pseudo_surround = true;
        let out = mix(&sound, 2, &Mix::default());
        assert!(near(out.channels[0][0], -DB_MINUS_3));
        assert!(near(out.channels[1][0], DB_MINUS_3));
        let mut sound = only(13, &[Speaker::TopFrontCentre]);
        sound.mixdown.twenty_two = Some([1.0, 1.0, 0.5, 1.0, 1.0, 1.0]);
        let to = Mix {
            twenty_two: TwentyTwo::FiveOne,
            ..Mix::default()
        };
        let out = mix(&sound, 2, &to);
        assert!(near(out.channels[0][0], 0.5));
    }

    fn seven(loud: Speaker, mix_with: &Mix, sent: Option<[f32; 6]>) -> Vec<f32> {
        let mut sound = only(13, &[loud]);
        sound.mixdown.twenty_two = sent;
        let out = mix(&sound, 8, mix_with);
        assert_eq!(
            out.speakers,
            [
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCentre,
                Speaker::LowFrequency,
                Speaker::BackLeft,
                Speaker::BackRight,
                Speaker::SideLeft,
                Speaker::SideRight,
            ]
        );
        out.channels.iter().map(|channel| channel[0]).collect()
    }

    #[test]
    fn an_eight_channel_device_folds_twenty_two_to_seven_one() {
        let auto = Mix::default();
        let (g4, g5) = (DB_MINUS_6, DB_MINUS_3);
        let expect = |got: Vec<f32>, want: [f32; 8]| {
            for (at, (got, want)) in got.iter().zip(want).enumerate() {
                assert!(near(*got, want), "{at}: {got} ≠ {want}");
            }
        };
        expect(
            seven(Speaker::SideLeft, &auto, None),
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        );
        expect(
            seven(Speaker::BackCentre, &auto, None),
            [0.0, 0.0, 0.0, 0.0, g5, g5, 0.0, 0.0],
        );
        expect(
            seven(Speaker::TopCentre, &auto, None),
            [0.0, 0.0, g4, 0.0, g4, g4, 0.0, 0.0],
        );
        expect(
            seven(Speaker::FrontLeftCentre, &auto, None),
            [DB_MINUS_4_5, 0.0, DB_MINUS_4_5, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        assert_eq!(
            mix(&only(13, &[Speaker::FrontCentre]), 6, &auto)
                .channels
                .len(),
            6
        );
    }

    #[test]
    fn a_seven_one_matrix_written_by_hand_replaces_the_formula() {
        let table = seven_one_default();
        assert_eq!(table[0][3], 596);
        assert_eq!(table[2][3], 596);
        assert_eq!(table[6][9], 707);
        assert_eq!(table[7][9], 707);
        for speaker in TWENTY_TWO {
            let sound = only(13, &[speaker]);
            let by_formula = mix(&sound, 8, &Mix::default());
            let by_table = mix(
                &sound,
                8,
                &Mix {
                    seven_one: Some(table),
                    ..Mix::default()
                },
            );
            for wanted in SEVEN_ONE {
                let at = |out: &Sound| {
                    out.speakers
                        .iter()
                        .position(|placed| *placed == wanted)
                        .map(|at| out.channels[at][0])
                };
                let (a, b) = (at(&by_formula), at(&by_table));
                assert!(
                    (a.unwrap_or(0.0) - b.unwrap_or(0.0)).abs() < 0.0006,
                    "{speaker:?} → {wanted:?}: {a:?} と {b:?}"
                );
            }
        }
        let mut only_side = [[0; 24]; 8];
        only_side[4][9] = 1000;
        let mut sound = only(13, &[Speaker::BackCentre]);
        sound.mixdown.twenty_two = Some([1.0; 6]);
        let out = mix(
            &sound,
            8,
            &Mix {
                seven_one: Some(only_side),
                ..Mix::default()
            },
        );
        assert_eq!(out.speakers, SEVEN_ONE);
        assert!(near(out.channels[4][0], 1.0));
        assert!(near(out.channels[6][0], 0.0));
    }

    #[test]
    fn a_matrix_written_by_hand_replaces_the_formula() {
        let mut sound = only(6, &[Speaker::LowFrequency]);
        sound.mixdown.lfe = Some(1.0);
        let down = Mix {
            downmix: Some([[0, 0, 0, 500, 0, 0], [0; 6]]),
            ..Mix::default()
        };
        let out = mix(&sound, 2, &down);
        assert_eq!(out.speakers, [Speaker::FrontLeft, Speaker::FrontRight]);
        assert!(near(out.channels[0][0], 0.5));
        assert!(near(out.channels[1][0], 0.0));
        let mut swap = [[0; 6]; 6];
        for (at, row) in swap.iter_mut().enumerate() {
            row[at] = 1000;
        }
        swap[4] = [0, 0, 0, 0, 0, 1000];
        swap[5] = [0, 0, 0, 0, 1000, 0];
        let surround = Mix {
            surround: Some(swap),
            ..Mix::default()
        };
        let back = only(6, &[Speaker::BackLeft]);
        let out = mix(&back, 6, &surround);
        assert_eq!(out.speakers, FIVE_ONE);
        assert!(near(out.channels[4][0], 0.0));
        assert!(near(out.channels[5][0], 1.0));
    }

    #[test]
    fn a_six_channel_device_takes_five_one_as_it_is() {
        let sound = only(6, &[Speaker::FrontCentre]);
        let out = mix(&sound, 6, &Mix::default());
        assert!(matches!(out, Cow::Borrowed(_)));
        let forced = Mix {
            five_one: FiveOne::Stereo,
            ..Mix::default()
        };
        assert_eq!(mix(&sound, 6, &forced).channels.len(), 2);
        let off = Mix {
            five_one: FiveOne::Off,
            ..Mix::default()
        };
        assert!(matches!(mix(&sound, 2, &off), Cow::Borrowed(_)));
    }

    #[test]
    fn mono_goes_to_both_sides_unless_told_not_to() {
        let sound = Sound {
            rate: 48_000,
            channels: vec![vec![0.5; 4]],
            speakers: vec![Speaker::FrontCentre],
            mixdown: Mixdown::default(),
        };
        let out = mix(&sound, 2, &Mix::default());
        assert_eq!(out.channels.len(), 2);
        assert!(near(out.channels[1][0], 0.5));
        let kept = Mix {
            mono_to_stereo: false,
            ..Mix::default()
        };
        assert!(matches!(mix(&sound, 2, &kept), Cow::Borrowed(_)));
    }

    #[test]
    fn stereo_is_not_touched() {
        let sound = only(2, &[Speaker::FrontLeft]);
        assert!(matches!(mix(&sound, 2, &Mix::default()), Cow::Borrowed(_)));
    }

    #[test]
    fn twenty_two_folds_to_five_one_by_the_standard_formula() {
        use Speaker::*;
        let c = Coefficients::DEFAULT;
        let to = Mix {
            twenty_two: TwentyTwo::FiveOne,
            ..Mix::default()
        };
        let cases: &[(Speaker, [f32; 6])] = &[
            (FrontCentre, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (FrontLeftCentre, [c.g1, c.g1, 0.0, 0.0, 0.0, 0.0]),
            (TopFrontCentre, [c.g3, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (
                TopCentre,
                [c.g3 * c.g4, 0.0, 0.0, c.g3 * c.g4, c.g3 * c.g4, 0.0],
            ),
            (BottomFrontCentre, [c.g3, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (SideLeft, [0.0, c.g2, 0.0, c.g2, 0.0, 0.0]),
            (TopSideRight, [0.0, 0.0, c.g3 * c.g2, 0.0, c.g3 * c.g2, 0.0]),
            (BackCentre, [0.0, 0.0, 0.0, c.g5, c.g5, 0.0]),
            (
                TopBackCentre,
                [0.0, 0.0, 0.0, c.g3 * c.g5, c.g3 * c.g5, 0.0],
            ),
            (LowFrequency, [0.0, 0.0, 0.0, 0.0, 0.0, c.g6]),
            (LowFrequencyRight, [0.0, 0.0, 0.0, 0.0, 0.0, c.g6]),
        ];
        for (speaker, wanted) in cases {
            let sound = only(13, &[*speaker]);
            assert_eq!(sound.channels.len(), 24);
            let out = mix(&sound, 2, &to);
            assert_eq!(out.channels.len(), 6);
            for (at, want) in wanted.iter().enumerate() {
                assert!(
                    near(out.channels[at][0], *want),
                    "{speaker:?} at {at}: {} against {want}",
                    out.channels[at][0]
                );
            }
        }
    }

    #[test]
    fn twenty_two_reaches_stereo_by_way_of_five_one() {
        let sound = only(13, &[Speaker::TopFrontCentre]);
        let out = mix(&sound, 2, &Mix::default());
        assert_eq!(out.channels.len(), 2);
        let c = Coefficients::DEFAULT;
        assert!(near(out.channels[0][0], c.g3 * c.centre));
        assert!(near(out.channels[1][0], c.g3 * c.centre));
        let six = mix(&sound, 6, &Mix::default());
        assert_eq!(six.channels.len(), 6);
        let off = Mix {
            twenty_two: TwentyTwo::Off,
            ..Mix::default()
        };
        assert!(matches!(mix(&sound, 2, &off), Cow::Borrowed(_)));
    }
}
