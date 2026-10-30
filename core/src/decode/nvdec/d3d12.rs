// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! NVDEC が書いたフレームを、レンダラーが渡した `ID3D12Device` の `ID3D12Resource` へ。
//! NV12 を CUDA へ配列としてインポートする方法はドキュメントに無いので、共有した線形のバッファへ CUDA がコピーし、
//! D3D12 がプレーンごとにテクスチャへコピーする。CUDA はコピーキューのフェンスを共有して前のコピーを待つ。
//! CPU が待つのは CUDA のコピーだけ（NVDEC のフレームを手放す前）。D3D12 のコピーはレンダラーが GPU の上で待つ。

use std::sync::Arc;

use mediadecoder::es::Format;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_SAMPLE_DESC,
};

use super::cu;
use crate::decode::Error;
use crate::decode::d3d12::dx::{CopyQueue, Owned, failed};
use crate::decode::d3d12::{Frame, dx, pool::SLACK};

pub struct Bridge {
    device: ID3D12Device,
    copy: CopyQueue,
    /// `copy` のフェンスを CUDA から待つための物。
    copied: cu::Fence,
    stream: cu::Stream,
    staging: ID3D12Resource,
    mapped: cu::Shared,
    layouts: [D3D12_PLACED_SUBRESOURCE_FOOTPRINT; 2],
    textures: Vec<Slot>,
    description: D3D12_RESOURCE_DESC,
    /// 次に見るスロット。空いている中でいちばん前に渡したものへ書くため、渡した順に回る。
    next: usize,
    format: Format,
    ten_bit: bool,
    width: u32,
    height: u32,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.copy.wait(self.copy.last());
    }
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Bridge({}x{})", self.width, self.height)
    }
}


impl Bridge {
    pub fn open(context: &Arc<cu::Context>, format: Format) -> Result<Self, Error> {
        let Some(device) = dx::adopted() else {
            return Err(Error::Unavailable(String::from(
                "レンダラーが D3D12 のデバイスをまだ渡していません",
            )));
        };
        let wanted = luid_of(&device)?;
        if context.luid() != Some(wanted) {
            return Err(Error::Unavailable(String::from(
                "CUDA のデバイスと、レンダリングするデバイスが違います",
            )));
        }

        let ten_bit = format.depth > 8;
        let dxgi = if ten_bit {
            DXGI_FORMAT_P010
        } else {
            DXGI_FORMAT_NV12
        };
        // サーフェスは全体の大きさで作る（NV12・P010 は偶数でなければならない）。
        let width = format.width.next_multiple_of(16);
        let height = format.height.next_multiple_of(16);

        let description = texture_description(dxgi, width, height);
        let (layouts, _, total) = dx::footprints(&device, &description, 2);
        if total == 0 {
            return Err(Error::Unavailable(String::from(
                "プレーンのレイアウトを取得できません",
            )));
        }

        let staging = dx::shared_buffer(&device, total)?;
        let mapped = import(context, &device, &staging, total)?;

        let copy = CopyQueue::shared(&device)?;
        let copied = import_fence(context, &device, copy.fence())?;
        let stream = context.stream()?;

        let count = SLACK;
        let mut textures = Vec::with_capacity(count);
        for _ in 0..count {
            textures.push(Slot::new(texture(&device, &description)?));
        }

        Ok(Self {
            device,
            copy,
            copied,
            stream,
            staging,
            mapped,
            layouts,
            textures,
            description,
            next: 0,
            format,
            ten_bit,
            width,
            height,
        })
    }

