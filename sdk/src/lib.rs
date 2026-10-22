// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ZeroWave のプラグイン（wasm）の SDK。重い処理は本体が native で行い、import で提供する。

pub mod abi;
pub mod config;
pub mod control;
#[cfg(feature = "host")]
pub mod draw;
pub mod fonts;
pub mod image;
pub mod layer;
pub mod net;
pub mod plug;
mod slots;
pub mod ui;
pub mod video;

#[cfg(feature = "host")]
pub mod host;
