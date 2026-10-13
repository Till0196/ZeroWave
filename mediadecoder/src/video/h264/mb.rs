// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What a decoded macroblock leaves for its neighbours, deblocking and temporal direct.
//! Indexed by 4x4 block in raster order, not the standard's block order.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// Not decoded, which a macroblock in a slice that was lost stays.
    #[default]
    Missing,
    Intra4x4,
    Intra8x8,
    Intra16x16,
    Pcm,
    Inter,
}

impl Kind {
    pub fn intra(self) -> bool {
        matches!(
            self,
            Self::Intra4x4 | Self::Intra8x8 | Self::Intra16x16 | Self::Pcm
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Mb {
    pub kind: Kind,
    pub skipped: bool,
    pub direct: bool,
    /// `direct` per 8x8 quarter; CABAC ref_idx contexts treat inferred quarters as having none.
    pub direct_quarters: u8,
    pub transform8x8: bool,
    /// Luma quarters with coefficients in the low four bits, chroma above.
    pub cbp: u8,
    pub qpy: i8,
    /// Chroma QP per component; depends on PPS offsets, so deblocking cannot derive it.
    pub qpc: [i8; 2],
    pub slice: u16,
    /// Intra mode per 4x4 luma block, -1 if not intra. An 8x8 mode fills all four 4x4s.
    pub modes: [i8; 16],
    /// Coefficient count per block: 16 luma, then 4 per chroma component.
    pub nnz: [u8; 24],
    /// DC blocks with coefficients: luma in bit 0, chroma in bits 1-2 (not in `nnz`).
    pub dc_nnz: u8,
    pub chroma_mode: u8,
    pub mv: [[[i16; 2]; 16]; 2],
    pub refs: [[i8; 4]; 2],
    /// Picture identity per quarter and list, -1 if unused; indices differ between slices.
    pub pics: [[i32; 4]; 2],
    /// Deblocking flags: enable, cross slice boundaries, and the two offsets.
    pub deblock: u8,
    pub alpha: i8,
    pub beta: i8,
}

impl Default for Mb {
    fn default() -> Self {
        Self {
            kind: Kind::Missing,
            skipped: false,
            direct: false,
            direct_quarters: 0,
            transform8x8: false,
            cbp: 0,
            qpy: 0,
            qpc: [0; 2],
            slice: u16::MAX,
            modes: [-1; 16],
            nnz: [0; 24],
            dc_nnz: 0,
            chroma_mode: 0,
            mv: [[[0; 2]; 16]; 2],
            refs: [[-1; 4]; 2],
            pics: [[-1; 4]; 2],
            deblock: 0,
            alpha: 0,
            beta: 0,
        }
    }
}

impl Mb {
    pub fn coded(&self) -> bool {
        self.kind != Kind::Missing
    }

    /// Whether the transform covering a 4x4 block has coefficients; an 8x8 transform counts once.
    pub fn coded_block(&self, block: usize) -> bool {
        if !self.transform8x8 || block >= 16 {
            return self.nnz[block] != 0;
        }
        let (x, y) = ((block % 4) & !1, (block / 4) & !1);
        (0..2).any(|dy| (0..2).any(|dx| self.nnz[(y + dy) * 4 + x + dx] != 0))
    }
}
