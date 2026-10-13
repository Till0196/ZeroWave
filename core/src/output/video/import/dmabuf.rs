// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Linux のデコーダーが書いた DMA-BUF からコピーなしでレンダリングする（`SEPARATE_LAYERS` で輝度と色差の 2 つのイメージ）。
//! イメージはサーフェスごとに一度作り、ファイルディスクリプターの inode で検索する。

use std::ffi::CStr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

use ash::vk;
use wgpu::hal::api::Vulkan;

use crate::decode::{DmaBuf, DmaBufPlane};

use crate::output::video::Error;

/// wgpu が有効にせず、DMA-BUF の取り込みに要る拡張。
pub const EXTENSIONS: &[&CStr] = &[
    ash::khr::external_memory_fd::NAME,
    ash::ext::external_memory_dma_buf::NAME,
    ash::ext::image_drm_format_modifier::NAME,
];

/// 書き出したプレーンの `DRM_FORMAT_*` の Vulkan と wgpu での形式と、一サンプルが覆う輝度のサンプル数。
fn formats(drm: u32) -> Option<(vk::Format, wgpu::TextureFormat, u32)> {
    match &drm.to_le_bytes() {
        b"R8  " => Some((vk::Format::R8_UNORM, wgpu::TextureFormat::R8Unorm, 1)),
        b"GR88" => Some((vk::Format::R8G8_UNORM, wgpu::TextureFormat::Rg8Unorm, 2)),
        // NVIDIA は `RG88`、Intel は `GR88` と報告するが並びは同じ（どちらも `.r` が Cb）。
        b"RG88" => Some((vk::Format::R8G8_UNORM, wgpu::TextureFormat::Rg8Unorm, 2)),
        b"R16 " => Some((vk::Format::R16_UNORM, wgpu::TextureFormat::R16Unorm, 1)),
        b"GR32" => Some((vk::Format::R16G16_UNORM, wgpu::TextureFormat::Rg16Unorm, 2)),
        b"RG32" => Some((vk::Format::R16G16_UNORM, wgpu::TextureFormat::Rg16Unorm, 2)),
        _ => None,
    }
}

/// デコーダーが生きている間、一つのサーフェスの一プレーンを指す物（inode・オフセット・形式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    inode: u64,
    offset: u32,
    format: u32,
}

pub struct Import {
    device: wgpu::Device,
    raw: ash::Device,
    external: ash::khr::external_memory_fd::Device,
    made: super::Kept<Key, wgpu::TextureView>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Import({} プレーン)", self.made.len())
    }
}

