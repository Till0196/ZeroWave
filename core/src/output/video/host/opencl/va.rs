// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レンダリング用 GPU（Intel）の輝度を VA-API のサーフェスとして OpenCL の VME へ渡す（Linux、DMA-BUF 経由）。
//! VME は線形レイアウトのサーフェスを参照にできない（`CL_OUT_OF_RESOURCES`）ので、サーフェスから開いたイメージを GPU でコピーする。

use std::os::fd::AsRawFd;

use ash::vk;

use super::{Api, Handle, Int, MEM_READ_ONLY, SUCCESS, check};
use crate::decode::vaapi::va::{Display, VASurfaceID};
use crate::output::video::host::Gpu;
use crate::output::video::host::vulkan::{Context, Exported};

const CONTEXT_PLATFORM: isize = 0x1084;
const CONTEXT_INTEROP_USER_SYNC: isize = 0x1085;
const CONTEXT_VA_API_DISPLAY_INTEL: isize = 0x4097;
const INTEL: u16 = 0x8086;

type Create = unsafe extern "system" fn(Handle, u64, *mut VASurfaceID, u32, *mut Int) -> Handle;
type Enqueue =
    unsafe extern "system" fn(Handle, u32, *const Handle, u32, *const Handle, *mut Handle) -> Int;

pub(super) struct Opened {
    pub display: Display,
    vulkan: Context,
}

impl Opened {
    /// レンダリング用 GPU が Intel で、DMA-BUF をエクスポートできる Vulkan デバイスなら。
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        if std::env::var("ZW_BRIDGE").is_ok_and(|way| way == "host") {
            return Err(String::from("ZW_BRIDGE=host"));
        }
        let vulkan = Context::new(device)?;
        if !vulkan.has_dma_buf() {
            return Err(String::from(
                "レンダリング用 GPU のデバイスに VK_EXT_external_memory_dma_buf が無い",
            ));
        }
        let (node, vendor) = crate::decode::vaapi::drawing_node()
            .ok_or("レンダリング用 GPU のレンダーノードが分からない")?;
        if vendor != INTEL {
            return Err(format!("レンダリング用 GPU が Intel でない（{node}）"));
        }
        let display = Display::open(&node).map_err(|e| e.to_string())?;
        Ok(Self { display, vulkan })
    }

    pub fn properties(&self, platform: Handle) -> [isize; 7] {
        [
            CONTEXT_PLATFORM,
            platform as isize,
            CONTEXT_VA_API_DISPLAY_INTEL,
            self.display.raw() as isize,
            CONTEXT_INTEROP_USER_SYNC,
            1,
            0,
        ]
    }
}

/// レベル・スロットごとの、エクスポートしたバッファと VA のサーフェスと OpenCL のイメージ（並びはレベル × 2 + スロット）。
pub(super) struct Share {
    display: Display,
    exported: Vec<Exported>,
    surfaces: Vec<VASurfaceID>,
    images: Vec<Handle>,
    acquire: Enqueue,
    release: Enqueue,
    release_mem: unsafe extern "system" fn(Handle) -> Int,
}

