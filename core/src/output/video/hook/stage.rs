// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィルターの実行：テクスチャ、リングバッファ、毎フレームのコマンドの記録、サンプラー。

use super::*;

impl Tex {
    pub(super) fn new(
        device: &wgpu::Device,
        label: &str,
        size: &[u32],
        format: wgpu::TextureFormat,
        linear: bool,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let (dimension, view_dimension, extent) = match size {
            [w] => (
                wgpu::TextureDimension::D1,
                wgpu::TextureViewDimension::D1,
                [*w, 1, 1],
            ),
            [w, h] => (
                wgpu::TextureDimension::D2,
                wgpu::TextureViewDimension::D2,
                [*w, *h, 1],
            ),
            [w, h, d, ..] => (
                wgpu::TextureDimension::D3,
                wgpu::TextureViewDimension::D3,
                [*w, *h, *d],
            ),
            [] => (
                wgpu::TextureDimension::D2,
                wgpu::TextureViewDimension::D2,
                [1, 1, 1],
            ),
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: extent[0],
                height: extent[1],
                depth_or_array_layers: extent[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let filterable = format
            .guaranteed_format_features(device.features())
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE);
        Self {
            texture,
            view,
            format,
            size: extent,
            dimension: view_dimension,
            linear: linear && filterable,
        }
    }
}

impl Ring {
    pub(super) fn new(texs: Vec<Tex>) -> Self {
        Self { texs, head: 0 }
    }

    fn advance(&mut self) {
        self.head = (self.head + 1) % self.texs.len();
    }

    pub(super) fn current(&self) -> &Tex {
        &self.texs[self.head]
    }

    fn back(&self, k: u32) -> &Tex {
        let len = self.texs.len();
        &self.texs[(self.head + len - (k as usize % len)) % len]
    }
}

impl Stage {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 測り終えていれば 1 フレームの時間（ミリ秒、GPU の時刻で）。
    pub(super) fn settled(&self) -> Option<f64> {
        if let Some(timing) = &self.timing {
            return timing.settled(SETTLE);
        }
        let times = self.measured.as_ref()?;
        if times.len() < MEASURE {
            return None;
        }
        Some(times[1..].iter().copied().fold(f64::INFINITY, f64::min))
    }

    pub(super) fn has_host(&self) -> bool {
        self.passes.iter().any(|pass| pass.job.is_some())
    }

    pub(super) fn interpolates(&self) -> bool {
        !self.host_failed
            && self
                .passes
                .iter()
                .any(|pass| pass.job == Some(super::host::Job::Interpolate))
    }

    pub(super) fn rejudge(&mut self) {
        self.judging = true;
        match self.timing.as_mut() {
            Some(timing) => timing.restart(),
            None => self.measured = Some(Vec::new()),
        }
    }

