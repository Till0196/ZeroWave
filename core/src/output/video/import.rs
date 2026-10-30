// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラットフォームのデコーダーが書いたフレームからコピーなしでレンダリングする（渡し方ごとに一つのモジュール）。

#[cfg(all(feature = "android", target_os = "android"))]
pub mod android;
#[cfg(all(feature = "d3d12", windows))]
pub mod d3d12;
/// Vulkan でレンダリングするとき、Windows のデコーダーのフレームを共有したバッファで受け取る。
#[cfg(all(feature = "d3d12", windows))]
pub mod d3d12_vulkan;
#[cfg(feature = "vaapi")]
pub mod dmabuf;
/// `CVMetalTextureCache` でプレーンごとの `MTLTexture` にし、wgpu が包む。
#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
pub mod videotoolbox;

/// 作ったラッパーのテーブル。ラッパーは元の参照を保持するので、上限が無いと替わったデコーダーのリングバッファが残る。
/// 使用中のフレームはプールがコピーを持つので捨てても困らない。
#[cfg(any(
    all(feature = "android", target_os = "android"),
    all(feature = "d3d12", windows),
    feature = "vaapi"
))]
#[derive(Debug)]
pub(crate) struct Kept<K, V> {
    made: std::collections::HashMap<K, (u64, V)>,
    tick: u64,
}

#[cfg(any(
    all(feature = "android", target_os = "android"),
    all(feature = "d3d12", windows),
    feature = "vaapi"
))]
impl<K: std::hash::Hash + Eq + Clone, V: Clone> Kept<K, V> {
    pub(crate) const MOST: usize = 128;

    pub(crate) fn new() -> Self {
        Self {
            made: std::collections::HashMap::new(),
            tick: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.made.len()
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        self.made.get_mut(key).map(|(used, value)| {
            *used = tick;
            value.clone()
        })
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.tick += 1;
        if self.made.len() >= Self::MOST
            && !self.made.contains_key(&key)
            && let Some(oldest) = self
                .made
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(key, _)| key.clone())
        {
            self.made.remove(&oldest);
        }
        self.made.insert(key, (self.tick, value));
    }

    pub(crate) fn clear(&mut self) {
        self.made.clear();
    }
}

#[cfg(all(test, feature = "vaapi"))]
mod tests {
    use super::Kept;

    #[test]
    fn the_least_recently_used_goes_first() {
        let mut kept = Kept::new();
        for key in 0..Kept::<u32, u32>::MOST as u32 {
            kept.insert(key, key);
        }
        assert_eq!(kept.get(&0), Some(0));
        kept.insert(1000, 1000);
        assert_eq!(kept.len(), Kept::<u32, u32>::MOST);
        assert_eq!(kept.get(&0), Some(0));
        assert_eq!(kept.get(&1), None);
        assert_eq!(kept.get(&1000), Some(1000));
    }
}
