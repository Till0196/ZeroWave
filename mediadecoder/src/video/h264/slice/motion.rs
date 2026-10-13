// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Motion: reading the vectors and compensating from the references.

use super::*;

impl Coder<'_> {
    pub(super) fn read_ref(&mut self, at: usize, piece: &Piece, list: usize) -> i32 {
        let count = self.header.num_ref_idx[list];
        if count <= 1 {
            return 0;
        }
        if !self.cabac() {
            return match &mut self.entropy {
                Entropy::Bits(reader) => reader.te(count as u32 - 1) as i32,
                Entropy::Arithmetic(_) => 0,
            }
            .min(count as i32 - 1);
        }
        let (x, y) = (piece.x as i32, piece.y as i32);
        let current = piece.y * 4 + piece.x;
        let mut inc = 0;
        for (place, (dx, dy)) in [(-1i32, 0i32), (0, -1)].into_iter().enumerate() {
            let Some((mb, block)) = self.decoded_block(at, x + dx, y + dy, current) else {
                continue;
            };
            let neighbour = &self.mbs[mb];
            let quarter = Self::quarter(block);
            if neighbour.kind.intra()
                || neighbour.skipped
                || neighbour.direct
                || neighbour.direct_quarters >> quarter & 1 == 1
            {
                continue;
            }
            if self.block_ref(mb, block, list) > 0 {
                inc += 1 << place;
            }
        }
        if self.decision(54 + inc) == 0 {
            return 0;
        }
        if self.decision(54 + 4) == 0 {
            return 1;
        }
        let mut value = 2;
        while value < 32 && self.decision(54 + 5) == 1 {
            value += 1;
        }
        value
    }

    /// Clause 9.3.3.1.1.7.
    pub(super) fn mvd_context(&self, at: usize, piece: &Piece, list: usize, axis: usize) -> usize {
        let (x, y) = (piece.x as i32, piece.y as i32);
        let current = piece.y * 4 + piece.x;
        let mut sum = 0i32;
        for (dx, dy) in [(-1i32, 0i32), (0, -1)] {
            let Some((mb, block)) = self.decoded_block(at, x + dx, y + dy, current) else {
                continue;
            };
            sum += i32::from(self.mvd[mb][list][block][axis]).abs();
        }
        if sum < 3 {
            0
        } else if sum <= 32 {
            1
        } else {
            2
        }
    }

    pub(super) fn read_mvd(&mut self, at: usize, piece: &Piece, list: usize, axis: usize) -> i32 {
        if !self.cabac() {
            return self.se();
        }
        let base = if axis == 0 { 40 } else { 47 };
        let inc = self.mvd_context(at, piece, list, axis);
        if self.decision(base + inc) == 0 {
            return 0;
        }
        let mut value = 1u32;
        while value < 9 && self.decision(base + (value as usize + 2).min(6)) == 1 {
            value += 1;
        }
        if value == 9 {
            value += match &mut self.entropy {
                Entropy::Arithmetic(cabac) => cabac.golomb(3),
                Entropy::Bits(_) => 0,
            };
        }
        if self.bit() == 1 {
            -(value as i32)
        } else {
            value as i32
        }
    }

    pub(super) fn read_motion(&mut self, at: usize, current: &mut Current) {
        if current.direct {
            self.mbs[at].direct = true;
            self.direct_motion(at, current);
            return;
        }
        for piece in current.pieces().to_vec() {
            if piece.direct {
                self.mbs[at].direct_quarters |= 1 << Self::quarter(piece.y * 4 + piece.x);
            }
        }
        // All references before any motion: motion is predicted from the neighbours' references.
        let mut references = [[-1i32; 4]; 2];
        for (list, references) in references.iter_mut().enumerate() {
            for (part, reference) in references.iter_mut().enumerate().take(current.parts) {
                let Some(piece) = current
                    .pieces()
                    .iter()
                    .find(|piece| piece.part == part)
                    .copied()
                else {
                    continue;
                };
                if piece.direct || piece.lists >> list & 1 == 0 {
                    continue;
                }
                *reference = if current.ref0 {
                    0
                } else {
                    self.read_ref(at, &piece, list)
                };
                let chosen = *reference;
                for piece in current.pieces().to_vec() {
                    if piece.part == part {
                        self.put_reference(at, &piece, list, chosen);
                    }
                }
            }
        }
        // Direct quarters are filled in first: later pieces may predict from them.
        for piece in current.pieces().to_vec() {
            if piece.direct {
                self.direct_piece(at, &piece);
            }
        }
        for (list, references) in references.iter().enumerate() {
            for piece in current.pieces().to_vec() {
                if piece.direct || piece.lists >> list & 1 == 0 {
                    continue;
                }
                let reference = references[piece.part];
                let predicted = self.predict_motion(
                    at,
                    piece.x,
                    piece.y,
                    piece.wide,
                    list,
                    reference,
                    current.parts,
                    current.across,
                    piece.part,
                );
                let dx = self.read_mvd(at, &piece, list, 0);
                let dy = self.read_mvd(at, &piece, list, 1);
                for row in 0..piece.tall {
                    for column in 0..piece.wide {
                        let block = (piece.y + row) * 4 + piece.x + column;
                        self.mvd[at][list][block] = [dx as i16, dy as i16];
                    }
                }
                let mv = [
                    predicted[0].wrapping_add(dx as i16),
                    predicted[1].wrapping_add(dy as i16),
                ];
                self.put_motion(at, &piece, list, reference, mv);
            }
        }
    }

    pub(super) fn colocated(&self, at: usize, block: usize) -> Option<(i32, [i16; 2])> {
        let mbs = self.colocated.mbs?;
        let mb = mbs.get(at)?;
        if mb.kind.intra() {
            return Some((-1, [0; 2]));
        }
        // direct_8x8_inference: the quarter's corner block is read.
        let block = if self.sps.direct_8x8_inference {
            usize::from(BLOCK_TO_RASTER[5 * (Self::quarter(block))])
        } else {
            block
        };
        let quarter = Self::quarter(block);
        if mb.refs[0][quarter] >= 0 {
            Some((i32::from(mb.refs[0][quarter]), mb.mv[0][block]))
        } else {
            Some((i32::from(mb.refs[1][quarter]), mb.mv[1][block]))
        }
    }

    pub(super) fn direct_motion(&mut self, at: usize, current: &Current) {
        for piece in current.pieces().to_vec() {
            self.direct_piece(at, &piece);
        }
    }

    pub(super) fn direct_piece(&mut self, at: usize, piece: &Piece) {
        if self.header.direct_spatial {
            self.direct_spatial(at, piece);
        } else {
            self.direct_temporal(at, piece);
        }
    }

    /// Clause 8.4.1.2.2.
    pub(super) fn direct_spatial(&mut self, at: usize, piece: &Piece) {
        let mut reference = [-1i32; 2];
        let mut predicted = [[0i16; 2]; 2];
        for (list, reference) in reference.iter_mut().enumerate() {
            let a = self.neighbour_motion(at, -1, 0, list, 0).0;
            let b = self.neighbour_motion(at, 0, -1, list, 0).0;
            let c = match self.decoded_block(at, 4, -1, 0) {
                Some(_) => self.neighbour_motion(at, 4, -1, list, 0).0,
                None => self.neighbour_motion(at, -1, -1, list, 0).0,
            };
            *reference = smallest(a, smallest(b, c));
        }
        let nothing = reference[0] < 0 && reference[1] < 0;
        if nothing {
            reference = [0; 2];
        }
        for list in 0..2 {
            if reference[list] >= 0 {
                predicted[list] =
                    self.predict_motion(at, 0, 0, 4, list, reference[list], 1, false, 0);
            }
        }
        // colZeroFlag: the piece is left still.
        let short_term = self.refs[1].first().is_some_and(|first| !first.long_term);
        for list in 0..2 {
            let used = reference[list];
            for row in 0..piece.tall {
                for column in 0..piece.wide {
                    let block = (piece.y + row) * 4 + piece.x + column;
                    let still = short_term
                        && self.colocated(at, block).is_some_and(|(reference, mv)| {
                            reference == 0 && mv[0].abs() <= 1 && mv[1].abs() <= 1
                        });
                    self.mbs[at].mv[list][block] = if nothing || used < 0 || (used == 0 && still) {
                        [0; 2]
                    } else {
                        predicted[list]
                    };
                }
            }
            self.put_reference(at, piece, list, used);
        }
    }

    /// Clause 8.4.1.2.3. The colocated block's reference, found again in this slice's list.
    pub(super) fn direct_temporal(&mut self, at: usize, piece: &Piece) {
        let colocated_poc = self.colocated.poc;
        let mut chosen = 0i32;
        for row in 0..piece.tall {
            for column in 0..piece.wide {
                let block = (piece.y + row) * 4 + piece.x + column;
                let quarter = Self::quarter(block);
                let (used, mv) = self.colocated(at, block).unwrap_or((-1, [0; 2]));
                let target = self
                    .colocated
                    .mbs
                    .and_then(|mbs| mbs.get(at))
                    .map(|mb| mb.pics[usize::from(mb.refs[0][quarter] < 0)][quarter])
                    .unwrap_or(-1);
                let index = if used < 0 {
                    0
                } else {
                    self.refs[0]
                        .iter()
                        .position(|found| found.id == target)
                        .unwrap_or(0) as i32
                };
                chosen = index;
                let entry = self.refs[0].get(index as usize).copied();
                let zero = entry.map_or(0, |found| found.poc);
                let long = entry.is_some_and(|found| found.long_term);
                let td = (colocated_poc - zero).clamp(-128, 127);
                let (first, second) = if long || td == 0 {
                    // Long term or the same moment: no distance to divide by.
                    (mv, [0i16; 2])
                } else {
                    let tb = (self.poc - zero).clamp(-128, 127);
                    let tx = (16384 + (td / 2).abs()) / td;
                    let scale = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                    let scaled = [
                        ((scale * i32::from(mv[0]) + 128) >> 8) as i16,
                        ((scale * i32::from(mv[1]) + 128) >> 8) as i16,
                    ];
                    (scaled, [scaled[0] - mv[0], scaled[1] - mv[1]])
                };
                self.mbs[at].mv[0][block] = first;
                self.mbs[at].mv[1][block] = second;
            }
        }
        self.put_reference(at, piece, 0, chosen);
        self.put_reference(at, piece, 1, 0);
    }
}

