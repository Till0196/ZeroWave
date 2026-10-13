// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 毎フレームフィルターを記録し、測って、重すぎるフィルターを外す。

use super::*;

impl Hooks {
    pub fn frames_layout(&self) -> Option<&wgpu::BindGroupLayout> {
        self.deinterlace.as_ref()?;
        self.frames_layout.as_ref().map(|(layout, _)| layout)
    }

    pub fn frames(&self) -> Option<[i32; 2]> {
        self.deinterlace.as_ref().map(Stage::frames)
    }

    pub fn next_zw(&mut self, mut zw: Zw, phase: f32, outputs: u32) -> Zw {
        self.frame = self.frame.wrapping_add(1);
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        let outputs = outputs.max(1);
        zw.frame = self.frame;
        zw.outputs = outputs;
        zw.phase = phase;
        zw.random = (self.seed >> 8) as f32 / (1u32 << 24) as f32;
        zw
    }

    #[allow(clippy::too_many_arguments)]
    pub fn deinterlace(
        &mut self,
        ctx: &Ctx<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &Input<'_>,
        picture: &wgpu::TextureView,
        frame: &Frame,
        frames: &wgpu::BindGroup,
        newest: Option<(&wgpu::TextureView, bool)>,
    ) {
        let Some(samplers) = self.samplers.as_ref() else {
            return;
        };
        let Some(stage) = self.deinterlace.as_mut() else {
            return;
        };
        let host = &mut self.host;
        let after = self.after.take();
        let out = measured(ctx, encoder, stage, |stage, encoder| {
            stage.encode(
                ctx,
                encoder,
                input,
                frame,
                Some(frames),
                samplers,
                newest,
                after.as_ref(),
                host,
            )
        });
        self.after = Some(out.clone());
        let Some((copy, layout)) = self.copy.as_ref() else {
            return;
        };
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw copy"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&out),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&samplers.linear),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zw copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: picture,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(copy);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.tell_host();
        self.drop_heavy();
    }

