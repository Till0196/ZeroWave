// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レンダリングする GPU（wgpu の Vulkan）のリソースを、バックエンドの API（CUDA・VA-API）へエクスポートするヘルパー（Linux）。
//! `OPAQUE_FD` は同じ GPU の CUDA へ、`DMA_BUF` は VA-API へ。

use std::ffi::CStr;
use std::os::fd::{FromRawFd, OwnedFd};

use ash::vk;
use wgpu::hal::api::Vulkan;

/// 対応している拡張だけ足す。
pub const EXTENSIONS: &[&CStr] = &[
    ash::khr::external_memory_fd::NAME,
    ash::ext::external_memory_dma_buf::NAME,
    ash::khr::external_semaphore_fd::NAME,
];

pub(super) struct Context {
    raw: ash::Device,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    memory_fd: ash::khr::external_memory_fd::Device,
    semaphore_fd: ash::khr::external_semaphore_fd::Device,
    dma_buf: bool,
}

impl Context {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        // SAFETY: as_hal の間だけ借り、持ち出すのはデバイスと同じだけ生きる ash のハンドルだけ。
        unsafe {
            let hal = device
                .as_hal::<Vulkan>()
                .ok_or("レンダリングする GPU が Vulkan でない")?;
            let enabled = hal.enabled_device_extensions();
            for name in [
                ash::khr::external_memory_fd::NAME,
                ash::khr::external_semaphore_fd::NAME,
            ] {
                if !enabled.contains(&name) {
                    return Err(format!(
                        "レンダリングする GPU のデバイスに {} が無い",
                        name.to_string_lossy()
                    ));
                }
            }
            let raw = hal.raw_device().clone();
            let instance = hal.shared_instance().raw_instance().clone();
            Ok(Self {
                memory_fd: ash::khr::external_memory_fd::Device::new(&instance, &raw),
                semaphore_fd: ash::khr::external_semaphore_fd::Device::new(&instance, &raw),
                dma_buf: enabled.contains(&ash::ext::external_memory_dma_buf::NAME),
                physical: hal.raw_physical_device(),
                raw,
                instance,
            })
        }
    }

    /// CUDA の `cuDeviceGetUuid` と突き合わせる。
    pub fn uuid(&self) -> [u8; 16] {
        self.ids().0
    }

    /// エクスポートしたメモリをインポートできるのは、両方が同じ GPU のとき。
    pub fn ids(&self) -> ([u8; 16], [u8; 16]) {
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        // SAFETY: ローカルの構造体に書くだけ。
        unsafe {
            self.instance
                .get_physical_device_properties2(self.physical, &mut properties)
        };
        (id.device_uuid, id.driver_uuid)
    }

    #[cfg_attr(not(feature = "vaapi"), allow(dead_code))]
    pub fn has_dma_buf(&self) -> bool {
        self.dma_buf
    }

    pub fn export_buffer(
        &self,
        device: &wgpu::Device,
        size: u64,
        kind: vk::ExternalMemoryHandleTypeFlags,
        label: &str,
    ) -> Result<Exported, String> {
        self.export_buffer_for(device, size, kind, false, label)
    }

    /// `storage` なら shader の storage にもバインドできるバッファ。
    pub fn export_buffer_for(
        &self,
        device: &wgpu::Device,
        size: u64,
        kind: vk::ExternalMemoryHandleTypeFlags,
        storage: bool,
        label: &str,
    ) -> Result<Exported, String> {
        let mut external = vk::ExternalMemoryBufferCreateInfo::default().handle_types(kind);
        let mut usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
        let mut wgpu_usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        if storage {
            usage |= vk::BufferUsageFlags::STORAGE_BUFFER;
            wgpu_usage |= wgpu::BufferUsages::STORAGE;
        }
        let create = vk::BufferCreateInfo::default()
            .push_next(&mut external)
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: ローカルの記述。
        let buffer = unsafe { self.raw.create_buffer(&create, None) }
            .map_err(|e| format!("vkCreateBuffer（エクスポート用）：{e}"))?;
        let memory = match self.allocate(buffer, kind) {
            Ok(memory) => memory,
            Err(why) => {
                // SAFETY: いま作り、何もバインドしていないバッファ。
                unsafe { self.raw.destroy_buffer(buffer, None) };
                return Err(why);
            }
        };
        let raw = self.raw.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || {
            // SAFETY: wgpu がバッファを使い終えた後に一度だけ呼ばれる。
            unsafe {
                raw.destroy_buffer(buffer, None);
                raw.free_memory(memory, None);
            }
        });
        // SAFETY: このデバイスで作った、記述どおりのバッファ。メモリはコールバックまで生きる。
        let wrapped = unsafe {
            let hal = wgpu::hal::vulkan::Buffer::from_raw_externally_owned(buffer, drop_callback);
            device.create_buffer_from_hal::<Vulkan>(
                hal,
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu_usage,
                    mapped_at_creation: false,
                },
            )
        };
        // SAFETY: 読むだけ。
        let allocated = unsafe { self.raw.get_buffer_memory_requirements(buffer) }.size;
        Ok(Exported {
            buffer: wrapped,
            allocated,
            memory,
            memory_fd: self.memory_fd.clone(),
        })
    }

    fn allocate(
        &self,
        buffer: vk::Buffer,
        kind: vk::ExternalMemoryHandleTypeFlags,
    ) -> Result<vk::DeviceMemory, String> {
        // SAFETY: このデバイスのバッファ。
        let needs = unsafe { self.raw.get_buffer_memory_requirements(buffer) };
        // SAFETY: 読むだけ。
        let types = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical)
        };
        let index = (0..types.memory_type_count)
            .find(|&i| {
                needs.memory_type_bits & (1 << i) != 0
                    && types.memory_types[i as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .ok_or("エクスポートするバッファに合うメモリが無い")?;
        let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(kind);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        let allocate = vk::MemoryAllocateInfo::default()
            .push_next(&mut export)
            .push_next(&mut dedicated)
            .allocation_size(needs.size)
            .memory_type_index(index);
        // SAFETY: チェーンした構造体は生きたローカル変数。
        let memory = unsafe { self.raw.allocate_memory(&allocate, None) }
            .map_err(|e| format!("vkAllocateMemory（エクスポート用）：{e}"))?;
        // SAFETY: メモリの無いバッファに、同じデバイスで専用に確保したメモリ。
        if let Err(e) = unsafe { self.raw.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: 未使用のメモリ。
            unsafe { self.raw.free_memory(memory, None) };
            return Err(format!("vkBindBufferMemory：{e}"));
        }
        Ok(memory)
    }

    pub fn timeline(&self) -> Result<Timeline, String> {
        let mut kind = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let mut export = vk::ExportSemaphoreCreateInfo::default()
            .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
        let create = vk::SemaphoreCreateInfo::default()
            .push_next(&mut kind)
            .push_next(&mut export);
        // SAFETY: ローカルの記述。
        let semaphore = unsafe { self.raw.create_semaphore(&create, None) }
            .map_err(|e| format!("vkCreateSemaphore（エクスポート用）：{e}"))?;
        Ok(Timeline {
            raw: self.raw.clone(),
            semaphore_fd: self.semaphore_fd.clone(),
            semaphore,
            value: 0,
        })
    }
}

/// `buffer` は wgpu のコピーに使う。
pub(super) struct Exported {
    pub buffer: wgpu::Buffer,
    pub allocated: u64,
    memory: vk::DeviceMemory,
    memory_fd: ash::khr::external_memory_fd::Device,
}

impl Exported {
    pub fn fd(&self, kind: vk::ExternalMemoryHandleTypeFlags) -> Result<OwnedFd, String> {
        let info = vk::MemoryGetFdInfoKHR::default()
            .memory(self.memory)
            .handle_type(kind);
        // SAFETY: エクスポートのフラグで確保した生きたメモリ。返る記述子は新しく開いた物で、所有者はここだけ。
        unsafe {
            self.memory_fd
                .get_memory_fd(&info)
                .map(|fd| OwnedFd::from_raw_fd(fd))
        }
        .map_err(|e| format!("vkGetMemoryFdKHR：{e}"))
    }
}

pub(super) struct Timeline {
    raw: ash::Device,
    semaphore_fd: ash::khr::external_semaphore_fd::Device,
    pub semaphore: vk::Semaphore,
    pub value: u64,
}

impl Timeline {
    pub fn fd(&self) -> Result<OwnedFd, String> {
        let info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(self.semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
        // SAFETY: エクスポートのフラグで作った生きたセマフォ。返る記述子は新しく開いた物で、所有者はここだけ。
        unsafe {
            self.semaphore_fd
                .get_semaphore_fd(&info)
                .map(|fd| OwnedFd::from_raw_fd(fd))
        }
        .map_err(|e| format!("vkGetSemaphoreFdKHR：{e}"))
    }

    pub fn signal_after_next_submit(&mut self, queue: &wgpu::Queue) -> Result<u64, String> {
        self.value += 1;
        // SAFETY: as_hal の間だけ借りる。セマフォはこのデバイスの物。
        unsafe {
            let hal = queue
                .as_hal::<Vulkan>()
                .ok_or("レンダリングする GPU が Vulkan でない")?;
            hal.add_signal_semaphore(self.semaphore, Some(self.value));
        }
        Ok(self.value)
    }

    pub fn wait_before_next_submit(&self, queue: &wgpu::Queue, value: u64) -> Result<(), String> {
        // SAFETY: 同上。
        unsafe {
            let hal = queue
                .as_hal::<Vulkan>()
                .ok_or("レンダリングする GPU が Vulkan でない")?;
            hal.add_wait_semaphore(
                self.semaphore,
                Some(value),
                vk::PipelineStageFlags::ALL_COMMANDS,
            );
        }
        Ok(())
    }

    pub fn reached(&self, value: u64) -> bool {
        // SAFETY: 生きているセマフォの今の値を読むだけ。
        unsafe { self.raw.get_semaphore_counter_value(self.semaphore) }
            .is_ok_and(|now| now >= value)
    }

    /// 2 秒でタイムアウトする。もう GPU がアクセスしないと言えるなら true（値に達したか、デバイスロスト）。
    pub fn wait(&self, value: u64) -> bool {
        let semaphores = [self.semaphore];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: 生きているセマフォ。
        match unsafe { self.raw.wait_semaphores(&info, 2_000_000_000) } {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Vulkan: セマフォの待機エラー（{e}）");
                e == vk::Result::ERROR_DEVICE_LOST
            }
        }
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        // 待ち切れなかった物は、まだ GPU が使っているかもしれないので破棄しない。
        if self.wait(self.value) {
            // SAFETY: 待ち終えたセマフォを一度だけ。
            unsafe { self.raw.destroy_semaphore(self.semaphore, None) };
        }
    }
}
