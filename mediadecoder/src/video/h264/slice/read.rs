// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Reading a macroblock: its kind, and the coded blocks it carries.

use super::*;

impl Coder<'_> {
    /// Clause 9.3.3.1.1.3.
    pub(super) fn kind_context(&self, at: usize, intra: bool) -> usize {
        let mut context = 0;
        for (dx, dy) in [(-1i32, 0i32), (0, -1)] {
            let Some(other) = self.neighbour_mb(at, dx, dy) else {
                continue;
            };
            let mb = &self.mbs[other];
            let plain = if intra {
                matches!(mb.kind, Kind::Intra4x4 | Kind::Intra8x8)
            } else {
                mb.skipped || mb.direct
            };
            if !plain {
                context += 1;
            }
        }
        context
    }

    pub(super) fn intra_kind(&mut self, base: usize, own_slice: bool, at: usize) -> u32 {
        // First bin: 4x4/8x8 or 16x16. Only an intra slice's context depends on the neighbours.
        let mut base = base;
        let first = if own_slice {
            base + self.kind_context(at, true)
        } else {
            base
        };
        if self.decision(first) == 0 {
            return 0;
        }
        if own_slice {
            base += 2;
        }
        let terminated = match &mut self.entropy {
            Entropy::Bits(_) => false,
            Entropy::Arithmetic(cabac) => cabac.terminate(),
        };
        if terminated {
            return 25;
        }
        let step = usize::from(own_slice);
        let mut kind = 1 + 12 * self.decision(base + 1);
        if self.decision(base + 2) == 1 {
            kind += 4 + 4 * self.decision(base + 2 + step);
        }
        kind += 2 * self.decision(base + 3 + step);
        kind += self.decision(base + 3 + 2 * step);
        kind
    }

    pub(super) fn read_kind(&mut self, at: usize) -> u32 {
        if !self.cabac() {
            return self.ue();
        }
        match self.header.kind {
            SliceKind::I | SliceKind::Si => self.intra_kind(3, true, at),
            SliceKind::P | SliceKind::Sp => {
                if self.decision(14) == 1 {
                    return self.intra_kind(17, false, at) + 5;
                }
                if self.decision(15) == 0 {
                    3 * self.decision(16)
                } else {
                    2 - self.decision(17)
                }
            }
            SliceKind::B => {
                let context = 27 + self.kind_context(at, false);
                if self.decision(context) == 0 {
                    return 0;
                }
                if self.decision(27 + 3) == 0 {
                    return 1 + self.decision(27 + 5);
                }
                let mut bits = self.decision(27 + 4) << 3;
                bits |= self.decision(27 + 5) << 2;
                bits |= self.decision(27 + 5) << 1;
                bits |= self.decision(27 + 5);
                match bits {
                    0..=7 => bits + 3,
                    13 => self.intra_kind(32, false, at) + 23,
                    14 => 11,
                    15 => 22,
                    _ => (bits << 1 | self.decision(27 + 5)) - 4,
                }
            }
        }
    }

    pub(super) fn read_sub_kind(&mut self) -> u32 {
        if !self.cabac() {
            return self.ue();
        }
        if self.header.kind == SliceKind::B {
            if self.decision(36) == 0 {
                return 0;
            }
            if self.decision(37) == 0 {
                return 1 + self.decision(39);
            }
            let mut kind = 3;
            if self.decision(38) == 1 {
                if self.decision(39) == 1 {
                    return 11 + self.decision(39);
                }
                kind += 4;
            }
            kind += 2 * self.decision(39);
            kind + self.decision(39)
        } else {
            if self.decision(21) == 1 {
                return 0;
            }
            if self.decision(22) == 0 {
                return 1;
            }
            if self.decision(23) == 1 { 2 } else { 3 }
        }
    }

    pub(super) fn interpret(&mut self, raw: u32) -> Option<Current> {
        let mut current = Current::default();
        let intra_at = match self.header.kind {
            SliceKind::I | SliceKind::Si => Some(0),
            SliceKind::P | SliceKind::Sp if raw >= 5 => Some(5),
            SliceKind::B if raw >= 23 => Some(23),
            _ => None,
        };
        if let Some(offset) = intra_at {
            let kind = raw - offset;
            if kind == 0 {
                current.kind = Kind::Intra4x4;
            } else if kind == 25 {
                current.kind = Kind::Pcm;
            } else if kind <= 24 {
                let index = kind - 1;
                current.kind = Kind::Intra16x16;
                current.intra16_mode = (index % 4) as usize;
                current.cbp_chroma = ((index / 4) % 3) as u8;
                current.cbp_luma = if index >= 12 { 15 } else { 0 };
            } else {
                return None;
            }
            return Some(current);
        }

        current.kind = Kind::Inter;
        match self.header.kind {
            SliceKind::B if raw == 0 => {
                current.direct = true;
                current.parts = 4;
                for quarter in 0..4 {
                    self.quarter_pieces(&mut current, quarter, 2, 2, 3, true);
                }
            }
            SliceKind::B if raw == 22 => self.sub_pieces(&mut current),
            SliceKind::B => {
                let (count, across, first, second) = *B_PARTS.get(raw as usize - 1)?;
                self.whole_pieces(&mut current, count as usize, across, first, second);
            }
            _ => match raw {
                0 => self.whole_pieces(&mut current, 1, false, 1, 1),
                1 => self.whole_pieces(&mut current, 2, false, 1, 1),
                2 => self.whole_pieces(&mut current, 2, true, 1, 1),
                3 | 4 => {
                    current.ref0 = raw == 4;
                    self.sub_pieces(&mut current);
                }
                _ => return None,
            },
        }
        Some(current)
    }

    pub(super) fn whole_pieces(
        &self,
        current: &mut Current,
        count: usize,
        across: bool,
        one: u8,
        two: u8,
    ) {
        current.parts = count;
        current.across = across;
        for part in 0..count {
            let lists = if part == 0 { one } else { two };
            let (x, y, wide, tall) = match (count, across) {
                (1, _) => (0, 0, 4, 4),
                (_, true) => (part * 2, 0, 2, 4),
                (_, false) => (0, part * 2, 4, 2),
            };
            current.push(Piece {
                x,
                y,
                wide,
                tall,
                lists,
                direct: false,
                part,
            });
        }
    }

    pub(super) fn sub_pieces(&mut self, current: &mut Current) {
        current.parts = 4;
        let mut shapes = [(1usize, 1usize, 0u8, false); 4];
        for shape in &mut shapes {
            let raw = self.read_sub_kind() as usize;
            *shape = if self.header.kind == SliceKind::B {
                let (across, down, lists) = *B_SUB.get(raw).unwrap_or(&B_SUB[0]);
                (across, down, lists, raw == 0)
            } else {
                let (across, down) = *P_SUB.get(raw).unwrap_or(&P_SUB[0]);
                (across, down, 1, false)
            };
        }
        for (quarter, &(across, down, lists, direct)) in shapes.iter().enumerate() {
            self.quarter_pieces(current, quarter, across, down, lists, direct);
        }
    }

    pub(super) fn quarter_pieces(
        &self,
        current: &mut Current,
        quarter: usize,
        across: usize,
        down: usize,
        lists: u8,
        direct: bool,
    ) {
        let (qx, qy) = ((quarter % 2) * 2, (quarter / 2) * 2);
        for row in 0..down {
            for column in 0..across {
                current.push(Piece {
                    x: qx + column * (2 / across),
                    y: qy + row * (2 / down),
                    wide: 2 / across,
                    tall: 2 / down,
                    lists,
                    direct,
                    part: quarter,
                });
            }
        }
    }
}

