// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Which surface a picture is decoded into, and when one may be used again.
//! 管理テーブルは [`crate::decode::slots`]。ここはスロット番号から `VASurfaceID` を求めるテーブルだけ足す。

use crate::decode::slots::{Numbered, Slots};
use crate::decode::vaapi::va::{self, VASurfaceID};

/// How many surfaces past the ones the decoder needs are kept back after being handed out.
/// DMA-BUF はコピーではないので、レンダリング前のサーフェスへデコードすると次のフレームが表示される。
pub const SLACK: usize = crate::decode::MOST_HELD;

#[derive(Debug)]
pub struct Pool {
    ids: Vec<VASurfaceID>,
    slots: Numbered,
}

impl Pool {
    pub fn new(ids: Vec<VASurfaceID>) -> Self {
        let slots = Slots::numbered(ids.len(), SLACK);
        Self { ids, slots }
    }

    pub fn id(&self, slot: usize) -> VASurfaceID {
        self.ids
            .get(slot)
            .copied()
            .unwrap_or(va::VA_INVALID_SURFACE)
    }

    pub fn take(&mut self) -> Option<usize> {
        self.slots.take()
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

    pub fn forget_recent(&mut self) {
        self.slots.forget_recent();
    }

    pub fn ids_mut(&mut self) -> &mut [VASurfaceID] {
        &mut self.ids
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
}
