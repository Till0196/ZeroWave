// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィールドマッチの GPU 側。c か p かを、同じサブミットの中で
//! レンダリングするシェーダーがコーミングの数を読んで選ぶので CPU を待たない。数は遅れて [`Judge`] へ。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::super::fieldmatch::{Counts, Judge, MOTION, STRIPE_SHIFT};
use super::super::shader::{self, Shape};

/// 一度に要求しておける読み戻しの数。溢れたら数えるだけで戻さない。
const IN_FLIGHT: usize = 8;

const WORDS: usize = 8;

struct Flight {
    done: Arc<AtomicU8>,
    which: usize,
    index: u64,
}

pub struct Matcher {
    compute: wgpu::ComputePipeline,
    compute_layout: wgpu::BindGroupLayout,
    pub render: wgpu::RenderPipeline,
    pub layout: wgpu::BindGroupLayout,
    /// c と p のコーミングの画素の数。コンピュートのパスが書き、レンダリングのパスが読む。
    pub combed: wgpu::Buffer,
    comb_params: wgpu::Buffer,
    staging: Vec<wgpu::Buffer>,
    flights: VecDeque<Flight>,
    next: usize,
    to_map: Option<(usize, u64)>,
    written_row: Option<u32>,
    judge: Judge,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Matcher({}x{}, matching {})",
            self.width,
            self.height,
            self.judge.matching()
        )
    }
}

impl Matcher {
    pub fn new(
        device: &wgpu::Device,
        depth: u8,
        shape: Shape,
        width: u32,
        height: u32,
        planes: &[wgpu::BindGroupLayoutEntry],
        target: wgpu::TextureFormat,
    ) -> Self {
        let uint = depth > 8 && shape == Shape::Planar;
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: if uint {
                    wgpu::TextureSampleType::Uint
                } else {
                    wgpu::TextureSampleType::Float { filterable: false }
                },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("comb"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("comb"),
            source: wgpu::ShaderSource::Wgsl(shader::comb(depth, shape).into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("comb"),
            bind_group_layouts: &[Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("comb"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = planes.to_vec();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: shape.bindings() + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("match"),
            entries: &entries,
        });
        let render = super::sheet::build(
            device,
            &layout,
            shader::matched(depth, shape),
            target,
            None,
            "match",
        );
        let combed = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("combed"),
            size: (WORDS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let comb_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("comb params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let staging = (0..IN_FLIGHT)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("combed staging"),
                    size: (WORDS * 4) as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            compute,
            compute_layout,
            render,
            layout,
            combed,
            comb_params,
            staging,
            flights: VecDeque::new(),
            next: 0,
            to_map: None,
            written_row: None,
            judge: Judge::new(width, height),
            width,
            height,
        }
    }

    /// 画素ごとの補間に使う縞のしきい値。
    pub const STRIPE_DEINT: f32 = 3.0 / 255.0;
    pub const MOTION: f32 = MOTION;

    pub fn judge(&self) -> &Judge {
        &self.judge
    }

    pub fn judge_mut(&mut self) -> &mut Judge {
        &mut self.judge
    }

    /// コーミングを数えるパスをレンダリングのパスの前に記録する。サブミットの後に [`Matcher::finish`] を呼ぶ。
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        cur: &wgpu::TextureView,
        before: &wgpu::TextureView,
        first_row: u32,
        index: u64,
    ) {
        if self.written_row != Some(first_row) {
            let words = [
                self.width,
                self.height,
                first_row,
                STRIPE_SHIFT.to_bits(),
                MOTION.to_bits(),
                0,
                0,
                0,
            ];
            let mut bytes = [0u8; 32];
            for (at, word) in words.into_iter().enumerate() {
                bytes[at * 4..at * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            queue.write_buffer(&self.comb_params, 0, &bytes);
            self.written_row = Some(first_row);
        }
        queue.write_buffer(&self.combed, 0, &[0u8; WORDS * 4]);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("comb"),
            layout: &self.compute_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(cur),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(before),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.combed.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.comb_params.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("comb"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                self.width.div_ceil(16),
                self.height.div_ceil(2).div_ceil(8),
                1,
            );
        }
        if self.flights.len() < IN_FLIGHT && self.to_map.is_none() {
            let which = self.next;
            self.next = (self.next + 1) % IN_FLIGHT;
            encoder.copy_buffer_to_buffer(
                &self.combed,
                0,
                &self.staging[which],
                0,
                (WORDS * 4) as u64,
            );
            self.to_map = Some((which, index));
        }
    }

    /// サブミットの後に読み戻しを要求し、届いた数を判定に足す（待たない）。film と 60i が替わったら true。
    pub fn finish(&mut self, device: &wgpu::Device) -> bool {
        self.submitted();
        self.take(device)
    }

    // サブミットの前には map を要求できない。
    fn submitted(&mut self) {
        let Some((which, index)) = self.to_map.take() else {
            return;
        };
        let done = Arc::new(AtomicU8::new(0));
        let flag = done.clone();
        self.staging[which].map_async(wgpu::MapMode::Read, .., move |result| {
            flag.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
        self.flights.push_back(Flight { done, which, index });
    }

    fn take(&mut self, device: &wgpu::Device) -> bool {
        if self.flights.is_empty() {
            return false;
        }
        device.poll(wgpu::PollType::Poll).ok();
        let mut changed = false;
        while let Some(flight) = self.flights.front() {
            let state = flight.done.load(Ordering::Acquire);
            if state == 0 {
                break;
            }
            let flight = self.flights.pop_front().expect("just seen");
            let staging = &self.staging[flight.which];
            if state != 1 {
                continue;
            }
            let read = staging.get_mapped_range(..).ok().map(|data| {
                let word = |at: usize| {
                    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
                };
                Counts {
                    c: word(0),
                    p: word(4),
                    moving: word(8),
                    first: word(12),
                    other: word(16),
                }
            });
            staging.unmap();
            if let Some(counts) = read {
                changed ^= self.judge.push(flight.index, counts);
            }
        }
        changed
    }
}
