// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ウィンドウ。イベントループ、別ウィンドウ、メニュー、終了処理。

pub mod ask;
pub mod cursor;
pub mod ending;
#[cfg(any(windows, target_os = "linux"))]
pub mod handoff;
#[cfg(target_os = "macos")]
pub mod mac_open;
pub mod menu;
pub mod opening;
pub mod place;
pub mod shell;
pub mod sub;
pub mod text;
#[cfg(target_os = "linux")]
pub mod wayland_drop;
#[cfg(windows)]
pub mod win32;
