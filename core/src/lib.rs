// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The player: a broadcast into pictures and sound.

pub mod captions;
#[cfg(feature = "plugins")]
pub mod data;
pub mod decode;
pub mod dl;
pub mod engine;
pub mod filters;
pub mod guide;
pub mod image;
pub mod input;
#[cfg(feature = "plugins")]
pub mod layers;
pub mod memory;
pub mod output;
/// データ放送のスレッドを使うので `plugins` のときだけ。
#[cfg(feature = "plugins")]
pub mod player;
#[cfg(feature = "plugins")]
pub mod plugins;
pub mod record;
pub mod region;
#[cfg(feature = "coaxd")]
pub mod scan;
pub mod screen;
pub mod si;
pub mod sound;
pub mod thread;
#[cfg(any(
    feature = "plugins",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows),
    all(feature = "mediafoundation", windows),
    feature = "vaapi",
    all(feature = "videotoolbox", target_vendor = "apple")
))]
mod trace;
pub mod wake;
