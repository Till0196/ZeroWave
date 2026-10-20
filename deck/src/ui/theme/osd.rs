// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const HEIGHT: f32 = 94.6;
pub const PAD_X: f32 = 24.0;
/// 寸法は映像の幅に付いて伸び縮みする。この幅で等倍、[`DP_LEAST`] より小さくはしない。
pub const REFERENCE_WIDTH: f32 = 1100.0;
pub const DP_LEAST: f32 = 0.5;
pub const GROWTH: f32 = 0.5;
/// 背景のグラデーション（折れ線）。明るい映像の上でも二行目の右端が読めるよう、バーの 1.35 倍まで伸ばす。
pub const SHADE: &[(f32, f32)] = &[
    (0.0, 0.92),
    (0.3, 0.86),
    (0.6, 0.72),
    (0.85, 0.35),
    (1.0, 0.0),
];
pub const SHADE_DEPTH: f32 = 1.35;
/// SDR／HDR のときの背景の濃さの倍率。SDR は `overlay::osd::dim` でリニアで混ぜた濃さに直すので同じ値。
pub const SHADE_SDR: f32 = 1.08;
pub const SHADE_HDR: f32 = 1.08;

pub const TITLE: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
/// メッセージの文字。通らなかった操作を伝えるので、番組名とは色を分ける。
pub const TROUBLE: Color32 = Color32::from_rgb(0xf0, 0xb0, 0x60);
pub const TITLE_TOP: f32 = 12.0;
pub const TITLE_AREA: f32 = 33.6;
pub const TITLE_SIZE: f32 = 28.0;
pub const CLOCK: Color32 = Color32::from_rgba_premultiplied(191, 191, 191, 191);
pub const CLOCK_SIZE: f32 = 15.0;

pub const LINE_TOP: f32 = 51.6;
pub const LINE_HEIGHT: f32 = 27.0;
pub const KEY_BADGE: f32 = 20.0;
pub const KEY_GAP: f32 = 8.0;
pub const KEY_SIZE: f32 = 8.4;
pub const KEY_TEXT: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
pub const LOGO_SIZE: f32 = 12.0;
pub const LOGO_PAD_X: f32 = 6.0;
pub const LOGO_MIN_WIDTH: f32 = 34.0;
pub const LOGO_GAP: f32 = 8.0;
pub const CHANNEL: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
pub const CHANNEL_SIZE: f32 = 14.0;
pub const BAND: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
pub const BAND_SIZE: f32 = 11.0;
pub const AIRTIME: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
pub const AIRTIME_SIZE: f32 = 13.0;
pub const AIRTIME_AT: f32 = 632.0 / 1920.0;
pub const AIRTIME_GAP: f32 = 24.0;
pub const INFO: Color32 = Color32::from_rgba_premultiplied(204, 204, 204, 204);
pub const INFO_SIZE: f32 = 12.0;

pub const TOAST_BACK: Color32 = Color32::from_rgba_premultiplied(48, 48, 48, 235);
pub const TOAST_TEXT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
pub const TOAST_SIZE: f32 = 15.0;
pub const TOAST_DETAIL: f32 = 0.8;
pub const TOAST_DETAIL_TEXT: Color32 = Color32::from_rgba_premultiplied(150, 150, 150, 255);
pub const TOAST_WARN: Color32 = Color32::from_rgb(0xf2, 0xc2, 0x7a);
pub const TOAST_BAD: Color32 = Color32::from_rgb(0xf2, 0x9a, 0x8a);
pub const TOAST_GOOD: Color32 = Color32::from_rgb(0x9a, 0xdc, 0xb4);
pub const TOAST_TRACK: Color32 = Color32::from_rgba_premultiplied(30, 30, 30, 76);
pub const TOAST_PAD_X: f32 = 22.0;
pub const TOAST_PAD_Y: f32 = 10.0;
pub const TOAST_MARGIN: f32 = 40.0;
/// トーストの文字列の幅の上限（画面の幅に対する割合）。越える文は折り返す。
pub const TOAST_WIDEST: f32 = 0.5;

pub const FADE: f32 = 0.5;

pub const VOLUME_INK: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
pub const VOLUME_MUTED: Color32 = Color32::from_rgb(0xf2, 0xc2, 0x7a);
pub const VOLUME_SHADE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 184);
pub const VOLUME_TRACK: Color32 = Color32::from_rgba_premultiplied(46, 46, 46, 46);
pub const VOLUME_MUTED_BAR: Color32 = Color32::from_rgba_premultiplied(76, 76, 76, 82);

pub const IDLE_TEXT: Color32 = Color32::from_rgb(0xee, 0xee, 0xee);
pub const IDLE_DIM: Color32 = Color32::from_rgb(0x9a, 0x9e, 0xa8);
pub const IDLE_HEADING: Color32 = Color32::from_rgb(0x8f, 0xb0, 0xf0);
pub const IDLE_BAR: Color32 = Color32::from_rgb(0x6a, 0x9c, 0xff);
pub const IDLE_BOX: Color32 = Color32::from_rgba_premultiplied(12, 12, 12, 12);
pub const IDLE_TRACK: Color32 = Color32::from_rgba_premultiplied(38, 38, 38, 38);
pub const IDLE_RULE: Color32 = Color32::from_rgba_premultiplied(26, 26, 26, 26);
/// 映らないときの画面（`ui::overlay::trouble`）：番号のラベルのオレンジと、E201 で最後のフレームに被せる暗さ。
pub const TROUBLE_CODE: Color32 = Color32::from_rgb(0xff, 0xb0, 0x70);
pub const TROUBLE_DIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 140);
