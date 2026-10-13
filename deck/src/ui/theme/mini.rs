// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

/// 見出し（タブの列）28 + 種別 36 + 局 28 + 行 52.7×行の数 + 下の行 18。下に出すときは上につまみ 8。
pub const BAR_HEIGHT: f32 = 28.0;
pub const HEAD_HEIGHT: f32 = 28.0;
pub const ROW_HEIGHT: f32 = 52.7;
pub const CLOCK_WIDTH: f32 = 36.0;
/// 局一つ分の幅。ウィンドウの幅で割らず、この幅で並べる（七局で 900）。
pub const COLUMN: f32 = 123.4;

pub const TAB_ROW: f32 = 36.0;
pub const KEY_SIZE: f32 = 10.0;
pub const BAR_TITLE_SIZE: f32 = 11.5;

pub const CHANNEL_SIZE: f32 = 10.5;
pub const BADGE_WIDTH: f32 = 26.0;
pub const BADGE_HEIGHT: f32 = 13.0;
pub const BADGE_SIZE: f32 = 7.5;

pub const PAD: f32 = 5.0;
pub const MINUTE_SIZE: f32 = 9.0;
pub const TITLE_SIZE: f32 = 11.0;
pub const TITLE_LEADING: f32 = 14.3;
pub const HOUR_SIZE: f32 = 12.0;

pub const GRIP: f32 = 8.0;
pub const FOOT: f32 = 18.0;
pub const FOOT_SIZE: f32 = 10.5;
pub const STEP_SIZE: f32 = 10.0;
pub const TAB_PAD: f32 = 12.0;
pub const ICON_WIDTH: f32 = 30.0;
pub const TAG_SIZE: f32 = 9.0;
pub const MOST_ROWS: u32 = 24;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    /// パネル全体の背景と、映像との境。パネルは映像を透かさない。
    pub background: Color32,
    pub top_edge: Color32,
    pub bar: Color32,
    pub bar_edge: Color32,
    pub clock_edge: Color32,
    pub hour: Color32,
    pub row_edge: Color32,
    pub tab_row_edge: Color32,
    pub head_watching: Color32,
    pub head_watching_text: Color32,
    pub key: Color32,
    pub bar_title: Color32,
    pub bar_clock: Color32,
    pub close: Color32,
    pub channel: Color32,
    pub edge: Color32,
    pub minute: Color32,
    pub title: Color32,
    pub tab_text: Color32,
    pub tab_dim: Color32,
    pub tab_mark: Color32,
    pub tab_tag: Color32,
    pub tab_icon: Color32,
    pub grip: Color32,
    pub grip_mark: Color32,
    pub step: Color32,
    pub step_text: Color32,
    pub step_edge: Color32,
    pub foot: Color32,
    pub foot_text: Color32,
    pub foot_clock: Color32,
    pub foot_dim: Color32,
    pub watch_edge: Color32,
    pub now_hour: Color32,
    pub now_edge: Color32,
}

