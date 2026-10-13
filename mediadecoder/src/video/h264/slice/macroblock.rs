// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! One macroblock, whole: read, then built.

use super::*;

impl Coder<'_> {
    pub(super) fn sample(&self, plane: usize, x: usize, y: usize) -> usize {
        self.geometry.at[plane] + y * self.geometry.row[plane] + x
    }

    pub(super) fn quarter(block: usize) -> usize {
        (block / 8) * 2 + (block % 4) / 2
    }

    pub(super) fn block_ref(&self, mb: usize, block: usize, list: usize) -> i32 {
        i32::from(self.mbs[mb].refs[list][Self::quarter(block)])
    }

    pub(super) fn neighbour_motion(
        &self,
        at: usize,
        x: i32,
        y: i32,
        list: usize,
        current: usize,
    ) -> (i32, [i16; 2]) {
        let Some((mb, block)) = self.decoded_block(at, x, y, current) else {
            return (-1, [0; 2]);
        };
        if self.mbs[mb].kind.intra() {
            return (-1, [0; 2]);
        }
        (
            self.block_ref(mb, block, list),
            self.mbs[mb].mv[list][block],
        )
    }

    /// Clause 8.4.1.3.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn predict_motion(
        &self,
        at: usize,
        x: usize,
        y: usize,
        wide: usize,
        list: usize,
        reference: i32,
        parts: usize,
        across: bool,
        part: usize,
    ) -> [i16; 2] {
        let (x, y) = (x as i32, y as i32);
        let current = (y * 4 + x) as usize;
        let a = self.neighbour_motion(at, x - 1, y, list, current);
        let mut b = self.neighbour_motion(at, x, y - 1, list, current);
        let right = self.decoded_block(at, x + wide as i32, y - 1, current);
        let mut c = match right {
            Some(_) => self.neighbour_motion(at, x + wide as i32, y - 1, list, current),
            // Above-right not decoded: above-left stands in.
            None => self.neighbour_motion(at, x - 1, y - 1, list, current),
        };
        if self.decoded_block(at, x, y - 1, current).is_none()
            && right.is_none()
            && self.decoded_block(at, x - 1, y - 1, current).is_none()
            && self.decoded_block(at, x - 1, y, current).is_some()
        {
            b = a;
            c = a;
        }

        if parts == 2 {
            if !across && part == 0 && b.0 == reference {
                return b.1;
            }
            if !across && part == 1 && a.0 == reference {
                return a.1;
            }
            if across && part == 0 && a.0 == reference {
                return a.1;
            }
            if across && part == 1 && c.0 == reference {
                return c.1;
            }
        }
        let matches = [a, b, c].iter().filter(|n| n.0 == reference).count();
        if matches == 1 {
            for neighbour in [a, b, c] {
                if neighbour.0 == reference {
                    return neighbour.1;
                }
            }
        }
        [
            median(a.1[0], b.1[0], c.1[0]),
            median(a.1[1], b.1[1], c.1[1]),
        ]
    }

    pub(super) fn put_motion(
        &mut self,
        at: usize,
        piece: &Piece,
        list: usize,
        reference: i32,
        mv: [i16; 2],
    ) {
        for row in 0..piece.tall {
            for column in 0..piece.wide {
                let block = (piece.y + row) * 4 + piece.x + column;
                self.mbs[at].mv[list][block] = mv;
            }
        }
        self.put_reference(at, piece, list, reference);
    }

    pub(super) fn put_reference(&mut self, at: usize, piece: &Piece, list: usize, reference: i32) {
        let id = self.refs[list]
            .get(reference.max(0) as usize)
            .map_or(-1, |found| found.id);
        for row in 0..piece.tall.div_ceil(2) {
            for column in 0..piece.wide.div_ceil(2) {
                let quarter = ((piece.y + row * 2) / 2) * 2 + (piece.x + column * 2) / 2;
                self.mbs[at].refs[list][quarter] = reference as i8;
                self.mbs[at].pics[list][quarter] = if reference < 0 { -1 } else { id };
            }
        }
    }
}

pub(super) fn median(a: i16, b: i16, c: i16) -> i16 {
    a.max(b).min(a.min(b).max(c))
}

