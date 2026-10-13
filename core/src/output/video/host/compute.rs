// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! シェーダーで求めるオプティカルフロー（`compute`）。輝度を 4 レベルのピラミッドにし、粗いレベルから 4×4 のブロックごとに探索する。
//! ホスト側のバックエンドのパスにはパスごとの GPU タイマーが付かないので、自分で測る（[`Gauge`]）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::{Convert, Gpu, Run};

/// 最初の探索の始まり〜合成の終わり。
struct Gauge {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 空き、1 書き込みを発行した、2 読み出しを要求した、3 読める。
    state: Arc<AtomicU8>,
    calls: u64,
}

impl Gauge {
    fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        Some(Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("zw flow gauge"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zw flow gauge"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            read: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zw flow gauge"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(0)),
            calls: 0,
        })
    }

    /// 初めの何回かと、以後は 32 回に一回。
    fn wants(&mut self) -> bool {
        self.calls += 1;
        self.state.load(Ordering::Acquire) == 0
            && (self.calls <= 8 || self.calls.is_multiple_of(32))
    }

    /// 前の回に書き込ませた物をサブミットした後で読み出しを要求する。
    fn collect(&self, queue: &wgpu::Queue) -> Option<f64> {
        match self.state.load(Ordering::Acquire) {
            1 => {
                self.state.store(2, Ordering::Release);
                let state = self.state.clone();
                self.read.map_async(wgpu::MapMode::Read, .., move |result| {
                    state.store(if result.is_ok() { 3 } else { 0 }, Ordering::Release);
                });
                None
            }
            3 => {
                let ms = self.read.get_mapped_range(..).ok().and_then(|view| {
                    let a = u64::from_le_bytes(view[0..8].try_into().ok()?);
                    let b = u64::from_le_bytes(view[8..16].try_into().ok()?);
                    (b > a).then(|| (b - a) as f64 * f64::from(queue.get_timestamp_period()) / 1e6)
                });
                self.read.unmap();
                self.state.store(0, Ordering::Release);
                ms
            }
            _ => None,
        }
    }
}

const LEVELS: usize = 4;
/// そのレベルの画素。上のレベルの予測があるので下のレベルほど狭くてよい。
const RADII: [i32; LEVELS] = [1, 2, 3, 4];
/// 一番上のレベルだけ全範囲を探索する。
const STEPS: [i32; LEVELS] = [2, 2, 2, 1];

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    top: u32,
    bottom: u32,
    radius: i32,
    step: i32,
}

impl Params {
    fn bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.top.to_le_bytes());
        out[4..8].copy_from_slice(&self.bottom.to_le_bytes());
        out[8..12].copy_from_slice(&self.radius.to_le_bytes());
        out[12..16].copy_from_slice(&self.step.to_le_bytes());
        out
    }
}

pub struct Flow {
    size: [u32; 2],
    wanted: u32,
    /// 新しい入力のたびに役割を入れ替える。
    pyramid: [[wgpu::Texture; LEVELS]; 2],
    /// そのレベルの 4×4 ごと。
    flows: [[wgpu::Texture; LEVELS]; 2],
    params: [wgpu::Buffer; LEVELS],
    empty: wgpu::Texture,
    search_layout: wgpu::BindGroupLayout,
    pair_layout: wgpu::BindGroupLayout,
    search: wgpu::RenderPipeline,
    combine: wgpu::RenderPipeline,
    confidence: wgpu::RenderPipeline,
    cur: usize,
    has_prev: bool,
    gauge: Option<Gauge>,
    outside: f64,
}

fn level_size(size: [u32; 2], level: usize) -> [u32; 2] {
    [(size[0] >> level).max(1), (size[1] >> level).max(1)]
}