/// 番組の欄はジャンルの色のまま。
pub const DARK: Skin = Skin {
    background: Color32::from_rgb(0x11, 0x11, 0x13),
    top_edge: Color32::from_rgb(0x25, 0x25, 0x29),
    bar: Color32::from_rgb(0x1a, 0x1a, 0x1d),
    bar_edge: Color32::from_rgb(0x25, 0x25, 0x29),
    clock_edge: Color32::from_rgb(0xb4, 0xbc, 0xcb),
    hour: Color32::from_rgb(0xb4, 0xbc, 0xcb),
    row_edge: Color32::from_rgba_premultiplied(9, 9, 12, 64),
    tab_row_edge: Color32::from_rgb(0x25, 0x25, 0x29),
    head_watching: super::epg::DARK.head_watching,
    head_watching_text: super::epg::DARK.head_watching_text,
    key: Color32::from_rgb(0xa8, 0xa8, 0xad),
    bar_title: Color32::from_rgb(0xa8, 0xa8, 0xad),
    bar_clock: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    close: Color32::from_rgb(0x6c, 0x6c, 0x72),
    channel: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    edge: super::epg::DARK.edge,
    minute: super::epg::DARK.minute,
    title: super::epg::DARK.title,
    tab_text: Color32::from_rgb(0xff, 0xff, 0xff),
    tab_dim: Color32::from_rgb(0x9a, 0x9a, 0x9f),
    tab_mark: Color32::from_rgb(0x4a, 0x7a, 0xe0),
    tab_tag: Color32::from_rgb(0x6c, 0x6c, 0x72),
    tab_icon: Color32::from_rgb(0xbb, 0xbb, 0xc0),
    grip: Color32::from_rgb(0x0d, 0x0d, 0x0f),
    grip_mark: Color32::from_rgb(0x44, 0x44, 0x4a),
    step: Color32::from_rgb(0x1e, 0x1e, 0x22),
    step_text: Color32::from_rgb(0xdd, 0xdd, 0xe1),
    step_edge: Color32::from_rgb(0x2c, 0x2c, 0x31),
    foot: Color32::from_rgb(0x1e, 0x1e, 0x22),
    foot_text: Color32::from_rgb(0xcf, 0xcf, 0xd3),
    foot_clock: Color32::from_rgb(0xff, 0xff, 0xff),
    foot_dim: Color32::from_rgb(0x8a, 0x8a, 0x8f),
    watch_edge: Color32::from_rgba_premultiplied(20, 49, 62, 64),
    now_hour: Color32::WHITE,
    now_edge: Color32::from_rgb(0x4f, 0x97, 0xf0),
};

pub const LIGHT: Skin = Skin {
    background: Color32::from_rgb(0xec, 0xed, 0xf1),
    top_edge: Color32::from_rgb(0xd4, 0xd6, 0xde),
    bar: Color32::from_rgb(0xd8, 0xdb, 0xe3),
    bar_edge: Color32::from_rgb(0xd0, 0xd2, 0xdc),
    clock_edge: Color32::from_rgb(0xb8, 0x98, 0x3a),
    hour: Color32::from_rgb(0x8a, 0x6a, 0x10),
    row_edge: Color32::from_rgba_premultiplied(0, 0, 0, 24),
    tab_row_edge: Color32::from_rgba_premultiplied(0, 0, 0, 20),
    head_watching: super::epg::LIGHT.head_watching,
    head_watching_text: super::epg::LIGHT.head_watching_text,
    key: Color32::from_rgb(0x70, 0x74, 0x80),
    bar_title: Color32::from_rgb(0x3c, 0x40, 0x4c),
    bar_clock: Color32::from_rgb(0x24, 0x28, 0x34),
    close: Color32::from_rgb(0x9a, 0x9e, 0xaa),
    channel: Color32::from_rgb(0x20, 0x24, 0x30),
    edge: super::epg::LIGHT.edge,
    minute: super::epg::LIGHT.minute,
    title: super::epg::LIGHT.title,
    tab_text: Color32::from_rgb(0x20, 0x24, 0x30),
    tab_dim: Color32::from_rgb(0x6a, 0x6e, 0x7a),
    tab_mark: Color32::from_rgb(0x3a, 0x6a, 0xd0),
    tab_tag: Color32::from_rgb(0x8a, 0x8e, 0x9a),
    tab_icon: Color32::from_rgb(0x50, 0x54, 0x5e),
    grip: Color32::from_rgb(0xd0, 0xd3, 0xdb),
    grip_mark: Color32::from_rgb(0xa0, 0xa4, 0xae),
    step: Color32::from_rgb(0xd0, 0xd3, 0xdb),
    step_text: Color32::from_rgb(0x30, 0x34, 0x40),
    step_edge: Color32::from_rgb(0xbf, 0xc3, 0xcc),
    foot: Color32::from_rgb(0xd8, 0xdb, 0xe3),
    foot_text: Color32::from_rgb(0x30, 0x34, 0x40),
    foot_clock: Color32::from_rgb(0x10, 0x14, 0x20),
    foot_dim: Color32::from_rgb(0x6a, 0x6e, 0x7a),
    watch_edge: Color32::from_rgba_premultiplied(7, 27, 38, 64),
    now_hour: Color32::from_rgb(0x10, 0x14, 0x20),
    now_edge: Color32::from_rgb(0x1a, 0x7a, 0xd8),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
