// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Building the samples of a macroblock from prediction and residual.

use super::*;

impl Coder<'_> {
    /// Whether a neighbour may be predicted from (constrained_intra_pred: only if intra).
    pub(super) fn predictable(
        &self,
        at: usize,
        x: i32,
        y: i32,
        current: usize,
    ) -> Option<(usize, usize)> {
        let found = self.decoded_block(at, x, y, current)?;
        if self.pps.constrained_intra && !self.mbs[found.0].kind.intra() {
            return None;
        }
        Some(found)
    }

    pub(super) fn edge_block(
        &self,
        at: usize,
        px: usize,
        py: usize,
        size: usize,
        raster: usize,
    ) -> Edge {
        let (x, y) = ((raster % 4) as i32, (raster / 4) as i32);
        let blocks = (size / 4) as i32;
        let mut edge = Edge {
            has_top: self.predictable(at, x, y - 1, raster).is_some(),
            has_left: self.predictable(at, x - 1, y, raster).is_some(),
            has_corner: self.predictable(at, x - 1, y - 1, raster).is_some(),
            ..Edge::default()
        };
        let right = self.predictable(at, x + blocks, y - 1, raster).is_some();
        if edge.has_top {
            let row = self.sample(0, px, py - 1);
            for column in 0..size {
                edge.top[column] = self.planes[0][row + column];
            }
            for column in size..2 * size {
                edge.top[column] = if right {
                    self.planes[0][row + column]
                } else {
                    edge.top[size - 1]
                };
            }
        }
        if edge.has_left {
            for row in 0..size {
                edge.left[row] = self.planes[0][self.sample(0, px - 1, py + row)];
            }
        }
        if edge.has_corner {
            edge.corner = self.planes[0][self.sample(0, px - 1, py - 1)];
        }
        edge
    }

    pub(super) fn edge_macroblock(
        &self,
        at: usize,
        plane: usize,
        px: usize,
        py: usize,
        size: usize,
    ) -> Edge {
        let above = self.neighbour_mb(at, 0, -1);
        let left = self.neighbour_mb(at, -1, 0);
        let corner = self.neighbour_mb(at, -1, -1);
        let allowed = |found: Option<usize>| {
            found.filter(|&mb| !self.pps.constrained_intra || self.mbs[mb].kind.intra())
        };
        let mut edge = Edge {
            has_top: allowed(above).is_some(),
            has_left: allowed(left).is_some(),
            has_corner: allowed(corner).is_some(),
            ..Edge::default()
        };
        if edge.has_top {
            let row = self.sample(plane, px, py - 1);
            for column in 0..size {
                edge.top[column] = self.planes[plane][row + column];
            }
        }
        if edge.has_left {
            for row in 0..size {
                edge.left[row] = self.planes[plane][self.sample(plane, px - 1, py + row)];
            }
        }
        if edge.has_corner {
            edge.corner = self.planes[plane][self.sample(plane, px - 1, py - 1)];
        }
        edge
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn add_4x4(
        &mut self,
        plane: usize,
        px: usize,
        py: usize,
        coefficients: &[i32; 16],
        dc: Option<i32>,
        list: usize,
        qp: i32,
    ) {
        if dc.is_none() && coefficients.iter().all(|&value| value == 0) {
            return;
        }
        let scan = if self.geometry.field {
            &tables::FIELD_4X4
        } else {
            &tables::ZIGZAG_4X4
        };
        let mut block = [0i32; 16];
        for (at, &value) in coefficients.iter().enumerate() {
            block[usize::from(scan[at])] = value;
        }
        if let Some(value) = dc {
            block[0] = value;
        }
        let qp = qp.clamp(0, 51);
        transform::scale_4x4(
            &mut block,
            &self.levels[list].four[(qp % 6) as usize],
            qp,
            dc.is_some(),
        );
        transform::transform_4x4(&mut block);
        let stride = self.geometry.row[plane];
        let corner = self.sample(plane, px, py);
        for row in 0..4 {
            for column in 0..4 {
                let sample = &mut self.planes[plane][corner + row * stride + column];
                *sample = (i32::from(*sample) + block[row * 4 + column]).clamp(0, 255) as u8;
            }
        }
    }

    pub(super) fn add_8x8(
        &mut self,
        px: usize,
        py: usize,
        coefficients: &[i32; 64],
        list: usize,
        qp: i32,
    ) {
        if coefficients.iter().all(|&value| value == 0) {
            return;
        }
        let scan = if self.geometry.field {
            &tables::FIELD_8X8
        } else {
            &tables::ZIGZAG_8X8
        };
        let mut block = [0i32; 64];
        for (at, &value) in coefficients.iter().enumerate() {
            block[usize::from(scan[at])] = value;
        }
        let qp = qp.clamp(0, 51);
        transform::scale_8x8(&mut block, &self.levels[list].eight[(qp % 6) as usize], qp);
        transform::transform_8x8(&mut block);
        let stride = self.geometry.row[0];
        let corner = self.sample(0, px, py);
        for row in 0..8 {
            for column in 0..8 {
                let sample = &mut self.planes[0][corner + row * stride + column];
                *sample = (i32::from(*sample) + block[row * 8 + column]).clamp(0, 255) as u8;
            }
        }
    }

    pub(super) fn build(&mut self, at: usize, current: &Current, residual: &Residual) {
        if current.kind == Kind::Pcm {
            return;
        }
        let (mb_x, mb_y) = (at % self.geometry.mb_width, at / self.geometry.mb_width);
        let intra = current.kind.intra();
        let list = usize::from(!intra) * 3;
        let stride = self.geometry.row[0];

        match current.kind {
            Kind::Intra4x4 | Kind::Intra8x8 => {
                let size = if current.kind == Kind::Intra4x4 { 4 } else { 8 };
                let step = size / 4;
                let count = (4 / step) * (4 / step);
                // In the standard's order: each block predicts from the ones before.
                let order: Vec<usize> = if step == 1 {
                    BLOCK_TO_RASTER.iter().map(|&at| usize::from(at)).collect()
                } else {
                    (0..count).map(|at| (at / 2) * 8 + (at % 2) * 2).collect()
                };
                for raster in order {
                    let px = mb_x * 16 + (raster % 4) * 4;
                    let py = mb_y * 16 + (raster / 4) * 4;
                    let edge = self.edge_block(at, px, py, size, raster);
                    let edge = if step == 1 { edge } else { edge.filtered() };
                    let mode = (self.mbs[at].modes[raster].max(0) as usize).min(8);
                    let corner = self.sample(0, px, py);
                    let mut target = intra::Target {
                        data: &mut self.planes[0],
                        at: corner,
                        stride,
                    };
                    intra::square(&mut target, size, mode, &edge);
                    if step == 1 {
                        let coefficients = residual.luma[raster];
                        self.add_4x4(0, px, py, &coefficients, None, list, current.qp);
                    } else {
                        let quarter = (raster / 8) * 2 + (raster % 4) / 2;
                        let coefficients = residual.luma8[quarter];
                        self.add_8x8(px, py, &coefficients, list, current.qp);
                    }
                }
            }
            Kind::Intra16x16 => {
                let (px, py) = (mb_x * 16, mb_y * 16);
                let edge = self.edge_macroblock(at, 0, px, py, 16);
                let corner = self.sample(0, px, py);
                let mut target = intra::Target {
                    data: &mut self.planes[0],
                    at: corner,
                    stride,
                };
                intra::luma_16x16(&mut target, current.intra16_mode, &edge);
                let qp = current.qp.clamp(0, 51);
                let mut dc = residual.dc;
                let mut ordered = [0i32; 16];
                let scan = if self.geometry.field {
                    &tables::FIELD_4X4
                } else {
                    &tables::ZIGZAG_4X4
                };
                for (at, &value) in dc.iter().enumerate() {
                    ordered[usize::from(scan[at])] = value;
                }
                dc = ordered;
                transform::luma_dc(&mut dc, self.levels[list].four[(qp % 6) as usize][0], qp);
                for block in BLOCK_TO_RASTER {
                    let raster = usize::from(block);
                    let px = mb_x * 16 + (raster % 4) * 4;
                    let py = mb_y * 16 + (raster / 4) * 4;
                    let coefficients = residual.luma[raster];
                    self.add_4x4(0, px, py, &coefficients, Some(dc[raster]), list, current.qp);
                }
            }
            _ => {
                self.compensate(at, current);
                for quarter in 0..4usize {
                    if current.cbp_luma >> quarter & 1 == 0 {
                        continue;
                    }
                    if current.transform8x8 {
                        let px = mb_x * 16 + (quarter % 2) * 8;
                        let py = mb_y * 16 + (quarter / 2) * 8;
                        let coefficients = residual.luma8[quarter];
                        self.add_8x8(px, py, &coefficients, list, current.qp);
                        continue;
                    }
                    for piece in 0..4usize {
                        let raster = usize::from(BLOCK_TO_RASTER[quarter * 4 + piece]);
                        let px = mb_x * 16 + (raster % 4) * 4;
                        let py = mb_y * 16 + (raster / 4) * 4;
                        let coefficients = residual.luma[raster];
                        self.add_4x4(0, px, py, &coefficients, None, list, current.qp);
                    }
                }
            }
        }

        if self.sps.chroma_array() == 0 {
            return;
        }
        for component in 0..2usize {
            let plane = component + 1;
            let (px, py) = (mb_x * 8, mb_y * 8);
            if intra {
                let edge = self.edge_macroblock(at, plane, px, py, 8);
                let corner = self.sample(plane, px, py);
                let stride = self.geometry.row[plane];
                let mut target = intra::Target {
                    data: &mut self.planes[plane],
                    at: corner,
                    stride,
                };
                intra::chroma(&mut target, current.chroma_mode, &edge);
            }
            let qp = self.chroma_qp(at, component);
            let mut dc = residual.chroma_dc[component];
            transform::chroma_dc(
                &mut dc,
                self.levels[if intra { 1 + component } else { 4 + component }].four
                    [(qp.clamp(0, 51) % 6) as usize][0],
                qp.clamp(0, 51),
            );
            for (block, coefficients) in residual.chroma[component].iter().enumerate() {
                self.add_4x4(
                    plane,
                    px + (block % 2) * 4,
                    py + (block / 2) * 4,
                    coefficients,
                    Some(dc[block]),
                    if intra { 1 + component } else { 4 + component },
                    qp,
                );
            }
        }
    }
}
