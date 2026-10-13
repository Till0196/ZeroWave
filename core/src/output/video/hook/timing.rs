// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィルターのパスごとの時間を GPU のタイムスタンプ（`timestamp_writes`）で測る。読み出しは待たず、
//! 読み出し用のバッファが空いていないフレームは測らない。`TIMESTAMP_QUERY` が無ければ作らない。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(2);

/// 掛け始めてからこの間は重さの判定に使わない（クロックが上がりきらず重く出る）。
const WARM: Duration = Duration::from_secs(3);

const READERS: usize = 3;

const FREE: u8 = 0;
const ENCODED: u8 = 1;
const MAPPING: u8 = 2;
const READY: u8 = 3;

struct Reader {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

pub(super) struct Timing {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readers: Vec<Reader>,
    passes: usize,
    period: f64,
    writing: Option<usize>,
    samples: VecDeque<(Instant, Vec<f64>)>,
    born: Instant,
    lead: Lead,
}

/// フィルターの開始を取る小さなパス（Apple の GPU は開始の時刻を入力待ちの前に書くので終了の時刻を使う）。
struct Lead {
    float: (wgpu::RenderPipeline, wgpu::BindGroupLayout),
    uint: (wgpu::RenderPipeline, wgpu::BindGroupLayout),
    target: wgpu::TextureView,
}

impl Lead {
    fn new(device: &wgpu::Device) -> Self {
        let target = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("zw timing lead"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            float: Self::pipeline(
                device,
                wgpu::TextureSampleType::Float { filterable: false },
                "f32",
                "x",
            ),
            uint: Self::pipeline(device, wgpu::TextureSampleType::Uint, "u32", "f32(x)"),
            target,
        }
    }

    /// レンダーパスにする（Apple の GPU はレンダリングと compute を別に流すので compute では捉えられない）。
    fn pipeline(
        device: &wgpu::Device,
        sample: wgpu::TextureSampleType,
        texel: &str,
        cast: &str,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw timing lead"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: sample,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let source = format!(
            "@group(0) @binding(0) var src: texture_2d<{texel}>;\n\
             @group(0) @binding(1) var after: texture_2d<f32>;\n\
             @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{\n\
                 let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));\n\
                 return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);\n\
             }}\n\
             @fragment fn fs() -> @location(0) vec4<f32> {{\n\
                 let x = textureLoad(src, vec2<i32>(0, 0), 0).r;\n\
                 let y = textureLoad(after, vec2<i32>(0, 0), 0).r;\n\
                 return vec4<f32>({cast}, y, 0.0, 1.0);\n\
             }}\n"
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw timing lead"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("zw timing lead"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("zw timing lead"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        (pipeline, layout)
    }
}

impl std::fmt::Debug for Timing {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Timing")
            .field("passes", &self.passes)
            .field("samples", &self.samples.len())
            .finish_non_exhaustive()
    }
}

impl Timing {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, passes: usize) -> Option<Self> {
        if passes == 0 || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let count = ((passes + 1) * 2) as u32;
        let size = u64::from(count) * 8;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("zw timing"),
            ty: wgpu::QueryType::Timestamp,
            count,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("zw timing resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readers = (0..READERS)
            .map(|_| Reader {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("zw timing read"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        Some(Self {
            set,
            resolve,
            readers,
            passes,
            period: f64::from(queue.get_timestamp_period()),
            writing: None,
            samples: VecDeque::new(),
            born: Instant::now(),
            lead: Lead::new(device),
        })
    }

    pub(super) fn begin(&mut self) -> bool {
        self.collect();
        self.writing = self
            .readers
            .iter()
            .position(|reader| reader.state.load(Ordering::Acquire) == FREE);
        self.writing.is_some()
    }

    pub(super) fn render_writes(&self, i: usize) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.writing?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some((i * 2 + 2) as u32),
            end_of_pass_write_index: Some((i * 2 + 3) as u32),
        })
    }

