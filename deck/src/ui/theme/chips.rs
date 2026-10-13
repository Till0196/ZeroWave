// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const HEIGHT: f32 = 24.0;
pub const PAD_X: f32 = 14.0;
pub const GAP: f32 = 4.0;
pub const ROUNDING: f32 = 12.0;
pub const SIZE: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub selected_fill: Color32,
    pub selected_text: Color32,
    pub text: Color32,
}

pub const DARK: Skin = Skin {
    selected_fill: Color32::from_rgb(0x2c, 0x2c, 0x31),
    selected_text: Color32::WHITE,
    text: Color32::from_rgb(0xa8, 0xa8, 0xad),
};

pub const LIGHT: Skin = Skin {
    selected_fill: Color32::from_rgba_premultiplied(20, 49, 62, 64),
    selected_text: Color32::from_rgb(0x1a, 0x6a, 0x96),
    text: Color32::from_rgb(0x4a, 0x4f, 0x5c),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
