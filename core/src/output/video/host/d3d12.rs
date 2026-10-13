// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! D3D12 のバックエンドが使うヘルパー：リソース、wgpu へのラップ、フェンス、生のキュー。

use wgpu::hal::api::Dx12;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device1, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

pub(super) fn device(device: &wgpu::Device) -> Result<ID3D12Device, String> {
    // SAFETY: デバイスを借りて、参照カウントを一つ足して持つ。
    unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) }
        .ok_or_else(|| String::from("D3D12 のデバイスではない"))
}

pub(super) fn queue(queue: &wgpu::Queue) -> Result<ID3D12CommandQueue, String> {
    // SAFETY: キューを借りて、参照カウントを一つ足して持つ。
    unsafe { queue.as_hal::<Dx12>().map(|hal| hal.as_raw().clone()) }
        .ok_or_else(|| String::from("D3D12 のキューではない"))
}

/// 同時アクセスのフラグ付き：キューや API を跨ぐと COMMON に戻る。
pub(super) fn resource(
    raw: &ID3D12Device,
    size: [u32; 2],
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    heap_flags: D3D12_HEAP_FLAGS,
) -> Result<ID3D12Resource, String> {
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(size[0]),
        Height: size[1],
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: flags | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: 生きているローカルの記述。
    unsafe {
        raw.CreateCommittedResource(
            &heap,
            heap_flags,
            &description,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut made,
        )
        .map_err(|e| format!("CreateCommittedResource：{e}"))?;
    }
    made.ok_or_else(|| String::from("リソースが作れない"))
}

pub(super) fn wrap(
    device: &wgpu::Device,
    resource: ID3D12Resource,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    // SAFETY: このデバイスで今作った、記述どおりのリソース。
    unsafe {
        let hal = wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            format,
            wgpu::TextureDimension::D2,
            extent,
            1,
            1,
        );
        device.create_texture_from_hal::<Dx12>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            },
            wgpu::TextureUses::UNINITIALIZED,
        )
    }
}

/// hal のテクスチャにプレーンの番号を付ける（`R8Unorm` は Y、`Rg8Unorm` は UV）。
#[cfg_attr(not(feature = "qsv"), allow(dead_code))]
pub(super) fn wrap_plane(
    device: &wgpu::Device,
    resource: ID3D12Resource,
    plane: u32,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    // SAFETY: このデバイスで作った 2 プレーンのリソースで、プレーンの番号も大きさもそのリソースの範囲内。
    unsafe {
        let hal = wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            format,
            wgpu::TextureDimension::D2,
            extent,
            1,
            1,
        )
        .with_plane_slice(plane);
        device.create_texture_from_hal::<Dx12>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            },
            wgpu::TextureUses::UNINITIALIZED,
        )
    }
}

pub(super) fn luid(raw: &ID3D12Device) -> windows::Win32::Foundation::LUID {
    // SAFETY: 読むだけ。
    unsafe { raw.GetAdapterLuid() }
}

/// キューに送ったコマンドが完了したら `fence` を `value` に上げる。
pub(super) fn signal(
    queue: &ID3D12CommandQueue,
    fence: &ID3D12Fence,
    value: u64,
) -> Result<(), String> {
    // SAFETY: 生きているキューとフェンスでシグナルか待機を投入するだけ。
    unsafe { queue.Signal(fence, value) }.map_err(|e| format!("Signal：{e}"))
}

/// `fence` が `value` に達するまで、キューの後続のコマンドを GPU 上で待たせる。
pub(super) fn wait(
    queue: &ID3D12CommandQueue,
    fence: &ID3D12Fence,
    value: u64,
) -> Result<(), String> {
    // SAFETY: 生きているキューとフェンスでシグナルか待機を投入するだけ。
    unsafe { queue.Wait(fence, value) }.map_err(|e| format!("Wait：{e}"))
}

/// D3D11 の側で、投入したコマンドが完了したら `fence` を上げ、すぐフラッシュする。
pub(super) fn d3d11_signal(
    context: &ID3D11DeviceContext4,
    fence: &ID3D11Fence,
    value: u64,
) -> Result<(), String> {
    // SAFETY: 生きているデバイスコンテキストとフェンスでシグナルか待機を投入するだけ。
    unsafe {
        context
            .Signal(fence, value)
            .map_err(|e| format!("D3D11 の Signal：{e}"))?;
        context.Flush();
    }
    Ok(())
}

