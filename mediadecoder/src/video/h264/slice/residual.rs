// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The residual of a macroblock.

use super::*;

impl Coder<'_> {
    pub(super) fn luma_nc(&self, at: usize, raster: usize) -> i32 {
        let (x, y) = ((raster % 4) as i32, (raster / 4) as i32);
        let a = self
            .neighbour_block(at, x - 1, y)
            .map(|(mb, block)| i32::from(self.mbs[mb].nnz[block]));
        let b = self
            .neighbour_block(at, x, y - 1)
            .map(|(mb, block)| i32::from(self.mbs[mb].nnz[block]));
        combine(a, b)
    }

    pub(super) fn chroma_neighbour(
        &self,
        at: usize,
        x: i32,
        y: i32,
        component: usize,
    ) -> Option<(usize, usize)> {
        let (mb, cx, cy) = if y < 0 {
            (self.neighbour_mb(at, 0, -1)?, x, 1)
        } else if x < 0 {
            (self.neighbour_mb(at, -1, 0)?, 1, y)
        } else {
            (at, x, y)
        };
        Some((mb, CHROMA_BLOCK + component * 4 + (cy * 2 + cx) as usize))
    }

    pub(super) fn chroma_nc(&self, at: usize, block: usize, component: usize) -> i32 {
        let (x, y) = ((block % 2) as i32, (block / 2) as i32);
        let a = self
            .chroma_neighbour(at, x - 1, y, component)
            .map(|(mb, block)| i32::from(self.mbs[mb].nnz[block]));
        let b = self
            .chroma_neighbour(at, x, y - 1, component)
            .map(|(mb, block)| i32::from(self.mbs[mb].nnz[block]));
        combine(a, b)
    }

    /// Clause 9.3.3.1.1.9. Outside the picture counts as coded for intra and uncoded for inter.
    pub(super) fn coded_inc(
        &self,
        intra: bool,
        neighbours: [Option<(usize, usize)>; 2],
        dc: Option<usize>,
    ) -> usize {
        let mut inc = 0;
        for (place, found) in neighbours.into_iter().enumerate() {
            let flag = match found {
                None => intra,
                Some((mb, block)) => {
                    let mb = &self.mbs[mb];
                    if mb.kind == Kind::Pcm {
                        true
                    } else {
                        match dc {
                            Some(bit) => mb.dc_nnz >> bit & 1 == 1,
                            None => mb.nnz[block] != 0,
                        }
                    }
                }
            };
            inc += usize::from(flag) << place;
        }
        inc
    }

    pub(super) fn read_residual(&mut self, at: usize, current: &Current, residual: &mut Residual) {
        let intra = current.kind.intra();
        let cabac = self.cabac();

        if current.kind == Kind::Intra16x16 {
            let mut coefficients = [0i32; 16];
            let count = if cabac {
                let neighbours = [
                    self.neighbour_mb(at, -1, 0).map(|mb| (mb, 0)),
                    self.neighbour_mb(at, 0, -1).map(|mb| (mb, 0)),
                ];
                let inc = self.coded_inc(intra, neighbours, Some(0));
                self.cabac_block(&mut coefficients, Cat::LumaDc, Some(inc))
            } else {
                let nc = self.luma_nc(at, 0);
                self.cavlc_block(&mut coefficients, nc)
            };
            residual.dc = coefficients;
            if count > 0 {
                self.mbs[at].dc_nnz |= 1;
            }
        }

        for quarter in 0..4usize {
            let coded = current.cbp_luma >> quarter & 1 == 1;
            if current.transform8x8 && cabac {
                if coded {
                    let count = self.cabac_block(&mut residual.luma8[quarter], Cat::Luma8x8, None);
                    let (qx, qy) = ((quarter % 2) * 2, (quarter / 2) * 2);
                    for row in 0..2 {
                        for column in 0..2 {
                            self.mbs[at].nnz[(qy + row) * 4 + qx + column] = count.min(255) as u8;
                        }
                    }
                }
                continue;
            }
            for piece in 0..4usize {
                let block = quarter * 4 + piece;
                let raster = usize::from(BLOCK_TO_RASTER[block]);
                if coded {
                    let count = if current.kind == Kind::Intra16x16 {
                        self.read_luma_block(at, raster, Cat::LumaAc, intra, 1, residual)
                    } else {
                        self.read_luma_block(at, raster, Cat::Luma, intra, 0, residual)
                    };
                    self.mbs[at].nnz[raster] = count.min(255) as u8;
                }
            }
            // CAVLC writes an 8x8 block as four interleaved 4x4 ones.
            if !cabac && current.transform8x8 && coded {
                for piece in 0..4usize {
                    let raster = usize::from(BLOCK_TO_RASTER[quarter * 4 + piece]);
                    for index in 0..16 {
                        residual.luma8[quarter][4 * index + piece] = residual.luma[raster][index];
                    }
                }
            }
        }

        if self.sps.chroma_array() == 0 {
            return;
        }
        if current.cbp_chroma != 0 {
            for component in 0..2usize {
                let mut coefficients = [0i32; 4];
                let count = if cabac {
                    let neighbours = [
                        self.neighbour_mb(at, -1, 0).map(|mb| (mb, 0)),
                        self.neighbour_mb(at, 0, -1).map(|mb| (mb, 0)),
                    ];
                    let inc = self.coded_inc(intra, neighbours, Some(1 + component));
                    self.cabac_block(&mut coefficients, Cat::ChromaDc, Some(inc))
                } else {
                    self.cavlc_block(&mut coefficients, -1)
                };
                residual.chroma_dc[component] = coefficients;
                if count > 0 {
                    self.mbs[at].dc_nnz |= 1 << (1 + component);
                }
            }
        }
        if current.cbp_chroma == 2 {
            for component in 0..2usize {
                for block in 0..4usize {
                    let mut coefficients = [0i32; 15];
                    let count = if cabac {
                        let (x, y) = ((block % 2) as i32, (block / 2) as i32);
                        let neighbours = [
                            self.chroma_neighbour(at, x - 1, y, component),
                            self.chroma_neighbour(at, x, y - 1, component),
                        ];
                        let inc = self.coded_inc(intra, neighbours, None);
                        self.cabac_block(&mut coefficients, Cat::ChromaAc, Some(inc))
                    } else {
                        let nc = self.chroma_nc(at, block, component);
                        self.cavlc_block(&mut coefficients, nc)
                    };
                    residual.chroma[component][block][1..16].copy_from_slice(&coefficients);
                    self.mbs[at].nnz[CHROMA_BLOCK + component * 4 + block] = count.min(255) as u8;
                }
            }
        }
    }

    /// One luma block, written at `offset` so a 16x16 DC coefficient keeps its place.
    pub(super) fn read_luma_block(
        &mut self,
        at: usize,
        raster: usize,
        cat: Cat,
        intra: bool,
        offset: usize,
        residual: &mut Residual,
    ) -> usize {
        let mut coefficients = [0i32; 16];
        let count = if self.cabac() {
            let (x, y) = ((raster % 4) as i32, (raster / 4) as i32);
            let neighbours = [
                self.neighbour_block(at, x - 1, y),
                self.neighbour_block(at, x, y - 1),
            ];
            let inc = self.coded_inc(intra, neighbours, None);
            self.cabac_block(&mut coefficients[..16 - offset], cat, Some(inc))
        } else {
            let nc = self.luma_nc(at, raster);
            self.cavlc_block(&mut coefficients[..16 - offset], nc)
        };
        if offset > 0 {
            coefficients.rotate_right(offset);
            coefficients[0] = 0;
        }
        residual.luma[raster] = coefficients;
        count
    }
}

/// Clause 9.2.1.
pub(super) fn combine(a: Option<i32>, b: Option<i32>) -> i32 {
    match (a, b) {
        (Some(a), Some(b)) => (a + b + 1) >> 1,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 0,
    }
}