impl Coder<'_> {
    pub(super) fn macroblock(&mut self, at: usize) {
        let raw = self.read_kind(at);
        let Some(mut current) = self.interpret(raw) else {
            return;
        };
        self.begin(at, current.kind);

        if current.kind == Kind::Pcm {
            self.pcm(at);
            return;
        }
        if current.kind == Kind::Intra4x4 && self.pps.transform_8x8 && self.read_transform(at) {
            current.kind = Kind::Intra8x8;
            current.transform8x8 = true;
        }
        self.mbs[at].kind = current.kind;
        self.mbs[at].transform8x8 = current.transform8x8;

        if current.kind.intra() {
            self.read_intra(at, &mut current);
        } else {
            self.read_motion(at, &mut current);
        }

        if current.kind != Kind::Intra16x16 {
            let cbp = self.read_cbp(at, current.kind.intra());
            current.cbp_luma = (cbp & 15) as u8;
            current.cbp_chroma = (cbp >> 4) as u8;
            // A transform may not straddle two predictions.
            let small = current.parts == 4
                && current.pieces().iter().any(|piece| {
                    if piece.direct {
                        !self.sps.direct_8x8_inference
                    } else {
                        piece.wide < 2 || piece.tall < 2
                    }
                });
            if current.cbp_luma > 0
                && self.pps.transform_8x8
                && current.kind == Kind::Inter
                && !small
                && (!current.direct || self.sps.direct_8x8_inference)
                && self.read_transform(at)
            {
                current.transform8x8 = true;
                self.mbs[at].transform8x8 = true;
            }
        }
        self.mbs[at].cbp = current.cbp_luma | current.cbp_chroma << 4;

        let mut residual = Residual::default();
        if current.cbp_luma != 0 || current.cbp_chroma != 0 || current.kind == Kind::Intra16x16 {
            let delta = self.read_qp_delta();
            self.last_delta = delta != 0;
            self.qp = (self.qp + delta + 52).rem_euclid(52);
            self.read_residual(at, &current, &mut residual);
        } else {
            self.last_delta = false;
        }
        current.qp = self.qp;
        self.settle(at, current.qp);
        self.build(at, &current, &residual);
    }

    pub(super) fn skipped_macroblock(&mut self, at: usize) {
        let mut current = Current {
            kind: Kind::Inter,
            qp: self.qp,
            ..Current::default()
        };
        self.begin(at, Kind::Inter);
        self.mbs[at].skipped = true;
        self.last_delta = false;
        if self.header.kind == SliceKind::B {
            current.direct = true;
            current.parts = 4;
            for quarter in 0..4 {
                self.quarter_pieces(&mut current, quarter, 2, 2, 3, true);
            }
            self.mbs[at].direct = true;
            self.direct_motion(at, &current);
        } else {
            current.push(Piece {
                x: 0,
                y: 0,
                wide: 4,
                tall: 4,
                lists: 1,
                direct: false,
                part: 0,
            });
            let piece = current.pieces()[0];
            self.put_motion(at, &piece, 0, 0, [0; 2]);
            let mv = self.skip_motion(at);
            self.put_motion(at, &piece, 0, 0, mv);
            self.put_motion(at, &piece, 1, -1, [0; 2]);
        }
        self.settle(at, self.qp);
        self.build(at, &current, &Residual::default());
    }

    /// Clause 8.4.1.1.
    pub(super) fn skip_motion(&self, at: usize) -> [i16; 2] {
        let a = self.decoded_block(at, -1, 0, 0);
        let b = self.decoded_block(at, 0, -1, 0);
        if a.is_none() || b.is_none() {
            return [0; 2];
        }
        for (x, y) in [(-1i32, 0i32), (0, -1)] {
            let (reference, mv) = self.neighbour_motion(at, x, y, 0, 0);
            if reference == 0 && mv == [0, 0] {
                return [0; 2];
            }
        }
        self.predict_motion(at, 0, 0, 4, 0, 0, 1, false, 0)
    }

    pub(super) fn begin(&mut self, at: usize, kind: Kind) {
        self.mbs[at] = Mb {
            kind,
            slice: self.slice,
            deblock: self.header.disable_deblocking as u8,
            alpha: self.header.alpha_offset as i8,
            beta: self.header.beta_offset as i8,
            ..Mb::default()
        };
        self.mvd[at] = [[[0; 2]; 16]; 2];
    }

    pub(super) fn settle(&mut self, at: usize, qp: i32) {
        self.mbs[at].qpy = qp as i8;
        for component in 0..2 {
            let index = (qp + self.pps.chroma_qp_offset[component]).clamp(0, 51) as usize;
            self.mbs[at].qpc[component] = tables::CHROMA_QP[index] as i8;
        }
    }

    pub(super) fn chroma_qp(&self, at: usize, component: usize) -> i32 {
        i32::from(self.mbs[at].qpc[component])
    }

    pub(super) fn pcm(&mut self, at: usize) {
        let (mb_x, mb_y) = (at % self.geometry.mb_width, at / self.geometry.mb_width);
        let Entropy::Bits(reader) = &mut self.entropy else {
            // CABAC would need the engine restarted after the samples; no broadcast does this.
            return;
        };
        reader.align();
        let mut plane_samples = [[0u8; 256]; 3];
        for (plane, samples) in plane_samples.iter_mut().enumerate() {
            let count = if plane == 0 { 256 } else { 64 };
            for sample in &mut samples[..count] {
                *sample = reader.bits(8) as u8;
            }
        }
        for (plane, samples) in plane_samples.iter().enumerate() {
            let size = if plane == 0 { 16 } else { 8 };
            for y in 0..size {
                let at = self.sample(plane, mb_x * size, mb_y * size + y);
                self.planes[plane][at..at + size]
                    .copy_from_slice(&samples[y * size..y * size + size]);
            }
        }
        // PCM edges are filtered at full strength.
        self.mbs[at].nnz = [16; 24];
        self.mbs[at].dc_nnz = 7;
        self.mbs[at].qpy = 0;
        self.settle(at, 0);
    }
}

