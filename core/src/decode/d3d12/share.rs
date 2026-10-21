// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! D3D11 で動くデコーダー（QSV・AMF）のフレームを、レンダラーの D3D12 のデバイスへ渡す。
//! 同じ GPU の上で共有テクスチャへ写し、共有フェンスで待ち合わせる（メインメモリを通さない）。
//! 共有テクスチャは、受け取る側が全部持っていれば足す。

use std::sync::Arc;

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010};
use windows::core::Interface;

use super::Frame;
use super::d3d11::{shared_fence, shared_texture};
use super::dx::{Owned, failed};
use crate::decode::Error;

pub(crate) struct Sharer {
    raw12: ID3D12Device,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    signaller: ID3D11DeviceContext4,
    fence11: ID3D11Fence,
    fence12: ID3D12Fence,
    event: Owned,
    value: u64,
    ring: Vec<Slot>,
    ring_size: (u32, u32, DXGI_FORMAT),
}

struct Slot {
    texture: ID3D11Texture2D,
    opened: ID3D12Resource,
    /// レンダラーが持っている間は 2 以上。
    lease: Arc<()>,
}

impl std::fmt::Debug for Sharer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Sharer({} 枚)", self.ring.len())
    }
}

// SAFETY: デコードのスレッドだけが触り、D3D11 のデバイスはマルチスレッド保護を有効にしてある。
unsafe impl Send for Sharer {}

impl Sharer {
    /// `device` は `raw12` と同じ GPU の D3D11 のデバイス（[`super::d3d11::device_on`]）。
    pub fn open(
        raw12: &ID3D12Device,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self, Error> {
        let signaller: ID3D11DeviceContext4 = context
            .cast()
            .map_err(|error| failed("ID3D11DeviceContext4", error))?;
        let (fence12, fence11) = shared_fence(raw12, device)?;
        Ok(Self {
            raw12: raw12.clone(),
            device: device.clone(),
            context: context.clone(),
            signaller,
            fence11,
            fence12,
            event: Owned::event()?,
            value: 0,
            ring: Vec::new(),
            ring_size: (0, 0, DXGI_FORMAT_NV12),
        })
    }

    pub fn raw12(&self) -> &ID3D12Device {
        &self.raw12
    }

    #[cfg(feature = "amf")]
    pub fn finish(&self) -> Result<(), Error> {
        self.event.wait_fence(&self.fence12, self.value)
    }

    /// これまでに積んだ D3D11 の処理の後でシグナルし、その値を返す（渡したテクスチャの書き終わり）。
    pub fn signal(&mut self) -> Result<(ID3D12Fence, u64), Error> {
        self.value += 1;
        // SAFETY: 生きたフェンス。D3D11 の処理の後に積む。
        unsafe {
            self.signaller
                .Signal(&self.fence11, self.value)
                .map_err(|error| failed("Signal", error))?;
            self.context.Flush();
        }
        Ok((self.fence12.clone(), self.value))
    }

    /// `texture` の `index` 番目のサブリソース（`width`×`height` の NV12・P010）を共有テクスチャへ写して渡す。
    /// 写し終えるまで待つ（写し元はこの後デコーダーが書き換え、D3D11 はビデオの回路とコピーの間の順を守らない）。
    pub fn hand(
        &mut self,
        texture: &ID3D11Texture2D,
        index: u32,
        width: u32,
        height: u32,
        wide: bool,
    ) -> Result<Frame, Error> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 生きているテクスチャ。
        unsafe { texture.GetDesc(&mut description) };
        let at = self.take(description.Width, description.Height, wide)?;
        // SAFETY: どちらもこのデバイスのテクスチャで、形式と大きさが同じ。
        unsafe {
            self.context.CopySubresourceRegion(
                &self.ring[at].texture,
                0,
                0,
                0,
                0,
                texture,
                index,
                None,
            );
        }
        let frame = self.give(at, width, height, wide)?;
        self.event.wait_fence(&self.fence12, self.value)?;
        Ok(frame)
    }

    /// 空いている共有テクスチャ（`width`×`height`、ビデオプロセッサーの出力にもできる）の番号。
    /// 書いたら [`Sharer::give`] で渡す。
    pub fn take(&mut self, width: u32, height: u32, wide: bool) -> Result<usize, Error> {
        let format = if wide {
            DXGI_FORMAT_P010
        } else {
            DXGI_FORMAT_NV12
        };
        let shape = (width, height, format);
        if self.ring_size != shape {
            self.event.wait_fence(&self.fence12, self.value)?;
            self.ring.clear();
            self.ring_size = shape;
        }
        if let Some(at) = self
            .ring
            .iter()
            .position(|slot| Arc::strong_count(&slot.lease) == 1)
        {
            return Ok(at);
        }
        let slot = self.slot(width, height, format)?;
        self.ring.push(slot);
        Ok(self.ring.len() - 1)
    }

    #[cfg(feature = "amf")]
    pub fn texture(&self, at: usize) -> &ID3D11Texture2D {
        &self.ring[at].texture
    }

    /// [`Sharer::take`] の共有テクスチャを、これまでに積んだ書き込みの後で渡す。
    pub fn give(&mut self, at: usize, width: u32, height: u32, wide: bool) -> Result<Frame, Error> {
        let ready = self.signal()?;
        let slot = &self.ring[at];
        Ok(Frame {
            device: self.raw12.clone(),
            texture: slot.opened.clone(),
            width,
            height,
            ten_bit: wide,
            lease: Some(slot.lease.clone()),
            ready: Some(ready),
        })
    }

    fn slot(&self, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Slot, Error> {
        // ビデオプロセッサーの出力にもする（[`Sharer::take`]）。
        let bind = D3D11_BIND_FLAG(D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0);
        let (texture, opened) =
            shared_texture(&self.raw12, &self.device, width, height, format, bind)?;
        Ok(Slot {
            texture,
            opened,
            lease: Arc::new(()),
        })
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        let _ = self.event.wait_fence(&self.fence12, self.value);
    }
}
