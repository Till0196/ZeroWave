// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! オプティカルフローのバックエンドの共通インターフェース。フローは `rgba16float` の `(前→今 x, y, 今→前 x, y)`（画素）、
//! 信頼度は `rg16float`。対応しないグリッドなら一番近いグリッドで求めてリサンプリングする。

use super::{Convert, Gpu, Run};

pub(super) trait Provider: Send {
    fn size(&self) -> [u32; 2];
    fn wanted_grid(&self) -> u32;

    fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String>;

    /// 出さなければ false。
    fn confidence(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        target: &wgpu::Texture,
    ) -> bool;

    /// GPU のキューで待つバックエンドは 0。
    fn outside_ms(&self) -> f64 {
        0.0
    }

    /// 破棄すると困るバックエンド（NVOFA）は取っておく。
    fn retire(self: Box<Self>) {}
}

/// フレームの何画素に一つか。
pub(super) fn grid_of(size: [u32; 2], target: &wgpu::Texture) -> u32 {
    let width = target.width().max(1);
    (f64::from(size[0]) / f64::from(width)).round().max(1.0) as u32
}

/// 同じ近さなら細かい方。
#[cfg_attr(not(all(feature = "d3d12", windows)), allow(dead_code))]
pub(super) fn nearest_grid(have: &[u32], wanted: u32) -> Option<u32> {
    have.iter()
        .copied()
        .min_by_key(|&g| (g.abs_diff(wanted), g))
}

#[cfg(test)]
mod tests {
    use super::nearest_grid;

    #[test]
    fn the_nearest_grid_prefers_the_finer_on_a_tie() {
        assert_eq!(nearest_grid(&[1, 2, 4], 2), Some(2));
        assert_eq!(nearest_grid(&[4], 1), Some(4));
        assert_eq!(nearest_grid(&[2, 4], 3), Some(2));
        assert_eq!(nearest_grid(&[], 4), None);
    }
}
