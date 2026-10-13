// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! キャプチャの色変換を GPU で（`convert.wgsl`）。CPU で変換すると時間がかかる。

use zwcore::output::video::Space;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Signal = 0,
    Linear = 1,
    PqSdr = 2,
    HlgSdr = 3,
    ScRgbSdr = 4,
    ScRgbPq = 5,
}

const LABEL: &str = "zw capture convert";

pub(super) const SDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(super) const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

impl Mode {
    pub(super) fn to_sdr(format: wgpu::TextureFormat, space: Space) -> Self {
        use wgpu::TextureFormat as F;
        match (format, space) {
            (F::Bgra8UnormSrgb | F::Rgba8UnormSrgb, _) => Self::Linear,
            (F::Rgba16Float, _) => Self::ScRgbSdr,
            (F::Rgb10a2Unorm, Space::Pq) => Self::PqSdr,
            (F::Rgb10a2Unorm, Space::Hlg) => Self::HlgSdr,
            _ => Self::Signal,
        }
    }
}

/// 変換のレンダラー。作るのに時間がかかるので、再生スレッドが立ち上がったときにバックグラウンドで作っておく。
pub(crate) struct Converter {
    layout: wgpu::BindGroupLayout,
    sdr: wgpu::RenderPipeline,
    half: wgpu::RenderPipeline,
    params: wgpu::Buffer,
}

impl std::fmt::Debug for Converter {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Converter").finish_non_exhaustive()
    }
}

impl Converter {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        // ラベルは ASCII で（DXC は日本語のラベルで失敗し、真っ黒になる）。
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(include_str!("convert.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(LABEL),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(LABEL),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            sdr: pipeline(SDR),
            half: pipeline(HALF),
            layout,
            params,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        mode: Mode,
        white: f32,
        one: f32,
    ) {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&(mode as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&white.max(1.0).to_le_bytes());
        bytes[8..12].copy_from_slice(&one.to_le_bytes());
        queue.write_buffer(&self.params, 0, &bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(LABEL),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(if target_format == HALF {
            &self.half
        } else {
            &self.sdr
        });
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_and_pipeline_labels_are_ascii() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = Vec::new();
        let mut stack = vec![root.join("deck/src"), root.join("core/src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for kind in [
                    "ShaderModuleDescriptor",
                    "RenderPipelineDescriptor",
                    "ComputePipelineDescriptor",
                ] {
                    for (at, _) in text.match_indices(kind) {
                        let mut end = text.len().min(at + 200);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        let after = &text[at..end];
                        let Some(label) = after.split("label: Some(\"").nth(1) else {
                            continue;
                        };
                        let label = label.split('"').next().unwrap_or_default();
                        if !label.is_ascii() {
                            found.push(format!("{}: {label}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "ASCII でないラベル: {found:?}");
    }
}