impl Coder<'_> {
    pub(super) fn read_transform(&mut self, at: usize) -> bool {
        if !self.cabac() {
            return self.bit() == 1;
        }
        let mut inc = 0;
        for (dx, dy) in [(-1i32, 0i32), (0, -1)] {
            if let Some(other) = self.neighbour_mb(at, dx, dy)
                && self.mbs[other].transform8x8
            {
                inc += 1;
            }
        }
        self.decision(399 + inc) == 1
    }

    /// Clause 8.3.1.1: a neighbour's mode, or nothing. One present but not intra NxN counts as DC.
    pub(super) fn mode_of(&self, at: usize, x: i32, y: i32, current: usize) -> Option<i32> {
        let (mb, block) = self.decoded_block(at, x, y, current)?;
        let mb = &self.mbs[mb];
        if self.pps.constrained_intra && !mb.kind.intra() {
            return None;
        }
        if !matches!(mb.kind, Kind::Intra4x4 | Kind::Intra8x8) {
            return Some(2);
        }
        Some(i32::from(mb.modes[block]).max(0))
    }

    /// `dcPredModePredictedFlag` is one flag for both neighbours: either missing means DC.
    pub(super) fn read_mode(&mut self, at: usize, raster: usize) -> i8 {
        let (x, y) = ((raster % 4) as i32, (raster / 4) as i32);
        let predicted = match (
            self.mode_of(at, x - 1, y, raster),
            self.mode_of(at, x, y - 1, raster),
        ) {
            (Some(left), Some(above)) => left.min(above),
            _ => 2,
        };
        let previous = if self.cabac() {
            self.decision(68)
        } else {
            self.bit()
        };
        if previous == 1 {
            return predicted as i8;
        }
        let rest = if self.cabac() {
            // Fixed length bins are written least significant first.
            self.decision(69) | self.decision(69) << 1 | self.decision(69) << 2
        } else {
            match &mut self.entropy {
                Entropy::Bits(reader) => reader.bits(3),
                Entropy::Arithmetic(_) => 0,
            }
        } as i32;
        (if rest >= predicted { rest + 1 } else { rest }) as i8
    }

    pub(super) fn read_chroma_mode(&mut self, at: usize) -> usize {
        if !self.cabac() {
            return self.ue().min(3) as usize;
        }
        let mut inc = 0;
        for (dx, dy) in [(-1i32, 0i32), (0, -1)] {
            if let Some(other) = self.neighbour_mb(at, dx, dy) {
                let mb = &self.mbs[other];
                if mb.kind.intra() && mb.kind != Kind::Pcm && mb.chroma_mode != 0 {
                    inc += 1;
                }
            }
        }
        if self.decision(64 + inc) == 0 {
            return 0;
        }
        if self.decision(64 + 3) == 0 {
            return 1;
        }
        if self.decision(64 + 3) == 0 { 2 } else { 3 }
    }

    pub(super) fn read_intra(&mut self, at: usize, current: &mut Current) {
        match current.kind {
            Kind::Intra4x4 => {
                for block in BLOCK_TO_RASTER {
                    let raster = usize::from(block);
                    let mode = self.read_mode(at, raster);
                    self.mbs[at].modes[raster] = mode;
                }
            }
            Kind::Intra8x8 => {
                for block in 0..4 {
                    let raster = (block / 2) * 8 + (block % 2) * 2;
                    let mode = self.read_mode(at, raster);
                    for row in 0..2 {
                        for column in 0..2 {
                            self.mbs[at].modes[raster + row * 4 + column] = mode;
                        }
                    }
                }
            }
            _ => {}
        }
        if self.sps.chroma_array() != 0 {
            current.chroma_mode = self.read_chroma_mode(at);
            self.mbs[at].chroma_mode = current.chroma_mode as u8;
        }
    }

    pub(super) fn read_cbp(&mut self, at: usize, intra: bool) -> u32 {
        if !self.cabac() {
            let code = self.ue() as usize;
            let table = if intra {
                &tables::CBP_INTRA
            } else {
                &tables::CBP_INTER
            };
            return u32::from(table.get(code).copied().unwrap_or(0));
        }
        let mut luma = 0u32;
        for quarter in 0..4usize {
            let (qx, qy) = (quarter % 2, quarter / 2);
            let left = if qx > 0 {
                Some(luma >> (quarter - 1) & 1 == 1)
            } else {
                self.quarter_cbp(at, -1, 0, qy * 2 + 1)
            };
            let above = if qy > 0 {
                Some(luma >> (quarter - 2) & 1 == 1)
            } else {
                self.quarter_cbp(at, 0, -1, 2 + qx)
            };
            let inc = usize::from(left == Some(false)) + 2 * usize::from(above == Some(false));
            luma |= self.decision(73 + inc) << quarter;
        }
        let mut chroma = 0u32;
        let mut inc = 0;
        for (place, (dx, dy)) in [(-1i32, 0i32), (0, -1)].into_iter().enumerate() {
            if let Some(other) = self.neighbour_mb(at, dx, dy) {
                let mb = &self.mbs[other];
                if mb.kind == Kind::Pcm || (!mb.skipped && mb.cbp >> 4 != 0) {
                    inc += 1 << place;
                }
            }
        }
        if self.decision(77 + inc) == 1 {
            let mut inc = 0;
            for (place, (dx, dy)) in [(-1i32, 0i32), (0, -1)].into_iter().enumerate() {
                if let Some(other) = self.neighbour_mb(at, dx, dy) {
                    let mb = &self.mbs[other];
                    if mb.kind == Kind::Pcm || (!mb.skipped && mb.cbp >> 4 == 2) {
                        inc += 1 << place;
                    }
                }
            }
            chroma = 1 + self.decision(77 + 4 + inc);
        }
        luma | chroma << 4
    }

    pub(super) fn quarter_cbp(&self, at: usize, dx: i32, dy: i32, quarter: usize) -> Option<bool> {
        let other = self.neighbour_mb(at, dx, dy)?;
        let mb = &self.mbs[other];
        if mb.kind == Kind::Pcm {
            return Some(true);
        }
        Some(mb.cbp >> quarter & 1 == 1)
    }

    pub(super) fn read_qp_delta(&mut self) -> i32 {
        if !self.cabac() {
            return self.se();
        }
        let mut inc = usize::from(self.last_delta);
        let mut value = 0;
        while self.decision(60 + inc) == 1 && value < 88 {
            value += 1;
            inc = 2 + (inc >> 1);
        }
        if value % 2 == 1 {
            (value + 1) >> 1
        } else {
            -((value + 1) >> 1)
        }
    }
}