impl Flow {
    pub fn open(device: &wgpu::Device, size: [u32; 2], wanted: u32) -> Result<Self, String> {
        let texture = |label, size: [u32; 2], format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0].max(1),
                    height: size[1].max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let both = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let pyramid = [0, 1].map(|_| {
            std::array::from_fn(|l| {
                texture(
                    "zw flow luma",
                    level_size(size, l),
                    wgpu::TextureFormat::R8Unorm,
                    both,
                )
            })
        });
        let flows = [0, 1].map(|_| {
            std::array::from_fn(|l| {
                let s = level_size(size, l);
                texture(
                    "zw flow level",
                    [s[0].div_ceil(4), s[1].div_ceil(4)],
                    wgpu::TextureFormat::Rgba16Float,
                    both,
                )
            })
        });
        let params = std::array::from_fn(|l| {
            let value = Params {
                top: u32::from(l == LEVELS - 1),
                bottom: u32::from(l == 0),
                radius: RADII[l],
                step: STEPS[l],
            };
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zw flow params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            });
            if let Ok(mut view) = buffer.slice(..).get_mapped_range_mut() {
                view.copy_from_slice(&value.bytes());
            }
            buffer.unmap();
            buffer
        });
        let empty = texture(
            "zw flow empty",
            [1, 1],
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw flow compute"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compute.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let float = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let search_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw flow search"),
            entries: &[
                entry(0, float),
                entry(1, float),
                entry(2, float),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pair_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw flow pair"),
            entries: &[entry(0, float), entry(1, float)],
        });
        let pipeline = |layout: &wgpu::BindGroupLayout, entry: &str, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("zw flow compute"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("zw flow compute"),
                        bind_group_layouts: &[Some(layout)],
                        immediate_size: 0,
                    }),
                ),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
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
        };
        let search = pipeline(
            &search_layout,
            "fs_search",
            wgpu::TextureFormat::Rgba16Float,
        );
        let combine = pipeline(&pair_layout, "fs_combine", wgpu::TextureFormat::Rgba16Float);
        let confidence = pipeline(
            &pair_layout,
            "fs_confidence",
            wgpu::TextureFormat::Rg16Float,
        );
        tracing::info!(
            "シェーダーのオプティカルフロー: {}x{}、グリッド 4（要求 {wanted}）、ピラミッド {LEVELS} レベル",
            size[0],
            size[1]
        );
        Ok(Self {
            size,
            wanted,
            pyramid,
            flows,
            params,
            empty,
            search_layout,
            pair_layout,
            search,
            combine,
            confidence,
            cur: 1,
            has_prev: false,
            gauge: Gauge::new(device),
            outside: 0.0,
        })
    }

    fn draw(
        gpu: &mut Gpu<'_>,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
        target: &wgpu::Texture,
        stamp: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zw flow compute"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes: stamp,
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn pair(&self, gpu: &Gpu<'_>, a: &wgpu::Texture, b: &wgpu::Texture) -> wgpu::BindGroup {
        let (a, b) = (
            a.create_view(&Default::default()),
            b.create_view(&Default::default()),
        );
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw flow pair"),
            layout: &self.pair_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&b),
                },
            ],
        })
    }

    /// `src` のピラミッドのブロックが `dst` のピラミッドのどこにあるか、粗いレベルから順に。
    fn direction(&self, gpu: &mut Gpu<'_>, k: usize, src: usize, dst: usize, measure: bool) {
        for level in (0..LEVELS).rev() {
            let views = [
                self.pyramid[src][level].create_view(&Default::default()),
                self.pyramid[dst][level].create_view(&Default::default()),
                if level + 1 < LEVELS {
                    self.flows[k][level + 1].create_view(&Default::default())
                } else {
                    self.empty.create_view(&Default::default())
                },
            ];
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("zw flow search"),
                layout: &self.search_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.params[level].as_entire_binding(),
                    },
                ],
            });
            let first = measure && k == 0 && level == LEVELS - 1;
            let stamp = self.gauge.as_ref().filter(|_| first).map(|gauge| {
                wgpu::RenderPassTimestampWrites {
                    query_set: &gauge.set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: None,
                }
            });
            Self::draw(gpu, &self.search, &group, &self.flows[k][level], stamp);
        }
    }
}

impl super::flow::Provider for Flow {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn wanted_grid(&self) -> u32 {
        self.wanted
    }

    fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        if !run.new_input {
            return Ok(());
        }
        if run.target.format() != wgpu::TextureFormat::Rgba16Float {
            return Err(format!(
                "オプティカルフローの出力先が rgba16float でない（{:?}）",
                run.target.format()
            ));
        }
        self.cur ^= 1;
        for level in 0..LEVELS {
            convert.luma(gpu, run.cur, &self.pyramid[self.cur][level]);
        }
        if !self.has_prev {
            self.has_prev = true;
            convert.clear(gpu, run.target);
            return Ok(());
        }
        if let Some(ms) = self
            .gauge
            .as_ref()
            .and_then(|gauge| gauge.collect(gpu.queue))
        {
            self.outside = if self.outside > 0.0 {
                self.outside * 0.8 + ms * 0.2
            } else {
                ms
            };
        }
        let measure = self.gauge.as_mut().is_some_and(Gauge::wants);
        let (prev, cur) = (self.cur ^ 1, self.cur);
        self.direction(gpu, 0, prev, cur, measure);
        self.direction(gpu, 1, cur, prev, measure);
        let group = self.pair(gpu, &self.flows[0][0], &self.flows[1][0]);
        let gauge = self.gauge.as_ref().filter(|_| measure);
        let stamp = gauge.map(|gauge| wgpu::RenderPassTimestampWrites {
            query_set: &gauge.set,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(1),
        });
        Self::draw(gpu, &self.combine, &group, run.target, stamp);
        if let Some(gauge) = gauge {
            gpu.encoder
                .resolve_query_set(&gauge.set, 0..2, &gauge.resolve, 0);
            gpu.encoder
                .copy_buffer_to_buffer(&gauge.resolve, 0, &gauge.read, 0, 16);
            gauge.state.store(1, Ordering::Release);
        }
        Ok(())
    }

    fn outside_ms(&self) -> f64 {
        self.outside
    }

    fn confidence(
        &mut self,
        gpu: &mut Gpu<'_>,
        _convert: &mut Convert,
        target: &wgpu::Texture,
    ) -> bool {
        if target.format() != wgpu::TextureFormat::Rg16Float {
            return false;
        }
        let group = self.pair(gpu, &self.flows[0][0], &self.flows[1][0]);
        Self::draw(gpu, &self.confidence, &group, target, None);
        true
    }
}
