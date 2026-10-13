// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Pictures drawn on a surface and sound handed to a device.

/// OpenGL ES 2.0。`libEGL` を `dlopen` で開くので unix だけ。
#[cfg(unix)]
pub mod present;
/// エンコードされた音声をデコードせずにレシーバーへ渡す経路。
pub mod sound;
pub(crate) mod template;
pub mod video;
