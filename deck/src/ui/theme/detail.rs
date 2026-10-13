// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const WIDTH: f32 = 760.0;
pub const HEIGHT: f32 = 560.0;
pub const PAD: f32 = 22.0;
pub const PAD_TOP: f32 = 18.0;
pub const GAP: f32 = 12.0;

pub const TITLE_SIZE: f32 = 20.0;
pub const TITLE_LEADING: f32 = 30.0;
pub const WHEN_SIZE: f32 = 12.0;
pub const CHANNEL_SIZE: f32 = 13.0;
pub const FIELD_SIZE: f32 = 12.0;
pub const FIELD_LEADING: f32 = 20.4;
pub const SCROLL_ROOM: f32 = 10.0;
pub const FOOT_SIZE: f32 = 11.0;

pub const BOX_PAD: f32 = 14.0;
pub const BOX_ROUNDING: f32 = 4.0;

pub const KEY_BADGE: f32 = 22.0;
pub const KEY_SIZE: f32 = 9.24;

/// 「いま放送中」「次の番組」の切り替え（メニューから開いたウィンドウだけ）。
pub const TAB_HEIGHT: f32 = 24.0;
pub const TAB_SIZE: f32 = 12.0;
pub const TAB_PAD_X: f32 = 12.0;
pub const TAB_ROUNDING: f32 = 4.0;

/// 下の一行（番組情報の出どころと「閉じる」）。
pub const FOOT_HEIGHT: f32 = 32.0;
pub const CLOSE_WIDTH: f32 = 96.0;
pub const CLOSE_HEIGHT: f32 = 30.0;
pub const CLOSE_ROUNDING: f32 = 5.0;

/// 下のパネルの「番組情報」のタブ。この高さより低く、この幅より広ければ左右の二段組。
pub const INFO_STACK_HEIGHT: f32 = 300.0;
pub const INFO_TWO_MIN_WIDTH: f32 = 640.0;
pub const INFO_PAD_X: f32 = 18.0;
pub const INFO_PAD_Y: f32 = 12.0;
/// 二段組の左の段の幅（全体に対する割合と、その下限・上限）と段の間。
pub const INFO_LEFT_SHARE: f32 = 0.42;
pub const INFO_LEFT_MIN: f32 = 300.0;
pub const INFO_LEFT_MAX: f32 = 560.0;
pub const INFO_GUTTER: f32 = 24.0;
pub const INFO_LOGO_HEIGHT: f32 = 28.0;
pub const INFO_LOGO_SIZE: f32 = 11.0;
pub const INFO_CHANNEL_SIZE: f32 = 14.0;
pub const INFO_BAND_SIZE: f32 = 11.0;
pub const INFO_TITLE_SIZE: f32 = 18.0;
pub const INFO_TITLE_LEADING: f32 = 25.0;
pub const INFO_TIME_SIZE: f32 = 12.0;
pub const INFO_REMAIN_SIZE: f32 = 11.0;
pub const INFO_BAR_HEIGHT: f32 = 4.0;
pub const INFO_GENRE_SIZE: f32 = 11.0;
pub const INFO_TEXT_SIZE: f32 = 12.0;
pub const INFO_TEXT_LEADING: f32 = 19.0;
pub const INFO_HEADING_SIZE: f32 = 11.0;
pub const INFO_NEXT_HEIGHT: f32 = 24.0;
pub const INFO_NEXT_SIZE: f32 = 11.5;
pub const INFO_GAP: f32 = 7.0;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub background: Color32,
    pub edge: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub box_fill: Color32,
    pub key_fill: Color32,
    pub key_text: Color32,
    pub tab: Color32,
    pub button_edge: Color32,
    /// 下のパネルの番組情報：拡張形式の見出し、進行状況のバーとそのトラック。
    pub heading: Color32,
    pub bar: Color32,
    pub track: Color32,
}

pub const LIGHT: Skin = Skin {
    background: Color32::WHITE,
    edge: Color32::from_rgba_premultiplied(0, 0, 0, 26),
    text: Color32::from_rgb(0x1f, 0x22, 0x29),
    dim: Color32::from_rgb(0x5a, 0x5f, 0x6c),
    box_fill: Color32::from_rgba_premultiplied(0, 0, 0, 8),
    key_fill: Color32::WHITE,
    key_text: Color32::from_rgb(0x11, 0x11, 0x11),
    tab: Color32::from_rgba_premultiplied(0, 0, 0, 20),
    button_edge: Color32::from_rgba_premultiplied(0, 0, 0, 51),
    heading: Color32::from_rgb(0x3a, 0x6a, 0xd0),
    bar: Color32::from_rgb(0x3a, 0x6a, 0xd0),
    track: Color32::from_rgba_premultiplied(0, 0, 0, 24),
};

/// 番組表と並べて開くので同じ背景色にする（違うと片方が選ばれて見える）。
pub const DARK: Skin = Skin {
    background: super::epg::DARK.background,
    edge: Color32::from_rgb(0x25, 0x25, 0x29),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    dim: Color32::from_rgb(0xa8, 0xa8, 0xad),
    box_fill: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    key_fill: Color32::WHITE,
    key_text: Color32::from_rgb(0x0a, 0x0a, 0x0b),
    tab: Color32::from_rgb(0x24, 0x24, 0x28),
    button_edge: Color32::from_rgb(0x2c, 0x2c, 0x31),
    heading: Color32::from_rgb(0x8f, 0xb0, 0xf0),
    bar: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    track: Color32::from_rgb(0x25, 0x25, 0x29),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
