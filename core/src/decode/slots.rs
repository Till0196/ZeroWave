// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! サーフェスの管理テーブル。ID（`Id`）と参照カウントだけを持つ。渡したフレームはレンダラーが
//! 読み終えるまで再利用しない（リングかリースで守る）。間違えるとクラッシュせずに映像が乱れる。

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Arc;

#[derive(Debug)]
pub struct Lease;

#[derive(Debug)]
pub struct Slots<Id> {
    refs: HashMap<Id, u32>,
    recent: VecDeque<Id>,
    slack: usize,
    /// 解放されたかで判定する貸し出し（リングの数え合わせは一枚ずれると別のフレームが出る）。
    leased: VecDeque<(Id, Arc<Lease>)>,
}

impl<Id: Copy + Eq + Hash> Slots<Id> {
    pub fn new(slack: usize) -> Self {
        Self {
            refs: HashMap::new(),
            recent: VecDeque::new(),
            slack,
            leased: VecDeque::new(),
        }
    }

    pub fn take_from(&mut self, candidates: impl IntoIterator<Item = Id>) -> Option<Id> {
        self.sweep();
        let id = candidates
            .into_iter()
            .find(|id| !self.refs.contains_key(id))?;
        self.refs.insert(id, 1);
        Some(id)
    }

    pub fn insert(&mut self, id: Id) -> bool {
        if self.refs.contains_key(&id) {
            return false;
        }
        self.refs.insert(id, 1);
        true
    }

    pub fn holds(&self, id: Id) -> bool {
        self.refs.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.refs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    /// 空きが無いとき、いちばん古い貸し出しを回収する（次の選局まで映像が出ないよりまし）。
    pub fn reclaim_from(&mut self, candidates: impl IntoIterator<Item = Id> + Clone) -> Option<Id> {
        while let Some(old) = self.recent.pop_front() {
            self.release(old);
            if let Some(id) = self.take_from(candidates.clone()) {
                return Some(id);
            }
        }
        while let Some((old, _)) = self.leased.pop_front() {
            // リースの参照ごと解放する。一つだけ返すと、リースが解放されたときに数が狂う。
            self.refs.remove(&old);
            if let Some(id) = self.take_from(candidates.clone()) {
                return Some(id);
            }
        }
        None
    }

    /// フレームをリース付きで渡した。クローンが全部消えるまでサーフェスは確保したまま。
    pub fn leased(&mut self, id: Id) -> Arc<Lease> {
        // リースは参照を取らない。取るとスロットが足りなくなり、[`Self::sweep`] が取っていない参照を返す。
        // 既知の問題：H.264 の階層 B は非参照ピクチャーをすぐ解放するので、渡した直後に再利用されうる。
        let lease = Arc::new(Lease);
        self.leased.push_back((id, lease.clone()));
        lease
    }

    pub fn live_leases(&self) -> usize {
        self.leased
            .iter()
            .filter(|(_, lease)| Arc::strong_count(lease) > 1)
            .count()
    }

    /// 誰も保持しなくなったリースのスロットを解放し、空いたものを返す（自分でバッファを戻す V4L2 用）。
    pub fn collect(&mut self) -> Vec<Id> {
        let mut dropped = Vec::new();
        self.leased.retain(|(id, lease)| {
            let held = Arc::strong_count(lease) > 1;
            if !held {
                dropped.push(*id);
            }
            held
        });
        dropped.into_iter().filter(|id| self.release(*id)).collect()
    }

    /// リースがまだ保持されていても、いちばん古い貸し出しを回収する（[`Self::reclaim_from`] と同じ）。
    pub fn steal_oldest(&mut self) -> Option<Id> {
        let (old, _) = self.leased.pop_front()?;
        self.refs.remove(&old);
        Some(old)
    }

    fn sweep(&mut self) {
        let mut dropped = Vec::new();
        self.leased.retain(|(id, lease)| {
            let held = Arc::strong_count(lease) > 1;
            if !held {
                dropped.push(*id);
            }
            held
        });
        for id in dropped {
            self.release(id);
        }
    }

    pub fn hold(&mut self, id: Id) {
        if let Some(count) = self.refs.get_mut(&id) {
            *count += 1;
        }
    }

    pub fn release(&mut self, id: Id) -> bool {
        let Some(count) = self.refs.get_mut(&id) else {
            return false;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.refs.remove(&id);
            return true;
        }
        false
    }

    /// フレームを渡した。キューの参照はリングへ移り、リングが解放して空いたものが古い順に返る。
    pub fn issued(&mut self, id: Id) -> Vec<Id> {
        self.recent.push_back(id);
        let mut freed = Vec::new();
        while self.recent.len() > self.slack {
            if let Some(old) = self.recent.pop_front()
                && self.release(old)
            {
                freed.push(old);
            }
        }
        freed
    }

    pub fn forget_recent(&mut self) -> Vec<Id> {
        let mut freed = Vec::new();
        while let Some(old) = self.recent.pop_front() {
            if self.release(old) {
                freed.push(old);
            }
        }
        while let Some((old, _)) = self.leased.pop_front() {
            if self.release(old) {
                freed.push(old);
            }
        }
        freed
    }

    pub fn clear(&mut self) {
        self.refs.clear();
        self.recent.clear();
        self.leased.clear();
    }
}

impl Slots<usize> {
    pub fn numbered(count: usize, slack: usize) -> Numbered {
        Numbered {
            slots: Self::new(slack),
            count,
        }
    }
}

#[derive(Debug)]
pub struct Numbered {
    slots: Slots<usize>,
    count: usize,
}

impl Numbered {
    pub fn take(&mut self) -> Option<usize> {
        self.slots.take_from(0..self.count)
    }

