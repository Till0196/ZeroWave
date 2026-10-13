// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ピクチャーをどのテクスチャへデコードし、いつ再利用してよいか。
//! DXVA のピクチャーパラメーターは参照を `ReferenceFrames` の番号（`Index7Bits`）で指定する。番号の数は
//! DPB の枚数にし（`MaxDecodePictureBufferCount` も同じ）、[`Pool::prepare`] がピクチャーごとに割り当てる。
//! テクスチャはそれより多い（渡したフレームはレンダリングが済むまで書かない）。D3D11VA では番号が出力ビューの
//! 番号なので、スロット番号がそのまま入る。

use super::dx::Twin;
use crate::decode::slots::{Numbered, Slots};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

#[derive(Debug)]
pub enum Twins {
    None,
    Each(Vec<ID3D12Resource>),
    /// Tier 1 の一つの配列。スライスの番号が DXVA の番号。
    Array {
        array: ID3D12Resource,
        slices: usize,
        direct: bool,
    },
}

/// 渡したあと手元に残すテクスチャの枚数。戻るのは渡したフレームのリース（[`Pool::leased`]）が解放された時。
pub const SLACK: usize = crate::decode::MOST_HELD;

/// 代わりのピクチャーの書き込み済みの印（0 でなければよい。表示しないので待たない）。
const STOOD_IN: u64 = 1;

/// サーフェスが空くのを何回待つか（一回 2 ms、合計 0.5 秒）。
const WAITS: usize = 250;

#[derive(Debug)]
pub struct Pool {
    textures: Vec<ID3D12Resource>,
    /// 参照専用のコピー。`twins` のスロット `n` は `textures` のスロット `n` のコピー。
    twins: Twins,
    references: Vec<Option<ID3D12Resource>>,
    subresources: Vec<u32>,
    slots: Numbered,
    marks: Marks,
    hidden: Vec<bool>,
    slack: usize,
    warned: bool,
    /// スロットごとの DXVA の番号。
    slice_of: Vec<Option<usize>>,
    /// 代わりのピクチャー（規格 8.3.3）に貸して、まだ埋めていないスロット。
    standing: Vec<usize>,
    /// 代わりのピクチャー専用の、空いているスロット（[`Pool::new`] の `stand_ins`）。
    spare: Vec<usize>,
    /// これより後ろのスロットは代わりのピクチャー専用。
    ordinary: usize,
}

impl Pool {
    /// `entries` は DXVA の番号の数（Tier 1 の配列ならスライスの数）。テクスチャの数以上なら
    /// スロット番号をそのまま使う。後ろの `stand_ins` 枚は代わりのピクチャー（規格 8.3.3）専用にする。
    /// 代わりのピクチャーはストリームが示す DPB の枚数の外に増える（C.5.2.2 は規格どおりの入力が前提）ので、
    /// 表示を待つフレームのサーフェスとは分ける。
    pub fn new(
        textures: Vec<ID3D12Resource>,
        twins: Twins,
        slack: usize,
        entries: usize,
        stand_ins: usize,
    ) -> Self {
        if let Twins::Each(each) = &twins {
            debug_assert_eq!(each.len(), textures.len());
        }
        if let Twins::Array { slices, .. } = &twins {
            debug_assert_eq!(*slices, entries);
        }
        let ordinary = textures.len().saturating_sub(stand_ins);
        let slots = Slots::numbered(ordinary, slack);
        let (entries, slice_of) = if entries < textures.len() {
            (entries, vec![None; textures.len()])
        } else {
            (textures.len(), (0..textures.len()).map(Some).collect())
        };
        let references = vec![None; entries];
        let subresources = vec![0; entries];
        let marks = Marks::new(textures.len());
        let hidden = vec![false; textures.len()];
        let textures_len = textures.len();
        Self {
            textures,
            twins,
            references,
            subresources,
            slots,
            marks,
            hidden,
            slack,
            warned: false,
            slice_of,
            standing: Vec::new(),
            spare: (ordinary..textures_len).rev().collect(),
            ordinary,
        }
    }

    /// このピクチャーに DXVA の番号を割り当てる。`references` には、このピクチャーか後のピクチャーが
    /// 参照し得るものを全部渡す。渡さなかったスロットの番号は空く。
    pub fn prepare(
        &mut self,
        slot: usize,
        references: &[usize],
    ) -> Result<(), crate::decode::Error> {
        let entries = self.references.len();
        if entries >= self.textures.len() {
            return Ok(());
        }
        for (at, slice) in self.slice_of.iter_mut().enumerate() {
            if at != slot && !references.contains(&at) {
                *slice = None;
            }
        }
        if self.slice_of.get(slot).copied().flatten().is_none() {
            let taken: Vec<usize> = self.slice_of.iter().flatten().copied().collect();
            let free = (0..entries).find(|slice| !taken.contains(slice)).ok_or(
                crate::decode::Error::Damaged("more references than the DPB holds"),
            )?;
            self.slice_of[slot] = Some(free);
        }
        Ok(())
    }

