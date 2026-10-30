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
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::decode::d3d12::Frame;
use crate::decode::d3d12::dx::{self, Owned, borrow};
use crate::output::video::Error;

use super::d3d12::Taken;

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
    let luid = vulkan_luid(device)?;
    dx::device_by_luid(luid).ok()
}

fn vulkan_luid(device: &wgpu::Device) -> Option<LUID> {
    // SAFETY: as_hal の間だけ借りる。プロパティの構造体はローカル変数。
    let bytes = unsafe {
        device.as_hal::<Vulkan>().and_then(|hal| {
            if !hal.enabled_device_extensions().contains(&EXTENSIONS[0]) {
                return None;
            }
            let instance = hal.shared_instance().raw_instance();
            let mut id = vk::PhysicalDeviceIDProperties::default();
            {
                let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
                instance
                    .get_physical_device_properties2(hal.raw_physical_device(), &mut properties);
            }
            (id.device_luid_valid == vk::TRUE).then_some(id.device_luid)
        })
    }?;
    Some(LUID {
        LowPart: u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        HighPart: i32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
    })
}

/// D3D12 が書き、Vulkan が読むバッファ。
struct Shared {
    resource: ID3D12Resource,
    buffer: wgpu::Buffer,
    size: u64,
    /// このバッファから読む最後の送信。D3D12 が次に書く前に終わっていなければならない。
    read: Option<wgpu::SubmissionIndex>,
}

/// スロットへ渡すプレーンのテクスチャの組。スロットが手放すまで（`Arc` が一つになるまで）書かない。
struct Out {
    key: (u32, u32, bool),
    textures: [wgpu::Texture; 2],
    views: Vec<wgpu::TextureView>,
}