impl Share {
    pub fn new(
        api: &Api,
        context: Handle,
        platform: Handle,
        opened: Opened,
        device: &wgpu::Device,
        sizes: [[usize; 2]; 2],
    ) -> Result<Self, String> {
        let find = |name: &std::ffi::CStr| {
            // SAFETY: 名前は NUL 終端。
            let found = unsafe { (api.extension)(platform, name.as_ptr()) };
            (!found.is_null())
                .then_some(found)
                .ok_or_else(|| format!("{} が無い", name.to_string_lossy()))
        };
        // SAFETY: `cl_va_api_media_sharing_intel.h` の署名。
        let (create, acquire, release): (Create, Enqueue, Enqueue) = unsafe {
            (
                std::mem::transmute::<*mut std::ffi::c_void, Create>(find(
                    c"clCreateFromVA_APIMediaSurfaceINTEL",
                )?),
                std::mem::transmute::<*mut std::ffi::c_void, Enqueue>(find(
                    c"clEnqueueAcquireVA_APIMediaSurfacesINTEL",
                )?),
                std::mem::transmute::<*mut std::ffi::c_void, Enqueue>(find(
                    c"clEnqueueReleaseVA_APIMediaSurfacesINTEL",
                )?),
            )
        };
        let mut share = Self {
            display: opened.display,
            exported: Vec::new(),
            surfaces: Vec::new(),
            images: Vec::new(),
            acquire,
            release,
            release_mem: api.release_mem,
        };
        let result = (|| {
            for size in sizes {
                let [width, height] = size.map(|v| v as u32);
                let luma = width * height;
                for _ in 0..2 {
                    let exported = opened.vulkan.export_buffer(
                        device,
                        u64::from(luma + luma / 2),
                        vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                        "zw vme va",
                    )?;
                    let fd = exported.fd(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)?;
                    let surface = share
                        .display
                        .import_nv12(
                            fd.as_raw_fd(),
                            exported.allocated as u32,
                            [width, height],
                            width,
                            luma,
                        )
                        .map_err(|e| e.to_string())?;
                    share.exported.push(exported);
                    share.surfaces.push(surface);
                    let mut id = surface;
                    let mut code = SUCCESS;
                    // SAFETY: 有効なコンテキストと、この VADisplay のサーフェス。Y プレーン（0）をイメージとして開く。
                    let image = unsafe { create(context, MEM_READ_ONLY, &mut id, 0, &mut code) };
                    check("clCreateFromVA_APIMediaSurfaceINTEL", code)?;
                    share.images.push(image);
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(why) = result {
            for &image in &share.images {
                // SAFETY: 作ったイメージを一度ずつ解放する。
                unsafe { (api.release_mem)(image) };
            }
            share.images.clear();
            return Err(why);
        }
        Ok(share)
    }

    pub fn record(&self, gpu: &mut Gpu<'_>, written: &[wgpu::Buffer; 2], slot: usize) {
        for (k, buffer) in written.iter().enumerate() {
            gpu.encoder.copy_buffer_to_buffer(
                buffer,
                0,
                &self.exported[k * 2 + slot].buffer,
                0,
                Some(buffer.size()),
            );
        }
    }

    pub fn bring(
        &self,
        api: &Api,
        queue: Handle,
        slot: usize,
        targets: [(Handle, [usize; 2]); 2],
    ) -> Result<(), String> {
        let sources = [self.images[slot], self.images[2 + slot]];
        let enqueue = |f: Enqueue, what| {
            // SAFETY: 有効なキューと、このコンテキストで開いたイメージ。
            check(what, unsafe {
                f(
                    queue,
                    2,
                    sources.as_ptr(),
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            })
        };
        enqueue(self.acquire, "clEnqueueAcquireVA_APIMediaSurfacesINTEL")?;
        let mut copied = Ok(());
        for (source, (target, size)) in sources.into_iter().zip(targets) {
            let origin = [0usize; 3];
            let region = [size[0], size[1], 1];
            // SAFETY: 有効なキューとイメージ。大きさはどちらのイメージにも合う。インオーダーのキュー。
            copied = copied.and_then(|()| {
                check("clEnqueueCopyImage", unsafe {
                    (api.copy_image)(
                        queue,
                        source,
                        target,
                        origin.as_ptr(),
                        origin.as_ptr(),
                        region.as_ptr(),
                        0,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                    )
                })
            });
        }
        let released = enqueue(self.release, "clEnqueueReleaseVA_APIMediaSurfacesINTEL");
        copied.and(released)
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        for &image in &self.images {
            // SAFETY: 作ったイメージを一度ずつ解放する。
            unsafe { (self.release_mem)(image) };
        }
        self.display.destroy_surfaces(&mut self.surfaces);
    }
}
