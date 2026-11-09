// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ビデオプロセスとフィールドの合成が書き出す先のテクスチャ。受け取った側が持っている間は使い回さず、
//! 書き終えるまでは書く処理が読んだ入力を持つ（デコーダーが次のピクチャーをそこへ書かないように）。

use std::sync::Arc;

use windows::Win32::Graphics::Direct3D12::{ID3D12CommandAllocator, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use super::Frame;
use super::dx::{self, Owned};
use crate::decode::Error;

pub(super) struct Out {
    pub texture: ID3D12Resource,
    pub size: [u32; 2],
    pub format: DXGI_FORMAT,
    pub allocator: ID3D12CommandAllocator,
    /// 受け取った側が持っている間は 2 以上。
    pub lease: Arc<()>,
    /// この出力を書き終えるフェンスの値。
    pub written: u64,
    /// この出力を書く処理が読む入力。書き終えるまでデコーダーに返さない。
    pub holding: Vec<Frame>,
}

pub(super) struct Outputs {
    outs: Vec<Out>,
    /// 作る出力の上限。
    most: usize,
}

impl Outputs {
    pub fn new(most: usize) -> Self {
        Self {
            outs: Vec::new(),
            most,
        }
    }

    pub fn len(&self) -> usize {
        self.outs.len()
    }

    /// 大きさと形式の合う空いた出力の番号。前の書き込みが終わるのを待ってから渡す。無ければ `make` で作る。
    /// 合わない出力は空いたものから捨てる。
    pub fn free(
        &mut self,
        event: &Owned,
        fence: &ID3D12Fence,
        size: [u32; 2],
        format: DXGI_FORMAT,
        make: impl FnOnce() -> Result<(ID3D12Resource, ID3D12CommandAllocator), Error>,
    ) -> Result<usize, Error> {
        let fits = |out: &Out| out.size == size && out.format == format;
        self.outs
            .retain(|out| fits(out) || Arc::strong_count(&out.lease) > 1);
        if let Some(at) = self
            .outs
            .iter()
            .position(|out| fits(out) && Arc::strong_count(&out.lease) == 1)
        {
            event.wait_fence(fence, self.outs[at].written)?;
            self.outs[at].holding.clear();
            return Ok(at);
        }
        if self.outs.len() >= self.most {
            return Err(Error::Unavailable(String::from(
                "D3D12: 書き出す先のテクスチャが足りない",
            )));
        }
        let (texture, allocator) = make()?;
        self.outs.push(Out {
            texture,
            size,
            format,
            allocator,
            lease: Arc::new(()),
            written: 0,
            holding: Vec::new(),
        });
        Ok(self.outs.len() - 1)
    }

    /// 書き終えた出力が読んだ入力をデコーダーに返す（出力を使い回すまで持つと、デコーダーのサーフェスが尽きる）。
    pub fn release_read(&mut self, fence: &ID3D12Fence) {
        let done = dx::completed(fence);
        for out in &mut self.outs {
            if out.written <= done {
                out.holding.clear();
            }
        }
    }

    /// 書き終えるのを待って、読んでいた入力を全部デコーダーに返す。
    pub fn release_all(&mut self, event: &Owned, fence: &ID3D12Fence, value: u64) {
        if event.wait_fence(fence, value).is_ok() {
            for out in &mut self.outs {
                out.holding.clear();
            }
        }
    }
}

impl std::ops::Index<usize> for Outputs {
    type Output = Out;

    fn index(&self, at: usize) -> &Out {
        &self.outs[at]
    }
}

impl std::ops::IndexMut<usize> for Outputs {
    fn index_mut(&mut self, at: usize) -> &mut Out {
        &mut self.outs[at]
    }
}
