// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Motion vector prediction and merge candidates, clause 8.5.3.2.
//! Pitfalls: merge duplicates compare both vectors and both reference indices; a neighbour's
//! vector is scaled only when both references are short-term; and the second half of the left
//! AMVP search is skipped whenever a left neighbour existed (`isScaledFlag`).

use super::params::Kind;

/// What one 4x4 square turned out to be; later blocks and both loop filters read it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unit {
    pub mv: [[i16; 2]; 2],
    pub ref_idx: [i8; 2],
    pub flags: u8,
    /// POC beside the index: deblocking compares across slices, temporal needs long-term-ness.
    pub poc: [i32; 2],
    pub long: [bool; 2],
    pub intra: bool,
    pub intra_mode: u8,
    /// Not the slice's QP wherever a coding unit sent a delta.
    pub qp: i8,
    /// Whether the covering luma transform block had a nonzero coefficient (deblocking strength).
    pub cbf: bool,
    pub edge: u8,
    /// Transquant bypass or PCM: the loop filters may have to leave it untouched.
    pub bypass: bool,
    pub pcm: bool,
}

pub const EDGE_TU_LEFT: u8 = 1;
pub const EDGE_TU_TOP: u8 = 2;
pub const EDGE_PU_LEFT: u8 = 4;
pub const EDGE_PU_TOP: u8 = 8;

/// Motion of one 16x16 square of a decoded picture, for temporal prediction. 16x16 because
/// clause 8.5.3.2.8 rounds down to it; references as POCs since indices die with the slice.
#[derive(Debug, Clone, Copy, Default)]
pub struct Col {
    pub mv: [[i16; 2]; 2],
    pub poc: [i32; 2],
    pub long: [bool; 2],
    pub flags: u8,
}

pub type Entry = super::dpb::Entry<usize>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cand {
    pub mv: [[i16; 2]; 2],
    pub ref_idx: [i8; 2],
    pub flags: u8,
}

impl Cand {
    fn same(&self, other: &Self) -> bool {
        self.flags == other.flags && self.mv == other.mv && self.ref_idx == other.ref_idx
    }
}

