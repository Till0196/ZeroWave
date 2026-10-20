// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const WIDTH: f32 = 940.0;
pub const HEIGHT: f32 = 580.0;
pub const CLOCK_WIDTH: f32 = 32.0;
pub const COLUMN: f32 = 130.0;
pub const HEAD_HEIGHT: f32 = 46.0;
/// 1 画面の時間幅を広げても、番組名が一行は読める高さ。
pub const HOUR_LEAST: f32 = 40.0;

pub const CLOCK_SIZE: f32 = 12.5;

/// 上のバー（種別のタブと日付の送り）。三つを一つに並べると列が二百を超えるため。
pub const BAR_HEIGHT: f32 = 36.0;
pub const BAR_SIZE: f32 = 12.0;

pub const HEAD_SIZE: f32 = 10.5;
pub const HEAD_PAD: f32 = 5.0;
pub const HEAD_GAP: f32 = 3.0;
pub const KEY_SIZE: f32 = 8.5;
pub const BADGE_WIDTH: f32 = 30.0;
pub const BADGE_HEIGHT: f32 = 15.0;
pub const BADGE_SIZE: f32 = 8.0;

pub const PAD: f32 = 5.0;
pub const MINUTE_SIZE: f32 = 9.5;
pub const TITLE_SIZE: f32 = 11.0;
pub const TITLE_LEADING: f32 = 14.85;
pub const TITLE_LEAST: f32 = 8.0;
pub const DETAIL_SIZE: f32 = 10.0;
pub const DETAIL_LEADING: f32 = 14.0;
pub const NOW_WIDTH: f32 = 2.0;
pub const FOCUS_WIDTH: f32 = 2.0;
pub const MARK_WIDTH: f32 = 3.0;
/// 終わった番組。背景に被せる層の濃さと、文字を残す濃さ。
pub const ENDED_VEIL: f32 = 0.55;
pub const ENDED_INK: f32 = 0.75;

/// スクロールバーのつまみの太さ（`SCROLL_MARGIN` はその両脇）。いつも出す。
pub const SCROLL_BAR: f32 = 8.0;
pub const SCROLL_MARGIN: f32 = 3.0;

/// 番組検索のウィンドウ。デザインは OS のタイトルバー（32）を含めて 1100×640。
pub const FOUND_WIDTH: f32 = 1100.0;
pub const FOUND_HEIGHT: f32 = 608.0;
pub const FOUND_BAR: f32 = 46.0;
pub const FOUND_FILTER: f32 = 47.0;
pub const FOUND_SEARCH_WIDTH: f32 = 320.0;
pub const FOUND_SIDE: f32 = 320.0;
pub const FOUND_ROW: f32 = 50.0;
pub const FOUND_DAY: f32 = 28.0;
pub const FOUND_PAD: f32 = 14.0;
pub const FOUND_TIME: f32 = 96.0;
pub const FOUND_CHANNEL: f32 = 140.0;
pub const FOUND_GENRE: f32 = 110.0;
pub const FOUND_GAP: f32 = 12.0;
pub const FOUND_SIZE: f32 = 12.0;
pub const FOUND_TITLE_SIZE: f32 = 13.0;
pub const FOUND_DETAIL_SIZE: f32 = 11.5;
pub const FOUND_SIDE_TITLE: f32 = 16.0;
pub const FOUND_ENDED: f32 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub mode: super::Mode,
    pub background: Color32,
    pub clock_background: Color32,
    pub clock_edge: Color32,
    pub clock_text: Color32,
    pub bar_background: Color32,
    pub bar_edge: Color32,
    pub bar_text: Color32,
    pub head_background: Color32,
    pub head_edge: Color32,
    pub head_text: Color32,
    pub head_key: Color32,
    pub head_watching: Color32,
    pub head_watching_text: Color32,
    pub edge: Color32,
    pub minute: Color32,
    pub title: Color32,
    pub detail: Color32,
    /// ポインターを重ねたセル。`focus_lift` は背景を白へ寄せる度合い（八分の幾つ）、0 なら枠だけ。
    pub focus_edge: Color32,
    pub focus_lift: u8,
    pub focus_text: Color32,
    pub focus_detail: Color32,
    pub now: Color32,
    pub mark: Color32,
    /// ジャンルごとのセルの上端の色。並びは ARIB STD-B10 の大分類の番号。
    pub genres: [Color32; 16],
    pub genre_end: Color32,
    /// ジャンルの無い番組。背景と同じ色にすると穴が空いて見える。
    pub no_genre: Color32,
    pub scroll_track: Color32,
    pub scroll_handle: Color32,
    pub scroll_hover: Color32,
    pub scroll_held: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_hover: Color32,
    pub button_hover: Color32,
    pub row_hover: Color32,
    pub side_background: Color32,
    pub control_edge: Color32,
    pub field: Color32,
    pub day_text: Color32,
    pub chosen_row: Color32,
    pub found_mark: Color32,
}

