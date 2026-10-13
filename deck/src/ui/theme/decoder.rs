// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコーダーの情報のウィンドウ（`ui::panes::decoder`）の色と寸法。

use super::Color32;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub ground: Color32,
    pub card: Color32,
    /// 枠の上に浮くもの（タイル、スピーカーのマス）。
    pub raised: Color32,
    /// 動いていないラベルの背景。
    pub pill: Color32,
    pub edge: Color32,
    pub text: Color32,
    pub label: Color32,
    /// セクションの見出し、タイルの名前、鳴っていないマスの文字。
    pub dim: Color32,
    pub strong: Color32,
    /// 動いている物のラベルと、鳴っているマス。
    pub accent: Color32,
    pub accent_text: Color32,
    pub good: Color32,
    pub good_ground: Color32,
    pub bad: Color32,
    /// I・P・B のバーの色と、何も無いときの背景。
    pub bars: [Color32; 3],
    pub track: Color32,
}

pub const DARK: Skin = Skin {
    ground: Color32::from_rgb(0x0a, 0x0a, 0x0b),
    card: Color32::from_rgb(0x11, 0x11, 0x13),
    raised: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    pill: Color32::from_rgb(0x22, 0x22, 0x26),
    edge: Color32::from_rgb(0x25, 0x25, 0x29),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    label: Color32::from_rgb(0xa8, 0xa8, 0xad),
    dim: Color32::from_rgb(0x6c, 0x6c, 0x72),
    strong: Color32::WHITE,
    accent: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    accent_text: Color32::WHITE,
    good: Color32::from_rgb(0x7f, 0xc7, 0x9a),
    good_ground: Color32::from_rgb(0x21, 0x2c, 0x27),
    bad: Color32::from_rgb(0xe2, 0x78, 0x6c),
    bars: [
        Color32::from_rgb(0xd9, 0xd9, 0xdd),
        Color32::from_rgb(0x9a, 0x9a, 0x9f),
        Color32::from_rgb(0x5a, 0x5a, 0x5f),
    ],
    track: Color32::from_rgb(0x34, 0x34, 0x3a),
};

pub const LIGHT: Skin = Skin {
    ground: Color32::from_rgb(0xf3, 0xf4, 0xf6),
    card: Color32::WHITE,
    raised: Color32::from_rgb(0xf0, 0xf1, 0xf4),
    pill: Color32::from_rgb(0xe8, 0xea, 0xee),
    edge: Color32::from_rgb(0xdc, 0xde, 0xe3),
    text: Color32::from_rgb(0x1f, 0x22, 0x29),
    label: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    dim: Color32::from_rgb(0x80, 0x85, 0x90),
    strong: Color32::from_rgb(0x11, 0x14, 0x1a),
    accent: Color32::from_rgb(0x2a, 0x60, 0xd8),
    accent_text: Color32::WHITE,
    good: Color32::from_rgb(0x23, 0x7a, 0x4b),
    good_ground: Color32::from_rgb(0xe1, 0xf2, 0xe7),
    bad: Color32::from_rgb(0xc8, 0x3e, 0x32),
    bars: [
        Color32::from_rgb(0x3a, 0x3f, 0x4a),
        Color32::from_rgb(0x7a, 0x80, 0x8c),
        Color32::from_rgb(0xb8, 0xbc, 0xc4),
    ],
    track: Color32::from_rgb(0xe2, 0xe4, 0xe8),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}

/// ウィンドウの幅。これより狭ければ映像と音声を縦に積む。
pub const WIDTH: f32 = 900.0;
pub const HEIGHT: f32 = 640.0;
/// 映像と音声を左右に並べる、一つの枠の幅の下限。
pub const CARD_MIN: f32 = 330.0;
pub const GAP: f32 = 10.0;
pub const KEY: f32 = 96.0;
pub const SPEAKERS: f32 = 100.0;
