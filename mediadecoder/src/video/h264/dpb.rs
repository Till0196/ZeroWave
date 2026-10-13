// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The decoded picture buffer: clause 8.2 (POC, FrameNumWrap, lists, marking, bumping).
//! Shared by the software decoder and every platform path.

use super::params::{self, Header, Kind, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Marked {
    #[default]
    No,
    Short,
    Long,
}

#[derive(Debug, Clone, Copy)]
pub struct Store<I, H> {
    pub slot: I,
    /// Outlives the store's place in the list, which moves as stores fall out.
    pub serial: i32,
    pub frame_num: u32,
    /// `FrameNumWrap`.
    pub wrap: i32,
    pub long_index: i32,
    pub present: [bool; 2],
    /// Which POC is known; not `present`, since a field's POC comes from its slice header.
    pub has_poc: [bool; 2],
    pub poc: [i32; 2],
    pub marked: [Marked; 2],
    pub output: bool,
    pub held: H,
}

impl<I, H> Store<I, H> {
    pub fn complete(&self) -> bool {
        self.present[0] && self.present[1]
    }

    pub fn reference(&self) -> bool {
        self.marked.iter().any(|&marked| marked != Marked::No)
    }

    pub fn order(&self) -> i32 {
        match (self.present[0], self.present[1]) {
            (true, true) => self.poc[0].min(self.poc[1]),
            (true, false) => self.poc[0],
            _ => self.poc[1],
        }
    }

    pub fn id(&self, parity: Option<usize>) -> i32 {
        match parity {
            None => self.serial * 4 + 2,
            Some(parity) => self.serial * 4 + parity as i32,
        }
    }

    pub fn as_reference(&self) -> Option<usize> {
        match (self.marked[0] != Marked::No, self.marked[1] != Marked::No) {
            (true, true) => None,
            (true, false) => Some(0),
            _ => Some(1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Reference {
    pub at: usize,
    pub parity: Option<usize>,
    pub id: i32,
    pub long_term: bool,
    pub num: i32,
}

#[derive(Debug, Clone)]
pub struct Marking {
    pub reference: bool,
    pub idr: bool,
    pub long_term_reference: bool,
    pub adaptive: bool,
    pub commands: Vec<params::Marking>,
    pub frame_num: u32,
    pub max_ref_frames: u32,
    pub field: bool,
}

impl Marking {
    pub fn new(header: &Header, sps: &Sps) -> Self {
        Self {
            reference: header.nal_ref_idc != 0,
            idr: header.idr,
            long_term_reference: header.long_term_reference,
            adaptive: header.adaptive_marking,
            commands: header.marking[..header.markings].to_vec(),
            frame_num: header.frame_num,
            max_ref_frames: sps.max_ref_frames,
            field: header.field_pic,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Current<E = ()> {
    pub serial: i32,
    pub parity: usize,
    pub field: bool,
    pub marking: Marking,
    pub extra: E,
}

pub trait Slots {
    type Id: Copy;
    type Held;

    fn take(&mut self, stores: &[Store<Self::Id, Self::Held>]) -> Option<Self::Id>;

    fn release(&mut self, id: Self::Id);

    fn emit(&mut self, store: &Store<Self::Id, Self::Held>);
}

pub trait Ordered {
    fn set_order(&mut self, order: i32);
}

#[derive(Debug)]
pub struct Dpb<I, H> {
    pub stores: Vec<Store<I, H>>,
    pub pending: Option<(i32, usize)>,
    pub reorder: usize,
    pub capacity: usize,
    pub serial: i32,
    previous_msb: i32,
    previous_lsb: i32,
    previous_frame_num: u32,
    previous_offset: i32,
}

impl<I, H> Default for Dpb<I, H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I, H> Dpb<I, H> {
    pub const fn new() -> Self {
        Self {
            stores: Vec::new(),
            pending: None,
            reorder: 0,
            capacity: 0,
            serial: 0,
            previous_msb: 0,
            previous_lsb: 0,
            previous_frame_num: 0,
            previous_offset: 0,
        }
    }

    pub fn forget(&mut self) {
        self.previous_msb = 0;
        self.previous_lsb = 0;
        self.previous_frame_num = 0;
        self.previous_offset = 0;
    }

    pub fn index(&self, serial: i32) -> Option<usize> {
        self.stores.iter().position(|store| store.serial == serial)
    }

    pub fn next_serial(&mut self) -> i32 {
        self.serial = self.serial.wrapping_add(1) & 0x00ff_ffff;
        self.serial
    }

    /// Clause 8.2.1.
    pub fn order_count(&mut self, sps: &Sps, header: &Header) -> [i32; 2] {
        let max_frame = 1i32 << sps.log2_max_frame_num;
        match sps.poc_type {
            0 => {
                let max = 1i32 << sps.log2_max_poc_lsb;
                let (previous_msb, previous_lsb) = if header.idr {
                    (0, 0)
                } else {
                    (self.previous_msb, self.previous_lsb)
                };
                let lsb = header.poc_lsb as i32;
                let msb = if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
                    previous_msb + max
                } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
                    previous_msb - max
                } else {
                    previous_msb
                };
                if header.nal_ref_idc != 0 {
                    self.previous_msb = msb;
                    self.previous_lsb = lsb;
                }
                let top = msb + lsb;
                if header.field_pic {
                    [top, top]
                } else {
                    [top, top + header.delta_poc_bottom]
                }
            }
            1 => {
                let offset = if header.idr {
                    0
                } else if self.previous_frame_num > header.frame_num {
                    self.previous_offset + max_frame
                } else {
                    self.previous_offset
                };
                self.previous_offset = offset;
                self.previous_frame_num = header.frame_num;
                let length = sps.cycle_length as i32;
                let mut absolute = if length != 0 {
                    offset + header.frame_num as i32
                } else {
                    0
                };
                if header.nal_ref_idc == 0 && absolute > 0 {
                    absolute -= 1;
                }
                let mut expected = 0;
                if absolute > 0 && length > 0 {
                    let cycle = (absolute - 1) / length;
                    let index = (absolute - 1) % length;
                    let sum: i32 = sps.cycle[..length as usize].iter().sum();
                    expected = cycle * sum;
                    for step in 0..=index as usize {
                        expected += sps.cycle[step];
                    }
                }
                if header.nal_ref_idc == 0 {
                    expected += sps.offset_non_ref;
                }
                if header.field_pic && header.bottom_field {
                    let bottom = expected + sps.offset_top_to_bottom + header.delta_poc[0];
                    [bottom, bottom]
                } else {
                    let top = expected + header.delta_poc[0];
                    if header.field_pic {
                        [top, top]
                    } else {
                        [top, top + sps.offset_top_to_bottom + header.delta_poc[1]]
                    }
                }
            }
            _ => {
                let offset = if header.idr {
                    0
                } else if self.previous_frame_num > header.frame_num {
                    self.previous_offset + max_frame
                } else {
                    self.previous_offset
                };
                self.previous_offset = offset;
                self.previous_frame_num = header.frame_num;
                let count = if header.idr {
                    0
                } else if header.nal_ref_idc == 0 {
                    2 * (offset + header.frame_num as i32) - 1
                } else {
                    2 * (offset + header.frame_num as i32)
                };
                [count, count]
            }
        }
    }

    /// Clause 8.2.4.1.
    pub fn number_frames(&mut self, sps: &Sps, frame_num: u32) {
        let max = 1i32 << sps.log2_max_frame_num;
        for store in &mut self.stores {
            store.wrap = if store.frame_num as i32 > frame_num as i32 {
                store.frame_num as i32 - max
            } else {
                store.frame_num as i32
            };
        }
    }

    /// Clause 8.2.4.2.5: frames to fields, alternating parity from the current one.
    pub fn as_fields(&self, frames: &[usize], parity: usize, long: bool) -> Vec<(usize, usize)> {
        let usable = |at: usize, parity: usize| {
            let store = &self.stores[at];
            store.present[parity]
                && store.marked[parity] == if long { Marked::Long } else { Marked::Short }
        };
        let mut at = [0usize; 2];
        let mut want = parity;
        let mut out = Vec::new();
        loop {
            while at[want] < frames.len() && !usable(frames[at[want]], want) {
                at[want] += 1;
            }
            if at[want] < frames.len() {
                out.push((frames[at[want]], want));
                at[want] += 1;
                want = 1 - want;
                continue;
            }
            let other = 1 - want;
            while at[other] < frames.len() && !usable(frames[at[other]], other) {
                at[other] += 1;
            }
            if at[other] >= frames.len() {
                return out;
            }
            want = other;
        }
    }

    /// Clause 8.2.4.2: the initial lists.
    pub fn initial_lists(&self, header: &Header, poc: i32, parity: usize) -> [Vec<Reference>; 2] {
        let field = header.field_pic;
        let mut lists: [Vec<Reference>; 2] = [Vec::new(), Vec::new()];
        if header.kind.intra() {
            return lists;
        }
        let short: Vec<usize> = (0..self.stores.len())
            .filter(|&at| {
                let store = &self.stores[at];
                if field {
                    (0..2).any(|p| store.present[p] && store.marked[p] == Marked::Short)
                } else {
                    store.complete() && store.marked.iter().all(|&m| m == Marked::Short)
                }
            })
            .collect();
        let long: Vec<usize> = (0..self.stores.len())
            .filter(|&at| {
                let store = &self.stores[at];
                if field {
                    (0..2).any(|p| store.present[p] && store.marked[p] == Marked::Long)
                } else {
                    store.complete() && store.marked.iter().all(|&m| m == Marked::Long)
                }
            })
            .collect();
        let mut long_sorted = long.clone();
        long_sorted.sort_by_key(|&at| self.stores[at].long_index);

        if header.kind == Kind::B {
            let mut before: Vec<usize> = short
                .iter()
                .copied()
                .filter(|&at| self.stores[at].order() <= poc)
                .collect();
            let mut after: Vec<usize> = short
                .iter()
                .copied()
                .filter(|&at| self.stores[at].order() > poc)
                .collect();
            before.sort_by_key(|&at| -self.stores[at].order());
            after.sort_by_key(|&at| self.stores[at].order());
            let zero: Vec<usize> = before.iter().chain(after.iter()).copied().collect();
            let one: Vec<usize> = after.iter().chain(before.iter()).copied().collect();
            self.fill(&mut lists[0], &zero, &long_sorted, field, parity);
            self.fill(&mut lists[1], &one, &long_sorted, field, parity);
            // Where the two lists are equal, the second gets a different first entry (8.2.4.2.4).
            if lists[1].len() > 1
                && lists[0].len() == lists[1].len()
                && lists[0].iter().zip(&lists[1]).all(|(a, b)| a.id == b.id)
            {
                lists[1].swap(0, 1);
            }
        } else {
            let mut ordered = short.clone();
            ordered.sort_by_key(|&at| -self.stores[at].wrap);
            self.fill(&mut lists[0], &ordered, &long_sorted, field, parity);
        }
        lists
    }

    fn fill(
        &self,
        into: &mut Vec<Reference>,
        short: &[usize],
        long: &[usize],
        field: bool,
        parity: usize,
    ) {
        let entry = |at: usize, side: Option<usize>, num: i32| Reference {
            at,
            parity: side,
            id: self.stores[at].id(side),
            long_term: match side {
                None => self.stores[at].marked[0] == Marked::Long,
                Some(side) => self.stores[at].marked[side] == Marked::Long,
            },
            num,
        };
        if field {
            for (at, side) in self.as_fields(short, parity, false) {
                let number = 2 * self.stores[at].wrap + i32::from(side == parity);
                into.push(entry(at, Some(side), number));
            }
            for (at, side) in self.as_fields(long, parity, true) {
                let number = 2 * self.stores[at].long_index + i32::from(side == parity);
                into.push(entry(at, Some(side), number));
            }
            return;
        }
        for &at in short {
            into.push(entry(at, None, self.stores[at].wrap));
        }
        for &at in long {
            into.push(entry(at, None, self.stores[at].long_index));
        }
    }

    /// Clause 8.2.4.3.
    pub fn reorder_lists(
        &self,
        header: &Header,
        sps: &Sps,
        lists: &mut [Vec<Reference>; 2],
        full: &[Vec<Reference>; 2],
    ) {
        let max_frame = 1i32 << sps.log2_max_frame_num;
        let (current_num, max_num) = if header.field_pic {
            (2 * header.frame_num as i32 + 1, 2 * max_frame)
        } else {
            (header.frame_num as i32, max_frame)
        };
        for (list, into) in lists.iter_mut().enumerate() {
            if header.reorders[list] == 0 {
                into.truncate(header.num_ref_idx[list]);
                continue;
            }
            let mut predicted = current_num;
            let mut at = 0usize;
            for step in 0..header.reorders[list] {
                let command = header.reorder[list][step];
                let wanted = match command.op {
                    0 | 1 => {
                        let difference = command.value as i32 + 1;
                        let mut number = if command.op == 0 {
                            predicted - difference
                        } else {
                            predicted + difference
                        };
                        if number < 0 {
                            number += max_num;
                        } else if number >= max_num {
                            number -= max_num;
                        }
                        predicted = number;
                        let number = if number > current_num {
                            number - max_num
                        } else {
                            number
                        };
                        full[list]
                            .iter()
                            .find(|found| !found.long_term && found.num == number)
                            .copied()
                    }
                    _ => full[list]
                        .iter()
                        .find(|found| found.long_term && found.num == command.value as i32)
                        .copied(),
                };
                let Some(wanted) = wanted else {
                    continue;
                };
                if at > into.len() {
                    continue;
                }
                into.insert(at, wanted);
                at += 1;
                // Remove the later duplicate only; entries in front were placed by earlier commands.
                let mut step = at;
                while step < into.len() {
                    if into[step].id == wanted.id {
                        into.remove(step);
                    } else {
                        step += 1;
                    }
                }
            }
            into.truncate(header.num_ref_idx[list]);
        }
    }

    fn find_short(&self, number: i32, parity: usize, field: bool) -> Option<(usize, usize)> {
        for (at, store) in self.stores.iter().enumerate() {
            for side in 0..2 {
                if store.marked[side] != Marked::Short {
                    continue;
                }
                let picture = if field {
                    2 * store.wrap + i32::from(side == parity)
                } else {
                    store.wrap
                };
                if picture == number {
                    return Some((at, side));
                }
            }
        }
        None
    }

    fn find_long(&self, number: i32, parity: usize, field: bool) -> Option<(usize, usize)> {
        for (at, store) in self.stores.iter().enumerate() {
            for side in 0..2 {
                if store.marked[side] != Marked::Long {
                    continue;
                }
                let picture = if field {
                    2 * store.long_index + i32::from(side == parity)
                } else {
                    store.long_index
                };
                if picture == number {
                    return Some((at, side));
                }
            }
        }
        None
    }

    fn unmark(&mut self, at: usize, side: usize, field: bool) {
        if field {
            self.stores[at].marked[side] = Marked::No;
        } else {
            self.stores[at].marked = [Marked::No; 2];
        }
    }
}

impl<I: Copy, H> Dpb<I, H> {
    pub fn free_slot<S>(&mut self, slots: &mut S) -> Option<I>
    where
        S: Slots<Id = I, Held = H>,
    {
        loop {
            if let Some(slot) = slots.take(&self.stores) {
                return Some(slot);
            }
            if let Some(at) = self
                .stores
                .iter()
                .position(|store| !store.reference() && !store.output)
            {
                slots.release(self.stores[at].slot);
                self.stores.remove(at);
                continue;
            }
            if !self.bump(slots) {
                return None;
            }
        }
    }

    pub fn bump<S>(&mut self, slots: &mut S) -> bool
    where
        S: Slots<Id = I, Held = H>,
    {
        let Some(at) = self
            .stores
            .iter()
            .enumerate()
            .filter(|(_, store)| store.output && store.complete())
            .min_by_key(|(_, store)| store.order())
            .map(|(at, _)| at)
        else {
            return false;
        };
        slots.emit(&self.stores[at]);
        self.stores[at].output = false;
        if !self.stores[at].reference() {
            slots.release(self.stores[at].slot);
            self.stores.remove(at);
        }
        true
    }

    pub fn drain<S>(&mut self, slots: &mut S)
    where
        S: Slots<Id = I, Held = H>,
    {
        loop {
            let waiting = self
                .stores
                .iter()
                .filter(|store| store.output && store.complete())
                .count();
            let held = self.stores.len();
            if waiting <= self.reorder && held <= self.capacity {
                break;
            }
            if !self.bump(slots) {
                break;
            }
        }
    }

    pub fn refresh<S>(&mut self, slots: &mut S, no_output_of_prior_pics: bool)
    where
        S: Slots<Id = I, Held = H>,
    {
        if no_output_of_prior_pics {
            for store in &mut self.stores {
                store.output = false;
            }
        }
        while self.bump(slots) {}
        let dropped: Vec<I> = self
            .stores
            .iter()
            .filter(|store| !store.output)
            .map(|store| store.slot)
            .collect();
        for slot in dropped {
            slots.release(slot);
        }
        self.stores.retain(|store| store.output);
        for store in &mut self.stores {
            store.marked = [Marked::No; 2];
        }
        self.pending = None;
    }

    /// Clause 8.2.5.
    pub fn mark<S>(&mut self, slots: &mut S, serial: i32, parity: usize, marking: &Marking)
    where
        S: Slots<Id = I, Held = H>,
    {
        if !marking.reference {
            return;
        }
        let Some(at) = self.index(serial) else {
            return;
        };
        if marking.idr {
            for (other, store) in self.stores.iter_mut().enumerate() {
                if other != at {
                    store.marked = [Marked::No; 2];
                }
            }
            self.stores[at].long_index = 0;
            self.stores[at].marked[parity] = if marking.long_term_reference {
                Marked::Long
            } else {
                Marked::Short
            };
            if !marking.field {
                self.stores[at].marked = [self.stores[at].marked[parity]; 2];
            }
            return;
        }

        let mut long = false;
        if marking.adaptive {
            long = self.commands(slots, serial, parity, marking);
        } else if !(marking.field && self.stores[at].marked[1 - parity] != Marked::No) {
            // Sliding window. The second field of a reference pair pushes nothing out.
            let most = marking.max_ref_frames.max(1) as usize;
            loop {
                let held = self
                    .stores
                    .iter()
                    .enumerate()
                    .filter(|(other, store)| *other != at && store.reference())
                    .count();
                if held < most {
                    break;
                }
                let Some(oldest) = self
                    .stores
                    .iter()
                    .enumerate()
                    .filter(|(other, store)| *other != at && store.marked.contains(&Marked::Short))
                    .min_by_key(|(_, store)| store.wrap)
                    .map(|(other, _)| other)
                else {
                    break;
                };
                self.stores[oldest].marked = [Marked::No; 2];
                if !self.stores[oldest].output {
                    slots.release(self.stores[oldest].slot);
                    self.stores.remove(oldest);
                }
            }
        }
        let Some(at) = self.index(serial) else {
            return;
        };
        if !long {
            self.stores[at].marked[parity] = Marked::Short;
            if !marking.field {
                self.stores[at].marked = [Marked::Short; 2];
            }
        }
    }

    /// The MMCO commands. Returns whether the picture itself became long term.
    fn commands<S>(&mut self, slots: &mut S, serial: i32, parity: usize, marking: &Marking) -> bool
    where
        S: Slots<Id = I, Held = H>,
    {
        let current_num = if marking.field {
            2 * marking.frame_num as i32 + 1
        } else {
            marking.frame_num as i32
        };
        let mut long = false;
        for command in &marking.commands {
            match command.op {
                1 | 3 => {
                    let wanted = current_num - (command.first as i32 + 1);
                    if let Some((store, side)) = self.find_short(wanted, parity, marking.field) {
                        if command.op == 1 {
                            self.unmark(store, side, marking.field);
                        } else {
                            self.stores[store].long_index = command.second as i32;
                            self.stores[store].marked[side] = Marked::Long;
                            if !marking.field {
                                self.stores[store].marked = [Marked::Long; 2];
                            }
                        }
                    }
                }
                2 => {
                    let wanted = command.second as i32;
                    if let Some((store, side)) = self.find_long(wanted, parity, marking.field) {
                        self.unmark(store, side, marking.field);
                    }
                }
                4 => {
                    let most = command.second as i32 - 1;
                    for store in &mut self.stores {
                        if store.marked.contains(&Marked::Long) && store.long_index > most {
                            store.marked = [Marked::No; 2];
                        }
                    }
                }
                5 => {
                    let at = self.index(serial);
                    for (other, store) in self.stores.iter_mut().enumerate() {
                        if Some(other) != at {
                            store.marked = [Marked::No; 2];
                        }
                    }
                    self.forget();
                    if let Some(at) = at {
                        self.stores[at].frame_num = 0;
                    }
                }
                6 => {
                    if let Some(at) = self.index(serial) {
                        self.stores[at].long_index = command.second as i32;
                        self.stores[at].marked[parity] = Marked::Long;
                        if !marking.field {
                            self.stores[at].marked = [Marked::Long; 2];
                        }
                        long = true;
                    }
                }
                _ => {}
            }
        }
        let dropped: Vec<I> = self
            .stores
            .iter()
            .filter(|store| !store.reference() && !store.output)
            .map(|store| store.slot)
            .collect();
        for slot in dropped {
            slots.release(slot);
        }
        self.stores
            .retain(|store| store.reference() || store.output);
        long
    }
}

impl<I: Copy, H: Ordered> Dpb<I, H> {
    /// ピクチャーを一枚開く。片方のフィールドだけ開いているものがあれば、そこへ合流する。
    /// `stale` はデバイスがデコードを拒否して開いたままのピクチャー。
    pub fn start<S>(
        &mut self,
        slots: &mut S,
        stale: Option<Current>,
        sps: &Sps,
        header: &Header,
        held: impl FnOnce(I, i32) -> H,
    ) -> Result<Current, crate::video::Error>
    where
        S: Slots<Id = I, Held = H>,
    {
        // A begun but unfinished picture would hold its surface for ever and empty the pool.
        if let Some(stale) = stale
            && let Some(at) = self.index(stale.serial)
            && !self.stores[at].complete()
        {
            slots.release(self.stores[at].slot);
            self.stores.remove(at);
        }

        self.capacity = dpb_frames(sps);
        self.reorder = sps
            .max_reorder
            .map_or(self.capacity, |count| count as usize)
            .min(self.capacity);
        let poc = self.order_count(sps, header);
        let parity = usize::from(header.bottom_field);
        let field = header.field_pic;

        if header.idr {
            self.refresh(slots, header.no_output_of_prior_pics);
        }

        let waiting = self
            .pending
            .and_then(|(serial, first)| Some((self.index(serial)?, first)));
        let second = match waiting {
            Some((at, first)) if field && first != parity => {
                self.stores[at].frame_num == header.frame_num
            }
            _ => false,
        };

        let store = if second {
            self.pending = None;
            match waiting {
                Some((at, _)) => at,
                None => return Err(crate::video::Error::Damaged("a field that stopped waiting")),
            }
        } else {
            if let Some((at, _)) = waiting {
                self.pending = None;
                self.stores[at].output = false;
            }
            let Some(slot) = self.free_slot(slots) else {
                return Err(crate::video::Error::Damaged(
                    "a picture buffer that cannot be made room in",
                ));
            };
            let serial = self.next_serial();
            self.stores.push(Store {
                slot,
                serial,
                frame_num: header.frame_num,
                wrap: header.frame_num as i32,
                long_index: 0,
                present: [false; 2],
                has_poc: [false; 2],
                poc: [0; 2],
                marked: [Marked::No; 2],
                output: true,
                held: held(slot, poc[0].min(poc[1])),
            });
            self.stores.len() - 1
        };

        self.stores[store].poc[parity] = poc[parity];
        self.stores[store].has_poc[parity] = true;
        if !field {
            self.stores[store].poc = poc;
            self.stores[store].has_poc = [true; 2];
        }
        let order = self.stores[store].order();
        self.stores[store].held.set_order(order);
        Ok(Current {
            serial: self.stores[store].serial,
            parity,
            field,
            marking: Marking::new(header, sps),
            extra: (),
        })
    }

    pub fn finish<S, E>(&mut self, slots: &mut S, current: Current<E>)
    where
        S: Slots<Id = I, Held = H>,
    {
        let Some(at) = self.index(current.serial) else {
            return;
        };
        self.stores[at].present[current.parity] = true;
        if !current.field {
            self.stores[at].present = [true; 2];
        }
        let order = self.stores[at].order();
        self.stores[at].held.set_order(order);
        self.mark(slots, current.serial, current.parity, &current.marking);
        let complete = self
            .index(current.serial)
            .is_some_and(|at| self.stores[at].complete());
        self.pending = if complete {
            None
        } else {
            Some((current.serial, current.parity))
        };
        self.drain(slots);
    }
}

/// From the VUI where sent, otherwise from the level.
pub fn dpb_frames(sps: &Sps) -> usize {
    if let Some(count) = sps.max_dec_frame_buffering {
        return (count as usize).clamp(1, 16);
    }
    // Table A-1.
    let macroblocks: u32 = match sps.level {
        0..=10 => 396,
        11 => 900,
        12..=20 => 2376,
        21 => 4752,
        22..=30 => 8100,
        31 => 18000,
        32 => 20480,
        33..=41 => 32768,
        42 => 34816,
        50 => 110400,
        51..=52 => 184320,
        _ => 696320,
    };
    // u32 に詰めてから max(1) すると、積が 2^32 の倍数のとき 0 で割る。広い型のまま割る。
    let picture = sps.mb_width.saturating_mul(sps.mb_height()).max(1);
    (macroblocks as usize / picture).clamp(sps.max_ref_frames.max(1) as usize, 16)
}
