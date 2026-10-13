// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MediaCodec が書いた `AHardwareBuffer` からコピーなしでレンダリングする（[`crate::decode::android`]）。
//! 中身は `VK_QUEUE_FAMILY_FOREIGN_EXT` からの受け取りで得る（`UNDEFINED` からの遷移は捨てうる）。
//! 外部形式（`VK_FORMAT_UNDEFINED`）は wgpu で表せないので通らない。

use std::ffi::CStr;

use ash::vk;
use wgpu::hal::api::Vulkan;

use crate::decode::android::Frame;

use crate::output::video::Error;

/// wgpu が有効にせず、`AHardwareBuffer` の取り込みに要る拡張（ほかは Vulkan 1.1 の中核）。
pub const EXTENSIONS: &[&CStr] = &[
    ash::android::external_memory_android_hardware_buffer::NAME,
    ash::ext::queue_family_foreign::NAME,
];

fn formats(found: vk::Format) -> Option<(wgpu::TextureFormat, [wgpu::TextureFormat; 2])> {
    match found {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => Some((
            wgpu::TextureFormat::NV12,
            [wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm],
        )),
        vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16 => Some((
            wgpu::TextureFormat::P010,
            [
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm,
            ],
        )),
        _ => None,
    }
}

/// 取り込みがこれから何をするかを書く先。
pub type Trace = Box<dyn FnMut(&str) + Send>;

pub struct Import {
    device: wgpu::Device,
    raw: ash::Device,
    external: ash::android::external_memory_android_hardware_buffer::Device,
    physical: vk::PhysicalDevice,
    queue_family: u32,
    made: super::Kept<usize, Vec<wgpu::TextureView>>,
    trace: Option<Trace>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Import({} プレーン)", self.made.len())
    }
}