pub struct Import {
    device: wgpu::Device,
    raw: ash::Device,
    memory: ash::khr::external_memory_win32::Device,
    kinds: vk::PhysicalDeviceMemoryProperties,
    d3d: ID3D12Device,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    event: Owned,
    step: u64,
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

impl Drop for Import {
    fn drop(&mut self) {
        // 待ったコピーしか投入しないが、念のため最後の値まで待ってから D3D12 のオブジェクトを手放す。
        let _ = self.event.wait_fence(&self.fence, self.step);
    }
}

fn from_decode(error: crate::decode::Error) -> Error {
    Error::NoDevice(error.to_string())
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let luid = vulkan_luid(device)?;
        let d3d = dx::adopted().filter(|adopted| {
            let adopted = dx::luid(adopted);
            adopted.LowPart == luid.LowPart && adopted.HighPart == luid.HighPart
        })?;
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
        // SAFETY: どの引数も有効なローカル変数。
        let made = unsafe {
            (|| -> windows::core::Result<_> {
                let queue: ID3D12CommandQueue =
                    d3d.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                        Type: D3D12_COMMAND_LIST_TYPE_COPY,
                        ..Default::default()
                    })?;
                let allocator: ID3D12CommandAllocator =
                    d3d.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
                let list: ID3D12GraphicsCommandList =
                    d3d.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
                list.Close()?;
                let fence: ID3D12Fence = d3d.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
                Ok((queue, allocator, list, fence))
            })()
        };
        let (queue, allocator, list, fence) = match made {
            Ok(made) => made,
            Err(error) => {
                tracing::warn!("Vulkan へ渡す D3D12 のコピーキューを作れない: {error}");
                return None;
            }
        };
        let event = Owned::event().ok()?;
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
            queue,
            allocator,
            list,
            fence,
            event,
            step: 0,
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
        // 最後の行も RowPitch 分読むので、`total`（最後の行は実際の幅まで）より大きく取る。
        let total = layouts
            .iter()
            .zip(&rows)
            .map(|(layout, rows)| {
                layout.Offset + u64::from(layout.Footprint.RowPitch) * u64::from(*rows)
            })
            .fold(total, u64::max);
        let index = self.shared_for(total)?;
        let resource = self.shared[index].resource.clone();
        self.copy(frame, &layouts, &resource)?;

        let out = self.out(frame)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shared planes"),
            });
        for plane in 0..2 {
            let texture = &out.textures[plane];
            let layout = &layouts[plane];
            let extent = wgpu::Extent3d {
                width: texture.width().min(layout.Footprint.Width),
                height: texture.height().min(rows[plane]),
                depth_or_array_layers: 1,
            };
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.shared[index].buffer,
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
        // コピーし終えたのでデコーダーのフレーム（`lease`）はもう要らない。スロットが持つのはこちらのテクスチャ。
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
            .is_some_and(|shared| shared.size >= size)
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
        let resource = shared_buffer(&self.d3d, size)?;
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
        // ハンドルは取り込みが参照を持つので、この関数を出たら閉じてよい（NT ハンドルは引き取られない）。
        let (memory, allocated) = match self.bind(buffer, handle.raw().0 as vk::HANDLE) {
            Ok(bound) => bound,
            Err(error) => {
                // SAFETY: いま作り、何も結んでいないバッファ。
                unsafe { self.raw.destroy_buffer(buffer, None) };
                return Err(error);
            }
        };
        // SAFETY: このデバイスで作り、専用に確保したメモリを結んだバッファ。以後は wgpu が両方を手放す。
        let hal =
            unsafe { wgpu::hal::vulkan::Buffer::from_raw_managed(buffer, memory, 0, allocated) };
        // SAFETY: いま作った未使用のバッファで、記述どおりの大きさと用途。
        let buffer = unsafe {
            self.device.create_buffer_from_hal::<Vulkan>(
                hal,
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
            size,
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
        let usable = requirements.memory_type_bits & allowed;
        let count = (self.kinds.memory_type_count as usize).min(vk::MAX_MEMORY_TYPES);
        let local = |bit: usize| {
            self.kinds.memory_types[bit]
                .property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        };
        let kind = (0..count)
            .filter(|bit| usable & (1 << bit) != 0)
            .max_by_key(|bit| local(*bit))
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
            .memory_type_index(kind as u32);
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

    /// フレームのプレーンを共有したバッファへ D3D12 のコピーキューで置き、終わるまで待つ。
    fn copy(
        &mut self,
        frame: &Frame,
        layouts: &[D3D12_PLACED_SUBRESOURCE_FOOTPRINT; 2],
        into: &ID3D12Resource,
    ) -> Result<(), Error> {
        let failed =
            |what: &str, error: windows::core::Error| Error::NoDevice(format!("{what}: {error}"));
        // SAFETY: アロケーターとリストはこの構造体の物で、前のコピーは待ち終えている。
        unsafe {
            self.allocator
                .Reset()
                .map_err(|error| failed("CommandAllocator::Reset", error))?;
            self.list
                .Reset(&self.allocator, None)
                .map_err(|error| failed("CommandList::Reset", error))?;
            for (plane, layout) in layouts.iter().enumerate() {
                let destination = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: borrow(into),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: *layout,
                    },
                };
                let source = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: borrow(&frame.texture),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        SubresourceIndex: plane as u32,
                    },
                };
                self.list
                    .CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            }
            self.list
                .Close()
                .map_err(|error| failed("CommandList::Close", error))?;
            // デコーダーが書き終わるのを、このキューの GPU の上で待つ。
            if let Some((fence, value)) = frame.ready.as_ref() {
                self.queue
                    .Wait(fence, *value)
                    .map_err(|error| failed("Queue::Wait", error))?;
            }
            self.queue.ExecuteCommandLists(&[Some(
                self.list
                    .cast()
                    .map_err(|error| failed("cast to ID3D12CommandList", error))?,
            )]);
            self.step += 1;
            self.queue
                .Signal(&self.fence, self.step)
                .map_err(|error| failed("Queue::Signal", error))?;
        }
        self.event
            .wait_fence(&self.fence, self.step)
            .map_err(from_decode)
    }

    /// スロットが手放したテクスチャの組。無ければ作る。
    fn out(&mut self, frame: &Frame) -> Result<Arc<Out>, Error> {
        let key = (frame.width, frame.height, frame.ten_bit);
        // 大きさの替わった組は、手放されたものから捨てる。
        self.outs
            .retain(|out| out.key == key || Arc::strong_count(out) > 1);
        if let Some(free) = self
            .outs
            .iter()
            .find(|out| out.key == key && Arc::strong_count(out) == 1)
        {
            return Ok(free.clone());
        }
        if self.outs.len() >= OUTS_MOST {
            return Err(Error::Unsupported("every converted picture is still held"));
        }
        let planes = if frame.ten_bit {
            [
                (wgpu::TextureFormat::R16Unorm, 1u32),
                (wgpu::TextureFormat::Rg16Unorm, 2),
            ]
        } else {
            [
                (wgpu::TextureFormat::R8Unorm, 1),
                (wgpu::TextureFormat::Rg8Unorm, 2),
            ]
        };
        let textures = planes.map(|(format, divisor)| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shared plane"),
                size: wgpu::Extent3d {
                    width: frame.width.div_ceil(divisor),
                    height: frame.height.div_ceil(divisor),
                    depth_or_array_layers: 1,
                },
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
            key,
            textures,
            views,
        });
        self.outs.push(out.clone());
        Ok(out)
    }

    pub fn forget(&mut self) {
        // スロットが持っている組はスロットが捨てる。ここでは手放されたものだけ。
        self.outs.retain(|out| Arc::strong_count(out) > 1);
    }
}

/// D3D12 が書き、Vulkan が取り込む線形のバッファ。`D3D12_HEAP_FLAG_SHARED` が無いと共有できない。
fn shared_buffer(device: &ID3D12Device, size: u64) -> Result<ID3D12Resource, Error> {
    let properties = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT(0),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: both descriptions and `made` are live locals.
    unsafe {
        device.CreateCommittedResource(
            &properties,
            D3D12_HEAP_FLAG_SHARED,
            &description,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut made,
        )
    }
    .map_err(|error| Error::NoDevice(format!("CreateCommittedResource（共有）: {error}")))?;
    made.ok_or_else(|| Error::NoDevice(String::from("共有のバッファが確保できません")))
}
