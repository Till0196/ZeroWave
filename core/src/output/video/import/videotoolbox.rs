// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! VideoToolbox のフレームからコピーなしでレンダリングする（`CVMetalTextureCache` で `IOSurface` の `MTLTexture`）。

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLTexture, MTLTextureType};
use wgpu::hal::api::Metal;

use crate::decode::videotoolbox::Frame;
use crate::decode::videotoolbox::vt::{self, Owned};

use crate::output::video::Error;

/// 1 フレームのために作ったもの。`MTLTexture` を使う間は `CVMetalTexture` とバッファを持つ。
struct Made {
    views: Vec<wgpu::TextureView>,
    #[allow(dead_code)]
    textures: Vec<Owned<c_void>>,
    #[allow(dead_code)]
    buffer: Owned<c_void>,
}

// SAFETY: 作ってからは書き換えず持つだけで、CoreFoundation の参照数はスレッド安全。
unsafe impl Sync for Made {}

/// テーブルに残すフレームの数。上限が無いとデコーダーのプールがバッファを使い回せない。
const KEEP: usize = crate::decode::MOST_HELD;

pub struct Import {
    device: wgpu::Device,
    cache: Owned<c_void>,
    made: HashMap<usize, Arc<Made>>,
    /// 入れた順。古いほうから追い出す（プールは同じアドレスを順に回す）。
    order: VecDeque<usize>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Import({} プレーン)", self.made.len())
    }
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        // SAFETY: as_hal の間だけ借り、参照数付きの複製を持ち出す。
        let raw = unsafe {
            device
                .as_hal::<Metal>()
                .map(|hal| Retained::as_ptr(hal.raw_device()).cast_mut())
        }?;
        let mut cache: vt::CVMetalTextureCacheRef = std::ptr::null_mut();
        // SAFETY: `raw` は生きた `MTLDevice`。属性は NULL、出力は有効なローカル変数。
        let made = unsafe {
            vt::CVMetalTextureCacheCreate(
                vt::kCFAllocatorDefault,
                std::ptr::null(),
                raw.cast(),
                std::ptr::null(),
                &raw mut cache,
            )
        };
        if made != 0 {
            tracing::warn!("CVMetalTextureCacheCreate に失敗: CVReturn {made}");
            return None;
        }
        // SAFETY: Create が返したので所有物。
        let cache = unsafe { Owned::new(cache) }?;
        Some(Self {
            device: device.clone(),
            cache,
            made: HashMap::new(),
            order: VecDeque::new(),
        })
    }

    /// 1 フレームの二つの texture（輝度、色差の対）。返すハンドルは view を読む間保持すること。
    pub fn views(
        &mut self,
        frame: &Frame,
    ) -> Result<(Vec<wgpu::TextureView>, Arc<dyn std::any::Any + Send + Sync>), Error> {
        let key = frame.pixel_buffer() as usize;
        if let Some(found) = self.made.get(&key) {
            return Ok((found.views.clone(), found.clone()));
        }
        let made = Arc::new(self.wrap(frame)?);
        let views = made.views.clone();
        let keep: Arc<dyn std::any::Any + Send + Sync> = made.clone();
        // 古いほうから返す。返さないとデコーダーのプールが使い回せない（[`KEEP`]）。
        let mut evicted = false;
        while self.order.len() >= KEEP {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.made.remove(&oldest);
            evicted = true;
        }
        if evicted {
            // 手放したらキャッシュも Flush する。
            // SAFETY: 生きたキャッシュ。フラグは 0（既定）。
            unsafe { vt::CVMetalTextureCacheFlush(self.cache.as_ptr(), 0) };
        }
        if self.made.insert(key, made).is_none() {
            self.order.push_back(key);
        }
        Ok((views, keep))
    }

    fn wrap(&self, frame: &Frame) -> Result<Made, Error> {
        let planes: [(usize, vt::MTLPixelFormat, wgpu::TextureFormat, u32); 2] = if frame.ten_bit {
            [
                (
                    0,
                    vt::MTLPixelFormatR16Unorm,
                    wgpu::TextureFormat::R16Unorm,
                    1,
                ),
                (
                    1,
                    vt::MTLPixelFormatRG16Unorm,
                    wgpu::TextureFormat::Rg16Unorm,
                    2,
                ),
            ]
        } else {
            [
                (
                    0,
                    vt::MTLPixelFormatR8Unorm,
                    wgpu::TextureFormat::R8Unorm,
                    1,
                ),
                (
                    1,
                    vt::MTLPixelFormatRG8Unorm,
                    wgpu::TextureFormat::Rg8Unorm,
                    2,
                ),
            ]
        };
        let buffer = frame.pixel_buffer();
        let mut views = Vec::with_capacity(2);
        let mut textures = Vec::with_capacity(2);
        for (plane, metal, format, divisor) in planes {
            let width = frame.width.div_ceil(divisor);
            let height = frame.height.div_ceil(divisor);
            let mut made: vt::CVMetalTextureRef = std::ptr::null_mut();
            // SAFETY: キャッシュとバッファは生きていて、プレーンの番号と大きさは二プレーンのフレームの物。
            let status = unsafe {
                vt::CVMetalTextureCacheCreateTextureFromImage(
                    vt::kCFAllocatorDefault,
                    self.cache.as_ptr(),
                    buffer,
                    std::ptr::null(),
                    metal,
                    width as usize,
                    height as usize,
                    plane,
                    &raw mut made,
                )
            };
            if status != 0 {
                return Err(Error::Unsupported(
                    "CVMetalTextureCacheCreateTextureFromImage が失敗しました",
                ));
            }
            // SAFETY: Create が返したので所有物。Get は借用なので、`retain` で参照数を上げて所有に変える。
            let (made, texture) = unsafe {
                let made = Owned::new(made).ok_or(Error::Unsupported("an empty CVMetalTexture"))?;
                let raw = vt::CVMetalTextureGetTexture(made.as_ptr());
                let texture = Retained::retain(raw.cast::<ProtocolObject<dyn MTLTexture>>())
                    .ok_or(Error::Unsupported("an empty MTLTexture"))?;
                (made, texture)
            };
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            // SAFETY: いま作った二次元のテクスチャで、形と大きさは作ったときと同じ。
            let hal = unsafe {
                wgpu::hal::metal::Device::texture_from_raw(
                    texture,
                    format,
                    MTLTextureType::Type2D,
                    1,
                    1,
                    wgpu::hal::CopyExtent {
                        width,
                        height,
                        depth: 1,
                    },
                    None,
                )
            };
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
            // SAFETY: いまこの device のために作った物。VideoToolbox が書いた物に `UNINITIALIZED` は正しい。
            let texture = unsafe {
                self.device.create_texture_from_hal::<Metal>(
                    hal,
                    &descriptor,
                    wgpu::TextureUses::UNINITIALIZED,
                )
            };
            views.push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            textures.push(made);
        }
        // SAFETY: テーブルが持つ間、同じアドレスが別のフレームに使い回されないよう保持する。解放は `Owned` の `Drop`。
        let held = unsafe { vt::CFRetain(buffer.cast_const()) }.cast_mut();
        // SAFETY: いま参照数を上げた所有物。
        let Some(held) = (unsafe { Owned::new(held) }) else {
            return Err(Error::Unsupported("an empty pixel buffer"));
        };
        Ok(Made {
            views,
            textures,
            buffer: held,
        })
    }

    pub fn forget(&mut self) {
        self.made.clear();
        self.order.clear();
        // SAFETY: 生きたキャッシュ。テーブルを捨てた後。
        unsafe { vt::CVMetalTextureCacheFlush(self.cache.as_ptr(), 0) };
    }
}

impl Drop for Import {
    fn drop(&mut self) {
        // テーブルが先、キャッシュが後（`Made` の `CVMetalTexture` はキャッシュの物を指す）。
        self.made.clear();
    }
}
