// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 大きく縮めるときの前段：画素を残らず平均して 2ᵏ 分の 1 にする。
//! blit の縮小は二倍を超えると画素を読み飛ばしてちらつくので、残りを二倍以内にする。

const SHADER: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> factor: vec4<u32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let f = factor.x;
    let half = f / 2u;
    let corner = floor(at.xy) * f32(f);
    var sum = vec4<f32>(0.0);
    for (var j = 0u; j < half; j++) {
        for (var i = 0u; i < half; i++) {
            // 四画素の角（2i+1, 2j+1）を線形補間で読めば、二×二の平均。
            let p = corner + vec2<f32>(f32(2u * i + 1u), f32(2u * j + 1u));
            sum += textureSampleLevel(source, smp, p / size, 0.0);
        }
    }
    return sum / f32(half * half);
}
"#;

#[derive(Debug)]
pub(super) struct Reduce {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    factor: wgpu::Buffer,
    module: wgpu::ShaderModule,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    target: Option<(
        wgpu::TextureFormat,
        [u32; 2],
        wgpu::Texture,
        wgpu::TextureView,
    )>,
}

impl Reduce {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reduce"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reduce"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let factor = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reduce"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reduce"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        Self {
            layout,
            sampler,
            factor,
            module,
            pipelines: Vec::new(),
            target: None,
        }
    }

    /// 縮小率が 2 未満になる 2ᵏ。縮小率が 2 以下なら `None`。
    pub(super) fn factor(source: [u32; 2], shown: [f32; 2]) -> Option<u32> {
        let ratio =
            (source[0] as f32 / shown[0].max(1.0)).max(source[1] as f32 / shown[1].max(1.0));
        if ratio <= 2.0 {
            return None;
        }
        let k = ratio.log2().floor() as u32;
        Some(1u32 << k.clamp(1, 5))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        factor: u32,
    ) -> (wgpu::TextureView, [u32; 2]) {
        let out = [
            size[0].div_ceil(factor).max(1),
            size[1].div_ceil(factor).max(1),
        ];
        let reuse = self
            .target
            .as_ref()
            .is_some_and(|(f, s, _, _)| *f == format && *s == out);
        if !reuse {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("reduced"),
                size: wgpu::Extent3d {
                    width: out[0],
                    height: out[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.target = Some((format, out, texture, view));
        }
        if !self.pipelines.iter().any(|(f, _)| *f == format) {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("reduce"),
                bind_group_layouts: &[Some(&self.layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("reduce"),
                layout: Some(&pipeline_layout),
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
            });
            self.pipelines.push((format, pipeline));
        }
        let mut words = [0u8; 16];
        words[..4].copy_from_slice(&factor.to_le_bytes());
        queue.write_buffer(&self.factor, 0, &words);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reduce"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.factor.as_entire_binding(),
                },
            ],
        });
        let (_, _, _, view) = self.target.as_ref().expect("just made");
        let pipeline = &self
            .pipelines
            .iter()
            .find(|(f, _)| *f == format)
            .expect("just made")
            .1;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reduce"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        (view.clone(), out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_big_reduction_needs_the_first_step() {
        assert_eq!(Reduce::factor([1920, 1080], [2560.0, 1440.0]), None, "拡大");
        assert_eq!(
            Reduce::factor([3840, 2160], [1920.0, 1080.0]),
            None,
            "二倍までは blit で"
        );
        assert_eq!(
            Reduce::factor([7680, 4320], [1920.0, 1080.0]),
            Some(4),
            "8K → FHD"
        );
        assert_eq!(
            Reduce::factor([7680, 4320], [2560.0, 1440.0]),
            Some(2),
            "8K → WQHD は三倍"
        );
        assert_eq!(Reduce::factor([3840, 2160], [960.0, 540.0]), Some(4));
        assert_eq!(Reduce::factor([7680, 4320], [640.0, 360.0]), Some(8));
    }

    #[test]
    fn the_shader_is_valid() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("読める");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("正しい");
    }
}