    pub fn direct(&self, slot: usize) -> Option<(ID3D12Resource, [u32; 2])> {
        let Twins::Array {
            array,
            slices,
            direct: true,
        } = &self.twins
        else {
            return None;
        };
        let slice = self.slice_of.get(slot).copied().flatten()? as u32;
        Some((array.clone(), [slice, slice + *slices as u32]))
    }

    /// DXVA がこのスロットを指す番号。番号を持たないスロットは `None`
    /// （スロット番号で代用すると `ReferenceFrames` の外を指す）。
    pub fn index(&self, slot: usize) -> Option<usize> {
        self.slice_of.get(slot).copied().flatten()
    }

    pub fn texture(&self, slot: usize) -> Option<&ID3D12Resource> {
        self.textures.get(slot)
    }

    /// 代わりのピクチャーに貸したスロットのうち、まだ埋めていないもの。
    pub fn take_stand_ins(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.standing)
    }

    /// 参照として読まれるテクスチャと、輝度・色差のサブリソース。参照専用のコピーはコピー先にできない
    /// ので `None`。
    pub fn writable(&self, slot: usize) -> Option<(ID3D12Resource, [u32; 2])> {
        match &self.twins {
            Twins::None => Some((self.textures.get(slot)?.clone(), [0, 1])),
            _ => self.direct(slot),
        }
    }

    /// 代わりのピクチャーを参照に使えるようにする。表示はしないので、待つためのフェンスの値は持たない。
    pub fn stood_in(&mut self, slot: usize) {
        self.marks.wrote(slot, STOOD_IN);
    }

    fn twin<'a>(twins: &'a Twins, slot: usize, slice_of: &[Option<usize>]) -> Option<Twin<'a>> {
        match twins {
            Twins::None => None,
            Twins::Each(each) => each.get(slot).map(Twin::Own),
            Twins::Array {
                array,
                slices,
                direct,
            } => {
                let slot = slice_of.get(slot).copied().flatten().unwrap_or(slot) as u32;
                let count = *slices as u32;
                Some(if *direct {
                    Twin::Direct { array, slot, count }
                } else {
                    Twin::Slice { array, slot, count }
                })
            }
        }
    }

    /// `DecodeFrame` に渡す二つの配列。`CurrPic` の番号も `ReferenceFrames` を引くので、デコード中の
    /// ピクチャーも入れる（遷移させるのは書き込みの状態だけ）。
    pub fn references(
        &mut self,
        used: &[usize],
        slot: usize,
    ) -> (&mut [Option<ID3D12Resource>], &mut [u32], Option<Twin<'_>>) {
        let named: Vec<usize> = used
            .iter()
            .copied()
            .chain(std::iter::once(slot))
            .filter(|slot| self.slice_of.get(*slot).copied().flatten().is_some())
            .collect();
        for (at, (entry, subresource)) in self
            .references
            .iter_mut()
            .zip(&mut self.subresources)
            .enumerate()
        {
            let here = named
                .iter()
                .copied()
                .find(|slot| self.slice_of[*slot] == Some(at));
            *entry = here.and_then(|slot| match &self.twins {
                Twins::None => self.textures.get(slot).cloned(),
                Twins::Each(each) => each.get(slot).cloned(),
                Twins::Array { array, .. } => Some(array.clone()),
            });
            *subresource = match &self.twins {
                Twins::Array { .. } => at as u32,
                _ => 0,
            };
        }
        (
            &mut self.references,
            &mut self.subresources,
            Self::twin(&self.twins, slot, &self.slice_of),
        )
    }

    pub fn wrote(&mut self, slot: usize, value: u64) {
        self.marks.wrote(slot, value);
    }

    /// 今のフレームを書いた fence の値。まだ書いていなければ 0。
    pub fn written(&self, slot: usize) -> u64 {
        self.marks.written(slot)
    }

    /// このテクスチャへ一度も書いていない（中身が 0 のまま）。
    pub fn blank(&self, slot: usize) -> bool {
        self.marks.blank(slot)
    }

    pub fn hide(&mut self, slot: usize, hidden: bool) {
        if let Some(held) = self.hidden.get_mut(slot) {
            *held = hidden;
        }
    }

    pub fn hidden(&self, slot: usize) -> bool {
        self.hidden.get(slot).copied().unwrap_or(false)
    }

    pub fn was_copied(&mut self, slot: usize, value: u64) {
        self.marks.was_copied(slot, value);
    }

    pub fn copied(&self, slot: usize) -> u64 {
        self.marks.copied(slot)
    }

    pub fn take(&mut self) -> Option<usize> {
        let slot = self.wait()?;
        self.marks.fresh(slot);
        Some(slot)
    }

    fn wait(&mut self) -> Option<usize> {
        if let Some(slot) = self.slots.take() {
            return Some(slot);
        }
        // 回収せずに待つ。回収するとレンダラーが読んでいるサーフェスへ次のフレームが入る。
        for _ in 0..WAITS {
            std::thread::sleep(std::time::Duration::from_millis(2));
            if let Some(slot) = self.slots.take() {
                return Some(slot);
            }
        }
        // タイムアウトしたら諦める。ここへ来るのは MOST_HELD の見積もり不足。
        if !self.warned {
            self.warned = true;
            tracing::info!(
                "D3D12: サーフェスが足りないのでフレームを捨てた（リング {} 枚）",
                self.slack
            );
        }
        None
    }

    pub fn hold(&mut self, slot: usize) {
        self.slots.hold(slot);
    }

    pub fn release(&mut self, slot: usize) {
        if slot >= self.ordinary {
            if !self.spare.contains(&slot) {
                self.spare.push(slot);
            }
            return;
        }
        self.slots.release(slot);
    }

    pub fn leased(&mut self, slot: usize) -> std::sync::Arc<crate::decode::slots::Lease> {
        self.slots.leased(slot)
    }

    pub fn issued(&mut self, slot: usize) {
        self.slots.issued(slot);
    }

    pub fn forget_recent(&mut self) {
        self.slots.forget_recent();
    }
}