/// セルの上端の色。見分けるのは四つ（映画・音楽・スポーツ・ドラマ）で、残りは同じ灰。
const MOVIE: Color32 = Color32::from_rgb(0x38, 0x2c, 0x45);
const MUSIC: Color32 = Color32::from_rgb(0x3d, 0x3a, 0x35);
const SPORTS: Color32 = Color32::from_rgb(0x26, 0x3e, 0x60);
const SERIES: Color32 = Color32::from_rgb(0x25, 0x3f, 0x33);
const OTHER: Color32 = Color32::from_rgb(0x20, 0x20, 0x24);
pub const GENRE_END: Color32 = Color32::from_rgb(0x2a, 0x30, 0x38);
const GENRES: [Color32; 16] = [
    OTHER,  // 0 ニュース／報道
    SPORTS, // 1 スポーツ
    OTHER,  // 2 情報／ワイドショー
    SERIES, // 3 ドラマ
    MUSIC,  // 4 音楽
    OTHER,  // 5 バラエティ
    MOVIE,  // 6 映画
    SERIES, // 7 アニメ／特撮
    OTHER,  // 8 ドキュメンタリー／教養
    MUSIC,  // 9 劇場／公演
    OTHER,  // 10 趣味／教育
    OTHER,  // 11 福祉
    OTHER,  // 12 予備
    OTHER,  // 13 予備
    OTHER,  // 14 拡張
    OTHER,  // 15 その他
];

/// 明るいほうのジャンル色は手で選ぶ。計算で白へ寄せると全部が同じ灰になる。
const LIGHT_MOVIE: Color32 = Color32::from_rgb(0xe0, 0xc8, 0xdd);
const LIGHT_MUSIC: Color32 = Color32::from_rgb(0xe8, 0xd6, 0xb6);
const LIGHT_SPORTS: Color32 = Color32::from_rgb(0xc2, 0xd6, 0xee);
const LIGHT_SERIES: Color32 = Color32::from_rgb(0xc6, 0xe0, 0xcc);
const LIGHT_OTHER: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
pub const LIGHT_GENRE_END: Color32 = Color32::from_rgb(0xcc, 0xd1, 0xd8);

/// 局のバッジの背景。名前から決める色で、白い文字が乗るので明暗で変えない。
pub const BADGE_COLOURS: [Color32; 6] = [
    Color32::from_rgb(0xcc, 0x00, 0x00),
    Color32::from_rgb(0x1a, 0x7a, 0x3a),
    Color32::from_rgb(0x1a, 0x4a, 0xaa),
    Color32::from_rgb(0x8a, 0x3a, 0x9a),
    Color32::from_rgb(0xc8, 0x6a, 0x10),
    Color32::from_rgb(0x0a, 0x6a, 0x8a),
];
const LIGHT_GENRES: [Color32; 16] = [
    LIGHT_OTHER,
    LIGHT_SPORTS,
    LIGHT_OTHER,
    LIGHT_SERIES,
    LIGHT_MUSIC,
    LIGHT_OTHER,
    LIGHT_MOVIE,
    LIGHT_SERIES,
    LIGHT_OTHER,
    LIGHT_MUSIC,
    LIGHT_OTHER,
    LIGHT_OTHER,
    LIGHT_OTHER,
    LIGHT_OTHER,
    LIGHT_OTHER,
    LIGHT_OTHER,
];