    pub fn reclaim(&mut self) -> Option<usize> {
        self.slots.reclaim_from(0..self.count)
    }

    pub fn hold(&mut self, slot: usize) {
        self.slots.hold(slot);
    }

    pub fn release(&mut self, slot: usize) {
        self.slots.release(slot);
    }

    pub fn issued(&mut self, slot: usize) {
        self.slots.issued(slot);
    }

    pub fn leased(&mut self, slot: usize) -> Arc<Lease> {
        self.slots.leased(slot)
    }

    pub fn live_leases(&self) -> usize {
        self.slots.live_leases()
    }

    pub fn forget_recent(&mut self) {
        self.slots.forget_recent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLACK: usize = 8;

    #[test]
    fn a_surface_handed_out_is_kept_back_for_the_length_of_the_ring() {
        let mut slots = Slots::numbered(SLACK + 2, SLACK);
        let first = slots.take().expect("a free surface");
        slots.issued(first);
        for _ in 0..SLACK {
            let slot = slots.take().expect("a free surface");
            assert_ne!(slot, first, "the surface came back while it was in use");
            slots.issued(slot);
        }
        assert_eq!(slots.take(), Some(first));
    }

    #[test]
    fn a_surface_two_things_need_is_free_when_both_have_done() {
        let mut slots = Slots::numbered(2, SLACK);
        let slot = slots.take().expect("a free surface");
        slots.hold(slot);
        slots.release(slot);
        assert_ne!(slots.take(), Some(slot));
        slots.release(slot);
        assert_eq!(slots.take(), Some(slot));
    }

    #[test]
    fn the_oldest_handed_out_surface_comes_back_when_nothing_is_free() {
        let mut slots = Slots::numbered(2, SLACK);
        let first = slots.take().expect("a free surface");
        slots.issued(first);
        let second = slots.take().expect("a free surface");
        slots.issued(second);
        assert_eq!(slots.take(), None, "both are spoken for");
        assert_eq!(slots.reclaim(), Some(first), "the oldest comes back first");
    }

    #[test]
    #[ignore = "リースに参照を取らせるとスロットが足りなくなる。枚数の決め方を直してから戻す"]
    fn a_lease_holds_a_surface_the_decoder_has_already_let_go() {
        let mut slots = Slots::<usize>::numbered(3, SLACK);
        let first = slots.take().expect("a free surface");
        slots.release(first);
        let lease = slots.leased(first);
        for _ in 0..2 {
            let slot = slots.take().expect("a free surface");
            assert_ne!(slot, first, "リースが有効なスロットを割り当てた");
        }
        assert!(slots.take().is_none(), "四枚目は無い");
        drop(lease);
        assert_eq!(slots.take(), Some(first), "リースが解放されたら戻る");
    }

    #[test]
    fn a_leased_surface_stays_out_until_the_lease_is_dropped() {
        let mut slots = Slots::numbered(SLACK + 4, SLACK);
        let first = slots.take().expect("a free surface");
        let lease = slots.leased(first);
        let mut others = Vec::new();
        for _ in 0..SLACK + 3 {
            let slot = slots.take().expect("a free surface");
            assert_ne!(slot, first, "the surface came back while it was held");
            others.push(slots.leased(slot));
        }
        assert_eq!(slots.take(), None, "everything is out");
        drop(lease);
        assert_eq!(slots.take(), Some(first), "dropping the lease frees it");
        let middle = others.remove(2);
        drop(middle);
        assert_eq!(slots.take(), Some(3));
        assert_eq!(slots.take(), None);
    }

    #[test]
    fn the_oldest_leased_surface_comes_back_when_nothing_is_free() {
        let mut slots = Slots::numbered(2, SLACK);
        let first = slots.take().expect("a free surface");
        let _first = slots.leased(first);
        let second = slots.take().expect("a free surface");
        let _second = slots.leased(second);
        assert_eq!(slots.take(), None);
        assert_eq!(slots.reclaim(), Some(first));
    }

    #[test]
    fn reclaiming_an_empty_ring_gives_nothing() {
        let mut slots = Slots::numbered(1, SLACK);
        let only = slots.take().expect("a free surface");
        slots.hold(only);
        assert_eq!(slots.reclaim(), None);
    }

    #[test]
    fn a_named_surface_says_when_it_falls_free() {
        let mut slots: Slots<u64> = Slots::new(2);
        assert!(slots.insert(10));
        assert!(!slots.insert(10), "held twice by one insert");
        slots.hold(10);
        assert!(!slots.release(10));
        assert!(slots.release(10), "the last reference frees it");
        assert!(!slots.holds(10));
        for id in [1, 2] {
            slots.insert(id);
            assert!(slots.issued(id).is_empty());
        }
        slots.insert(3);
        assert_eq!(slots.issued(3), vec![1]);
        assert!(!slots.holds(1));
        assert_eq!(slots.forget_recent(), vec![2, 3]);
        assert!(slots.is_empty());
    }
}