/// レンダリングする GPU の render node を VA-API の側へ伝える（無ければ VA-API の既定に任せる）。
pub fn adopt_render_node(adapter: &wgpu::Adapter) {
    // SAFETY: hal のアダプターは呼び出しの間だけ借りて持ち出さず、プロパティの構造体はローカル変数。
    let node = unsafe {
        adapter.as_hal::<Vulkan>().and_then(|hal| {
            let instance = hal.shared_instance().raw_instance();
            let physical = hal.raw_physical_device();
            let has = instance
                .enumerate_device_extension_properties(physical)
                .ok()?
                .iter()
                .any(|found| {
                    found.extension_name_as_c_str() == Ok(ash::ext::physical_device_drm::NAME)
                });
            if !has {
                return None;
            }
            let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
            instance.get_physical_device_properties2(physical, &mut properties);
            (drm.has_render == vk::TRUE)
                .then_some((drm.render_major as u32, drm.render_minor as u32))
        })
    };
    if let Some((major, minor)) = node {
        crate::decode::vaapi::adopt_render_node(major, minor);
    }
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        // SAFETY: as_hal の間だけ借り、持ち出すのは `wgpu::Device` と同じだけ生きる `ash` のハンドルだけ。
        let opened = unsafe {
            device.as_hal::<Vulkan>().map(|hal| {
                let raw = hal.raw_device().clone();
                let instance = hal.shared_instance().raw_instance().clone();
                let external = ash::khr::external_memory_fd::Device::new(&instance, &raw);
                (raw, external)
            })
        };
        let (raw, external) = opened?;
        Some(Self {
            device: device.clone(),
            raw,
            external,
            made: super::Kept::new(),
        })
    }

    pub fn views(&mut self, buffer: &DmaBuf) -> Result<Vec<wgpu::TextureView>, Error> {
        let mut out = Vec::with_capacity(buffer.planes.len());
        for plane in &buffer.planes {
            out.push(self.view(buffer, plane)?);
        }
        Ok(out)
    }

    fn view(&mut self, buffer: &DmaBuf, plane: &DmaBufPlane) -> Result<wgpu::TextureView, Error> {
        let (format, wgpu_format, divisor) = formats(plane.format)
            .ok_or(Error::Unsupported("a plane in a format nothing samples"))?;
        let width = buffer.width.div_ceil(divisor);
        let height = buffer.height.div_ceil(divisor);
        let key = Key {
            inode: inode(&plane.fd)?,
            offset: plane.offset,
            format: plane.format,
        };
        if let Some(found) = self.made.get(&key) {
            return Ok(found);
        }
        let view = self.make(buffer.modifier, plane, format, wgpu_format, width, height)?;
        self.made.insert(key, view.clone());
        Ok(view)
    }

    fn make(
        &self,
        modifier: u64,
        plane: &DmaBufPlane,
        format: vk::Format,
        wgpu_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Result<wgpu::TextureView, Error> {
        let layouts = [vk::SubresourceLayout {
            offset: u64::from(plane.offset),
            size: 0,
            row_pitch: u64::from(plane.stride),
            array_pitch: 0,
            depth_pitch: 0,
        }];
        let mut tiling = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let create = vk::ImageCreateInfo::default()
            .push_next(&mut tiling)
            .push_next(&mut external)
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `create` のポインタはどれも有効なローカル変数で、デバイスはこの構造体の物。
        let image = unsafe { self.raw.create_image(&create, None) }
            .map_err(|error| Error::NoDevice(format!("vkCreateImage: {error}")))?;

        match self.bind(image, plane) {
            Ok(memory) => Ok(self.wrap(image, memory, wgpu_format, width, height)),
            Err(error) => {
                // SAFETY: いま作り、何も結んでいないイメージ。
                unsafe { self.raw.destroy_image(image, None) };
                Err(error)
            }
        }
    }

    fn bind(&self, image: vk::Image, plane: &DmaBufPlane) -> Result<vk::DeviceMemory, Error> {
        let fd = plane.fd.as_raw_fd();
        let mut properties = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: `fd` は呼び出しの間開いている DMA-BUF、`properties` は有効なローカル変数。
        unsafe {
            self.external.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd,
                &mut properties,
            )
        }
        .map_err(|error| Error::NoDevice(format!("vkGetMemoryFdPropertiesKHR: {error}")))?;

        // SAFETY: このデバイスで作った、メモリの無いイメージ。
        let requirements = unsafe { self.raw.get_image_memory_requirements(image) };
        let kind = (0..32)
            .find(|bit| {
                (requirements.memory_type_bits & properties.memory_type_bits) & (1 << bit) != 0
            })
            .ok_or(Error::Unsupported("no memory this frame can be read from"))?;

        // `vkAllocateMemory` はファイルディスクリプターを引き取るので、複製を渡す（呼び出し側は自分のを持つ）。
        let mine = duplicate(&plane.fd)?;
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(mine.as_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .push_next(&mut import)
            .push_next(&mut dedicated)
            .allocation_size(requirements.size)
            .memory_type_index(kind);
        // SAFETY: 繋いだ構造体は有効なローカル変数、`mine` は他に所有者の無いファイルディスクリプター。
        // 失敗したとき Vulkan は引き取らないので、`mine` が落ちて閉じる。
        let memory = unsafe { self.raw.allocate_memory(&allocate, None) }
            .map_err(|error| Error::NoDevice(format!("vkAllocateMemory: {error}")))?;
        // 引き取ったのは Vulkan なので閉じない。
        let _ = std::os::fd::IntoRawFd::into_raw_fd(mine);
        // SAFETY: メモリの無いイメージに、同じデバイスで専用に確保したメモリ。
        unsafe { self.raw.bind_image_memory(image, memory, 0) }.map_err(|error| {
            // SAFETY: メモリは未使用で、イメージは呼び出し側が壊す。
            unsafe { self.raw.free_memory(memory, None) };
            Error::NoDevice(format!("vkBindImageMemory: {error}"))
        })?;
        Ok(memory)
    }

    /// wgpu が作っていないイメージを包む（イメージとメモリはコールバックで手放す）。
    fn wrap(
        &self,
        image: vk::Image,
        memory: vk::DeviceMemory,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let raw = self.raw.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || {
            // SAFETY: テクスチャに届く物が無く、参照したコマンドも全部終わった後に一度だけ呼ばれる。
            unsafe {
                raw.destroy_image(image, None);
                raw.free_memory(memory, None);
            }
        });
        let hal = wgpu::hal::TextureDescriptor {
            label: Some("imported plane"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        // SAFETY: このデバイスの記述どおりの `VkImage` で、メモリはコールバックまで生きる。
        let hal_texture = unsafe {
            self.device.as_hal::<Vulkan>().map(|device| {
                device.texture_from_raw(
                    image,
                    &hal,
                    Some(drop_callback),
                    wgpu::hal::vulkan::TextureMemory::External,
                )
            })
        };
        let hal_texture = hal_texture.expect("a Vulkan device the importer was opened on");
        let descriptor = wgpu::TextureDescriptor {
            label: Some("imported plane"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: いま作った未使用の物。デコーダーが Vulkan の外で書いたので `UNINITIALIZED`。
        let texture = unsafe {
            self.device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                &descriptor,
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub fn forget(&mut self) {
        self.made.clear();
    }
}

fn inode(fd: &OwnedFd) -> Result<u64, Error> {
    std::fs::File::from(duplicate(fd)?)
        .metadata()
        .map(|found| found.ino())
        .map_err(|error| Error::NoDevice(format!("fstat: {error}")))
}

fn duplicate(fd: &OwnedFd) -> Result<OwnedFd, Error> {
    fd.try_clone()
        .map_err(|_| Error::NoDevice(String::from("ファイルディスクリプターを複製できません")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exported_plane_formats_are_the_ones_a_broadcast_gives() {
        let of = |name: &[u8; 4]| formats(u32::from_le_bytes(*name)).map(|found| found.1);
        assert_eq!(of(b"R8  "), Some(wgpu::TextureFormat::R8Unorm));
        assert_eq!(of(b"GR88"), Some(wgpu::TextureFormat::Rg8Unorm));
        assert_eq!(of(b"R16 "), Some(wgpu::TextureFormat::R16Unorm));
        assert_eq!(of(b"GR32"), Some(wgpu::TextureFormat::Rg16Unorm));
        assert_eq!(of(b"AR24"), None);
    }

    #[test]
    fn a_chroma_plane_is_half_the_picture_each_way() {
        assert_eq!(formats(u32::from_le_bytes(*b"R8  ")).map(|f| f.2), Some(1));
        assert_eq!(formats(u32::from_le_bytes(*b"GR88")).map(|f| f.2), Some(2));
    }
}