/// 真っ白だとセルの縁が見えないので、白を少し沈める。
pub const LIGHT: Skin = Skin {
    mode: super::Mode::Light,
    background: Color32::from_rgb(0xf6, 0xf7, 0xf9),
    clock_background: Color32::from_rgb(0xd8, 0xdb, 0xe3),
    clock_edge: Color32::from_rgb(0xb8, 0x98, 0x3a),
    clock_text: Color32::from_rgb(0x30, 0x34, 0x40),
    bar_background: Color32::from_rgb(0xea, 0xec, 0xf0),
    bar_edge: Color32::from_rgba_premultiplied(0, 0, 0, 26),
    bar_text: Color32::from_rgb(0x40, 0x44, 0x50),
    head_background: Color32::from_rgb(0xd8, 0xdb, 0xe3),
    head_edge: Color32::from_rgba_premultiplied(0, 0, 0, 44),
    head_text: Color32::from_rgb(0x20, 0x24, 0x30),
    head_key: Color32::from_rgb(0x70, 0x74, 0x80),
    head_watching: Color32::from_rgba_premultiplied(14, 35, 44, 46),
    head_watching_text: Color32::from_rgb(0x1a, 0x6a, 0x96),
    edge: Color32::from_rgba_premultiplied(0, 0, 0, 50),
    minute: Color32::from_rgb(0x40, 0x46, 0x54),
    title: Color32::from_rgb(0x18, 0x1c, 0x26),
    detail: Color32::from_rgb(0x50, 0x56, 0x68),
    focus_edge: Color32::from_rgb(0x1a, 0x7a, 0xd8),
    focus_lift: 0,
    focus_text: Color32::from_rgb(0x18, 0x1c, 0x26),
    focus_detail: Color32::from_rgb(0x50, 0x56, 0x68),
    now: Color32::from_rgb(0x1a, 0x7a, 0xd8),
    mark: Color32::from_rgb(0xe0, 0x8a, 0x00),
    genres: LIGHT_GENRES,
    genre_end: LIGHT_GENRE_END,
    no_genre: LIGHT_GENRES[15],
    scroll_track: Color32::from_rgb(0xe2, 0xe4, 0xe9),
    scroll_handle: Color32::from_rgb(0xb4, 0xb8, 0xc2),
    scroll_hover: Color32::from_rgb(0x8c, 0x91, 0x9d),
    scroll_held: Color32::from_rgb(0x6c, 0x71, 0x7e),
    accent: Color32::from_rgb(0x2b, 0x6a, 0xa0),
    accent_text: Color32::WHITE,
    // 明るい背景では白へ寄せると文字が沈むので、主ボタンは濃く、副ボタンと行は青のオーバーレイを濃く。
    accent_hover: Color32::from_rgb(0x22, 0x58, 0x86),
    button_hover: Color32::from_rgba_premultiplied(12, 29, 44, 70),
    row_hover: Color32::from_rgba_premultiplied(5, 12, 18, 28),
    side_background: Color32::from_rgb(0xee, 0xf0, 0xf3),
    control_edge: Color32::from_rgba_premultiplied(0, 0, 0, 46),
    field: Color32::WHITE,
    day_text: Color32::from_rgb(0x2b, 0x5a, 0xa0),
    chosen_row: Color32::from_rgba_premultiplied(8, 19, 29, 46),
    found_mark: Color32::from_rgb(0xff, 0xe2, 0x86),
};

pub const DARK: Skin = Skin {
    mode: super::Mode::Dark,
    background: Color32::from_rgb(0x11, 0x11, 0x13),
    clock_background: Color32::from_rgb(0x2a, 0x2a, 0x2f),
    clock_edge: Color32::from_rgb(0xb4, 0xbc, 0xcb),
    clock_text: Color32::from_rgb(0xff, 0xff, 0xff),
    bar_background: Color32::from_rgb(0x1a, 0x1c, 0x22),
    bar_edge: Color32::from_rgba_premultiplied(15, 15, 17, 17),
    bar_text: Color32::from_rgba_premultiplied(140, 140, 145, 145),
    head_background: Color32::from_rgb(0x11, 0x11, 0x13),
    head_edge: Color32::from_rgba_premultiplied(38, 38, 42, 42),
    head_text: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    head_key: Color32::from_rgb(0xbb, 0xbb, 0xc0),
    head_watching: Color32::from_rgba_premultiplied(14, 35, 44, 46),
    head_watching_text: Color32::from_rgb(0x4f, 0xc3, 0xf7),
    edge: Color32::from_rgba_premultiplied(38, 38, 42, 42),
    minute: Color32::from_rgb(0xdd, 0xdd, 0xe1),
    title: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    detail: Color32::from_rgb(0xbb, 0xbb, 0xc0),
    focus_edge: Color32::WHITE,
    focus_lift: 5,
    focus_text: Color32::from_rgb(0x0a, 0x0a, 0x0b),
    focus_detail: Color32::from_rgb(0x2c, 0x2c, 0x31),
    now: Color32::from_rgba_premultiplied(0x2b, 0x6a, 0xa0, 0xa0),
    mark: Color32::from_rgb(0xff, 0xc4, 0x3a),
    genres: GENRES,
    genre_end: GENRE_END,
    no_genre: GENRES[15],
    scroll_track: Color32::from_rgb(0x0d, 0x0d, 0x0f),
    scroll_handle: Color32::from_rgb(0x5a, 0x5a, 0x5f),
    scroll_hover: Color32::from_rgb(0x80, 0x80, 0x85),
    scroll_held: Color32::from_rgb(0x9a, 0x9a, 0x9f),
    accent: Color32::from_rgb(0x2b, 0x6a, 0xa0),
    accent_text: Color32::WHITE,
    accent_hover: super::lightened(Color32::from_rgb(0x2b, 0x6a, 0xa0), 1),
    button_hover: super::lightened(Color32::from_rgba_premultiplied(11, 27, 40, 64), 1),
    row_hover: Color32::from_rgba_premultiplied(6, 14, 20, 32),
    side_background: Color32::from_rgb(0x15, 0x16, 0x1b),
    control_edge: Color32::from_rgba_premultiplied(41, 41, 45, 45),
    field: Color32::from_rgb(0x0f, 0x10, 0x14),
    day_text: Color32::from_rgb(0x8f, 0xb0, 0xf0),
    chosen_row: Color32::from_rgba_premultiplied(11, 27, 40, 64),
    found_mark: Color32::from_rgba_premultiplied(71, 56, 17, 71),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
