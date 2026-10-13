// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! サーフェスへのコピー：[`Onto`]、コピーのパイプライン（`blit`）、UI のオーバーレイ（[`Sheet`]）。

use super::*;

/// サーフェスの上でフレームが占める領域（左上からの画素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Onto {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl Onto {
    pub fn all((width, height): (u32, u32)) -> Self {
        Self {
            left: 0,
            top: 0,
            width,
            height,
        }
    }
}

/// 読み込まずに黒で消す。フレームの外は黒い余白で、前のフレームが残ると比率が変わったときににじむ。
pub(super) fn attachment(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        resolve_target: None,
        depth_slice: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            store: wgpu::StoreOp::Store,
        },
    }
}

pub(super) fn blit_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blit"),
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
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

pub(super) fn display_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("display"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

pub(super) fn fit_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fit"),
        size: shader::FIT_SIZE as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(super) fn blit_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    picture: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    fit: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(picture),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: fit.as_entire_binding(),
            },
        ],
    })
}

/// 映像の上に重ねる UI のオーバーレイ（`Rgba8Unorm`、sRGB の曲線のまま乗算済み）。
/// egui はガンマの空間で混ぜる前提なので、PQ・HLG ではサーフェスのコピーを読んでリニア光で合成する（[`Over`]）。
#[derive(Debug)]
pub struct Sheet {
    pub(super) view: wgpu::TextureView,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) group: wgpu::BindGroup,
    pub(super) sampler: wgpu::Sampler,
    pub(super) fit: wgpu::Buffer,
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) size: (u32, u32),
    pub(super) target: wgpu::TextureFormat,
    pub(super) mapping: Mapping,
    /// SDR のサーフェスへは sRGB を外したビューへオーバーレイの値をそのまま合成する（egui のアンチエイリアスの前提に合わせる）。
    /// コピー先のサーフェスが sRGB の形式なら、外した物を `view_formats` に入れておくこと。
    pub(super) gamma: bool,
    pub(super) over: Option<Over>,
}

#[derive(Debug)]
pub struct Over {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    below: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup)>,
}

