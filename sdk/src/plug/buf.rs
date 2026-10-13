// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use crate::plug::abi;

/// 本体の側にあるバイト列。`input` で渡されたものはその呼び出しの間だけ生きる。
#[derive(Debug)]
pub struct Buf {
    handle: u32,
    owned: bool,
}

impl Buf {
    #[doc(hidden)]
    pub fn borrowed(handle: u32) -> Self {
        Self {
            handle,
            owned: false,
        }
    }

    #[doc(hidden)]
    pub fn owned(handle: u32) -> Self {
        Self {
            handle,
            owned: true,
        }
    }

    pub fn of(bytes: &[u8]) -> Self {
        Self {
            handle: abi::buf_create(bytes),
            owned: true,
        }
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }

    /// 本体へ戻り値として返すとき（本体が解放する）。
    pub fn into_handle(self) -> u32 {
        let handle = self.handle;
        std::mem::forget(self);
        handle
    }

    pub fn len(&self) -> usize {
        abi::buf_len(self.handle)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = vec![0; self.len()];
        let got = abi::buf_read(self.handle, 0, &mut out);
        out.truncate(got);
        out
    }

    pub fn read_at(&self, from: usize, into: &mut [u8]) -> usize {
        abi::buf_read(self.handle, from, into)
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        if self.owned {
            abi::buf_drop(self.handle);
        }
    }
}
