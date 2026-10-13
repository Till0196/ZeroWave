// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Windows のデコーダーのフレームからコピーなしでレンダリングする（デコーダーは wgpu のデバイスで開く、[`adopt`]）。
//! D3D12 の `textureDimensions` は 1088 を返すので、フレームの大きさは `Params` で渡す。

use wgpu::hal::api::Dx12;
use windows::core::Interface;

use crate::decode::d3d12::Frame;

use crate::output::video::Error;

/// このプロセスのデコーダーに、wgpu が開いたデバイスを使わせる。以後、デコード用に別のデバイスは開かない。
pub fn adopt(device: &wgpu::Device) {
    match raw_device(device) {
        Some(raw) => crate::decode::d3d12::dx::adopt(&raw),
        // D3D12 でない。対は一緒に動く（[`crate::decode::d3d12::dx::refuse`]）。
        None => crate::decode::d3d12::dx::refuse(),
    }
}

fn raw_device(device: &wgpu::Device) -> Option<windows::Win32::Graphics::Direct3D12::ID3D12Device> {
    // SAFETY: as_hal の間だけ借り、参照カウントを増やしたコピーだけを持ち出す。
    unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) }
}

pub struct Import {
    device: wgpu::Device,
    raw: windows::Win32::Graphics::Direct3D12::ID3D12Device,
    made: super::Kept<usize, Vec<wgpu::TextureView>>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Import({} プレーン)", self.made.len())
    }
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let raw = raw_device(device)?;
        Some(Self {
            device: device.clone(),
            raw,
            made: super::Kept::new(),
        })
    }

    pub fn views(&mut self, frame: &Frame) -> Result<Vec<wgpu::TextureView>, Error> {
        if frame.device.as_raw() != self.raw.as_raw() {
            // ウィンドウより先に作られ自前のデバイスを開いたデコーダー。別のデバイスのメモリはレンダリングしない。
            return Err(Error::Unsupported("a picture from another device"));
        }
        let key = frame.texture.as_raw() as usize;
        if let Some(found) = self.made.get(&key) {
            return Ok(found);
        }
        let views = self.wrap(frame);
        self.made.insert(key, views.clone());
        Ok(views)
    }

    fn wrap(&self, frame: &Frame) -> Vec<wgpu::TextureView> {
        let planes = if frame.ten_bit {
            [
                (0u32, wgpu::TextureFormat::R16Unorm, 1u32),
                (1, wgpu::TextureFormat::Rg16Unorm, 2),
            ]
        } else {
            [
                (0, wgpu::TextureFormat::R8Unorm, 1),
                (1, wgpu::TextureFormat::Rg8Unorm, 2),
            ]
        };
        planes
            .into_iter()
            .map(|(plane, format, divisor)| {
                let size = wgpu::Extent3d {
                    width: frame.width.div_ceil(divisor),
                    height: frame.height.div_ceil(divisor),
                    depth_or_array_layers: 1,
                };
                // SAFETY: このデバイスのテクスチャで、形式と大きさは宣言どおり。コピーを持つ間は有効。
                let hal = unsafe {
                    wgpu::hal::dx12::Device::texture_from_raw(
                        frame.texture.clone(),
                        format,
                        wgpu::TextureDimension::D2,
                        size,
                        1,
                        1,
                    )
                }
                .with_plane_slice(plane);
                let descriptor = wgpu::TextureDescriptor {
                    label: Some("decoded plane"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                };
                // SAFETY: いま作った未使用のテクスチャ。同時アクセスで作られ自ずと `COMMON` に戻る。
                let texture = unsafe {
                    self.device.create_texture_from_hal::<Dx12>(
                        hal,
                        &descriptor,
                        wgpu::TextureUses::UNINITIALIZED,
                    )
                };
                texture.create_view(&wgpu::TextureViewDescriptor::default())
            })
            .collect()
    }

    pub fn forget(&mut self) {
        self.made.clear();
    }
}
