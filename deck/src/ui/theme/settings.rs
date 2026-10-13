// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const WIDTH: f32 = 780.0;
pub const HEIGHT: f32 = 560.0;
/// タイトルバーと下のボタンのバー。タイトルバーは OS が表示するので、ウィンドウに要求する高さはこの分を引く。
pub const BAR_HEIGHT: f32 = 38.0;
pub const FOOT_HEIGHT: f32 = 46.0;
pub const SIDE: f32 = 186.0;

pub const GROUP_SIZE: f32 = 10.0;
pub const ITEM_SIZE: f32 = 12.5;
pub const ITEM_HEIGHT: f32 = 34.0;
pub const ITEM_ROUNDING: f32 = 5.0;
pub const HEAD_SIZE: f32 = 15.0;
pub const PAD: f32 = 24.0;
pub const SECTION_SIZE: f32 = 11.5;
pub const ROW: f32 = 32.0;
pub const ROW_GAP: f32 = 4.0;
/// 行の先頭から変更の目印の欄までと、その幅。
pub const INSET: f32 = 16.0;
pub const DOT: f32 = 8.0;
/// ラベルの幅と欄の間。配置は「ラベル＋値」の行と左端から始まる行の二つだけ。
pub const LABEL: f32 = 150.0;
pub const GAP: f32 = 12.0;
pub const TEXT_SIZE: f32 = 12.0;
pub const SELECT_WIDTH: f32 = 280.0;
pub const FONT_BUTTON: f32 = 64.0;
pub const FILE_BUTTON: f32 = 56.0;
pub const SELECT_HEIGHT: f32 = 28.0;
pub const SELECT_ROUNDING: f32 = 4.0;
pub const SLIDER: f32 = 200.0;
pub const NUMBER: f32 = 64.0;
pub const CHECK: f32 = 13.0;
pub const MARK: f32 = 2.0;
pub const BUTTON_ROUNDING: f32 = 5.0;
pub const ICON: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub dark: Color32,
    pub body: Color32,
    pub edge: Color32,
    pub group: Color32,
    pub item: Color32,
    pub chosen_fill: Color32,
    pub chosen_text: Color32,
    pub chosen_mark: Color32,
    pub section: Color32,
    pub label: Color32,
    pub text: Color32,
    pub select_fill: Color32,
    pub select_edge: Color32,
    pub button_fill: Color32,
    pub button_edge: Color32,
    pub hover: Color32,
    /// egui の選択欄（ComboBox）の背景。ポインターを重ねた時は `combo_hover`。
    pub combo: Color32,
    pub combo_hover: Color32,
    pub tick_fill: Color32,
    pub tick_edge: Color32,
    pub tick_mark: Color32,
    pub seam: Color32,
    pub primary: Color32,
    /// 「コマンドラインで指定中」の文字と、注意。
    pub typed: Color32,
    pub ok: Color32,
    pub error: Color32,
    pub coaxd: Color32,
    pub mirakurun: Color32,
    pub changed: Color32,
    pub plugin_row: Color32,
    pub plugin_tag: Color32,
    pub plugin_tag_edge: Color32,
    pub rule: Color32,
    /// 字幕の見本の背景（明暗によらず白）と枠。
    pub sample_ground: Color32,
    pub sample_edge: Color32,
}

