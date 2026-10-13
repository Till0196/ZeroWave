// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Decoding pictures, and what a decoded picture is. The platform decoders live outside this
//! crate and share [`Decoder`]. Nothing here has a thread or a clock.

pub mod bits;
#[cfg(any(feature = "h264-decoder", feature = "hevc-decoder"))]
pub mod cabac;
pub mod h264;
pub mod hevc;
pub mod mpeg2;

use crate::es::{Format, Picture, Structure};

/// ピクチャーサイズの上限（幅、高さ）。H.264・HEVC の最上位の level が許す 8192×4352。
/// これより大きいと宣言するストリームはパラメータセットを読む時点で拒否する（そのまま確保するとクラッシュする）。
pub const LARGEST: [u32; 2] = [8192, 4352];

/// 幅と高さが 0 でなく [`LARGEST`] に収まるか。掛け算の前に確認できるよう広い型で受ける。
pub fn fits(width: u64, height: u64) -> bool {
    (1..=u64::from(LARGEST[0])).contains(&width) && (1..=u64::from(LARGEST[1])).contains(&height)
}

/// One plane of a decoded picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// Eight bits to a sample up to a depth of eight, two bytes little-endian above it.
    pub data: Vec<u8>,
    /// Bytes per row; not the width, since a decoder hands its macroblock padding over.
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Plane {
    pub fn row(&self, y: usize) -> &[u8] {
        let at = y * self.stride;
        self.data.get(at..at + self.stride).unwrap_or(&[])
    }

    /// A plane that is not there.
    pub fn none() -> Self {
        Self {
            data: Vec::new(),
            stride: 0,
            width: 0,
            height: 0,
        }
    }
}

/// A decoded picture, ready to be shown or deinterlaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The caller's own value for the access unit, carried through; decoders reorder.
    pub tag: u64,
    pub format: Format,
    pub planes: [Plane; 3],
    /// What the picture was coded as; a deinterlacer needs it.
    pub structure: Structure,
    pub top_field_first: bool,
    pub repeat_first_field: bool,
    pub progressive: bool,
}

impl Frame {
    /// Whether the chroma arrived as one plane of interleaved pairs (as `NV12` and `P010`).
    /// Then `planes[1]` holds one pair to a sample, and an empty `planes[2]` ([`Plane::none`]) is
    /// what says so. Nothing here converts.
    pub fn chroma_woven(&self) -> bool {
        self.planes[2].height == 0
    }
}

pub use crate::Error;

/// What every decoder answers to.
pub trait Decoder {
    /// Takes one access unit as [`crate::es`] cut it. `tag` comes back on the [`Frame`].
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error>;

    fn take(&mut self) -> Option<Frame>;

    /// Says the stream has ended. A reordering decoder gives up what it held back, to be had from
    /// [`Decoder::take`]; without this the last picture of every file is lost.
    fn flush(&mut self);

    /// Gives up everything held (a change of channel). Nothing comes out, unlike a flush.
    fn reset(&mut self);
}
