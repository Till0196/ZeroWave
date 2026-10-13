// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `vulkan` の転送方式（Linux）：エクスポートしたメモリ（`OPAQUE_FD`）とタイムラインセマフォ二つ。
//! fd はメッセージに `SCM_RIGHTS` で付けて渡す。

use std::os::fd::AsFd;

use ash::vk;
use zwsdk::video::process::{self as wire, Format};

use super::super::Gpu;
use super::super::vulkan::{Context, Exported, Timeline};
use super::{Handed, Link};

const KIND: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD;

/// GPU とドライバーの UUID（Vulkan のデバイスで、エクスポートの拡張があれば）。
pub(super) fn id(device: &wgpu::Device) -> Option<Vec<u8>> {
    let context = Context::new(device).ok()?;
    let (gpu, driver) = context.ids();
    let mut id = gpu.to_vec();
    id.extend_from_slice(&driver);
    Some(id)
}

pub(super) struct Vulkan {
    inputs: Vec<Exported>,
    middles: Vec<Exported>,
    stride: u32,
    /// レンダリング側が上げて相手が待つ（書き込み終えた）、相手が上げてレンダリング側が確認する（書き終えた）。
    ready: Timeline,
    done: Timeline,
}

impl Vulkan {
    pub fn new(
        device: &wgpu::Device,
        format: Format,
        size: [u32; 2],
        inputs: usize,
        middles: usize,
    ) -> Result<Self, String> {
        let context = Context::new(device)?;
        let stride = wire::stride(size[0], format);
        let bytes = wire::picture_bytes(size[1], stride);
        let export = |count: usize, label| -> Result<Vec<Exported>, String> {
            (0..count)
                .map(|_| context.export_buffer_for(device, bytes, KIND, true, label))
                .collect()
        };
        Ok(Self {
            inputs: export(inputs, "zw process input")?,
            middles: export(middles, "zw process mid")?,
            stride,
            ready: context.timeline()?,
            done: context.timeline()?,
        })
    }
}

impl super::Way for Vulkan {
    fn stride(&self) -> u32 {
        self.stride
    }

    fn input(&self, slot: usize) -> &wgpu::Buffer {
        &self.inputs[slot].buffer
    }

    fn output(&self, slot: usize) -> &wgpu::Buffer {
        &self.middles[slot].buffer
    }

    fn share(&mut self, link: &mut Link) -> Result<(), String> {
        for (what, buffers) in [(wire::INPUT, &self.inputs), (wire::MIDDLE, &self.middles)] {
            for (index, buffer) in (0..).zip(buffers) {
                let fd = buffer.fd(KIND)?;
                link.share_fd(what, index, self.stride, buffer.allocated, fd.as_fd())?;
            }
        }
        for (what, timeline) in [(wire::READY, &self.ready), (wire::DONE, &self.done)] {
            let fd = timeline.fd()?;
            link.share_fd(what, 0, 0, 0, fd.as_fd())?;
        }
        Ok(())
    }

    fn hand_over(&mut self, gpu: &mut Gpu<'_>, _packed: &[usize]) -> Result<Handed, String> {
        let value = self.ready.signal_after_next_submit(gpu.queue)?;
        gpu.flush();
        Ok(Handed { value, after: None })
    }

    fn done(&self, value: u64) -> Option<bool> {
        Some(self.done.reached(value))
    }

    fn take_back(&mut self, gpu: &mut Gpu<'_>, value: u64, _slot: usize) -> Result<(), String> {
        self.done.wait_before_next_submit(gpu.queue, value)
    }
}