    pub(super) fn compute_writes(&self, i: usize) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        self.writing?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some((i * 2 + 2) as u32),
            end_of_pass_write_index: Some((i * 2 + 3) as u32),
        })
    }

    /// `after` も読んで待つので、前のフィルターと重なった間がフィルターの最初のパスに乗らない。
    pub(super) fn lead(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        uint: bool,
        after: &wgpu::TextureView,
    ) {
        if self.writing.is_none() {
            return;
        }
        let (pipeline, layout) = if uint {
            &self.lead.uint
        } else {
            &self.lead.float
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw timing lead"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(after),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zw timing lead"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.lead.target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                query_set: &self.set,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(super) fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(at) = self.writing.take() else {
            return;
        };
        let count = ((self.passes + 1) * 2) as u32;
        encoder.resolve_query_set(&self.set, 0..count, &self.resolve, 0);
        let reader = &self.readers[at];
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &reader.buffer, 0, u64::from(count) * 8);
        reader.state.store(ENCODED, Ordering::Release);
    }

    /// 送ったあとで読み出しを要求する（送る前だと wgpu がエラーにする）。
    pub(super) fn submitted(&self) {
        for reader in &self.readers {
            if reader.state.load(Ordering::Acquire) != ENCODED {
                continue;
            }
            reader.state.store(MAPPING, Ordering::Release);
            let state = Arc::clone(&reader.state);
            reader
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    state.store(if result.is_ok() { READY } else { FREE }, Ordering::Release);
                });
        }
    }

    fn collect(&mut self) {
        let now = Instant::now();
        for reader in &self.readers {
            if reader.state.load(Ordering::Acquire) != READY {
                continue;
            }
            let ms = {
                let Ok(view) = reader.buffer.slice(..).get_mapped_range() else {
                    reader.state.store(FREE, Ordering::Release);
                    continue;
                };
                let stamps: Vec<u64> = view
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| u64::from_le_bytes(*b))
                    .collect();
                let lead_end = stamps.get(1).copied().unwrap_or(0);
                pass_times(stamps.get(2..).unwrap_or_default(), lead_end, self.period)
            };
            reader.buffer.unmap();
            reader.state.store(FREE, Ordering::Release);
            if ms.iter().all(|m| m.is_finite() && *m < 1000.0) {
                self.samples.push_back((now, ms));
            }
        }
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > WINDOW)
        {
            self.samples.pop_front();
        }
    }

    pub(super) fn settled(&self, count: usize) -> Option<f64> {
        let from = self.born + WARM;
        let mut totals: Vec<f64> = self
            .samples
            .iter()
            .filter(|(at, _)| *at >= from)
            .map(|(_, ms)| ms.iter().sum())
            .collect();
        if totals.len() < count {
            return None;
        }
        totals.sort_by(f64::total_cmp);
        Some(totals[totals.len() / 2])
    }

    pub(super) fn restart(&mut self) {
        self.born = Instant::now();
        self.samples.clear();
    }

    pub(super) fn summary(&self) -> Option<(f64, f64, Vec<f64>)> {
        if self.samples.is_empty() {
            return None;
        }
        let n = self.samples.len() as f64;
        let totals: Vec<f64> = self.samples.iter().map(|(_, ms)| ms.iter().sum()).collect();
        let mean = totals.iter().sum::<f64>() / n;
        let max = totals.iter().copied().fold(0.0, f64::max);
        let mut each = vec![0.0; self.passes];
        for (_, ms) in &self.samples {
            for (sum, m) in each.iter_mut().zip(ms) {
                *sum += m / n;
            }
        }
        Some((mean, max, each))
    }
}

/// パスごとのミリ秒。時間は「終了 −（開始と前の終了の遅い方）」（Apple の GPU は続くパスを
/// 重ねて実行し、開始を早く書くため）。
fn pass_times(stamps: &[u64], start: u64, period: f64) -> Vec<f64> {
    let mut previous_end = start;
    stamps
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[begin, end]| {
            let from = (*begin).max(previous_end);
            previous_end = previous_end.max(*end);
            end.saturating_sub(from) as f64 * period / 1e6
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pass_times;

    #[test]
    fn sequential_passes_keep_their_own_length() {
        let ms = pass_times(&[0, 1_000_000, 1_500_000, 3_500_000], 0, 1.0);
        assert_eq!(ms, vec![1.0, 2.0]);
    }

    #[test]
    fn overlapping_passes_are_not_counted_twice() {
        let ms = pass_times(&[0, 1_000_000, 100, 1_700_000, 200, 2_400_000], 0, 1.0);
        assert_eq!(ms, vec![1.0, 0.7, 0.7]);
        assert!((ms.iter().sum::<f64>() - 2.4).abs() < 1e-9);
    }

    #[test]
    fn the_wait_before_the_first_pass_is_not_counted() {
        let ms = pass_times(&[0, 3_500_000, 3_500_000, 4_000_000], 3_000_000, 1.0);
        assert_eq!(ms, vec![0.5, 0.5]);
    }
}
