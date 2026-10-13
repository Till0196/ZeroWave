// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Which capture buffer a picture is in, and when it may be reused. A picture is named by
//! the timestamp the driver copies from output to capture buffer, bound on dequeue.

use std::collections::HashMap;

use crate::decode::slots::Slots;

/// Buffers beyond the references for pictures a caller holds. Exports come back by lease
/// when dropped, not by counting: a counted ring was outrun and pictures overwritten.
pub const SLACK: usize = crate::decode::MOST_HELD;

static STOLEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug)]
pub struct Pool {
    /// 割り当てられるタイムスタンプの数。貸し出しで埋まれば [`Pool::limit`] まで広がる。
    capacity: usize,
    limit: usize,
    slots: Slots<u64>,
    index: HashMap<u64, usize>,
    /// Starts at 1 ms so zero (an unset reference) never names a picture.
    next: u64,
    freed: Vec<usize>,
}

impl Pool {
    pub fn new(capacity: usize) -> Self {
        Self::growing(capacity, capacity)
    }

    /// 広がった分のバッファは、キューに入れるときに呼び出し側が追加する。
    pub fn growing(capacity: usize, limit: usize) -> Self {
        Self {
            capacity,
            limit: limit.max(capacity),
            slots: Slots::new(SLACK),
            index: HashMap::new(),
            next: 1_000_000,
            freed: Vec::new(),
        }
    }

    /// Stepped by 1 ms so `struct timeval` (µs) cannot merge two.
    pub fn take(&mut self) -> Option<u64> {
        if self.slots.len() >= self.capacity {
            for ts in self.slots.collect() {
                self.free(ts);
            }
        }
        if self.slots.len() >= self.capacity {
            if self.capacity < self.limit {
                self.capacity += 1;
            } else if !self.steal() {
                return None;
            }
        }
        let ts = self.next;
        self.next += 1_000_000;
        self.slots.insert(ts);
        Some(ts)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn stop_growing(&mut self, at: usize) {
        self.capacity = self.capacity.min(at);
        self.limit = self.capacity;
    }

    pub fn bind(&mut self, ts: u64, index: usize) {
        if self.slots.holds(ts) {
            self.index.insert(ts, index);
        }
    }

    /// A stateful decoder names its own pictures; unadopted, their buffers never come back.
    pub fn adopt(&mut self, ts: u64, index: usize) {
        self.slots.insert(ts);
        self.bind(ts, index);
    }

    pub fn known(&self, ts: u64) -> bool {
        self.slots.holds(ts)
    }

    pub fn index(&self, ts: u64) -> Option<usize> {
        self.index.get(&ts).copied()
    }

    pub fn hold(&mut self, ts: u64) {
        self.slots.hold(ts);
    }

    pub fn release(&mut self, ts: u64) {
        if self.slots.release(ts) {
            self.free(ts);
        }
    }

    fn free(&mut self, ts: u64) {
        if let Some(index) = self.index.remove(&ts) {
            self.freed.push(index);
        }
    }

    /// The buffer comes back when every copy of the lease is dropped, not after a count.
    pub fn leased(&mut self, ts: u64) -> std::sync::Arc<crate::decode::slots::Lease> {
        self.slots.leased(ts)
    }

    /// Where nothing is free, a torn picture rather than a stalled decoder.
    pub fn steal(&mut self) -> bool {
        let (held, lent) = (self.slots.len(), self.slots.live_leases());
        match self.slots.steal_oldest() {
            Some(ts) => {
                self.free(ts);
                tracing::debug!("V4L2: 表示中のバッファを回収（使用中 {held}、貸し出し中 {lent}）");
                if !STOLEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::warn!(
                        "V4L2: capture バッファが足りないので、表示中のバッファを回収した（{} 本のうち使用中 {held}、貸し出し中 {lent}）",
                        self.capacity
                    );
                }
                true
            }
            None => false,
        }
    }

    /// For the deinterlacer, which reads the picture a few pictures later.
    pub fn issued(&mut self, ts: u64) {
        for old in self.slots.issued(ts) {
            self.free(old);
        }
    }

    pub fn forget_recent(&mut self) {
        for old in self.slots.forget_recent() {
            self.free(old);
        }
    }

    pub fn reclaim(&mut self) -> Vec<usize> {
        for ts in self.slots.collect() {
            self.free(ts);
        }
        std::mem::take(&mut self.freed)
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.index.clear();
        self.freed.clear();
    }
}

