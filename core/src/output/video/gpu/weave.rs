// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一枚が一フィールドのピクチャー（HEVC の `field_seq_flag`）を、二つずつ GPU で一枚のフレームにウィーブする。
//! ウィーブしたフレームはほかのインターレースのフレームと同じくデインターレースする。

const SHADER: &str = r#"
@group(0) @binding(0) var top: texture_2d<f32>;
@group(0) @binding(1) var bottom: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(at.xy);
    let row = vec2<i32>(p.x, p.y >> 1u);
    if (p.y & 1) == 0 {
        return textureLoad(top, row, 0);
    }
    return textureLoad(bottom, row, 0);
}
"#;

#[derive(Debug)]
pub(super) struct Field {
    pub(super) views: Vec<wgpu::TextureView>,
    pub(super) keep: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    pub(super) top: bool,
}

#[derive(Debug)]
pub(super) struct Weave {
    layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl Weave {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("weave"),
            entries: &[texture(0), texture(1)],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("weave"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        Self {
            layout,
            module,
            pipelines: Vec::new(),
        }
    }

    pub(super) fn run(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        top: &[wgpu::TextureView],
        bottom: &[wgpu::TextureView],
        into: &[wgpu::Texture],
    ) {
        for ((top, bottom), into) in top.iter().zip(bottom).zip(into) {
            let format = into.format();
            if !self.pipelines.iter().any(|(have, _)| *have == format) {
                let pipeline = self.pipeline(device, format);
                self.pipelines.push((format, pipeline));
            }
            let Some((_, pipeline)) = self.pipelines.iter().find(|(have, _)| *have == format)
            else {
                continue;
            };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("weave"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(top),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(bottom),
                    },
                ],
            });
            let view = into.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("weave"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn pipeline(&self, device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("weave"),
            bind_group_layouts: &[Some(&self.layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("weave"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }
}