    pub fn write(
        &mut self,
        from: cu::CUdeviceptr,
        pitch: usize,
        surface_height: usize,
    ) -> Result<Frame, Error> {
        let slot = self.take_slot()?;
        let wide = usize::from(self.ten_bit) + 1;
        let row = self.format.width as usize * wide;

        // 前の D3D12 のコピーが共有したバッファを読み終わるのを、ストリームの上で待つ。
        self.copied.wait(&self.stream, self.copy.last())?;
        for plane in 0..2 {
            // サーフェスの高さ分コピーする。下の行もデインターレースが読むので、コピーしないと下端が崩れる。
            let rows = if plane == 0 {
                surface_height
            } else {
                surface_height.div_ceil(2)
            }
            .min(self.layouts[plane].Footprint.Height as usize);
            let source = from
                + if plane == 0 {
                    0
                } else {
                    (pitch * surface_height) as u64
                };
            let layout = &self.layouts[plane];
            self.stream.copy(&cu::Memcpy2D {
                src_memory_type: cu::CU_MEMORYTYPE_DEVICE,
                src_device: source,
                src_pitch: pitch,
                dst_memory_type: cu::CU_MEMORYTYPE_DEVICE,
                dst_device: self.mapped.pointer + layout.Offset,
                dst_pitch: layout.Footprint.RowPitch as usize,
                width_in_bytes: row,
                height: rows,
                ..cu::Memcpy2D::default()
            })?;
        }
        // 呼び出し側はこの後 NVDEC のフレームを手放すので、読み終わるまで待つ。D3D12 のコピーはその後に積むので
        // CUDA を待たなくてよい。
        self.stream.wait()?;

        let slot = &self.textures[slot];
        self.copy.begin(None)?;
        self.copy
            .planes_from(&self.staging, &slot.texture, &self.layouts);
        let value = self.copy.submit()?;
        Ok(Frame {
            device: self.device.clone(),
            texture: slot.texture.clone(),
            width: self.format.width,
            height: self.format.height,
            ten_bit: self.ten_bit,
            lease: Some(slot.lease.clone()),
            ready: Some((self.copy.fence().clone(), value)),
        })
    }

    /// 次に書いてよいスロット。まだ使われている（[`Frame::lease`] がある）ものへは書かない。
    /// 空きが無ければ待たずに増やす（待つとデッドロックするため）。
    fn take_slot(&mut self) -> Result<usize, Error> {
        let count = self.textures.len();
        for step in 0..count {
            let slot = (self.next + step) % count;
            if self.textures[slot].is_free() {
                self.next = (slot + 1) % count;
                return Ok(slot);
            }
        }
        if count >= SLOTS_MOST {
            return Err(Error::Unavailable(String::from(
                "NVDEC：渡したサーフェスが返ってこないので、このフレームは捨てる",
            )));
        }
        self.textures
            .push(Slot::new(texture(&self.device, &self.description)?));
        self.next = 0;
        Ok(count)
    }
}

const SLOTS_MOST: usize = crate::decode::MOST_HELD + 8;

struct Slot {
    texture: ID3D12Resource,
    lease: Arc<Lease>,
}

struct Lease;

impl Slot {
    fn new(texture: ID3D12Resource) -> Self {
        Self {
            texture,
            lease: Arc::new(Lease),
        }
    }

    fn is_free(&self) -> bool {
        Arc::strong_count(&self.lease) == 1
    }
}

fn texture_description(format: DXGI_FORMAT, width: u32, height: u32) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        // 受け取る側が前提にしている（`COMMON` へ自動で戻るのはこのフラグのおかげ）。
        Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    }
}

fn texture(
    device: &ID3D12Device,
    description: &D3D12_RESOURCE_DESC,
) -> Result<ID3D12Resource, Error> {
    let properties = dx::heap(D3D12_HEAP_TYPE_DEFAULT);
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: both descriptions and `made` are live locals.
    unsafe {
        device
            .CreateCommittedResource(
                &properties,
                D3D12_HEAP_FLAG_NONE,
                description,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut made,
            )
            .map_err(|error| failed("CreateCommittedResource", error))?;
    }
    made.ok_or_else(|| Error::Unavailable(String::from("サーフェスが確保できません")))
}

/// Hands the buffer to CUDA. インポートが参照を持つので、ハンドルはすぐ閉じてよい。
fn import(
    context: &Arc<cu::Context>,
    device: &ID3D12Device,
    buffer: &ID3D12Resource,
    size: u64,
) -> Result<cu::Shared, Error> {
    let handle = Owned::shared(device, buffer)?;
    context.import(handle.raw().0, size)
}

fn import_fence(
    context: &Arc<cu::Context>,
    device: &ID3D12Device,
    fence: &ID3D12Fence,
) -> Result<cu::Fence, Error> {
    let handle = Owned::shared(device, fence)?;
    context.import_fence(handle.raw().0)
}

fn luid_of(device: &ID3D12Device) -> Result<[u8; 8], Error> {
    let luid = dx::luid(device);
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&luid.LowPart.to_ne_bytes());
    bytes[4..].copy_from_slice(&luid.HighPart.to_ne_bytes());
    Ok(bytes)
}

/// レンダラーが渡したデバイスの LUID。CUDA を開くときに要る。
pub fn adopted_luid() -> Option<[u8; 8]> {
    luid_of(&dx::adopted()?).ok()
}