    pub(super) fn pass_times(&self) -> Vec<(String, f64)> {
        self.timing
            .as_ref()
            .and_then(timing::Timing::summary)
            .map(|(_, _, each)| {
                self.passes
                    .iter()
                    .filter(|pass| pass.slot.is_some())
                    .map(|pass| pass.name.clone())
                    .zip(each)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn outputs(&self) -> u32 {
        self.graph.outputs()
    }

    pub fn output_size(&self) -> [u32; 2] {
        self.graph.output
    }

    pub fn frames(&self) -> [i32; 2] {
        self.graph.frames
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        ctx: &Ctx<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &Input<'_>,
        frame: &Frame,
        frames: Option<&wgpu::BindGroup>,
        samplers: &Samplers,
        lead: Option<(&wgpu::TextureView, bool)>,
        after: Option<&wgpu::TextureView>,
        host: &mut super::host::Host,
    ) -> wgpu::TextureView {
        let device = ctx.device;
        if frame.new_input
            && let Some(inputs) = self.inputs.as_mut()
        {
            inputs.advance();
            let to = inputs.current();
            if to.size[0] == input.size[0] && to.size[1] == input.size[1] {
                encoder.copy_texture_to_texture(
                    input.texture.as_image_copy(),
                    to.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: input.size[0],
                        height: input.size[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        // 一つ前の内容を `名前:1` へコピーしてから、今の入力でレンダリングする。
        if frame.new_input {
            for name in &self.previous {
                let (Some(now), Some(back)) = (
                    self.textures.get(name),
                    self.textures.get(&format!("{name}:1")),
                ) else {
                    continue;
                };
                encoder.copy_texture_to_texture(
                    now.texture.as_image_copy(),
                    back.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: now.size[0],
                        height: now.size[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        self.outputs.advance();
        // 時刻は新しい入力の回だけで測る（二回目からは飛ばすパスがあり、タイムスタンプが書かれない）。
        let timed = frame.new_input && self.timing.as_mut().is_some_and(timing::Timing::begin);
        if timed && let Some(timing) = self.timing.as_ref() {
            let (view, uint) = lead.unwrap_or((input.view, false));
            let after = after.unwrap_or(&self.outputs.back(1).view);
            timing.lead(device, encoder, view, uint, after);
        }
        let from = if frame.new_input {
            0
        } else {
            self.graph.retime_from
        };
        let mut host_failed: Option<String> = None;
        let mut index = from;
        while index < self.passes.len() {
            let pass = &self.passes[index];
            if let Some(job) = pass.job {
                let texture_of = |name: &str| -> &wgpu::Texture {
                    match Source::of(name) {
                        Source::Picture(0) => input.texture,
                        Source::Picture(k) => self
                            .inputs
                            .as_ref()
                            .map_or(input.texture, |ring| &ring.back(k).texture),
                        Source::Output(k) => &self.outputs.back(k).texture,
                        Source::Named(name) => &self.textures[name].texture,
                    }
                };
                let target = if pass.writes == "output" {
                    &self.outputs.current().texture
                } else {
                    &self.textures[&pass.writes].texture
                };
                let run = super::host::Run {
                    prev: texture_of(&pass.reads[0]),
                    cur: texture_of(&pass.reads[1]),
                    target,
                    phase: frame.zw.phase,
                    new_input: frame.new_input,
                    who: pass.who.as_deref(),
                    process: pass.process.as_ref(),
                    held: pass.held,
                };
                let mut gpu = super::host::Gpu {
                    device,
                    queue: ctx.queue,
                    encoder: &mut *encoder,
                };
                if self.host_failed {
                    host.fallback(&mut gpu, job, &run);
                } else if let Err(why) = host.run(&mut gpu, job, &run) {
                    host.fallback(&mut gpu, job, &run);
                    host_failed = Some(why);
                }
                // ホスト側の処理は wgpu の外なので、タイムスタンプはそのあとの空のパスで書く。
                if timed
                    && let Some(writes) = self
                        .timing
                        .as_ref()
                        .zip(pass.slot)
                        .and_then(|(t, slot)| t.compute_writes(slot))
                {
                    let _ = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(&pass.name),
                        timestamp_writes: Some(writes),
                    });
                }
                index += 1;
                continue;
            }
            // ワークグループの数を指定した compute が続く間は、一つの compute パスに記録する。
            let mut end = index;
            while self.passes[end].slot.is_none() && end + 1 < self.passes.len() {
                end += 1;
            }
            let mut groups = Vec::with_capacity(end + 1 - index);
            for at in index..=end {
                let pass = &self.passes[at];
                if pass.uses_zw {
                    let mut zw = frame.zw;
                    zw.input_size = [self.input_size[0] as f32, self.input_size[1] as f32];
                    zw.output_size = [pass.target[0] as f32, pass.target[1] as f32];
                    ctx.queue.write_buffer(&pass.zw, 0, &zw.bytes());
                }
                let cached = pass.fixed.clone();
                let group = match cached {
                    Some(group) => group,
                    None => {
                        let group = self.group_of(device, at, input, samplers);
                        if self.fixed(at) {
                            self.passes[at].fixed = Some(group.clone());
                        }
                        group
                    }
                };
                groups.push(group);
            }
            let pass = &self.passes[end];
            let target = if pass.writes == "output" || pass.writes.is_empty() {
                &self.outputs.current().view
            } else {
                &self.textures[&pass.writes].view
            };
            let slot = pass.slot;
            let group1 = frames.or(self.empty.as_ref());
            match (&pass.render, &pass.compute) {
                (Some(pipeline), _) => {
                    let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(&pass.name),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        timestamp_writes: if timed {
                            self.timing
                                .as_ref()
                                .zip(slot)
                                .and_then(|(t, slot)| t.render_writes(slot))
                        } else {
                            None
                        },
                        ..Default::default()
                    });
                    render.set_pipeline(pipeline);
                    render.set_bind_group(0, &groups[0], &[]);
                    if let Some(frames) = frames {
                        render.set_bind_group(1, frames, &[]);
                    }
                    render.draw(0..3, 0..1);
                }
                (None, Some(_)) => {
                    let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(&pass.name),
                        timestamp_writes: if timed {
                            self.timing
                                .as_ref()
                                .zip(slot)
                                .and_then(|(t, slot)| t.compute_writes(slot))
                        } else {
                            None
                        },
                    });
                    for (at, group) in (index..=end).zip(&groups) {
                        let pass = &self.passes[at];
                        let Some(pipeline) = &pass.compute else {
                            continue;
                        };
                        compute.set_pipeline(pipeline);
                        compute.set_bind_group(0, group, &[]);
                        if let Some(group1) = group1
                            && (frames.is_some() || pass.buffers.is_some())
                        {
                            compute.set_bind_group(1, group1, &[]);
                        }
                        if let Some(buffers) = &pass.buffers {
                            compute.set_bind_group(2, buffers, &[]);
                        }
                        let [x, y, z] = if pass.groups != [0; 3] {
                            pass.groups
                        } else {
                            [
                                pass.target[0].div_ceil(pass.block[0].max(1)),
                                pass.target[1].div_ceil(pass.block[1].max(1)),
                                1,
                            ]
                        };
                        compute.dispatch_workgroups(x, y, z);
                    }
                }
                (None, None) => {}
            }
            index = end + 1;
        }
        if let Some(why) = host_failed {
            tracing::warn!(
                "映像フィルター {}: ホスト側の処理を使えないので現フレームのコピーで代用: {why}",
                self.name
            );
            self.host_failed = true;
            self.host_failure = Some(why);
        }
        if timed && let Some(timing) = self.timing.as_mut() {
            timing.end(encoder);
        }
        self.outputs.current().view.clone()
    }
}

impl Stage {
    fn fixed(&self, at: usize) -> bool {
        let pass = &self.passes[at];
        pass.writes != "output"
            && pass
                .reads
                .iter()
                .all(|read| matches!(Source::of(read), Source::Named(_)))
    }

    fn group_of(
        &self,
        device: &wgpu::Device,
        at: usize,
        input: &Input<'_>,
        samplers: &Samplers,
    ) -> wgpu::BindGroup {
        let pass = &self.passes[at];
        let view_of = |name: &str| -> (&wgpu::TextureView, bool) {
            match Source::of(name) {
                Source::Picture(0) => (input.view, true),
                Source::Picture(k) => {
                    let tex = self.inputs.as_ref().map(|ring| ring.back(k));
                    tex.map_or((input.view, true), |tex| (&tex.view, tex.linear))
                }
                Source::Output(k) => {
                    let tex = self.outputs.back(k);
                    (&tex.view, tex.linear)
                }
                Source::Named(name) => {
                    let tex = &self.textures[name];
                    (&tex.view, tex.linear)
                }
            }
        };
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: pass.zw.as_entire_binding(),
        }];
        for (i, read) in pass.reads.iter().enumerate() {
            let (view, linear) = view_of(read);
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + 2 * i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + 2 * i as u32,
                resource: wgpu::BindingResource::Sampler(if linear {
                    &samplers.linear
                } else {
                    &samplers.nearest
                }),
            });
        }
        if pass.kind == Kind::Compute && !pass.writes.is_empty() {
            let target = if pass.writes == "output" {
                &self.outputs.current().view
            } else {
                &self.textures[&pass.writes].view
            };
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + 2 * pass.reads.len() as u32,
                resource: wgpu::BindingResource::TextureView(target),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&pass.name),
            layout: &pass.layout,
            entries: &entries,
        })
    }
}

impl Samplers {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let make = |label, filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        Self {
            linear: make("zw linear", wgpu::FilterMode::Linear),
            nearest: make("zw nearest", wgpu::FilterMode::Nearest),
        }
    }
}
