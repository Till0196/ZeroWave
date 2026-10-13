// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

/// ポインタが端から離れてから、バーが消えるまで。
pub const HOLD: Duration = Duration::from_millis(600);
/// ←→ で動かしたあと、バーを見せておく長さ。
pub const AFTER_KEY: Duration = Duration::from_secs(3);
pub const IDLE_CURSOR: Duration = Duration::from_secs(3);
pub const MENU_HEIGHT: f32 = 26.0;
