// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `d3d12` の転送方式：共有ヒープのバッファ（入力のスロットと出力のスロット）と共有フェンス二つ。NT ハンドルを複製する。

use wgpu::hal::api::Dx12;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use zwsdk::video::process::{self as wire, Format};

use super::super::Gpu;
use super::super::d3d12::{self, Fence};
use super::{Handed, Link};

pub(super) fn id(device: &wgpu::Device) -> Option<Vec<u8>> {
    let raw = d3d12::device(device).ok()?;
    let luid = d3d12::luid(&raw);
    let mut id = luid.LowPart.to_le_bytes().to_vec();
    id.extend_from_slice(&luid.HighPart.to_le_bytes());
    Some(id)
}

pub(super) struct D3d12 {
    inputs: Vec<wgpu::Buffer>,
    middles: Vec<wgpu::Buffer>,
    handles: Vec<(u32, u32, Handle, u64)>,
    stride: u32,
    /// レンダリング側が上げて相手が待つ（書き込み終えた）、相手が上げてレンダリング側が確認する（書き終えた）。
    ready: Fence,
    done: Fence,
    raw: ID3D12Device,
}

// SAFETY: D3D12 のオブジェクトは参照カウント付きの COM、イベントはただのハンドルで、使うのは `&mut` の所有者だけ。
unsafe impl Send for D3d12 {}

impl D3d12 {
    pub fn new(
        device: &wgpu::Device,
        format: Format,
        size: [u32; 2],
        inputs: usize,
        middles: usize,
    ) -> Result<Self, String> {
        let raw = d3d12::device(device)?;
        let stride = wire::stride(size[0], format);
        let bytes = wire::picture_bytes(size[1], stride);
        let mut handles = Vec::new();
        let mut make =
            |what: u32, count: usize, label: &str| -> Result<Vec<wgpu::Buffer>, String> {
                (0..count as u32)
                    .map(|index| {
                        let (buffer, handle, allocation) = export(device, &raw, bytes, label)?;
                        handles.push((what, index, handle, allocation));
                        Ok(buffer)
                    })
                    .collect()
            };
        let inputs = make(wire::INPUT, inputs, "zw process input")?;
        let middles = make(wire::MIDDLE, middles, "zw process mid")?;
        Ok(Self {
            inputs,
            middles,
            handles,
            stride,
            ready: Fence::new(&raw, true)?,
            done: Fence::new(&raw, true)?,
            raw,
        })
    }
}

impl super::Way for D3d12 {
    fn stride(&self) -> u32 {
        self.stride
    }

    fn input(&self, slot: usize) -> &wgpu::Buffer {
        &self.inputs[slot]
    }

    fn output(&self, slot: usize) -> &wgpu::Buffer {
        &self.middles[slot]
    }

    fn share(&mut self, link: &mut Link) -> Result<(), String> {
        for (what, index, handle, allocation) in std::mem::take(&mut self.handles) {
            link.share_handle(what, index, self.stride, allocation, handle.0)?;
        }
        for (what, fence) in [(wire::READY, &self.ready), (wire::DONE, &self.done)] {
            // SAFETY: 共有フラグで作ったフェンス。
            let handle = unsafe {
                self.raw
                    .CreateSharedHandle(&fence.fence, None, GENERIC_ALL.0, None)
            }
            .map(Handle)
            .map_err(|e| format!("CreateSharedHandle（フェンス）：{e}"))?;
            link.share_handle(what, 0, 0, 0, handle.0)?;
        }
        Ok(())
    }

    fn hand_over(&mut self, gpu: &mut Gpu<'_>, _packed: &[usize]) -> Result<Handed, String> {
        gpu.flush();
        let queue = d3d12::queue(gpu.queue)?;
        let value = self.ready.next();
        // 書き込み終えたら上げ、相手はそれを待つ。
        d3d12::signal(&queue, &self.ready.fence, value)?;
        Ok(Handed { value, after: None })
    }

    fn done(&self, value: u64) -> Option<bool> {
        Some(self.done.reached(value))
    }

    fn take_back(&mut self, gpu: &mut Gpu<'_>, value: u64, _slot: usize) -> Result<(), String> {
        let queue = d3d12::queue(gpu.queue)?;
        d3d12::wait(&queue, &self.done.fence, value)
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: まだ閉じていないハンドルを一度だけ。
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn export(
    device: &wgpu::Device,
    raw: &ID3D12Device,
    size: u64,
    label: &str,
) -> Result<(wgpu::Buffer, Handle, u64), String> {
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: 生きているローカルの記述。
    let (resource, handle, allocation) = unsafe {
        raw.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_SHARED,
            &description,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut made,
        )
        .map_err(|e| format!("CreateCommittedResource：{e}"))?;
        let resource = made.ok_or("バッファが作れない")?;
        let handle = raw
            .CreateSharedHandle(&resource, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("CreateSharedHandle：{e}"))?;
        let allocation = raw.GetResourceAllocationInfo(0, &[description]).SizeInBytes;
        (resource, Handle(handle), allocation)
    };
    // SAFETY: このデバイスで今作った、記述どおりのバッファ。
    let buffer = unsafe {
        let hal = wgpu::hal::dx12::Device::buffer_from_raw(resource, size);
        device.create_buffer_from_hal::<Dx12>(
            hal,
            &wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    };
    Ok((buffer, handle, allocation))
}
