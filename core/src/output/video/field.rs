// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一フレームをどのフィールドとして見せ、それぞれを何と比べてフィルターに掛けるか。
//! GPU も状態も持たず、単独でテストできる。

use crate::decode::About;
use mediadecoder::es::Structure;

use super::Deinterlace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    Top,
    Bottom,
}

impl Parity {
    pub fn first_row(self) -> u32 {
        match self {
            Self::Top => 0,
            Self::Bottom => 1,
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

/// 参照のフィールドを保持する三フレームのどれから取るか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Previous,
    Current,
    Next,
}

/// 検出器（[`super::telecine`]）が見つけた周期。フラグからは来ない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cadence {
    #[default]
    None,
    /// 二つのフィールドが同じフィルムのコマ。ウィーブするだけでよい。
    Whole,
    /// この先のフィールドと前のフレームの後のフィールドが同じフィルムのコマ。
    Spanning,
    /// 自分のフィルムのコマを持たない。
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    pub field: Option<Parity>,
    /// ウィーブし直すとき、このパリティの行を `before` のフレームから取る（逆テレシネだけ）。
    pub weave: Option<Parity>,
    pub before: Source,
    pub after: Source,
    /// 表示の時刻はこれとフレームレートで決まる。
    pub at: u8,
    pub of: u8,
    /// フィールドマッチでレンダリングする（[`plan_matched`]）。wgpu のパスだけが解釈するフラグ。
    pub matched: bool,
}

pub type Plan = Vec<Output>;

/// MPEG-2 のフィールドピクチャーの `top_field_first` は 0 と定まっているので読まず、
/// 符号化の順（`Frame::structure`）で決める。
pub fn first_field(about: &About) -> Parity {
    match about.structure {
        Structure::Frame => {
            if about.top_field_first {
                Parity::Top
            } else {
                Parity::Bottom
            }
        }
        Structure::Top => Parity::Top,
        Structure::Bottom => Parity::Bottom,
    }
}

/// ウィーブなら 1 回、デインタレースなら 2 回、`repeat_first_field` なら 3 回出力する。
pub fn plan(about: &About, deinterlace: Deinterlace) -> Plan {
    if deinterlace == Deinterlace::Weave {
        return vec![Output {
            field: None,
            weave: None,
            before: Source::Current,
            after: Source::Current,
            at: 0,
            of: 1,
            matched: false,
        }];
    }
    let first = first_field(about);
    let of = if about.repeat_first_field { 3 } else { 2 };
    let mut plan = vec![
        Output {
            field: Some(first),
            weave: None,
            before: Source::Previous,
            after: Source::Current,
            at: 0,
            of,
            matched: false,
        },
        Output {
            field: Some(first.other()),
            weave: None,
            before: Source::Current,
            after: Source::Next,
            at: 1,
            of,
            matched: false,
        },
    ];
    if about.repeat_first_field {
        plan.push(Output {
            field: Some(first),
            weave: None,
            before: Source::Current,
            after: Source::Next,
            at: 2,
            of,
            matched: false,
        });
    }
    plan
}

/// 検出器の結果を受けて。2:3 のジャダーは放送のまま残る。
pub fn plan_of(about: &About, deinterlace: Deinterlace, cadence: Cadence) -> Plan {
    let woven = |weave| {
        vec![Output {
            field: None,
            weave,
            before: Source::Previous,
            after: Source::Current,
            at: 0,
            of: 1,
            matched: false,
        }]
    };
    match cadence {
        Cadence::None => plan(about, deinterlace),
        Cadence::Whole => woven(None),
        Cadence::Spanning => woven(Some(first_field(about).other())),
        Cadence::Drop => Vec::new(),
    }
}

/// 見せるのは先のフィールドで、反対の行は GPU が選ぶ。`Drop` のフレームだけ抜く。
pub fn plan_matched(about: &About, cadence: Cadence) -> Plan {
    if cadence == Cadence::Drop {
        return Vec::new();
    }
    let first = first_field(about);
    vec![Output {
        field: Some(first),
        weave: Some(first.other()),
        before: Source::Previous,
        after: Source::Current,
        at: 0,
        of: 1,
        matched: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::es::Format;

    #[test]
    fn a_matched_frame_is_one_output_and_only_a_drop_is_none() {
        let about = frame(Structure::Frame, false, false);
        for cadence in [Cadence::None, Cadence::Whole, Cadence::Spanning] {
            let plan = plan_matched(&about, cadence);
            assert_eq!(plan.len(), 1, "{cadence:?}");
            let output = plan[0];
            assert!(output.matched);
            assert_eq!(output.field, Some(Parity::Bottom));
            assert_eq!(output.weave, Some(Parity::Top));
            assert_eq!(output.before, Source::Previous);
        }
        assert!(plan_matched(&about, Cadence::Drop).is_empty());
    }

    fn frame(structure: Structure, top_field_first: bool, repeat_first_field: bool) -> About {
        About {
            tag: 0,
            format: Format::default(),
            structure,
            top_field_first,
            repeat_first_field,
            progressive: false,
        }
    }

    #[test]
    fn a_field_picture_takes_its_order_from_which_field_was_coded_first() {
        assert_eq!(
            first_field(&frame(Structure::Top, false, false)),
            Parity::Top
        );
        assert_eq!(
            first_field(&frame(Structure::Top, true, false)),
            Parity::Top
        );
        assert_eq!(
            first_field(&frame(Structure::Bottom, false, false)),
            Parity::Bottom
        );
        assert_eq!(
            first_field(&frame(Structure::Bottom, true, false)),
            Parity::Bottom
        );
    }

    #[test]
    fn a_frame_picture_takes_its_order_from_the_flag() {
        assert_eq!(
            first_field(&frame(Structure::Frame, true, false)),
            Parity::Top
        );
        assert_eq!(
            first_field(&frame(Structure::Frame, false, false)),
            Parity::Bottom
        );
    }

    #[test]
    fn the_two_fields_look_in_opposite_directions() {
        let plan = plan(&frame(Structure::Frame, true, false), Deinterlace::Bwdif);
        assert_eq!(
            plan,
            vec![
                Output {
                    field: Some(Parity::Top),
                    weave: None,
                    before: Source::Previous,
                    after: Source::Current,
                    at: 0,
                    of: 2,
                    matched: false,
                },
                Output {
                    field: Some(Parity::Bottom),
                    weave: None,
                    before: Source::Current,
                    after: Source::Next,
                    at: 1,
                    of: 2,
                    matched: false,
                },
            ]
        );
    }

    #[test]
    fn a_repeated_first_field_is_shown_a_third_time() {
        let plan = plan(&frame(Structure::Frame, true, true), Deinterlace::Yadif);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[2].field, Some(Parity::Top));
        assert!(plan.iter().all(|output| output.of == 3));
    }

    #[test]
    fn weaving_shows_the_frame_as_coded_whatever_it_was_coded_as() {
        for structure in [Structure::Frame, Structure::Top, Structure::Bottom] {
            let plan = plan(&frame(structure, false, true), Deinterlace::Weave);
            assert_eq!(plan.len(), 1);
            assert_eq!(plan[0].field, None);
        }
    }
}
