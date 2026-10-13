// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードした画像。UI ライブラリーに依存しない形で渡す。

/// RGBA、上から一行ずつ、乗算済みアルファではない。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Rgba {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            rgba,
        }
    }

    pub fn size(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// インデックスカラーの画像はデコードできない（呼び出し側が `EXPAND` で展開する）。
pub(crate) fn rgba_of_png(color: png::ColorType, pixels: &[u8]) -> Option<Vec<u8>> {
    Some(match color {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    })
}