impl Coder<'_> {
    /// Clause 9.2: one CAVLC block, in scan order. `nc` chooses the coeff_token table.
    pub(super) fn cavlc_block(&mut self, into: &mut [i32], nc: i32) -> usize {
        let tables = vlc::tables();
        let Entropy::Bits(reader) = &mut self.entropy else {
            return 0;
        };
        let table = if nc < 0 {
            &tables.coeff_token_chroma_dc
        } else if nc < 2 {
            &tables.coeff_token[0]
        } else if nc < 4 {
            &tables.coeff_token[1]
        } else if nc < 8 {
            &tables.coeff_token[2]
        } else {
            &tables.coeff_token[3]
        };
        let Some(token) = table.read(reader) else {
            reader.skip(1);
            return 0;
        };
        let ones = usize::from(token >> 5);
        let total = usize::from(token & 0x1f);
        if total == 0 || total > into.len() {
            return total.min(into.len());
        }

        let mut levels = [0i32; 64];
        let mut suffix = usize::from(total > 10 && ones < 3);
        for (at, level) in levels[..total].iter_mut().enumerate() {
            if at < ones {
                *level = 1 - 2 * reader.bit() as i32;
                continue;
            }
            let prefix = reader.leading_zeroes() as usize;
            let size = if prefix == 14 && suffix == 0 {
                4
            } else if prefix >= 15 {
                prefix - 3
            } else {
                suffix
            };
            let mut code = (prefix.min(15) << suffix) as i32 + reader.bits(size as u32) as i32;
            if prefix >= 15 && suffix == 0 {
                code += 15;
            }
            if prefix >= 16 {
                code += (1 << (prefix - 3)) - 4096;
            }
            // The first level after fewer than three trailing ones cannot be one: shift the range.
            if at == ones && ones < 3 {
                code += 2;
            }
            *level = if code % 2 == 0 {
                (code + 2) >> 1
            } else {
                (-code - 1) >> 1
            };
            if suffix == 0 {
                suffix = 1;
            }
            if level.abs() > (3 << (suffix - 1)) && suffix < 6 {
                suffix += 1;
            }
        }

        let mut left = 0usize;
        if total < into.len() {
            let table = if nc < 0 {
                &tables.total_zeros_chroma_dc[total - 1]
            } else {
                &tables.total_zeros[total - 1]
            };
            left = usize::from(table.read(reader).unwrap_or(0));
        }
        let mut runs = [0usize; 64];
        for slot in &mut runs[..total - 1] {
            if left == 0 {
                break;
            }
            let table = &tables.run_before[left.min(7) - 1];
            let run = usize::from(table.read(reader).unwrap_or(0)).min(left);
            *slot = run;
            left -= run;
        }
        runs[total - 1] = left;

        let mut place = 0usize;
        for at in (0..total).rev() {
            place += runs[at] + usize::from(at != total - 1);
            if place < into.len() {
                into[place] = levels[at];
            }
        }
        total
    }

    /// Clause 9.3: one CABAC block. Returns how many coefficients are not nought.
    pub(super) fn cabac_block(
        &mut self,
        into: &mut [i32],
        cat: Cat,
        coded: Option<usize>,
    ) -> usize {
        if let Some(inc) = coded {
            let context = 85 + CODED_BLOCK_OFFSET[cat as usize] + inc;
            if self.decision(context) == 0 {
                return 0;
            }
        }
        let count = into.len();
        let eight = cat == Cat::Luma8x8;
        let field = self.geometry.field;
        let (significant, last) = if eight {
            (if field { 436 } else { 402 }, if field { 451 } else { 417 })
        } else {
            (
                (if field { 277 } else { 105 }) + SIGNIFICANT_OFFSET[cat as usize],
                (if field { 338 } else { 166 }) + SIGNIFICANT_OFFSET[cat as usize],
            )
        };
        let level = if eight {
            426
        } else {
            227 + LEVEL_OFFSET[cat as usize]
        };

        let mut flags = [false; 64];
        let mut coded_count = count;
        #[allow(clippy::needless_range_loop)]
        for at in 0..count - 1 {
            let inc = if eight {
                usize::from(if field {
                    tables::SIGNIFICANT_8X8_FIELD[at]
                } else {
                    tables::SIGNIFICANT_8X8[at]
                })
            } else {
                at
            };
            if self.decision(significant + inc) == 1 {
                flags[at] = true;
                let inc = if eight {
                    usize::from(tables::LAST_8X8[at])
                } else {
                    at
                };
                if self.decision(last + inc) == 1 {
                    coded_count = at + 1;
                    break;
                }
            }
        }
        flags[coded_count - 1] = true;

        let mut ones = 0u32;
        let mut greater = 0u32;
        let mut found = 0;
        for at in (0..coded_count).rev() {
            if !flags[at] {
                continue;
            }
            let first = if greater != 0 {
                0
            } else {
                (1 + ones).min(4) as usize
            };
            let mut value = 1i32;
            if self.decision(level + first) == 1 {
                let rest = 5 + greater.min(if cat == Cat::ChromaDc { 3 } else { 4 }) as usize;
                let mut run = 1;
                while run < 14 && self.decision(level + rest) == 1 {
                    run += 1;
                }
                value = 1 + run;
                if run == 14 {
                    value += match &mut self.entropy {
                        Entropy::Arithmetic(cabac) => cabac.golomb(0) as i32,
                        Entropy::Bits(_) => 0,
                    };
                }
                greater += 1;
            } else {
                ones += 1;
            }
            if self.bit() == 1 {
                value = -value;
            }
            into[at] = value;
            found += 1;
        }
        found
    }
}