/// Table 8-7: the pairs each combined bi-predictive candidate is made from.
const COMBINE: [[usize; 2]; 12] = [
    [0, 1],
    [1, 0],
    [0, 2],
    [2, 0],
    [1, 2],
    [2, 1],
    [0, 3],
    [3, 0],
    [1, 3],
    [3, 1],
    [2, 3],
    [3, 2],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Partition {
    #[default]
    Whole = 0,
    Horizontal = 1,
    Vertical = 2,
    Quarters = 3,
    HorizontalUp = 4,
    HorizontalDown = 5,
    VerticalLeft = 6,
    VerticalRight = 7,
}

impl Partition {
    /// Split by a vertical line, so the second half's left neighbour is the first half.
    fn cut_vertically(self) -> bool {
        matches!(
            self,
            Self::Vertical | Self::VerticalLeft | Self::VerticalRight
        )
    }

    fn cut_horizontally(self) -> bool {
        matches!(
            self,
            Self::Horizontal | Self::HorizontalUp | Self::HorizontalDown
        )
    }

    pub fn parts(self) -> usize {
        match self {
            Self::Whole => 1,
            Self::Quarters => 4,
            _ => 2,
        }
    }

    pub fn block(self, size: usize, part: usize) -> ((usize, usize), (usize, usize)) {
        let half = size / 2;
        let quarter = size / 4;
        match self {
            Self::Whole => ((0, 0), (size, size)),
            Self::Horizontal => ((0, part * half), (size, half)),
            Self::Vertical => ((part * half, 0), (half, size)),
            Self::Quarters => ((part % 2 * half, part / 2 * half), (half, half)),
            Self::HorizontalUp => {
                if part == 0 {
                    ((0, 0), (size, quarter))
                } else {
                    ((0, quarter), (size, size - quarter))
                }
            }
            Self::HorizontalDown => {
                if part == 0 {
                    ((0, 0), (size, size - quarter))
                } else {
                    ((0, size - quarter), (size, quarter))
                }
            }
            Self::VerticalLeft => {
                if part == 0 {
                    ((0, 0), (quarter, size))
                } else {
                    ((quarter, 0), (size - quarter, size))
                }
            }
            Self::VerticalRight => {
                if part == 0 {
                    ((0, 0), (size - quarter, size))
                } else {
                    ((size - quarter, 0), (quarter, size))
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Predictor<'a> {
    pub units: &'a [Unit],
    pub unit_width: usize,
    pub width: usize,
    pub height: usize,
    pub min_tb_log2: usize,
    pub ctb_log2: usize,
    pub ctb_width: usize,
    pub ctb_order: &'a [u32],
    pub ctb_slice: &'a [u32],
    pub ctb_tile: &'a [u16],
    pub lists: &'a [Vec<Entry>; 2],
    pub poc: i32,
    pub kind: Kind,
    pub max_merge: usize,
    pub par_merge_log2: usize,
    pub temporal_enabled: bool,
    pub col: Option<&'a [Col]>,
    pub col_width: usize,
    pub col_poc: i32,
    /// No reference after this picture; decides which collocated motion is taken.
    pub no_backward: bool,
    pub collocated_from_l0: bool,
}

impl Predictor<'_> {
    /// Equation 6-10: coding order of a position (CTB in tile scan, then Morton bits inside it).
    fn z_order(&self, x: usize, y: usize) -> u64 {
        let depth = self.ctb_log2 - self.min_tb_log2;
        let ctb = (y >> depth) * self.ctb_width + (x >> depth);
        let Some(&order) = self.ctb_order.get(ctb) else {
            return u64::MAX;
        };
        let mut value = u64::from(order) << (2 * depth);
        for bit in 0..depth {
            value |= (((x >> bit) & 1) as u64) << (2 * bit);
            value |= (((y >> bit) & 1) as u64) << (2 * bit + 1);
        }
        value
    }

    /// Clause 6.4.1: decoded already, in the same slice and tile.
    pub fn available(&self, x: usize, y: usize, nx: i32, ny: i32) -> bool {
        if nx < 0 || ny < 0 || nx as usize >= self.width || ny as usize >= self.height {
            return false;
        }
        let (nx, ny) = (nx as usize, ny as usize);
        if self.z_order(nx >> self.min_tb_log2, ny >> self.min_tb_log2)
            > self.z_order(x >> self.min_tb_log2, y >> self.min_tb_log2)
        {
            return false;
        }
        let ctb_of =
            |x: usize, y: usize| (y >> self.ctb_log2) * self.ctb_width + (x >> self.ctb_log2);
        let here = ctb_of(x, y);
        let there = ctb_of(nx, ny);
        self.ctb_slice.get(here) == self.ctb_slice.get(there)
            && self.ctb_tile.get(here) == self.ctb_tile.get(there)
    }

    /// Clause 6.4.2: also unavailable where a split's second half would predict from the first.
    #[allow(clippy::too_many_arguments)]
    fn available_pb(
        &self,
        cb: (usize, usize),
        cb_size: usize,
        pb: (usize, usize),
        pb_size: (usize, usize),
        part: usize,
        nx: i32,
        ny: i32,
    ) -> bool {
        let same_cb = nx >= cb.0 as i32
            && ny >= cb.1 as i32
            && nx < (cb.0 + cb_size) as i32
            && ny < (cb.1 + cb_size) as i32;
        let available = if !same_cb {
            self.available(pb.0, pb.1, nx, ny)
        } else {
            !(pb_size.0 * 2 == cb_size
                && pb_size.1 * 2 == cb_size
                && part == 1
                && ny >= (cb.1 + pb_size.1) as i32
                && nx < (cb.0 + pb_size.0) as i32)
        };
        available && !self.unit(nx, ny).intra
    }

    fn unit(&self, x: i32, y: i32) -> Unit {
        let missing = Unit {
            intra: true,
            ..Unit::default()
        };
        if x < 0 || y < 0 {
            return missing;
        }
        let at = (y as usize >> 2) * self.unit_width + (x as usize >> 2);
        self.units.get(at).copied().unwrap_or(missing)
    }

    fn neighbour(&self, x: i32, y: i32) -> Cand {
        let unit = self.unit(x, y);
        Cand {
            mv: unit.mv,
            ref_idx: unit.ref_idx,
            flags: unit.flags,
        }
    }

    /// Equations 8-179 to 8-183, exactly as written.
    fn scale(mv: [i16; 2], from: i32, to: i32) -> [i16; 2] {
        let td = from.clamp(-128, 127);
        let tb = to.clamp(-128, 127);
        if td == 0 {
            return mv;
        }
        let tx = (16384 + (td.abs() >> 1)) / td;
        let factor = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
        let apply = |value: i16| -> i16 {
            let product = factor * i32::from(value);
            let magnitude = (product.abs() + 127) >> 8;
            let signed = if product < 0 { -magnitude } else { magnitude };
            signed.clamp(-32768, 32767) as i16
        };
        [apply(mv[0]), apply(mv[1])]
    }

    fn entry(&self, list: usize, index: i8) -> Option<Entry> {
        if index < 0 {
            return None;
        }
        self.lists[list].get(index as usize).copied()
    }

    /// Clause 8.5.3.2.2: the merge list, then the entry the slice asked for.
    #[allow(clippy::too_many_arguments)]
    pub fn merge(
        &self,
        cb: (usize, usize),
        cb_size: usize,
        pb: (usize, usize),
        pb_size: (usize, usize),
        part: usize,
        partition: Partition,
        index: usize,
    ) -> Cand {
        // Equations 8-110 to 8-113: with a coarse merge level an 8x8 unit's blocks share one list.
        let (pb, pb_size, part, partition) = if self.par_merge_log2 > 2 && cb_size == 8 {
            (cb, (cb_size, cb_size), 0, Partition::Whole)
        } else {
            (pb, pb_size, part, partition)
        };

        let mut list: Vec<Cand> = Vec::with_capacity(6);
        self.spatial_merge(cb, cb_size, pb, pb_size, part, partition, &mut list);
        list.truncate(4);

        if self.temporal_enabled && list.len() < self.max_merge {
            let first = self.temporal(pb, pb_size, 0, 0);
            let second = if self.kind == Kind::B {
                self.temporal(pb, pb_size, 1, 0)
            } else {
                None
            };
            if first.is_some() || second.is_some() {
                list.push(Cand {
                    mv: [first.unwrap_or_default(), second.unwrap_or_default()],
                    ref_idx: [
                        if first.is_some() { 0 } else { -1 },
                        if second.is_some() { 0 } else { -1 },
                    ],
                    flags: u8::from(first.is_some()) | u8::from(second.is_some()) << 1,
                });
            }
        }
        list.truncate(self.max_merge);

        // Clause 8.5.3.2.4: combined bi-predictive candidates (B slices only).
        let original = list.len();
        if self.kind == Kind::B && original > 1 && original < self.max_merge {
            for pair in COMBINE.iter().take(original * (original - 1)) {
                if list.len() >= self.max_merge {
                    break;
                }
                let zero = list[pair[0]];
                let one = list[pair[1]];
                if zero.flags & 1 == 0 || one.flags & 2 == 0 {
                    continue;
                }
                let first = self.entry(0, zero.ref_idx[0]);
                let second = self.entry(1, one.ref_idx[1]);
                let same = match (first, second) {
                    (Some(a), Some(b)) => a.poc == b.poc,
                    _ => false,
                };
                if same && zero.mv[0] == one.mv[1] {
                    continue;
                }
                list.push(Cand {
                    mv: [zero.mv[0], one.mv[1]],
                    ref_idx: [zero.ref_idx[0], one.ref_idx[1]],
                    flags: 3,
                });
            }
        }

        // Clause 8.5.3.2.5: zero candidates for each reference in turn.
        let references = if self.kind == Kind::B {
            self.lists[0].len().min(self.lists[1].len())
        } else {
            self.lists[0].len()
        };
        let mut zero = 0usize;
        while list.len() < self.max_merge {
            let which = if zero < references { zero as i8 } else { 0 };
            list.push(Cand {
                mv: [[0; 2]; 2],
                ref_idx: [which, if self.kind == Kind::B { which } else { -1 }],
                flags: if self.kind == Kind::B { 3 } else { 1 },
            });
            zero += 1;
        }

        let mut chosen = list.get(index).copied().unwrap_or_default();
        // Equations 8-126 and 8-127: 8x4 and 4x8 are restricted to one direction.
        if chosen.flags == 3 && pb_size.0 + pb_size.1 == 12 {
            chosen.flags = 1;
            chosen.ref_idx[1] = -1;
        }
        chosen
    }

    /// Clause 8.5.3.2.3: the five spatial merge candidates, in list order.
    #[allow(clippy::too_many_arguments)]
    fn spatial_merge(
        &self,
        cb: (usize, usize),
        cb_size: usize,
        pb: (usize, usize),
        pb_size: (usize, usize),
        part: usize,
        partition: Partition,
        list: &mut Vec<Cand>,
    ) {
        let (x, y) = (pb.0 as i32, pb.1 as i32);
        let (w, h) = (pb_size.0 as i32, pb_size.1 as i32);
        let level = self.par_merge_log2;
        // Neighbours inside the same parallel merge region are refused.
        let same_region =
            |nx: i32, ny: i32| (x >> level) == (nx >> level) && (y >> level) == (ny >> level);

        let a1_at = (x - 1, y + h - 1);
        let a1 = self.available_pb(cb, cb_size, pb, pb_size, part, a1_at.0, a1_at.1)
            && !same_region(a1_at.0, a1_at.1)
            && !(part == 1 && partition.cut_vertically());
        let a1_cand = self.neighbour(a1_at.0, a1_at.1);
        if a1 {
            list.push(a1_cand);
        }

        let b1_at = (x + w - 1, y - 1);
        let b1 = self.available_pb(cb, cb_size, pb, pb_size, part, b1_at.0, b1_at.1)
            && !same_region(b1_at.0, b1_at.1)
            && !(part == 1 && partition.cut_horizontally());
        let b1_cand = self.neighbour(b1_at.0, b1_at.1);
        let b1_used = b1 && !(a1 && a1_cand.same(&b1_cand));
        if b1_used {
            list.push(b1_cand);
        }

        let b0_at = (x + w, y - 1);
        let b0 = self.available_pb(cb, cb_size, pb, pb_size, part, b0_at.0, b0_at.1)
            && !same_region(b0_at.0, b0_at.1);
        let b0_cand = self.neighbour(b0_at.0, b0_at.1);
        let b0_used = b0 && !(b1 && b1_cand.same(&b0_cand));
        if b0_used {
            list.push(b0_cand);
        }

        let a0_at = (x - 1, y + h);
        let a0 = self.available_pb(cb, cb_size, pb, pb_size, part, a0_at.0, a0_at.1)
            && !same_region(a0_at.0, a0_at.1);
        let a0_cand = self.neighbour(a0_at.0, a0_at.1);
        let a0_used = a0 && !(a1 && a1_cand.same(&a0_cand));
        if a0_used {
            list.push(a0_cand);
        }

        // B2 is tried only when the other four did not all turn up.
        if a1 as usize + b1_used as usize + b0_used as usize + a0_used as usize == 4 {
            return;
        }
        let b2_at = (x - 1, y - 1);
        let b2 = self.available_pb(cb, cb_size, pb, pb_size, part, b2_at.0, b2_at.1)
            && !same_region(b2_at.0, b2_at.1);
        let b2_cand = self.neighbour(b2_at.0, b2_at.1);
        if b2 && !(a1 && a1_cand.same(&b2_cand)) && !(b1 && b1_cand.same(&b2_cand)) {
            list.push(b2_cand);
        }
    }

    /// Clause 8.5.3.2.6: the two AMVP candidates, and the one chosen.
    #[allow(clippy::too_many_arguments)]
    pub fn predictor(
        &self,
        cb: (usize, usize),
        cb_size: usize,
        pb: (usize, usize),
        pb_size: (usize, usize),
        part: usize,
        list: usize,
        ref_idx: usize,
        flag: usize,
    ) -> [i16; 2] {
        let Some(target) = self.lists[list].get(ref_idx).copied() else {
            return [0, 0];
        };
        let (x, y) = (pb.0 as i32, pb.1 as i32);
        let (w, h) = (pb_size.0 as i32, pb_size.1 as i32);
        let usable = |nx: i32, ny: i32| self.available_pb(cb, cb_size, pb, pb_size, part, nx, ny);

        let left = [(x - 1, y + h), (x - 1, y + h - 1)];
        let scaled_flag = left.iter().any(|&(nx, ny)| usable(nx, ny));
        let mut a = None;
        for &(nx, ny) in &left {
            if !usable(nx, ny) {
                continue;
            }
            if let Some(mv) = self.same_reference(nx, ny, list, target) {
                a = Some(mv);
                break;
            }
        }
        if a.is_none() {
            for &(nx, ny) in &left {
                if !usable(nx, ny) {
                    continue;
                }
                if let Some(mv) = self.stretched_reference(nx, ny, list, target) {
                    a = Some(mv);
                    break;
                }
            }
        }

        let top = [(x + w, y - 1), (x + w - 1, y - 1), (x - 1, y - 1)];
        let mut b = None;
        for &(nx, ny) in &top {
            if !usable(nx, ny) {
                continue;
            }
            if let Some(mv) = self.same_reference(nx, ny, list, target) {
                b = Some(mv);
                break;
            }
        }
        // No left neighbour: the top stands in for it and the top is retried with scaling.
        if !scaled_flag {
            if let Some(found) = b {
                a = Some(found);
            }
            b = None;
            for &(nx, ny) in &top {
                if !usable(nx, ny) {
                    continue;
                }
                if let Some(mv) = self.stretched_reference(nx, ny, list, target) {
                    b = Some(mv);
                    break;
                }
            }
        }

        let mut candidates: Vec<[i16; 2]> = Vec::with_capacity(2);
        match (a, b) {
            (Some(a), Some(b)) => {
                candidates.push(a);
                if a != b {
                    candidates.push(b);
                }
            }
            (Some(a), None) => candidates.push(a),
            (None, Some(b)) => candidates.push(b),
            (None, None) => {}
        }
        // Temporal only where the spatial ones did not already give two different answers.
        if candidates.len() < 2
            && let Some(mv) = self.temporal(pb, pb_size, list, ref_idx)
        {
            candidates.push(mv);
        }
        while candidates.len() < 2 {
            candidates.push([0, 0]);
        }
        candidates[flag.min(1)]
    }

    fn same_reference(&self, nx: i32, ny: i32, list: usize, target: Entry) -> Option<[i16; 2]> {
        let unit = self.unit(nx, ny);
        for offset in 0..2 {
            let which = (list + offset) % 2;
            if unit.flags >> which & 1 == 0 {
                continue;
            }
            let entry = self.entry(which, unit.ref_idx[which])?;
            if entry.poc == target.poc {
                return Some(unit.mv[which]);
            }
        }
        None
    }

    fn stretched_reference(
        &self,
        nx: i32,
        ny: i32,
        list: usize,
        target: Entry,
    ) -> Option<[i16; 2]> {
        let unit = self.unit(nx, ny);
        for offset in 0..2 {
            let which = (list + offset) % 2;
            if unit.flags >> which & 1 == 0 {
                continue;
            }
            let Some(entry) = self.entry(which, unit.ref_idx[which]) else {
                continue;
            };
            if entry.long != target.long {
                continue;
            }
            let mv = unit.mv[which];
            if entry.poc == target.poc || entry.long || target.long {
                return Some(mv);
            }
            return Some(Self::scale(mv, self.poc - entry.poc, self.poc - target.poc));
        }
        None
    }

    /// Clause 8.5.3.2.8: bottom right corner, then centre.
    fn temporal(
        &self,
        pb: (usize, usize),
        pb_size: (usize, usize),
        list: usize,
        ref_idx: usize,
    ) -> Option<[i16; 2]> {
        if !self.temporal_enabled {
            return None;
        }
        let target = *self.lists[list].get(ref_idx)?;
        let right = (pb.0 + pb_size.0, pb.1 + pb_size.1);
        // Bottom right only within the picture and the same CTB row.
        if right.1 >> self.ctb_log2 == pb.1 >> self.ctb_log2
            && right.1 < self.height
            && right.0 < self.width
            && let Some(mv) = self.collocated(right, target, list)
        {
            return Some(mv);
        }
        let middle = (pb.0 + (pb_size.0 >> 1), pb.1 + (pb_size.1 >> 1));
        self.collocated(middle, target, list)
    }

    /// Clause 8.5.3.2.9: which collocated motion to take, and how far to scale it.
    fn collocated(&self, at: (usize, usize), target: Entry, list: usize) -> Option<[i16; 2]> {
        let col = self.col?;
        let x = (at.0 >> 4) << 4;
        let y = (at.1 >> 4) << 4;
        let block = *col.get((y >> 4) * self.col_width + (x >> 4))?;
        if block.flags == 0 {
            return None;
        }
        let which = if block.flags & 1 == 0 {
            1
        } else if block.flags & 2 == 0 {
            0
        } else if self.no_backward {
            // 8.5.3.2.9: 後方の参照が無いときは、構築中のリストと同じ番号（mvLXCol）を取る。
            // 取り違えると 8.5.3.2.4 の結合候補が捨てられ、以降の merge 番号がずれる。
            list
        } else {
            usize::from(self.collocated_from_l0)
        };
        if block.long[which] != target.long {
            return None;
        }
        let mv = block.mv[which];
        let col_diff = self.col_poc - block.poc[which];
        let curr_diff = self.poc - target.poc;
        if target.long || col_diff == curr_diff {
            return Some(mv);
        }
        Some(Self::scale(mv, col_diff, curr_diff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_collocated_motion_of_the_list_being_built_is_the_one_taken() {
        let units = vec![Unit::default(); 16];
        let col = vec![Col {
            mv: [[2, 0], [4, 0]],
            poc: [4, 4],
            long: [false, false],
            flags: 3,
        }];
        let entry = |poc: i32| Entry {
            poc,
            long: false,
            id: Some(0),
        };
        let lists = [vec![entry(8)], vec![entry(8)]];
        let predictor = Predictor {
            units: &units,
            unit_width: 4,
            width: 16,
            height: 16,
            min_tb_log2: 2,
            ctb_log2: 4,
            ctb_width: 1,
            ctb_order: &[0],
            ctb_slice: &[0],
            ctb_tile: &[0],
            lists: &lists,
            poc: 12,
            kind: Kind::B,
            max_merge: 5,
            par_merge_log2: 2,
            temporal_enabled: true,
            col: Some(&col),
            col_width: 1,
            col_poc: 8,
            no_backward: true,
            collocated_from_l0: true,
        };
        let first = predictor.merge((0, 0), 16, (0, 0), (16, 16), 0, Partition::Whole, 0);
        assert_eq!(first.flags, 3);
        assert_eq!(
            first.mv,
            [[2, 0], [4, 0]],
            "リスト 0 にはコロケートのリスト 0 を、リスト 1 にはリスト 1 を"
        );
    }
}
