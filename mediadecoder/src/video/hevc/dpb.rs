// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The decoded picture buffer: POC, reference picture sets, lists and bumping (clauses 8.3.1,
//! 8.3.2, 8.3.4, Annex C), shared by the software decoder and every platform path.
//! No sliding window: each picture states its whole reference set. Paths supply [`Frames`].

use super::params::{self, Header, Kind, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Marked {
    #[default]
    No,
    Short,
    Long,
}

#[derive(Debug, Clone, Copy)]
pub struct Store<Id, H> {
    pub id: Id,
    pub poc: i32,
    pub marked: Marked,
    pub output: bool,
    pub held: H,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<Id> {
    pub poc: i32,
    pub long: bool,
    /// `None` for a reference this decoder never had and [`Frames::stand_in`] gave nothing for.
    pub id: Option<Id>,
}

#[derive(Debug, Clone, Copy)]
struct LongTerm {
    poc: i32,
    exact: bool,
    used: bool,
}

pub trait Frames {
    type Id: Copy + PartialEq + std::fmt::Debug;
    type Held;

    /// A name for a new picture. `live` is every name the DPB still holds.
    fn take(&mut self, live: &[Self::Id]) -> Option<Self::Id>;

    fn release(&mut self, id: Self::Id) {
        let _ = id;
    }

    fn output(&mut self, id: Self::Id, held: &Self::Held);

    /// Clause 8.3.3: a picture for a reference this decoder never had, never output. What it
    /// holds is up to the path (the clause fills it with `1 << (BitDepth - 1)`). `None` leaves the
    /// reference missing.
    fn stand_in(&mut self, live: &[Self::Id], poc: i32) -> Option<(Self::Id, Self::Held)> {
        let _ = (live, poc);
        None
    }
}

#[derive(Debug)]
pub struct Dpb<Id, H> {
    pub stores: Vec<Store<Id, H>>,
    /// The three sets of clause 8.3.2, as [`Dpb::begin`] last resolved them.
    pub sets: [Vec<Entry<Id>>; 3],
    pub total: usize,
    previous_lsb: i32,
    previous_msb: i32,
    drop_rasl: bool,
    started: bool,
    /// Where this decoder joined: the POC of the first picture it decoded, or of the last IRAP
    /// picture with `NoRaslOutputFlag` 1 (a stream need not have IRAP pictures at all). References
    /// before it were never this decoder's to have.
    start: Option<i32>,
    reorder: usize,
    capacity: usize,
}

impl<Id: Copy + PartialEq + std::fmt::Debug, H> Dpb<Id, H> {
    pub fn new(reorder: usize, capacity: usize) -> Self {
        Self {
            stores: Vec::new(),
            sets: [Vec::new(), Vec::new(), Vec::new()],
            total: 0,
            previous_lsb: 0,
            previous_msb: 0,
            drop_rasl: true,
            started: false,
            start: None,
            reorder,
            capacity,
        }
    }

    pub fn limits(&mut self, reorder: usize, capacity: usize) {
        self.reorder = reorder;
        self.capacity = capacity;
    }

    pub fn drop_rasl(&self) -> bool {
        self.drop_rasl
    }

    pub fn end_of_sequence(&mut self) {
        self.started = false;
        self.start = None;
    }

    /// Everything still referenced, in held order (the order a path writes its list in).
    pub fn references(&self) -> impl Iterator<Item = &Store<Id, H>> {
        self.stores
            .iter()
            .filter(|store| store.marked != Marked::No)
    }

    pub fn holds(&self, id: Id, poc: i32) -> bool {
        self.stores
            .iter()
            .any(|store| store.id == id && store.poc == poc)
    }

    /// Clauses 8.1.3, 8.3.1 and 8.3.2: what happens on a picture's first slice segment.
    /// `None`: a leading picture of the starting point, not to be decoded.
    pub fn begin<F: Frames<Id = Id, Held = H>>(
        &mut self,
        frames: &mut F,
        sps: &Sps,
        header: &Header,
        kind: u8,
        temporal: u8,
    ) -> Option<i32> {
        // Clause 8.1.3: just started or reset, so the leading pictures cannot be decoded.
        let no_rasl = params::is_idr(kind) || params::is_bla(kind) || !self.started;
        if params::is_irap(kind) {
            self.drop_rasl = no_rasl;
            self.started = true;
        }
        if params::is_rasl(kind) && self.drop_rasl {
            return None;
        }
        let poc = if params::is_idr(kind) {
            self.previous_lsb = 0;
            self.previous_msb = 0;
            0
        } else {
            self.order_count(sps, header, kind, no_rasl, temporal)
        };
        if self.start.is_none() || (params::is_irap(kind) && no_rasl) {
            self.start = Some(poc);
        }
        if params::is_irap(kind) && no_rasl {
            // C.5.2.2 `NoOutputOfPriorPicsFlag`: DPB のピクチャーを出力せずに捨ててよいのはランダムアクセスポイントが CRA のときと、
            // フラグが立っているときだけ。IDR を一律に扱うと、境界ごとに並べ替えの深さの分だけピクチャーが消える。
            if params::is_cra(kind) || header.no_output_of_prior_pics {
                for store in &mut self.stores {
                    store.output = false;
                }
            }
            while self.bump(frames) {}
            for store in self.stores.iter().filter(|store| !store.output) {
                frames.release(store.id);
            }
            self.stores.retain(|store| store.output);
            for store in &mut self.stores {
                store.marked = Marked::No;
            }
        }
        self.apply_reference_sets(frames, sps, header, poc, params::is_idr(kind));
        Some(poc)
    }

    fn order_count(
        &mut self,
        sps: &Sps,
        header: &Header,
        kind: u8,
        no_rasl: bool,
        temporal: u8,
    ) -> i32 {
        let max = 1i32 << sps.log2_max_poc_lsb;
        let lsb = header.poc_lsb as i32;
        let msb = if params::is_irap(kind) && no_rasl {
            0
        } else if lsb < self.previous_lsb && self.previous_lsb - lsb >= max / 2 {
            self.previous_msb + max
        } else if lsb > self.previous_lsb && lsb - self.previous_lsb > max / 2 {
            self.previous_msb - max
        } else {
            self.previous_msb
        };
        // Only temporal layer 0 non-leading pictures, so a decoder dropping higher layers agrees.
        if temporal == 0
            && !params::is_rasl(kind)
            && !params::is_radl(kind)
            && params::is_reference(kind)
        {
            self.previous_lsb = lsb;
            self.previous_msb = msb;
        }
        msb + lsb
    }

    fn reference_sets(
        &self,
        sps: &Sps,
        header: &Header,
        poc: i32,
        idr: bool,
    ) -> ([Vec<i32>; 2], Vec<LongTerm>) {
        let mut short = [Vec::new(), Vec::new()];
        let mut long = Vec::new();
        if idr {
            return (short, long);
        }
        for (i, &delta) in header.short_term.before.iter().enumerate() {
            if header
                .short_term
                .before_used
                .get(i)
                .copied()
                .unwrap_or(false)
            {
                short[0].push(poc - delta);
            }
        }
        for (i, &delta) in header.short_term.after.iter().enumerate() {
            if header
                .short_term
                .after_used
                .get(i)
                .copied()
                .unwrap_or(false)
            {
                short[1].push(poc + delta);
            }
        }
        let max = 1i32 << sps.log2_max_poc_lsb;
        for entry in &header.long_term {
            let (target, exact) = match entry.msb {
                Some(cycle) => (
                    poc - cycle * max - (poc & (max - 1)) + entry.poc_lsb as i32,
                    true,
                ),
                None => (entry.poc_lsb as i32, false),
            };
            long.push(LongTerm {
                poc: target,
                exact,
                used: entry.used,
            });
        }
        (short, long)
    }

    /// Unmarks every picture the set did not name and drops what is neither referenced nor waiting.
    fn apply_reference_sets<F: Frames<Id = Id, Held = H>>(
        &mut self,
        frames: &mut F,
        sps: &Sps,
        header: &Header,
        poc: i32,
        idr: bool,
    ) {
        let (short, long) = self.reference_sets(sps, header, poc, idr);
        let mask = (1i32 << sps.log2_max_poc_lsb) - 1;
        let mut kept = vec![false; self.stores.len()];
        let mut lists: [Vec<Entry<Id>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        // Resolved now, not kept as buffer positions: the sweep below would move them.
        let missing = Entry {
            poc: 0,
            long: false,
            id: None,
        };

        for (which, wanted) in short.iter().enumerate() {
            for &target in wanted {
                let found = self.stores.iter().position(|store| store.poc == target);
                match found
                    .or_else(|| self.stand_in(frames, target, true, Marked::Short, &mut kept))
                {
                    Some(at) => {
                        kept[at] = true;
                        self.stores[at].marked = Marked::Short;
                        lists[which].push(Entry {
                            poc: target,
                            long: false,
                            id: Some(self.stores[at].id),
                        });
                    }
                    // Never had it; keep the place so later indices stay right.
                    None => lists[which].push(missing),
                }
            }
        }
        // Pictures this one may not use still stay, because a later one may.
        for (i, &delta) in header.short_term.before.iter().enumerate() {
            if !header.short_term.before_used[i]
                && let Some(at) = self.stores.iter().position(|s| s.poc == poc - delta)
            {
                kept[at] = true;
                self.stores[at].marked = Marked::Short;
            }
        }
        for (i, &delta) in header.short_term.after.iter().enumerate() {
            if !header.short_term.after_used[i]
                && let Some(at) = self.stores.iter().position(|s| s.poc == poc + delta)
            {
                kept[at] = true;
                self.stores[at].marked = Marked::Short;
            }
        }
        for entry in &long {
            let found = self.stores.iter().position(|store| {
                if entry.exact {
                    store.poc == entry.poc
                } else {
                    store.poc & mask == entry.poc & mask
                }
            });
            let found = match found {
                None if entry.used => {
                    self.stand_in(frames, entry.poc, entry.exact, Marked::Long, &mut kept)
                }
                found => found,
            };
            if let Some(at) = found {
                kept[at] = true;
                self.stores[at].marked = Marked::Long;
            }
            if entry.used {
                lists[2].push(match found {
                    Some(at) => Entry {
                        poc: self.stores[at].poc,
                        long: true,
                        id: Some(self.stores[at].id),
                    },
                    None => missing,
                });
            }
        }

        for (at, keep) in kept.iter().enumerate() {
            if !keep {
                self.stores[at].marked = Marked::No;
            }
        }
        for store in self
            .stores
            .iter()
            .filter(|store| store.marked == Marked::No && !store.output)
        {
            frames.release(store.id);
        }
        self.stores
            .retain(|store| store.marked != Marked::No || store.output);
        self.total = lists[0].len() + lists[1].len() + lists[2].len();
        self.sets = lists;
    }

    /// Clause 8.3.3: a reference used by this picture that was lost after the decoder joined
    /// becomes a picture of its own, if the path makes one. Its place in `stores`. One from before
    /// the join is left missing, so the picture is dropped as RASL pictures are.
    fn stand_in<F: Frames<Id = Id, Held = H>>(
        &mut self,
        frames: &mut F,
        poc: i32,
        exact: bool,
        marked: Marked,
        kept: &mut Vec<bool>,
    ) -> Option<usize> {
        // A long-term reference sent by its LSBs alone cannot be placed against the start.
        let start = self.start?;
        if exact && poc < start {
            return None;
        }
        let live: Vec<Id> = self.stores.iter().map(|store| store.id).collect();
        let (id, held) = frames.stand_in(&live, poc)?;
        self.stores.push(Store {
            id,
            poc,
            marked,
            output: false,
            held,
        });
        kept.push(true);
        Some(self.stores.len() - 1)
    }

    /// Clause 8.3.4: the reference lists after any modification.
    pub fn lists(&self, header: &Header) -> [Vec<Entry<Id>>; 2] {
        let mut lists: [Vec<Entry<Id>>; 2] = [Vec::new(), Vec::new()];
        if header.kind.intra() || self.total == 0 {
            return lists;
        }
        let count = if header.kind == Kind::B { 2 } else { 1 };
        for (list, into) in lists.iter_mut().enumerate().take(count) {
            let order: [usize; 3] = if list == 0 { [0, 1, 2] } else { [1, 0, 2] };
            let mut temporary: Vec<Entry<Id>> = Vec::new();
            let wanted = header.num_ref_idx[list].max(self.total);
            while temporary.len() < wanted {
                for group in order {
                    for &found in &self.sets[group] {
                        if temporary.len() >= wanted {
                            break;
                        }
                        temporary.push(found);
                    }
                }
                if temporary.is_empty() {
                    break;
                }
            }
            for i in 0..header.num_ref_idx[list] {
                let index = if header.modified[list] {
                    header.list_entry[list].get(i).copied().unwrap_or(0)
                } else {
                    i
                };
                if let Some(&found) = temporary.get(index) {
                    into.push(found);
                } else if let Some(&first) = temporary.first() {
                    into.push(first);
                }
            }
        }
        lists
    }

    pub fn free<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) -> Option<Id> {
        loop {
            let live: Vec<Id> = self.stores.iter().map(|store| store.id).collect();
            if let Some(id) = frames.take(&live) {
                return Some(id);
            }
            if let Some(at) = self
                .stores
                .iter()
                .position(|store| store.marked == Marked::No && !store.output)
            {
                frames.release(self.stores[at].id);
                self.stores.remove(at);
                continue;
            }
            if !self.bump(frames) {
                return None;
            }
        }
    }

    pub fn bump<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) -> bool {
        let Some(at) = self
            .stores
            .iter()
            .enumerate()
            .filter(|(_, store)| store.output)
            .min_by_key(|(_, store)| store.poc)
            .map(|(at, _)| at)
        else {
            return false;
        };
        frames.output(self.stores[at].id, &self.stores[at].held);
        self.stores[at].output = false;
        if self.stores[at].marked == Marked::No {
            frames.release(self.stores[at].id);
            self.stores.remove(at);
        }
        true
    }

    pub fn bump_all<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) {
        while self.bump(frames) {}
    }

    fn drain<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) {
        loop {
            let waiting = self.stores.iter().filter(|store| store.output).count();
            if waiting <= self.reorder && self.stores.len() <= self.capacity {
                break;
            }
            if !self.bump(frames) {
                break;
            }
        }
    }

    pub fn push<F: Frames<Id = Id, Held = H>>(
        &mut self,
        frames: &mut F,
        id: Id,
        poc: i32,
        output: bool,
        held: H,
    ) {
        self.stores.push(Store {
            id,
            poc,
            marked: Marked::Short,
            output,
            held,
        });
        self.drain(frames);
    }

    pub fn clear<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) {
        for store in std::mem::take(&mut self.stores) {
            frames.release(store.id);
        }
    }

    /// Throws away everything and forgets where the stream stood (seek or channel change).
    pub fn reset<F: Frames<Id = Id, Held = H>>(&mut self, frames: &mut F) {
        self.clear(frames);
        self.previous_lsb = 0;
        self.previous_msb = 0;
        self.started = false;
        self.start = None;
        self.drop_rasl = true;
    }
}
