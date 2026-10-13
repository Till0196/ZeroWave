// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 音声出力デバイスを開く（デスクトップの音声出力）。Linux は libasound を実行時に開き（[`alsa`]）、
//! ほかの OS は cpal で。どちらも [`device`] として使う。

#[cfg(target_os = "linux")]
pub mod alsa;
#[cfg(not(target_os = "linux"))]
pub mod cpal;
pub mod queue;

#[cfg(target_os = "linux")]
pub use alsa as device;
#[cfg(not(target_os = "linux"))]
pub use cpal as device;