pub const DARK: Skin = Skin {
    dark: Color32::from_rgb(0x11, 0x11, 0x13),
    body: Color32::from_rgb(0x0a, 0x0a, 0x0b),
    edge: Color32::from_rgb(0x25, 0x25, 0x29),
    group: Color32::from_rgb(0x6c, 0x6c, 0x72),
    item: Color32::from_rgb(0xa8, 0xa8, 0xad),
    chosen_fill: Color32::from_rgb(0x24, 0x24, 0x28),
    chosen_text: Color32::WHITE,
    chosen_mark: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    section: Color32::from_rgb(0x6c, 0x6c, 0x72),
    label: Color32::from_rgb(0xa8, 0xa8, 0xad),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    select_fill: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    select_edge: Color32::from_rgb(0x25, 0x25, 0x29),
    button_fill: Color32::from_rgb(0x22, 0x22, 0x26),
    button_edge: Color32::from_rgb(0x22, 0x22, 0x26),
    hover: Color32::from_rgb(0x2c, 0x2c, 0x31),
    combo: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    combo_hover: Color32::from_rgb(0x22, 0x22, 0x26),
    tick_fill: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    tick_edge: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    tick_mark: Color32::WHITE,
    seam: Color32::from_rgb(0x25, 0x25, 0x29),
    primary: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    typed: Color32::from_rgb(0xe3, 0xb4, 0x5a),
    ok: Color32::from_rgb(0x7f, 0xc7, 0x9a),
    error: Color32::from_rgb(0xe2, 0x78, 0x6c),
    coaxd: Color32::from_rgb(0x6f, 0xd3, 0xc6),
    mirakurun: Color32::from_rgb(0xb9, 0xa9, 0xff),
    changed: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    plugin_row: Color32::from_rgba_premultiplied(10, 8, 14, 15),
    plugin_tag: Color32::from_rgb(0xc3, 0xb0, 0xff),
    plugin_tag_edge: Color32::from_rgba_premultiplied(67, 56, 96, 102),
    rule: Color32::from_rgb(0x25, 0x25, 0x29),
    sample_ground: Color32::WHITE,
    sample_edge: Color32::from_rgb(0x34, 0x34, 0x3a),
};

pub const LIGHT: Skin = Skin {
    dark: Color32::from_rgb(0xec, 0xed, 0xf1),
    body: Color32::from_rgb(0xf7, 0xf7, 0xf9),
    edge: Color32::from_rgba_premultiplied(0, 0, 0, 20),
    group: Color32::from_rgb(0x8a, 0x8e, 0x9a),
    item: Color32::from_rgb(0x3c, 0x40, 0x4c),
    // 青を薄く重ねると水色に見えるので無彩色。目印は左の青い線。
    chosen_fill: Color32::from_rgb(0xe2, 0xe4, 0xe9),
    chosen_text: Color32::from_rgb(0x1e, 0x4c, 0xb8),
    chosen_mark: Color32::from_rgb(0x2e, 0x62, 0xd0),
    section: Color32::from_rgb(0x2e, 0x62, 0xd0),
    label: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    text: Color32::from_rgb(0x24, 0x28, 0x34),
    select_fill: Color32::from_rgba_premultiplied(0, 0, 0, 6),
    select_edge: Color32::from_rgba_premultiplied(0, 0, 0, 30),
    button_fill: Color32::from_rgba_premultiplied(0, 0, 0, 8),
    button_edge: Color32::from_rgba_premultiplied(0, 0, 0, 36),
    hover: Color32::from_rgba_premultiplied(0, 0, 0, 36),
    combo: Color32::from_gray(230),
    combo_hover: Color32::from_gray(220),
    tick_fill: Color32::from_rgba_premultiplied(0, 0, 0, 6),
    tick_edge: Color32::from_rgba_premultiplied(0, 0, 0, 30),
    tick_mark: Color32::from_rgb(0x1e, 0x4c, 0xb8),
    seam: Color32::TRANSPARENT,
    primary: Color32::from_rgb(0x2e, 0x62, 0xd0),
    typed: Color32::from_rgb(0xb0, 0x74, 0x10),
    ok: Color32::from_rgb(0x1f, 0x8a, 0x5a),
    error: Color32::from_rgb(0xc8, 0x3c, 0x30),
    coaxd: Color32::from_rgb(0x1a, 0x8a, 0x7c),
    mirakurun: Color32::from_rgb(0x6c, 0x55, 0xd6),
    changed: Color32::from_rgb(0x2e, 0x62, 0xd0),
    plugin_row: Color32::from_rgba_premultiplied(6, 5, 13, 15),
    plugin_tag: Color32::from_rgb(0x6c, 0x55, 0xd6),
    plugin_tag_edge: Color32::from_rgba_premultiplied(43, 34, 86, 102),
    rule: Color32::from_rgba_premultiplied(0, 0, 0, 15),
    sample_ground: Color32::WHITE,
    sample_edge: Color32::from_rgba_premultiplied(0, 0, 0, 40),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