impl Import {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        // SAFETY: as_hal の間だけ借り、持ち出すのは `wgpu::Device` と同じだけ生きる `ash` のハンドルだけ。
        let opened = unsafe {
            device.as_hal::<Vulkan>().map(|hal| {
                let raw = hal.raw_device().clone();
                let instance = hal.shared_instance().raw_instance().clone();
                let external = ash::android::external_memory_android_hardware_buffer::Device::new(
                    &instance, &raw,
                );
                (
                    raw,
                    external,
                    hal.raw_physical_device(),
                    hal.queue_family_index(),
                )
            })
        };
        let (raw, external, physical, queue_family) = opened?;
        Some(Self {
            device: device.clone(),
            raw,
            external,
            physical,
            queue_family,
            made: super::Kept::new(),
            trace: None,
        })
    }

    pub fn tracing(&mut self, trace: Trace) {
        self.trace = Some(trace);
    }

    fn step(&mut self, what: &str) {
        if let Some(trace) = self.trace.as_mut() {
            trace(what);
        }
    }

    pub fn views(&mut self, frame: &Frame) -> Result<Vec<wgpu::TextureView>, Error> {
        let key = frame.key();
        if key != 0
            && let Some(found) = self.made.get(&key)
        {
            return Ok(found);
        }
        let views = self.make(frame)?;
        if key != 0 {
            self.made.insert(key, views.clone());
        }
        Ok(views)
    }

    fn make(&mut self, frame: &Frame) -> Result<Vec<wgpu::TextureView>, Error> {
        self.step("AImage から AHardwareBuffer を取る");
        let buffer = frame
            .buffer()
            .ok_or(Error::Unsupported("a picture with no hardware buffer"))?;
        let described = buffer.describe();
        self.step(&format!(
            "AHardwareBuffer は {}x{}、その形式 {:#x}（{:?}）",
            described.width,
            described.height,
            i32::from(described.format),
            described.format
        ));
        self.step("vkGetAndroidHardwareBufferPropertiesANDROID");
        let (found, size, kinds, external_format) = {
            let mut layout = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
            let mut properties =
                vk::AndroidHardwareBufferPropertiesANDROID::default().push_next(&mut layout);
            // SAFETY: バッファは呼び出しの間生き、`properties` は有効なローカル変数。
            unsafe {
                self.external
                    .get_android_hardware_buffer_properties(buffer.as_ptr().cast(), &mut properties)
            }
            .map_err(|error| {
                Error::NoDevice(format!(
                    "vkGetAndroidHardwareBufferPropertiesANDROID: {error}"
                ))
            })?;
            let size = properties.allocation_size;
            let kinds = properties.memory_type_bits;
            let _ = properties;
            (layout.format, size, kinds, layout.external_format)
        };
        // 名前ではなく数値を書く（0 なら外部形式、ほかはここで対応しないと判断した物）。
        self.step(&format!(
            "Vulkan が返す形式 {found:?} = {}  外部形式 {external_format:#018x}",
            found.as_raw()
        ));
        let Some((whole, planes)) = formats(found) else {
            if found == vk::Format::UNDEFINED {
                self.step(
                    "**外部形式のみ。VkSamplerYcbcrConversion が要る形で、wgpu では表せません。**",
                );
            } else {
                self.step(&format!(
                    "**名前は付いていますが、プレーンごとに分けられない形式です（{}）。**",
                    found.as_raw()
                ));
            }
            return Err(Error::Unsupported(
                "an AHardwareBuffer in a layout Vulkan will not name",
            ));
        };

        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let create = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            // 一プレーンの view は別の形式の view なので、このフラグが要る。
            .flags(vk::ImageCreateFlags::MUTABLE_FORMAT)
            .image_type(vk::ImageType::TYPE_2D)
            .format(found)
            // Vulkan はバッファ自身の大きさを求める。
            .extent(vk::Extent3D {
                width: described.width,
                height: described.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        self.step(&format!("vkCreateImage（{found:?}、MUTABLE_FORMAT）"));
        // SAFETY: `create` のポインタはどれも有効なローカル変数で、デバイスはこの構造体の物。
        let image = unsafe { self.raw.create_image(&create, None) }
            .map_err(|error| Error::NoDevice(format!("vkCreateImage: {error}")))?;

        let memory = match self.bind(image, &buffer, size, kinds) {
            Ok(memory) => memory,
            Err(error) => {
                // SAFETY: いま作り、何も結んでいないイメージ。
                unsafe { self.raw.destroy_image(image, None) };
                return Err(error);
            }
        };
        self.step("VK_QUEUE_FAMILY_FOREIGN_EXT からの受け取り + vkQueueWaitIdle");
        if let Err(error) = self.acquire(image) {
            // SAFETY: どちらも未使用で、ここで作った物。
            unsafe {
                self.raw.destroy_image(image, None);
                self.raw.free_memory(memory, None);
            }
            return Err(error);
        }
        self.step(&format!(
            "wgpu のテクスチャにする（{whole:?}）とプレーンごとのビュー"
        ));
        let views = self.wrap(image, memory, frame, whole, planes);
        self.step(&format!("ビューを {} 個作った", views.len()));
        Ok(views)
    }

    fn bind(
        &mut self,
        image: vk::Image,
        buffer: &ndk::hardware_buffer::HardwareBuffer,
        size: u64,
        kinds: u32,
    ) -> Result<vk::DeviceMemory, Error> {
        self.step("vkAllocateMemory（AHardwareBuffer を取り込む、専有）");
        let kind = self
            .memory_kind(kinds)
            .ok_or(Error::Unsupported("no memory this frame can be read from"))?;
        let mut import =
            vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(buffer.as_ptr().cast());
        // `AHardwareBuffer` の取り込みは一つのイメージ専用でなければならない。
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .push_next(&mut import)
            .push_next(&mut dedicated)
            .allocation_size(size)
            .memory_type_index(kind);
        // SAFETY: 繋いだ構造体は有効なローカル変数で、バッファは呼び出しの間生きる。
        let memory = unsafe { self.raw.allocate_memory(&allocate, None) }
            .map_err(|error| Error::NoDevice(format!("vkAllocateMemory: {error}")))?;
        self.step("vkBindImageMemory");
        // SAFETY: メモリの無いイメージに、同じデバイスで専用に確保したメモリ。
        unsafe { self.raw.bind_image_memory(image, memory, 0) }.map_err(|error| {
            // SAFETY: メモリは未使用で、イメージは呼び出し側が壊す。
            unsafe { self.raw.free_memory(memory, None) };
            Error::NoDevice(format!("vkBindImageMemory: {error}"))
        })?;
        Ok(memory)
    }

    fn memory_kind(&self, bits: u32) -> Option<u32> {
        // SAFETY: 物理デバイスはこの取り込みを開いた hal のアダプターの物で、より長く生きる。
        let properties = unsafe {
            self.device
                .as_hal::<Vulkan>()
                .map(|hal| hal.shared_instance().raw_instance().clone())
        }?;
        // SAFETY: as above; the instance is a reference counted copy.
        let memory = unsafe { properties.get_physical_device_memory_properties(self.physical) };
        let kinds = &memory.memory_types[..memory.memory_type_count as usize];
        let allowed = |index: usize| bits & (1 << index) != 0;
        kinds
            .iter()
            .enumerate()
            .find(|(index, kind)| {
                allowed(*index)
                    && kind
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .or_else(|| kinds.iter().enumerate().find(|(index, _)| allowed(*index)))
            .map(|(index, _)| index as u32)
    }

    /// 書いた側からイメージを受け取る（`VK_QUEUE_FAMILY_FOREIGN_EXT` から）。バッファごとに一度だけ。
    fn acquire(&mut self, image: vk::Image) -> Result<(), Error> {
        // SAFETY: wgpu がデバイスを開いたキューファミリーなので、その 0 番のキューはある。
        let queue = unsafe { self.raw.get_device_queue(self.queue_family, 0) };
        let pool = vk::CommandPoolCreateInfo::default().queue_family_index(self.queue_family);
        // SAFETY: `pool` は有効なローカル変数、デバイスはこれ。
        let pool = unsafe { self.raw.create_command_pool(&pool, None) }
            .map_err(|error| Error::NoDevice(format!("vkCreateCommandPool: {error}")))?;
        let result = self.acquire_in(pool, queue, image);
        // SAFETY: このデバイスで作ったプールで、送った物は下の `queue_wait_idle` で待った。
        unsafe { self.raw.destroy_command_pool(pool, None) };
        result
    }

    fn acquire_in(
        &mut self,
        pool: vk::CommandPool,
        queue: vk::Queue,
        image: vk::Image,
    ) -> Result<(), Error> {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: `allocate` は有効なローカル変数で、このデバイスのプールを指す。
        let buffers = unsafe { self.raw.allocate_command_buffers(&allocate) }
            .map_err(|error| Error::NoDevice(format!("vkAllocateCommandBuffers: {error}")))?;
        let command = buffers[0];
        let barriers = [vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .dst_queue_family_index(self.queue_family)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })];
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        let commands = [command];
        let submit = vk::SubmitInfo::default().command_buffers(&commands);
        // SAFETY: 未記録のコマンドバッファで、`queue_wait_idle` があるので戻ってすぐプールを壊せる。
        unsafe {
            self.raw
                .begin_command_buffer(command, &begin)
                .map_err(|error| Error::NoDevice(format!("vkBeginCommandBuffer: {error}")))?;
            self.raw.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
            self.raw
                .end_command_buffer(command)
                .map_err(|error| Error::NoDevice(format!("vkEndCommandBuffer: {error}")))?;
            self.raw
                .queue_submit(queue, &[submit], vk::Fence::null())
                .map_err(|error| Error::NoDevice(format!("vkQueueSubmit: {error}")))?;
            self.raw
                .queue_wait_idle(queue)
                .map_err(|error| Error::NoDevice(format!("vkQueueWaitIdle: {error}")))?;
        }
        Ok(())
    }

    fn wrap(
        &mut self,
        image: vk::Image,
        memory: vk::DeviceMemory,
        frame: &Frame,
        whole: wgpu::TextureFormat,
        planes: [wgpu::TextureFormat; 2],
    ) -> Vec<wgpu::TextureView> {
        // バッファではなくフレームの大きさ。
        let size = wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        };
        let raw = self.raw.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || {
            // SAFETY: テクスチャに届く物が無く、コマンドも全部終わった後に一度だけ呼ばれる。
            unsafe {
                raw.destroy_image(image, None);
                raw.free_memory(memory, None);
            }
        });
        let hal = wgpu::hal::TextureDescriptor {
            label: Some("decoded picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: whole,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: planes.to_vec(),
        };
        // SAFETY: このデバイスの、報告どおりの形式の `VkImage`。メモリはコールバックまで生きる。
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
        let Some(hal_texture) = hal_texture else {
            return Vec::new();
        };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("decoded picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: whole,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &planes,
        };
        // SAFETY: いま作った未使用の物。受け取り済みなので `RESOURCE`（中身を捨てさせない）。
        let texture = unsafe {
            self.device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                &descriptor,
                wgpu::TextureUses::RESOURCE,
            )
        };
        [wgpu::TextureAspect::Plane0, wgpu::TextureAspect::Plane1]
            .into_iter()
            .zip(planes)
            .map(|(aspect, format)| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("decoded plane"),
                    format: Some(format),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    aspect,
                    ..Default::default()
                })
            })
            .collect()
    }

    pub fn forget(&mut self) {
        self.made.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_plane_layouts_are_the_ones_a_broadcast_gives() {
        assert_eq!(
            formats(vk::Format::G8_B8R8_2PLANE_420_UNORM).map(|found| found.0),
            Some(wgpu::TextureFormat::NV12)
        );
        assert_eq!(
            formats(vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16).map(|found| found.0),
            Some(wgpu::TextureFormat::P010)
        );
        assert!(formats(vk::Format::UNDEFINED).is_none());
        assert!(formats(vk::Format::R8G8B8A8_UNORM).is_none());
    }

    #[test]
    fn each_plane_is_sampled_as_a_plain_texture() {
        let (_, planes) = formats(vk::Format::G8_B8R8_2PLANE_420_UNORM).expect("NV12");
        assert_eq!(
            planes,
            [wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm]
        );
        let (_, planes) =
            formats(vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16).expect("P010");
        assert_eq!(
            planes,
            [
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm
            ]
        );
    }
}