impl crate::decode::frames::Surfaces for Pool {
    type Id = u64;

    fn take(&mut self) -> Option<Self::Id> {
        Pool::take(self)
    }

    fn hold(&mut self, id: Self::Id) {
        Pool::hold(self, id);
    }

    fn release(&mut self, id: Self::Id) {
        Pool::release(self, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_handed_out_is_kept_back_for_the_length_of_the_ring() {
        let mut pool = Pool::new(SLACK + 2);
        let first = pool.take().expect("a free buffer");
        pool.bind(first, 0);
        pool.issued(first);
        for index in 1..=SLACK {
            let ts = pool.take().expect("a free buffer");
            pool.bind(ts, index);
            assert!(pool.reclaim().is_empty(), "a buffer came back while in use");
            pool.issued(ts);
        }
        assert_eq!(pool.reclaim(), vec![0]);
    }

    #[test]
    fn a_lent_picture_comes_back_when_its_lease_is_dropped() {
        let mut pool = Pool::new(SLACK + 4);
        let first = pool.take().expect("a free buffer");
        pool.bind(first, 0);
        let kept = pool.leased(first);
        let mut others = Vec::new();
        for index in 1..SLACK + 2 {
            let ts = pool.take().expect("a free buffer");
            pool.bind(ts, index);
            others.push(pool.leased(ts));
            assert!(pool.reclaim().is_empty(), "a buffer came back while in use");
        }
        drop(kept);
        assert_eq!(pool.reclaim(), vec![0]);
    }

    #[test]
    fn a_full_pool_grows_up_to_its_limit_before_taking_back() {
        let mut pool = Pool::growing(2, 3);
        let mut held = Vec::new();
        for index in 0..2 {
            let ts = pool.take().expect("a free buffer");
            pool.bind(ts, index);
            held.push(pool.leased(ts));
        }
        let third = pool.take().expect("広がる");
        assert_eq!(pool.capacity(), 3);
        assert!(pool.reclaim().is_empty(), "広げられるのに取り戻した");
        pool.bind(third, 2);
        held.push(pool.leased(third));
        assert!(pool.take().is_some(), "上限では取り戻す");
        assert_eq!(pool.reclaim(), vec![0]);
    }

    #[test]
    fn a_full_pool_takes_back_its_oldest_loan() {
        let mut pool = Pool::new(2);
        let mut held = Vec::new();
        for index in 0..2 {
            let ts = pool.take().expect("a free buffer");
            pool.bind(ts, index);
            held.push(pool.leased(ts));
        }
        assert!(pool.take().is_some(), "a full pool stalled");
        assert_eq!(pool.reclaim(), vec![0]);
    }

    #[test]
    fn a_picture_two_things_need_is_free_when_both_have_done() {
        let mut pool = Pool::new(4);
        let ts = pool.take().expect("a free buffer");
        pool.bind(ts, 3);
        pool.hold(ts);
        pool.release(ts);
        assert!(pool.reclaim().is_empty());
        pool.release(ts);
        assert_eq!(pool.reclaim(), vec![3]);
        assert!(!pool.known(ts));
    }

    #[test]
    fn a_picture_the_decoder_named_itself_comes_back_like_any_other() {
        let mut pool = Pool::new(SLACK + 2);
        pool.adopt(9_000_000, 5);
        assert_eq!(pool.index(9_000_000), Some(5));
        pool.issued(9_000_000);
        for index in 0..SLACK {
            let ts = 20_000_000 + index as u64 * 1_000_000;
            pool.adopt(ts, index);
            assert!(pool.reclaim().is_empty(), "a buffer came back while in use");
            pool.issued(ts);
        }
        assert_eq!(pool.reclaim(), vec![5]);
    }

    #[test]
    fn every_picture_gets_its_own_timestamp_and_none_is_nought() {
        let mut pool = Pool::new(64);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let ts = pool.take().expect("a free buffer");
            assert_ne!(ts, 0);
            assert!(seen.insert(ts), "two pictures with one timestamp");
        }
        assert_eq!(pool.take(), None, "the pool handed out more than it has");
    }

    #[test]
    fn a_timestamp_survives_the_kernels_microsecond_clock() {
        use crate::decode::v4l2::uapi::Timeval;
        let mut pool = Pool::new(64);
        for _ in 0..64 {
            let ts = pool.take().expect("a free buffer");
            assert_eq!(Timeval::from_ns(ts).to_ns(), ts);
        }
    }
}
