// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Vulkan でレンダリングするとき、Windows のデコーダー（D3D12・NVDEC・AMF・QSV）のフレームを CPU を通さずに受け取る。
//! デコーダーのテクスチャは共有のフラグ無しで作られるので Vulkan へは直接渡せない。同じ GPU の D3D12 のデバイスで
//! デコードさせ（[`beside`]）、D3D12 のコピーキューで共有したバッファへプレーンを置き、Vulkan がそこから自分の
//! テクスチャへコピーする。コピーは二回とも GPU の中。D3D12 のコピーの終わりだけ CPU で待つ（一フレームに一度）。

use std::ffi::CStr;
use std::sync::Arc;

use ash::vk;
use wgpu::hal::api::Vulkan;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Resource};
use windows::core::Interface;

use crate::decode::d3d12::Frame;
use crate::decode::d3d12::dx::{self, CopyQueue, Owned};
use crate::output::video::Error;

use super::d3d12::{Taken, planes};

/// wgpu が有効にせず、D3D12 のバッファの取り込みに要る拡張。
pub const EXTENSIONS: &[&CStr] = &[ash::khr::external_memory_win32::NAME];

/// 共有するバッファの数。Vulkan がコピーし終わる前に次のフレームを D3D12 が書けるように二つ。
const SHARED: usize = 2;

/// 渡したテクスチャの組の上限。スロットが返さないまま増え続けたら、それ以上は作らない。
const OUTS_MOST: usize = 32;

const HANDLE_TYPE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::D3D12_RESOURCE;

/// Vulkan のデバイスと同じ GPU の D3D12 のデバイス（デコーダーに使わせる）。取り込みの拡張が無ければ `None`。
/// D3D12 はアダプターごとにデバイスを一つしか作らないので、何度呼んでも同じデバイスが返る。
pub fn beside(device: &wgpu::Device) -> Option<ID3D12Device> {
    dx::device_by_luid(vulkan_luid(device)?).ok()
}

fn vulkan_luid(device: &wgpu::Device) -> Option<LUID> {
    // SAFETY: as_hal の間だけ借りる。プロパティの構造体はローカル変数。
    let bytes = unsafe {
        device.as_hal::<Vulkan>().and_then(|hal| {
            if !hal.enabled_device_extensions().contains(&EXTENSIONS[0]) {
                return None;
            }
            let mut id = vk::PhysicalDeviceIDProperties::default();
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_properties2(
                    hal.raw_physical_device(),
                    &mut vk::PhysicalDeviceProperties2::default().push_next(&mut id),
                );
            (id.device_luid_valid == vk::TRUE).then_some(id.device_luid)
        })
    }?;
    let [a, b, c, d, e, f, g, h] = bytes;
    Some(LUID {
        LowPart: u32::from_ne_bytes([a, b, c, d]),
        HighPart: i32::from_ne_bytes([e, f, g, h]),
    })
}

/// D3D12 が書き、Vulkan が読むバッファ。
struct Shared {
    resource: ID3D12Resource,
    buffer: wgpu::Buffer,
    /// このバッファから読む最後の送信。D3D12 が次に書く前に終わっていなければならない。
    read: Option<wgpu::SubmissionIndex>,
}

/// スロットへ渡すプレーンのテクスチャの組。スロットが手放すまで（`Arc` が一つになるまで）書かない。
struct Out {
    planes: [(wgpu::TextureFormat, wgpu::Extent3d); 2],
    textures: [wgpu::Texture; 2],
    views: Vec<wgpu::TextureView>,
}

pub struct Import {
    device: wgpu::Device,
    raw: ash::Device,
    memory: ash::khr::external_memory_win32::Device,
    kinds: vk::PhysicalDeviceMemoryProperties,
    d3d: ID3D12Device,
    copy: CopyQueue,
    shared: Vec<Shared>,
    next: usize,
    outs: Vec<Arc<Out>>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Import（Vulkan、共有 {}、テクスチャ {} 組）",
            self.shared.len(),
            self.outs.len()
        )
    }
}

// SAFETY: COM のインターフェイスはスレッドに縛られない。使うのは映像のスレッド一つだけ。
unsafe impl Send for Import {}

