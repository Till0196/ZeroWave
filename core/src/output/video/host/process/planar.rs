// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フレームとプロトコルのレイアウト（RGB の三つのプレーン、f32 か f16）の間を GPU 上でパック・アンパックする（`planar.wgsl`）。

use zwsdk::video::process::Format;

use super::super::Gpu;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Params {
    pub width: u32,
    pub height: u32,
    pub stride_words: u32,
    pub half: u32,
}

impl Params {
    pub fn new(size: [u32; 2], stride: u32, format: Format) -> Self {
        Self {
            width: size[0],
            height: size[1],
            stride_words: stride / 4,
            half: u32::from(format == Format::F16),
        }
    }

    pub fn bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, v) in [self.width, self.height, self.stride_words, self.half]
            .into_iter()
            .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// パックするパスのワークグループの数（一つの呼び出しが 1 ワード、64 × 1 で 1 グループ）。
    pub fn pack_groups(self) -> [u32; 3] {
        [self.stride_words.div_ceil(64), 3 * self.height, 1]
    }

    /// アンパックするパスのワークグループの数（一つの呼び出しが 1 画素、16 × 16 で 1 グループ）。
    pub fn unpack_groups(self) -> [u32; 3] {
        [self.width.div_ceil(16), self.height.div_ceil(16), 1]
    }
}

pub(super) struct Planar {
    params: Params,
    uniform: wgpu::Buffer,
    pack: wgpu::ComputePipeline,
    unpack: wgpu::ComputePipeline,
}

impl Planar {
    pub fn new(device: &wgpu::Device, size: [u32; 2], stride: u32, format: Format) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw process"),
            source: wgpu::ShaderSource::Wgsl(include_str!("planar.wgsl").into()),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("zw process"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let params = Params::new(size, stride, format);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("zw process params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        if let Ok(mut view) = uniform.slice(..).get_mapped_range_mut() {
            view.copy_from_slice(&params.bytes());
        }
        uniform.unmap();
        Self {
            params,
            uniform,
            pack: pipeline("pack"),
            unpack: pipeline("unpack"),
        }
    }

    pub fn pack(&self, gpu: &mut Gpu<'_>, picture: &wgpu::Texture, into: &wgpu::Buffer) {
        let view = picture.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw process pack"),
            layout: &self.pack.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: into.as_entire_binding(),
                },
            ],
        });
        let [x, y, z] = self.params.pack_groups();
        let mut pass = gpu
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("zw process pack"),
                timestamp_writes: None,
            });
        pass.set_pipeline(&self.pack);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(x, y, z);
    }

    pub fn unpack(&self, gpu: &mut Gpu<'_>, from: &wgpu::Buffer, picture: &wgpu::Texture) {
        let view = picture.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw process unpack"),
            layout: &self.unpack.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: from.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let [x, y, z] = self.params.unpack_groups();
        let mut pass = gpu
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("zw process unpack"),
                timestamp_writes: None,
            });
        pass.set_pipeline(&self.unpack);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(x, y, z);
    }
}