/// Clause 8.4.1.2.2 `MinPositive`.
pub(super) fn smallest(a: i32, b: i32) -> i32 {
    if a >= 0 && b >= 0 { a.min(b) } else { a.max(b) }
}

impl Coder<'_> {
    pub(super) fn view(&self, reference: &Reference, plane: usize) -> View<'_> {
        let planes: &[Vec<u8>; 3] = match reference.buffer {
            Some(index) => &self.others[index],
            None => &*self.planes,
        };
        let stride = self.geometry.stride[plane];
        let frame = self.geometry.height[plane] * if self.geometry.field { 2 } else { 1 };
        match reference.parity {
            None => View {
                data: &planes[plane],
                at: 0,
                stride,
                width: self.geometry.width[plane],
                height: frame,
            },
            Some(parity) => View {
                data: &planes[plane],
                at: parity * stride,
                stride: stride * 2,
                width: self.geometry.width[plane],
                height: frame / 2,
            },
        }
    }

    /// Table 8-10: a field from the other parity is half a chroma line out.
    pub(super) fn chroma_offset(&self, reference: &Reference) -> i32 {
        if !self.geometry.field {
            return 0;
        }
        let Some(parity) = reference.parity else {
            return 0;
        };
        let own = usize::from(self.geometry.bottom);
        if own == parity {
            0
        } else if own == 1 {
            2
        } else {
            -2
        }
    }

    pub(super) fn weighting(
        &self,
        component: Option<usize>,
        index: [i32; 2],
        both: bool,
    ) -> Weighting {
        if self.header.weighted {
            let which = usize::from(component.is_some());
            let mut weighting = Weighting {
                denominator: self.header.log2_weight_denom[which],
                weight: [1; 2],
                offset: [0; 2],
            };
            for (list, at) in index.iter().enumerate() {
                let entry = self.header.weights[list][at.clamp(&0, &31).unsigned_abs() as usize];
                let (weight, offset) = match component {
                    None => entry.luma,
                    Some(component) => entry.chroma[component],
                };
                weighting.weight[list] = weight;
                weighting.offset[list] = offset;
            }
            return weighting;
        }
        if both && self.pps.weighted_bipred == 2 && self.header.kind == SliceKind::B {
            let one = self
                .implicit
                .get(index[0].max(0) as usize)
                .and_then(|row| row.get(index[1].max(0) as usize))
                .copied()
                .unwrap_or(32);
            return Weighting {
                denominator: 5,
                weight: [64 - one, one],
                offset: [0; 2],
            };
        }
        Weighting::default()
    }

    pub(super) fn compensate(&mut self, at: usize, current: &Current) {
        let (mb_x, mb_y) = (at % self.geometry.mb_width, at / self.geometry.mb_width);
        let mut work = self.work.take().unwrap_or_default();
        let Work {
            scratch,
            luma,
            chroma,
        } = &mut *work;
        for piece in current.pieces() {
            let block = piece.y * 4 + piece.x;
            let quarter = Self::quarter(block);
            let (wide, tall) = (piece.wide * 4, piece.tall * 4);
            let px = mb_x * 16 + piece.x * 4;
            let py = mb_y * 16 + piece.y * 4;

            let mut used = [false; 2];
            let mut index = [0i32; 2];
            for list in 0..2 {
                let reference = i32::from(self.mbs[at].refs[list][quarter]);
                if reference < 0 {
                    continue;
                }
                let Some(found) = self.refs[list].get(reference as usize).copied() else {
                    continue;
                };
                used[list] = true;
                index[list] = reference;
                let mv = self.mbs[at].mv[list][block];
                let view = self.view(&found, 0);
                inter::luma(
                    &mut luma[list][..wide * tall],
                    wide,
                    tall,
                    &view,
                    px as i32 * 4 + i32::from(mv[0]),
                    py as i32 * 4 + i32::from(mv[1]),
                    scratch,
                );
                if self.sps.chroma_array() == 0 {
                    continue;
                }
                let offset = self.chroma_offset(&found);
                for (component, into) in chroma[list].iter_mut().enumerate() {
                    let view = self.view(&found, component + 1);
                    inter::chroma(
                        &mut into[..(wide / 2) * (tall / 2)],
                        wide / 2,
                        tall / 2,
                        &view,
                        (px / 2) as i32 * 8 + i32::from(mv[0]),
                        (py / 2) as i32 * 8 + i32::from(mv[1]) + offset,
                    );
                }
            }
            if !used[0] && !used[1] {
                continue;
            }
            let both = used[0] && used[1];
            let single = usize::from(used[1]);

            let weighting = self.weighting(None, index, both);
            let corner = self.sample(0, px, py);
            let stride = self.geometry.row[0];
            if both {
                inter::both(
                    &mut self.planes[0],
                    corner,
                    stride,
                    &luma[0][..wide * tall],
                    &luma[1][..wide * tall],
                    wide,
                    tall,
                    &weighting,
                );
            } else {
                inter::single(
                    &mut self.planes[0],
                    corner,
                    stride,
                    &luma[single][..wide * tall],
                    wide,
                    tall,
                    &weighting,
                    single,
                );
            }
            if self.sps.chroma_array() == 0 {
                continue;
            }
            for (component, (zero, one)) in chroma[0].iter().zip(&chroma[1]).enumerate() {
                let weighting = self.weighting(Some(component), index, both);
                let plane = component + 1;
                let corner = self.sample(plane, px / 2, py / 2);
                let stride = self.geometry.row[plane];
                let count = (wide / 2) * (tall / 2);
                if both {
                    inter::both(
                        &mut self.planes[plane],
                        corner,
                        stride,
                        &zero[..count],
                        &one[..count],
                        wide / 2,
                        tall / 2,
                        &weighting,
                    );
                } else {
                    inter::single(
                        &mut self.planes[plane],
                        corner,
                        stride,
                        if single == 0 {
                            &zero[..count]
                        } else {
                            &one[..count]
                        },
                        wide / 2,
                        tall / 2,
                        &weighting,
                        single,
                    );
                }
            }
        }
        self.work = Some(work);
    }
}
