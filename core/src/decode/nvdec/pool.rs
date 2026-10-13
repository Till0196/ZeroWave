// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! どのサーフェスへデコードするか、いつまた使ってよいか。管理テーブルは [`crate::decode::slots`]。
//! スロット番号がそのまま `CurrPicIdx`。`cuvidMapVideoFrame` でマップしている間は書き換えさせない。

use crate::decode::slots::{Numbered, Slots};

/// 渡した後も取っておくサーフェスの数（デコーダーが要る分のほか）。
pub const SLACK: usize = 8;

#[derive(Debug)]
pub struct Pool {
    slots: Numbered,
}

impl Pool {
    pub fn new(count: usize) -> Self {
        Self {
            slots: Slots::numbered(count, SLACK),
        }
    }

    pub fn id(&self, slot: usize) -> i32 {
        slot as i32
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