/// `fence` が `value` に達するまで、D3D11 の後続のコマンドを GPU 上で待たせる。
pub(super) fn d3d11_wait(
    context: &ID3D11DeviceContext4,
    fence: &ID3D11Fence,
    value: u64,
) -> Result<(), String> {
    // SAFETY: 生きているデバイスコンテキストとフェンスでシグナルか待機を投入するだけ。
    unsafe { context.Wait(fence, value) }.map_err(|e| format!("D3D11 の Wait：{e}"))
}

/// 共有フラグで作った D3D12 のフェンスを D3D11 の側で開く。
pub(super) fn d3d11_fence(
    raw: &ID3D12Device,
    fence: &ID3D12Fence,
    device: &ID3D11Device5,
) -> Result<ID3D11Fence, String> {
    // SAFETY: 生きているデバイスとフェンス。ハンドルは開いたら閉じる。
    unsafe {
        let handle = raw
            .CreateSharedHandle(fence, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("CreateSharedHandle（フェンス）：{e}"))?;
        let mut opened: Option<ID3D11Fence> = None;
        let result = device.OpenSharedFence(handle, &mut opened);
        let _ = CloseHandle(handle);
        result.map_err(|e| format!("OpenSharedFence：{e}"))?;
        opened.ok_or_else(|| String::from("OpenSharedFence が空"))
    }
}

/// 共有ヒープの D3D12 のリソースを D3D11 のテクスチャとして開く。
pub(super) fn d3d11_texture(
    raw: &ID3D12Device,
    resource: &ID3D12Resource,
    device: &ID3D11Device1,
) -> Result<ID3D11Texture2D, String> {
    // SAFETY: 生きているデバイスとリソース。ハンドルは開いたら閉じる。
    unsafe {
        let handle = raw
            .CreateSharedHandle(resource, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("CreateSharedHandle：{e}"))?;
        let opened = device.OpenSharedResource1::<ID3D11Texture2D>(handle);
        let _ = CloseHandle(handle);
        opened.map_err(|e| format!("OpenSharedResource1：{e}"))
    }
}

pub(super) fn completed(fence: &ID3D12Fence) -> u64 {
    // SAFETY: 読むだけ。
    unsafe { fence.GetCompletedValue() }
}

pub(super) struct Fence {
    pub fence: ID3D12Fence,
    event: HANDLE,
    pub value: u64,
}

impl Fence {
    pub fn new(raw: &ID3D12Device, shared: bool) -> Result<Self, String> {
        let flags = if shared {
            D3D12_FENCE_FLAG_SHARED
        } else {
            D3D12_FENCE_FLAG_NONE
        };
        Self::new_with(raw, flags)
    }

    /// GPU 間で共有するなら `SHARED | SHARED_CROSS_ADAPTER`。
    pub fn new_with(raw: &ID3D12Device, flags: D3D12_FENCE_FLAGS) -> Result<Self, String> {
        // SAFETY: 生きているデバイス。名前無し・自動リセットのイベント。
        unsafe {
            let fence: ID3D12Fence = raw
                .CreateFence(0, flags)
                .map_err(|e| format!("CreateFence：{e}"))?;
            let event =
                CreateEventW(None, false, false, None).map_err(|e| format!("CreateEvent：{e}"))?;
            Ok(Self {
                fence,
                event,
                value: 0,
            })
        }
    }

    pub fn next(&mut self) -> u64 {
        self.value += 1;
        self.value
    }

    pub fn reached(&self, value: u64) -> bool {
        completed(&self.fence) >= value
    }

    /// 作るときだけ。実行中は GPU 上で待つ。
    pub fn wait(&self, value: u64) -> Result<(), String> {
        // SAFETY: フェンスとイベントは self のもの。
        unsafe {
            if self.fence.GetCompletedValue() < value {
                self.fence
                    .SetEventOnCompletion(value, self.event)
                    .map_err(|e| e.to_string())?;
                if WaitForSingleObject(self.event, 2000) != WAIT_OBJECT_0 {
                    return Err(String::from("フェンスが進まない"));
                }
            }
        }
        Ok(())
    }
}

impl Drop for Fence {
    fn drop(&mut self) {
        // SAFETY: 作ったイベントを一度だけ。
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}
