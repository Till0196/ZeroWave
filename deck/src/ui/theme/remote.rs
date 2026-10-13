// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

/// デザインの幅。寸法は全部この幅でのもので、ウィンドウの幅に合わせて縮む。
pub const DESIGN_WIDTH: f32 = 300.0;
/// 種別のボタンの二行目の分（`BAND_HEIGHT` と `KEY_GAP`）を含む。
pub const DESIGN_HEIGHT: f32 = 970.0;
/// ウィンドウの大きさ（論理ピクセル）。デザインの三分の二。
pub const WIDTH: f32 = 200.0;
pub const SCALE: f32 = WIDTH / DESIGN_WIDTH;
pub const HEIGHT: f32 = DESIGN_HEIGHT * SCALE;
pub const PAD_X: f32 = 14.0;
pub const PAD_TOP: f32 = 14.0;
pub const GAP: f32 = 10.0;
pub const KEY_GAP: f32 = 6.0;
pub const ROUNDING: f32 = 8.0;
pub const KEY_HEIGHT: f32 = 40.0;
pub const KEY_SIZE: f32 = 13.0;
pub const BAND_HEIGHT: f32 = 32.0;
pub const BAND_ROUNDING: f32 = 6.0;
pub const BANDS_PER_ROW: usize = 4;
pub const BAND_ROWS: usize = 2;
pub const DIGIT_SIZE: f32 = 15.0;
/// シーソーボタン（音量・チャンネル）：幅、上下のボタン、真ん中のラベル。
pub const ROCKER_WIDTH: f32 = 80.0;
pub const ROCKER_KEY: f32 = 56.0;
pub const ROCKER_LABEL: f32 = 20.0;
pub const ROCKER_LABEL_SIZE: f32 = 10.0;
pub const ROCKER_GAP: f32 = 12.0;
pub const DISC: f32 = 170.0;
pub const ROUND_KEY: f32 = 44.0;
pub const ENTER: f32 = 64.0;
pub const ENTER_HALO: f32 = 4.0;
pub const CLUSTER_GAP: f32 = 7.0;
pub const CLUSTER_PAD: f32 = 6.0;
pub const BACK_WIDTH: f32 = 66.0;
pub const BACK_HEIGHT: f32 = 32.0;
pub const BACK_SIZE: f32 = 12.0;
pub const COLOUR_HEIGHT: f32 = 26.0;
pub const COLOUR_GAP: f32 = 8.0;
pub const COLOUR_PAD_X: f32 = 6.0;
pub const COLOUR_ROUNDING: f32 = 6.0;
pub const COLOUR_SIZE: f32 = 11.0;
pub const GROUP_PAD_X: f32 = 8.0;
pub const GROUP_PAD_Y: f32 = 10.0;
pub const GROUP_ROUNDING: f32 = 12.0;
pub const ICON: f32 = 14.0;
pub const ICON_LARGE: f32 = 18.0;
pub const ICON_SCALE: f32 = 1.3;

/// 色ボタンは放送の色で、明暗で変えない。
pub const BLUE: Color32 = Color32::from_rgb(0x3b, 0x6f, 0xd6);
pub const RED: Color32 = Color32::from_rgb(0xd6, 0x45, 0x45);
pub const GREEN: Color32 = Color32::from_rgb(0x3a, 0xa5, 0x57);
pub const YELLOW: Color32 = Color32::from_rgb(0xd9, 0xb2, 0x3a);
pub const COLOUR_TEXT: Color32 = Color32::from_rgba_premultiplied(230, 230, 230, 230);
pub const RECORD: Color32 = RED;

/// 明暗で変わる色。
#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub background: Color32,
    pub key_fill: Color32,
    pub key_edge: Color32,
    pub key_hover: Color32,
    pub key_pressed: Color32,
    pub text: Color32,
    pub label: Color32,
    pub chosen_fill: Color32,
    pub chosen_edge: Color32,
    pub chosen_text: Color32,
    pub disc: Color32,
    /// 決定。押している間だけ `chosen_*`。
    pub enter_fill: Color32,
    pub enter_edge: Color32,
    pub group: Color32,
}

pub const DARK: Skin = Skin {
    background: Color32::from_rgb(0x11, 0x11, 0x13),
    key_fill: Color32::from_rgb(0x22, 0x22, 0x26),
    key_edge: Color32::from_rgb(0x22, 0x22, 0x26),
    key_hover: Color32::from_rgb(0x2c, 0x2c, 0x31),
    key_pressed: Color32::from_rgb(0x34, 0x34, 0x3a),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    label: Color32::from_rgb(0xa8, 0xa8, 0xad),
    chosen_fill: Color32::from_rgb(0x24, 0x24, 0x28),
    chosen_edge: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    chosen_text: Color32::WHITE,
    disc: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    enter_fill: Color32::from_rgba_premultiplied(26, 26, 29, 29),
    enter_edge: Color32::from_rgba_premultiplied(56, 56, 61, 61),
    group: Color32::from_rgba_premultiplied(8, 8, 9, 9),
};

pub const LIGHT: Skin = Skin {
    background: Color32::from_rgb(0xf7, 0xf7, 0xf9),
    key_fill: Color32::from_rgba_premultiplied(0, 0, 0, 10),
    key_edge: Color32::from_rgba_premultiplied(0, 0, 0, 36),
    key_hover: Color32::from_rgba_premultiplied(0, 0, 0, 20),
    key_pressed: Color32::from_rgba_premultiplied(0, 0, 0, 36),
    text: Color32::from_rgb(0x24, 0x28, 0x34),
    label: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    chosen_fill: Color32::from_rgba_premultiplied(0x18, 0x2e, 0x60, 0x1e),
    chosen_edge: Color32::from_rgb(0x2e, 0x62, 0xd0),
    chosen_text: Color32::from_rgb(0x1e, 0x4c, 0xb8),
    disc: Color32::from_rgba_premultiplied(0, 0, 0, 9),
    enter_fill: Color32::from_rgba_premultiplied(0, 0, 0, 15),
    enter_edge: Color32::from_rgba_premultiplied(0, 0, 0, 66),
    group: Color32::from_rgba_premultiplied(0, 0, 0, 6),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