fn from_decode(error: crate::decode::Error) -> Error {
    Error::NoDevice(error.to_string())
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let luid = vulkan_luid(device)?;
        let d3d = dx::adopted().filter(|adopted| dx::same_luid(dx::luid(adopted), luid))?;
        // SAFETY: as_hal の間だけ借り、持ち出すのは `wgpu::Device` と同じだけ生きる `ash` のハンドルだけ。
        let (raw, memory, kinds) = unsafe {
            device.as_hal::<Vulkan>().map(|hal| {
                let raw = hal.raw_device().clone();
                let instance = hal.shared_instance().raw_instance();
                let memory = ash::khr::external_memory_win32::Device::new(instance, &raw);
                let kinds =
                    instance.get_physical_device_memory_properties(hal.raw_physical_device());
                (raw, memory, kinds)
            })
        }?;
        let copy = match CopyQueue::new(&d3d) {
            Ok(copy) => copy,
            Err(error) => {
                tracing::warn!("Vulkan へ渡す D3D12 のコピーキューを作れない: {error}");
                return None;
            }
        };
        // 映像を作り直すたびに作られるので、言うのは一度だけ。
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("デコーダーのフレームは D3D12 から共有したバッファで Vulkan へ渡す");
        }
        Some(Self {
            device: device.clone(),
            raw,
            memory,
            kinds,
            d3d,
            copy,
            shared: Vec::new(),
            next: 0,
            outs: Vec::new(),
        })
    }

    pub fn take(&mut self, frame: &Frame, queue: &wgpu::Queue) -> Result<Taken, Error> {
        if frame.device.as_raw() != self.d3d.as_raw() {
            return Err(Error::Unsupported("a picture from another device"));
        }
        let (layouts, rows, total) = dx::footprints(&self.d3d, &dx::describe(&frame.texture), 2);
        let index = self.shared_for(dx::whole_rows(&layouts, &rows, total))?;
        let out = self.out(frame)?;
        let shared = &self.shared[index];

        // デコーダーが書き終わるのをコピーキューの GPU の上で待ち、コピーの終わりは CPU で待つ。
        self.copy
            .begin(frame.ready.as_ref().map(|(fence, value)| (fence, *value)))
            .map_err(from_decode)?;
        self.copy
            .planes_into(&frame.texture, &shared.resource, &layouts);
        let value = self.copy.submit().map_err(from_decode)?;
        self.copy.wait(value).map_err(from_decode)?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shared planes"),
            });
        for ((texture, layout), rows) in out.textures.iter().zip(&layouts).zip(rows) {
            let extent = wgpu::Extent3d {
                width: texture.width().min(layout.Footprint.Width),
                height: texture.height().min(rows),
                depth_or_array_layers: 1,
            };
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &shared.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: layout.Offset,
                        bytes_per_row: Some(layout.Footprint.RowPitch),
                        rows_per_image: Some(extent.height),
                    },
                },
                texture.as_image_copy(),
                extent,
            );
        }
        self.shared[index].read = Some(queue.submit([encoder.finish()]));
        // デコーダーのフレーム（`lease`）はもう要らない。スロットが持つのはこちらのテクスチャ。
        let views = out.views.clone();
        Ok((views, Some(out as Arc<dyn std::any::Any + Send + Sync>)))
    }

    /// 次に D3D12 が書く共有のバッファ。前にそこから読んだ Vulkan のコピーが終わるのを待つ。
    fn shared_for(&mut self, size: u64) -> Result<usize, Error> {
        let index = self.next;
        self.next = (self.next + 1) % SHARED;
        if let Some(read) = self
            .shared
            .get_mut(index)
            .and_then(|shared| shared.read.take())
        {
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(read),
                timeout: None,
            });
        }
        if self
            .shared
            .get(index)
            .is_some_and(|shared| shared.buffer.size() >= size)
        {
            return Ok(index);
        }
        let made = self.make_shared(size.next_multiple_of(1 << 16))?;
        if index < self.shared.len() {
            self.shared[index] = made;
        } else {
            self.shared.push(made);
        }
        Ok(index)
    }

    fn make_shared(&self, size: u64) -> Result<Shared, Error> {
        let resource = dx::shared_buffer(&self.d3d, size).map_err(from_decode)?;
        // NT ハンドルは取り込んでも引き取られないので、この関数を出たら閉じる。
        let handle = Owned::shared(&self.d3d, &resource).map_err(from_decode)?;
        let mut external = vk::ExternalMemoryBufferCreateInfo::default().handle_types(HANDLE_TYPE);
        let create = vk::BufferCreateInfo::default()
            .push_next(&mut external)
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `create` のポインタはどれも有効なローカル変数で、デバイスはこの構造体の物。
        let buffer = unsafe { self.raw.create_buffer(&create, None) }
            .map_err(|error| Error::NoDevice(format!("vkCreateBuffer: {error}")))?;
        let (memory, allocated) = match self.bind(buffer, handle.raw().0 as vk::HANDLE) {
            Ok(bound) => bound,
            Err(error) => {
                // SAFETY: いま作り、何も結んでいないバッファ。
                unsafe { self.raw.destroy_buffer(buffer, None) };
                return Err(error);
            }
        };
        // SAFETY: このデバイスで作り、専用に確保したメモリを結んだ未使用のバッファ。以後は wgpu が両方を手放す。
        let buffer = unsafe {
            self.device.create_buffer_from_hal::<Vulkan>(
                wgpu::hal::vulkan::Buffer::from_raw_managed(buffer, memory, 0, allocated),
                &wgpu::BufferDescriptor {
                    label: Some("shared planes"),
                    size,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                },
            )
        };
        Ok(Shared {
            resource,
            buffer,
            read: None,
        })
    }

    /// D3D12 のバッファのメモリを取り込んで結ぶ。返すのはメモリと確保した大きさ。
    fn bind(
        &self,
        buffer: vk::Buffer,
        handle: vk::HANDLE,
    ) -> Result<(vk::DeviceMemory, u64), Error> {
        // SAFETY: このデバイスで作った、メモリの無いバッファ。
        let requirements = unsafe { self.raw.get_buffer_memory_requirements(buffer) };
        let mut properties = vk::MemoryWin32HandlePropertiesKHR::default();
        // SAFETY: `handle` は呼び出しの間開いている共有ハンドル、`properties` は有効なローカル変数。
        let allowed = match unsafe {
            self.memory
                .get_memory_win32_handle_properties(HANDLE_TYPE, handle, &mut properties)
        } {
            Ok(()) => properties.memory_type_bits,
            Err(_) => u32::MAX,
        };
        let kind = self
            .memory_kind(requirements.memory_type_bits & allowed)
            .ok_or(Error::Unsupported(
                "no memory a shared buffer can be read from",
            ))?;

        let mut import = vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(HANDLE_TYPE)
            .handle(handle);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        let allocate = vk::MemoryAllocateInfo::default()
            .push_next(&mut import)
            .push_next(&mut dedicated)
            .allocation_size(requirements.size)
            .memory_type_index(kind);
        // SAFETY: 繋いだ構造体は有効なローカル変数。ハンドルの所有は移らない。
        let memory = unsafe { self.raw.allocate_memory(&allocate, None) }
            .map_err(|error| Error::NoDevice(format!("vkAllocateMemory: {error}")))?;
        // SAFETY: メモリの無いバッファに、同じデバイスで専用に確保したメモリ。
        unsafe { self.raw.bind_buffer_memory(buffer, memory, 0) }.map_err(|error| {
            // SAFETY: メモリは未使用で、バッファは呼び出し側が壊す。
            unsafe { self.raw.free_memory(memory, None) };
            Error::NoDevice(format!("vkBindBufferMemory: {error}"))
        })?;
        Ok((memory, requirements.size))
    }

    /// `usable` のビットのうち、GPU のメモリーを優先した種類。
    fn memory_kind(&self, usable: u32) -> Option<u32> {
        let count = (self.kinds.memory_type_count as usize).min(vk::MAX_MEMORY_TYPES);
        self.kinds.memory_types[..count]
            .iter()
            .zip(0u32..)
            .filter(|(_, bit)| usable & (1 << bit) != 0)
            .max_by_key(|(kind, _)| {
                kind.property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .map(|(_, bit)| bit)
    }

    /// スロットが手放したテクスチャの組。無ければ作る。
    fn out(&mut self, frame: &Frame) -> Result<Arc<Out>, Error> {
        let planes = planes(frame);
        // 大きさの替わった組は、手放されたものから捨てる。
        self.outs
            .retain(|out| out.planes == planes || Arc::strong_count(out) > 1);
        if let Some(free) = self
            .outs
            .iter()
            .find(|out| out.planes == planes && Arc::strong_count(out) == 1)
        {
            return Ok(free.clone());
        }
        if self.outs.len() >= OUTS_MOST {
            return Err(Error::Unsupported("every converted picture is still held"));
        }
        let textures = planes.map(|(format, size)| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shared plane"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let views = textures
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let out = Arc::new(Out {
            planes,
            textures,
            views,
        });
        self.outs.push(out.clone());
        Ok(out)
    }

    pub fn forget(&mut self) {
        // スロットが持っている組はスロットが手放す。
        self.outs.retain(|out| Arc::strong_count(out) > 1);
    }
}
