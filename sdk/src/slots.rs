// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番号で検索するテーブル。番号は 1 からで、0 は境界で「無い」の意味。空いた番号は使い回す。

pub(crate) struct Slots<T>(Vec<Option<T>>, usize);

#[cfg_attr(not(feature = "host"), allow(dead_code))]
impl<T> Slots<T> {
    pub(crate) const fn new() -> Self {
        Self(Vec::new(), usize::MAX)
    }

    /// 超える `put` は 0 を返す（guest が作り続けても伸びない）。
    pub(crate) const fn with_most(most: usize) -> Self {
        Self(Vec::new(), most)
    }

    pub(crate) fn put(&mut self, it: T) -> u32 {
        if let Some(free) = self.0.iter().position(Option::is_none) {
            self.0[free] = Some(it);
            return free as u32 + 1;
        }
        if self.0.len() >= self.1 {
            return 0;
        }
        self.0.push(Some(it));
        self.0.len() as u32
    }

    pub(crate) fn slot(&mut self, handle: u32) -> Option<&mut Option<T>> {
        self.0.get_mut(handle.wrapping_sub(1) as usize)
    }

    pub(crate) fn get(&self, handle: u32) -> Option<&T> {
        self.0
            .get(handle.wrapping_sub(1) as usize)
            .and_then(Option::as_ref)
    }

    pub(crate) fn get_mut(&mut self, handle: u32) -> Option<&mut T> {
        self.slot(handle).and_then(Option::as_mut)
    }

    pub(crate) fn take(&mut self, handle: u32) -> Option<T> {
        self.slot(handle).and_then(Option::take)
    }

    pub(crate) fn pair(&mut self, into: u32, from: u32) -> Option<(&mut T, &T)> {
        let [into, from] = self
            .0
            .get_disjoint_mut([into.wrapping_sub(1) as usize, from.wrapping_sub(1) as usize])
            .ok()?;
        Some((into.as_mut()?, from.as_ref()?))
    }

    pub(crate) fn count(&self) -> usize {
        self.0.iter().filter(|it| it.is_some()).count()
    }
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::fmt::Debug for Slots<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Slots({})", self.count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_start_at_one_and_are_reused() {
        let mut slots = Slots::new();
        assert_eq!(slots.put('a'), 1);
        assert_eq!(slots.put('b'), 2);
        assert_eq!(slots.take(1), Some('a'));
        assert_eq!(slots.get(0), None, "0 は無い");
        assert_eq!(slots.put('c'), 1, "空いた番号を使う");
        assert_eq!(slots.pair(1, 2).map(|(a, b)| (*a, *b)), Some(('c', 'b')));
        assert!(slots.pair(2, 2).is_none(), "同じ番号は二重に取らない");
        assert_eq!(slots.count(), 2);
        let mut few = Slots::with_most(1);
        assert_eq!(few.put('a'), 1);
        assert_eq!(few.put('b'), 0, "上限を超えたら 0");
        few.take(1);
        assert_eq!(few.put('c'), 1, "空けば入る");
    }
}