impl Over {
    fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let layout = over_layout(device);
        let pipeline = build_with(
            device,
            &layout,
            shader::over().to_string(),
            target,
            None,
            "over",
            "fs_over",
            &[],
        );
        Self {
            layout,
            pipeline,
            below: None,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        onto: &wgpu::Texture,
        sheet: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        fit: &wgpu::Buffer,
    ) {
        let wanted = (onto.width(), onto.height(), onto.format());
        if let Some((texture, ..)) = &self.below
            && (texture.width(), texture.height(), texture.format()) == wanted
        {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("below"),
            size: wgpu::Extent3d {
                width: wanted.0,
                height: wanted.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wanted.2,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("over"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(sheet),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: fit.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.below = Some((texture, view, group));
    }
}

fn over_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("over"),
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
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

impl Sheet {
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// `target` と `space` は重ねる先のサーフェスのもの。`white` はそこでの SDR の白。
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        space: Space,
        white: f32,
        size: (u32, u32),
    ) -> Self {
        let layout = blit_layout(device);
        let sampler = display_sampler(device);
        let fit = fit_buffer(device);
        let view = Self::paper(device, size);
        let group = blit_group(device, &layout, &view, &sampler, &fit);
        let gamma = space == Space::Srgb;
        let pipeline = if gamma {
            build(
                device,
                &layout,
                shader::blit().to_string(),
                target.remove_srgb_suffix(),
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                "sheet",
            )
        } else {
            build(
                device,
                &layout,
                shader::blit().to_string(),
                target,
                Some(wgpu::BlendState::ALPHA_BLENDING),
                "sheet",
            )
        };
        Self {
            view,
            layout,
            group,
            sampler,
            fit,
            pipeline,
            size,
            target,
            // オーバーレイは sRGB の曲線のまま。SDR のサーフェスへはそのまま、HDR のサーフェスへはアルファで割り戻して曲線を外す。
            mapping: if gamma {
                Mapping::new(Transfer::Sdr, Primaries::Bt709, space)
            } else {
                Mapping::new(Transfer::Ui, Primaries::Bt709, space).with_white(white)
            },
            gamma,
            over: matches!(space, Space::Pq | Space::Hlg).then(|| Over::new(device, target)),
        }
    }

    pub(super) fn paper(device: &wgpu::Device, (width, height): (u32, u32)) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sheet"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// UI のレンダリング先。レンダリング前に透明で消すこと（`LoadOp::Clear` に `TRANSPARENT`）。
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if size == self.size {
            return;
        }
        let view = Self::paper(device, size);
        self.group = blit_group(device, &self.layout, &view, &self.sampler, &self.fit);
        self.view = view;
        self.size = size;
        if let Some(over) = &mut self.over {
            over.below = None;
        }
    }

    /// オーバーレイをサーフェスへ重ねる。`area` は画面がある矩形で、外は重ねない（`None` ならいっぱい）。
    pub fn lay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        onto: &wgpu::Texture,
        area: Option<[u32; 4]>,
    ) {
        let [x, y, w, h] = area
            .map(|[x, y, w, h]| {
                let x = x.min(onto.width());
                let y = y.min(onto.height());
                [x, y, w.min(onto.width() - x), h.min(onto.height() - y)]
            })
            .unwrap_or([0, 0, onto.width(), onto.height()]);
        if w == 0 || h == 0 {
            return;
        }
        queue.write_buffer(
            &self.fit,
            0,
            &shader::fit_bytes(
                [-1.0, -1.0, 1.0, 1.0],
                self.target.is_srgb() && !self.gamma,
                &self.mapping,
                false,
            ),
        );
        let view = onto.create_view(&wgpu::TextureViewDescriptor {
            format: self.gamma.then(|| self.target.remove_srgb_suffix()),
            ..Default::default()
        });
        let over = self
            .over
            .as_mut()
            .filter(|_| onto.usage().contains(wgpu::TextureUsages::COPY_SRC));
        let (pipeline, group) = match over {
            Some(over) => {
                over.prepare(device, onto, &self.view, &self.sampler, &self.fit);
                let (below, _, group) = over.below.as_ref().expect("just prepared");
                let origin = wgpu::Origin3d { x, y, z: 0 };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin,
                        ..onto.as_image_copy()
                    },
                    wgpu::TexelCopyTextureInfo {
                        origin,
                        ..below.as_image_copy()
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                (&over.pipeline, group)
            }
            None => (&self.pipeline, &self.group),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sheet"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..6, 0..1);
    }
}

pub(super) fn build(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: String,
    target: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    label: &str,
) -> wgpu::RenderPipeline {
    build_with(device, layout, source, target, blend, label, "fs", &[])
}

pub(super) fn build_specialised(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: String,
    target: wgpu::TextureFormat,
    label: &str,
    constants: &[(&str, f64)],
) -> wgpu::RenderPipeline {
    build_with(device, layout, source, target, None, label, "fs", constants)
}

#[allow(clippy::too_many_arguments)]
fn build_with(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: String,
    target: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    label: &str,
    fragment: &str,
    constants: &[(&str, f64)],
) -> wgpu::RenderPipeline {
    let descriptor = wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    };
    // ループの境界チェックは入れない（あると Metal がループを展開できない）。
    let checks = wgpu::ShaderRuntimeChecks {
        force_loop_bounding: false,
        ..wgpu::ShaderRuntimeChecks::checked()
    };
    // SAFETY: 本体のシェーダーだけで、ループはどれも有限回で終わる（プラグインは通らない）。
    let module = unsafe { device.create_shader_module_trusted(descriptor, checks) };
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fragment),
            targets: &[Some(wgpu::ColorTargetState {
                format: target,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions {
                constants,
                ..Default::default()
            },
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
