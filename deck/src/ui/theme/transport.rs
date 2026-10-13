// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

pub const HEIGHT: f32 = super::status::HEIGHT;
/// バーの太さ。ポインタが乗ると太くなる。
pub const TRACK: f32 = 3.0;
pub const TRACK_HOT: f32 = 5.0;
/// つまみの半径。乗っているときだけ出る。
pub const KNOB: f32 = 5.0;
/// これより狭ければバーは出さない。
pub const TRACK_LEAST: f32 = 60.0;
pub const BUTTON_EMS: f32 = 2.8;
pub const BUTTON_GAP: f32 = 2.0;
pub const BUTTON_SIZE: f32 = 11.0;
pub const ICON: f32 = 13.0;
pub const HINT_SIZE: f32 = 10.5;
/// ←→ で動く量（ミリ秒）。
pub const STEP_MS: i64 = 10_000;
