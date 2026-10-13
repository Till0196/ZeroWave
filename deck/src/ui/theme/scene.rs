// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

/// 映像の場所なので明暗に依らず暗いほうの配色。上から下へ暗くなる階調。
pub const TOP: Color32 = Color32::from_rgb(0x11, 0x11, 0x13);
pub const BOTTOM: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0b);
/// 真ん中に薄く置く名前。
pub const MARK: Color32 = Color32::from_rgb(0x4c, 0x4c, 0x51);
pub const MARK_SIZE: f32 = 26.0;
/// 文字の間隔。無いと別の字面に見える。
pub const MARK_SPACING: f32 = 6.0;