    /// picture のチェーンを記録し、最後のフィルターの出力を返す。形式は [`WORKING`]。
    pub fn picture(
        &mut self,
        ctx: &Ctx<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &Input<'_>,
        frame: &Frame,
    ) -> Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])> {
        let samplers = self.samplers.as_ref()?;
        let host = &mut self.host;
        let mut after = self.after.take();
        let mut last: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])> = None;
        for stage in &mut self.picture {
            let view = match &last {
                None => measured(ctx, encoder, stage, |stage, encoder| {
                    stage.encode(
                        ctx,
                        encoder,
                        input,
                        frame,
                        None,
                        samplers,
                        None,
                        after.as_ref(),
                        host,
                    )
                }),
                Some((texture, view, size)) => {
                    let before = Input {
                        texture,
                        view,
                        size: *size,
                    };
                    measured(ctx, encoder, stage, |stage, encoder| {
                        stage.encode(
                            ctx,
                            encoder,
                            &before,
                            frame,
                            None,
                            samplers,
                            None,
                            after.as_ref(),
                            host,
                        )
                    })
                }
            };
            after = Some(view.clone());
            last = Some((
                stage.outputs.current().texture.clone(),
                view,
                stage.graph.output,
            ));
        }
        self.after = after;
        self.tell_host();
        self.drop_heavy();
        last
    }

    pub(super) fn budget(&self) -> f64 {
        let share = if self.buffer_ms() > 0 {
            HEADROOM
        } else {
            AFFORDABLE
        };
        self.interval_ms * share / f64::from(self.outputs())
    }

    /// 重すぎたら一番重いフィルターを一つだけ外す（フィルターは GPU の一つのキューで順に実行されるので合計で見る）。
    fn drop_heavy(&mut self) {
        let budget = self.budget();
        let mut judging = false;
        for stage in self.deinterlace.iter().chain(&self.picture) {
            if stage.judging {
                judging = true;
                if stage.settled().is_none() {
                    return;
                }
            }
        }
        if !judging {
            return;
        }
        // ホスト側の処理のパスを持つフィルターは、フィルターの計時の外で動くホスト側の処理の時間も足す。
        let outside = self.host.outside_ms();
        for stage in self.deinterlace.iter_mut().chain(&mut self.picture) {
            if !stage.judging {
                continue;
            }
            let extra = if stage.has_host() { outside } else { 0.0 };
            stage.ms = stage.settled().map(|ms| ms + extra);
            stage.judging = false;
            stage.measured = None;
            if let Some(marker) = stage.marker.take() {
                marker.clear();
            }
        }
        let total: f64 = self
            .deinterlace
            .iter()
            .chain(&self.picture)
            .filter_map(|stage| stage.ms)
            .sum();
        if total <= budget {
            for stage in self.deinterlace.iter_mut().chain(&mut self.picture) {
                stage.over = false;
                tracing::info!(
                    "映像フィルター {}: 1 フレーム {:.2} ms（合計 {total:.2} ms / 上限 {budget:.2} ms）",
                    stage.name,
                    stage.ms.unwrap_or(0.0)
                );
            }
            return;
        }
        let weights: Vec<(f64, bool)> = self
            .deinterlace
            .iter()
            .chain(&self.picture)
            .map(|stage| (stage.ms.unwrap_or(0.0), stage.graph.keep))
            .collect();
        let at = match judge_chain(&weights, budget) {
            Verdict::Fits => return,
            Verdict::Drop(index) if self.deinterlace.is_some() => index.checked_sub(1),
            Verdict::Drop(index) => Some(index),
            Verdict::KeepOver => {
                for stage in self.deinterlace.iter_mut().chain(&mut self.picture) {
                    stage.over = stage.graph.keep;
                    tracing::info!(
                        "映像フィルター {}: 1 フレーム {:.2} ms（合計 {total:.2} ms が上限 {budget:.2} ms を超過、一時停止しない設定なので継続）",
                        stage.name,
                        stage.ms.unwrap_or(0.0)
                    );
                }
                return;
            }
        };
        let (stage, deinterlace) = match at {
            None => (self.deinterlace.take(), true),
            Some(index) => (Some(self.picture.remove(index)), false),
        };
        let Some(mut stage) = stage else {
            return;
        };
        let ms = stage.ms.unwrap_or(0.0);
        tracing::info!(
            "映像フィルター {}: 最も重いので停止（1 フレーム {ms:.2} ms、合計 {total:.2} ms / 上限 {budget:.2} ms）。残りを再計測する",
            stage.name
        );
        let pass_times = stage.pass_times();
        if !pass_times.is_empty() {
            let each: Vec<String> = pass_times
                .iter()
                .map(|(name, ms)| format!("{name} {ms:.2}"))
                .collect();
            tracing::info!(
                "映像フィルター {}: パスごとの時間（ms）: {}",
                stage.name,
                each.join("、")
            );
        }
        if let Some(marker) = stage.marker.take() {
            marker.clear();
        }
        for rest in self.deinterlace.iter_mut().chain(&mut self.picture) {
            rest.rejudge();
        }
        self.paused_interval = self.interval_ms;
        let paused = Paused {
            name: stage.name.clone(),
            deinterlace,
            ms,
            budget,
            pass_times,
        };
        let at = if deinterlace {
            At::Deinterlace
        } else {
            At::Picture
        };
        let event = zv::Event::Paused {
            at,
            ms: paused.ms,
            budget: paused.budget,
        };
        self.tell(&paused.name.clone(), Some(at), &event);
        self.paused.push(paused);
    }

    fn tell_host(&mut self) {
        let news = self.host.take_news();
        let mut told = Vec::new();
        let stages = (self
            .deinterlace
            .iter_mut()
            .map(|stage| (stage, At::Deinterlace)))
        .chain(self.picture.iter_mut().map(|stage| (stage, At::Picture)));
        for (stage, at) in stages {
            if let Some(reason) = stage.host_failure.take() {
                told.push((stage.name.clone(), at, zv::Event::Failed { at, reason }));
            }
            if stage.has_host() {
                told.extend(
                    news.iter()
                        .map(|event| (stage.name.clone(), at, event.clone())),
                );
            }
        }
        for (name, at, event) in told {
            self.tell(&name, Some(at), &event);
        }
    }

    /// 本体の補間がフレームを遅らせて出す長さ（ミリ秒）。映像そのものの遅れなのでバッファしても縮まない。
    pub fn held_ms(&self) -> f64 {
        let lagging = self
            .deinterlace
            .iter()
            .chain(&self.picture)
            .any(Stage::interpolates);
        if lagging {
            f64::from(self.host.held_inputs()) * self.interval_ms
        } else {
            0.0
        }
    }

    /// チェーンがフレームを遅らせる長さ（ミリ秒、バッファするなら 0）。
    pub fn latency_ms(&self) -> f64 {
        if self.buffer_ms() > 0 {
            return 0.0;
        }
        self.deinterlace
            .iter()
            .chain(&self.picture)
            .filter_map(|stage| stage.ms)
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Fits,
    Drop(usize),
    KeepOver,
}

pub(super) fn judge_chain(stages: &[(f64, bool)], budget: f64) -> Verdict {
    let total: f64 = stages.iter().map(|(ms, _)| ms).sum();
    if total <= budget {
        return Verdict::Fits;
    }
    stages
        .iter()
        .enumerate()
        .filter(|(_, (_, keep))| !keep)
        .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map_or(Verdict::KeepOver, |(index, _)| Verdict::Drop(index))
}

fn measured(
    ctx: &Ctx<'_>,
    encoder: &mut wgpu::CommandEncoder,
    stage: &mut Stage,
    run: impl FnOnce(&mut Stage, &mut wgpu::CommandEncoder) -> wgpu::TextureView,
) -> wgpu::TextureView {
    if stage.measured.is_none() {
        return run(stage, encoder);
    }
    let make = || {
        ctx.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zw measure"),
            })
    };
    let before = std::mem::replace(encoder, make());
    ctx.queue.submit([before.finish()]);
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let began = Instant::now();
    let mut own = make();
    let view = run(stage, &mut own);
    ctx.queue.submit([own.finish()]);
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let spent = began.elapsed().as_secs_f64() * 1000.0;
    if let Some(times) = stage.measured.as_mut() {
        times.push(spent);
    }
    view
}
