// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 分離可能なカーネルの縮小を、横と縦の二回に分ける（`blit.wgsl` の `PASS`）。
//! 中間テクスチャの列はサーフェスの画素の列と同じ並び。

use super::sheet::{Onto, blit_group, fit_buffer};
use crate::output::video::colour::Mapping;
use crate::output::video::shader;

pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Debug)]
pub(super) struct Split {
    fit: wgpu::Buffer,
    target: Option<([u32; 2], wgpu::TextureView)>,
}

impl Split {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            fit: fit_buffer(device),
            target: None,
        }
    }

    /// `source` を横だけ縮めた中間テクスチャを返す。`pipeline` は `PASS` が 1 のもの。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        source: &wgpu::TextureView,
        size: [u32; 2],
        onto: Onto,
        rect: [f32; 4],
        mapping: &Mapping,
    ) -> wgpu::TextureView {
        let wanted = [onto.left + onto.width, size[1]];
        if self.target.as_ref().is_none_or(|(have, _)| *have != wanted) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("split"),
                size: wgpu::Extent3d {
                    width: wanted[0],
                    height: wanted[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.target = Some((wanted, texture.create_view(&Default::default())));
        }
        let (_, view) = self.target.as_ref().expect("just made");
        // `PASS` 1 は色の前で返す。
        queue.write_buffer(
            &self.fit,
            0,
            &shader::fit_bytes([rect[0], -1.0, rect[2], 1.0], false, mapping, false),
        );
        let group = blit_group(device, layout, source, sampler, &self.fit);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("split"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                onto.left as f32,
                0.0,
                onto.width as f32,
                size[1] as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..6, 0..1);
        }
        view.clone()
    }
}