/// スロットごとの書き込み済みの値。スロットを新しいフレームに使うたびに戻す（前のフレームの値が残ると、参照が
/// そろわず書かなかったフレームを書いたものとして参照に使い、画面にも出す）。
#[derive(Debug)]
struct Marks {
    wrote: Vec<u64>,
    copied: Vec<u64>,
    used: Vec<bool>,
}

impl Marks {
    fn new(count: usize) -> Self {
        Self {
            wrote: vec![0; count],
            copied: vec![0; count],
            used: vec![false; count],
        }
    }

    fn fresh(&mut self, slot: usize) {
        if let Some(held) = self.wrote.get_mut(slot) {
            *held = 0;
        }
        if let Some(held) = self.copied.get_mut(slot) {
            *held = 0;
        }
    }

    fn wrote(&mut self, slot: usize, value: u64) {
        if let Some(held) = self.wrote.get_mut(slot) {
            *held = value;
        }
        if value != 0
            && let Some(used) = self.used.get_mut(slot)
        {
            *used = true;
        }
    }

    fn written(&self, slot: usize) -> u64 {
        self.wrote.get(slot).copied().unwrap_or(0)
    }

    fn blank(&self, slot: usize) -> bool {
        !self.used.get(slot).copied().unwrap_or(false)
    }

    fn was_copied(&mut self, slot: usize, value: u64) {
        if let Some(held) = self.copied.get_mut(slot) {
            *held = value;
        }
    }

    fn copied(&self, slot: usize) -> u64 {
        self.copied.get(slot).copied().unwrap_or(0)
    }
}

impl crate::decode::frames::Surfaces for Pool {
    type Id = usize;

    fn take(&mut self) -> Option<Self::Id> {
        Pool::take(self)
    }

    fn hold(&mut self, id: Self::Id) {
        Pool::hold(self, id);
    }

    fn release(&mut self, id: Self::Id) {
        Pool::release(self, id);
    }

    /// 専用のスロットが尽きたら作らない。同時に要る数は参照ピクチャーセットの参照の数まで。
    fn stand_in(&mut self) -> Option<Self::Id> {
        let slot = self.spare.pop()?;
        self.marks.fresh(slot);
        self.standing.push(slot);
        Some(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_slot_is_unwritten_until_decoded_again() {
        let mut marks = Marks::new(2);
        marks.fresh(0);
        marks.wrote(0, 7);
        marks.was_copied(0, 3);
        // 同じスロットを次のフレームに使う。
        marks.fresh(0);
        assert_eq!(marks.written(0), 0);
        assert_eq!(marks.copied(0), 0);
        // 一度書いたテクスチャは黒で埋め直さない。
        assert!(!marks.blank(0));
        assert!(marks.blank(1));
    }
}
