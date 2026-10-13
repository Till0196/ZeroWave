// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub background: Color32,
    pub edge: Color32,
    pub bar: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub selected: Color32,
    pub button: Color32,
    pub button_text: Color32,
    /// 選ばれている行。薄く敷いて、左に線を引く。
    pub chosen: Color32,
    pub chosen_text: Color32,
    pub trouble: Color32,
    pub tab_fill: Color32,
    pub tab_line: Color32,
    pub tab_text: Color32,
    pub leaf: Color32,
    pub hover: Color32,
    pub button_hover: Color32,
    pub button_hover_edge: Color32,
}

pub const DARK: Skin = Skin {
    background: Color32::from_rgb(0x11, 0x11, 0x13),
    edge: Color32::from_rgb(0x25, 0x25, 0x29),
    bar: Color32::from_rgb(0x11, 0x11, 0x13),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    dim: Color32::from_rgb(0xa8, 0xa8, 0xad),
    selected: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    button: Color32::from_rgb(0x22, 0x22, 0x26),
    button_text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    chosen: Color32::from_rgb(0x24, 0x24, 0x28),
    chosen_text: Color32::WHITE,
    trouble: super::status::DARK.trouble,
    tab_fill: Color32::TRANSPARENT,
    tab_line: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    tab_text: Color32::from_rgb(0xa8, 0xa8, 0xad),
    leaf: Color32::from_rgb(0xc4, 0xc4, 0xc8),
    hover: Color32::from_rgb(0x25, 0x25, 0x29),
    button_hover: Color32::from_rgb(0x2c, 0x2c, 0x31),
    button_hover_edge: Color32::TRANSPARENT,
};

pub const LIGHT: Skin = Skin {
    background: Color32::WHITE,
    edge: Color32::from_rgba_premultiplied(0, 0, 0, 38),
    bar: Color32::from_rgba_premultiplied(0, 0, 0, 10),
    text: Color32::from_rgb(0x1f, 0x22, 0x29),
    dim: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    selected: Color32::from_rgb(0x2a, 0x60, 0xd8),
    button: Color32::from_rgba_premultiplied(0, 0, 0, 13),
    button_text: Color32::from_rgb(0x1f, 0x22, 0x29),
    chosen: Color32::from_rgba_premultiplied(214, 227, 250, 255),
    chosen_text: Color32::from_rgb(0x11, 0x22, 0x44),
    trouble: super::status::LIGHT.trouble,
    tab_fill: Color32::from_rgb(0x2a, 0x60, 0xd8),
    tab_line: Color32::TRANSPARENT,
    tab_text: Color32::from_rgb(0x1f, 0x22, 0x29),
    leaf: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    hover: Color32::from_rgba_premultiplied(0, 0, 0, 10),
    button_hover: Color32::from_rgba_premultiplied(0, 0, 0, 13),
    button_hover_edge: Color32::from_rgb(0x2a, 0x60, 0xd8),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}

pub const WIDTH: f32 = 560.0;
pub const HEIGHT: f32 = 520.0;
pub const BAR_HEIGHT: f32 = 30.0;
pub const TEXT_SIZE: f32 = 12.0;
pub const ROW: f32 = 20.0;
pub const TREE_MARK: f32 = 10.0;
pub const LABEL: f32 = 90.0;
pub const PAD: f32 = 10.0;
/// 選んでいるタブの下線の太さと、選んだ行の左の目印の幅。
pub const TAB_LINE: f32 = 2.0;
pub const MARK: f32 = 2.0;
