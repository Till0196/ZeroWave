// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Windows のデコーダーのフレームからコピーなしでレンダリングする（デコーダーは wgpu のデバイスで開く、[`adopt`]）。
//! D3D12 の `textureDimensions` は 1088 を返すので、フレームの大きさは `Params` で渡す。

use wgpu::hal::api::Dx12;
use windows::core::Interface;

use crate::decode::d3d12::Frame;

use crate::output::video::Error;

/// このプロセスのデコーダーに、wgpu が開いたデバイスを使わせる。以後、デコード用に別のデバイスは開かない。
/// Vulkan でレンダリングするときは、同じ GPU の D3D12 のデバイスを使わせ、フレームは共有したバッファで受け取る。
pub fn adopt(device: &wgpu::Device) {
    match raw_device(device).or_else(|| super::d3d12_vulkan::beside(device)) {
        Some(raw) => crate::decode::d3d12::dx::adopt(&raw),
        // D3D12 でも、取り込める Vulkan でもない。対は一緒に動く（[`crate::decode::d3d12::dx::refuse`]）。
        None => crate::decode::d3d12::dx::refuse(),
    }
}

fn raw_device(device: &wgpu::Device) -> Option<windows::Win32::Graphics::Direct3D12::ID3D12Device> {
    // SAFETY: as_hal の間だけ借り、参照カウントを増やしたコピーだけを持ち出す。
    unsafe { device.as_hal::<Dx12>().map(|hal| hal.raw_device().clone()) }
}

/// 一フレームのプレーンのビューと、それを使い終わるまで持っておく物。
pub type Taken = (
    Vec<wgpu::TextureView>,
    Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
);

/// レンダラーのバックエンドごとの受け取り方。
#[derive(Debug)]
pub enum Import {
    /// 同じ `ID3D12Device` のテクスチャをそのまま包む。
    Dx12(Direct),
    /// 共有したバッファを通して Vulkan のテクスチャへ GPU の中でコピーする。
    Vulkan(Box<super::d3d12_vulkan::Import>),
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if let Some(direct) = Direct::new(device) {
            return Some(Self::Dx12(direct));
        }
        super::d3d12_vulkan::Import::new(device).map(|import| Self::Vulkan(Box::new(import)))
    }

    /// フレームをこのデバイスで読めるようにする。書き終わるのを待つのもここ（DX12 は GPU の上、Vulkan は CPU）。
    pub fn take(&mut self, frame: &Frame, queue: &wgpu::Queue) -> Result<Taken, Error> {
        match self {
            Self::Dx12(direct) => direct.take(frame, queue),
            Self::Vulkan(import) => import.take(frame, queue),
        }
    }

    pub fn forget(&mut self) {
        match self {
            Self::Dx12(direct) => direct.forget(),
            Self::Vulkan(import) => import.forget(),
        }
    }
}

pub struct Direct {
    device: wgpu::Device,
    raw: windows::Win32::Graphics::Direct3D12::ID3D12Device,
    made: super::Kept<usize, Vec<wgpu::TextureView>>,
}

impl std::fmt::Debug for Direct {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Direct({} プレーン)", self.made.len())
    }
}

impl Direct {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let raw = raw_device(device)?;
        Some(Self {
            device: device.clone(),
            raw,
            made: super::Kept::new(),
        })
    }

    fn take(&mut self, frame: &Frame, queue: &wgpu::Queue) -> Result<Taken, Error> {
        let views = self.views(frame)?;
        // 書き終わるのを、このキューの GPU の上で待つ（後の送信が読むのはそれから）。
        if let Some((fence, value)) = frame.ready.as_ref() {
            // SAFETY: キューを借りて、生きているフェンスで待機を投入するだけ。
            let waited = unsafe {
                queue
                    .as_hal::<Dx12>()
                    .map(|hal| hal.as_raw().Wait(fence, *value))
            };
            if !matches!(waited, Some(Ok(()))) {
                return Err(Error::Unsupported(
                    "a frame whose fence cannot be waited on",
                ));
            }
        }
        // スロットがこのフレームを持つ間、デコーダーにそのテクスチャを使わせない（`decode::d3d12::Frame::lease`）。
        Ok((views, frame.lease.clone()))
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
        planes(frame)
            .into_iter()
            .zip(0u32..)
            .map(|((format, size), plane)| {
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

/// プレーンごとの形式と大きさ（NV12・P010 の輝度と、縦横半分の色差）。
pub(super) fn planes(frame: &Frame) -> [(wgpu::TextureFormat, wgpu::Extent3d); 2] {
    let formats = if frame.ten_bit {
        [
            wgpu::TextureFormat::R16Unorm,
            wgpu::TextureFormat::Rg16Unorm,
        ]
    } else {
        [wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm]
    };
    let size = |divisor: u32| wgpu::Extent3d {
        width: frame.width.div_ceil(divisor),
        height: frame.height.div_ceil(divisor),
        depth_or_array_layers: 1,
    };
    [(formats[0], size(1)), (formats[1], size(2))]
}
