// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィルターチェーンを読み込み、別のスレッドでビルドし、できあがったフィルターを組み込む。

use super::build::{copy_pipeline, frames_layout};
use super::marker::marker_of;
use super::*;

struct Job {
    name: String,
    at: At,
    graph: Graph,
    input: ([u32; 2], wgpu::TextureFormat),
    frames: Option<(wgpu::BindGroupLayout, String)>,
    marker: PathBuf,
}

impl Hooks {
    pub fn new() -> Self {
        Self {
            after: None,
            generation: None,
            entries: Vec::new(),
            deinterlace: None,
            picture: Vec::new(),
            key: None,
            checked: None,
            frame: 0,
            seed: 0x9e37_79b9,
            samplers: None,
            frames_layout: None,
            copy: None,
            building: None,
            paused: Vec::new(),
            failed: Vec::new(),
            progress: None,
            building_for: Vec::new(),
            cancel: None,
            device_name: String::new(),
            program: None,
            program_seen: None,
            interval_ms: FIELD_BUDGET_MS,
            paused_interval: FIELD_BUDGET_MS,
            host: super::host::Host::default(),
            host_jobs: Vec::new(),
        }
    }

    /// 映像のフレーム間隔（ミリ秒）。大きく延びたら外したフィルターをビルドし直して測り直す。
    pub fn set_interval(&mut self, ms: f64) {
        if !(ms.is_finite() && ms > 0.0) {
            return;
        }
        self.interval_ms = ms;
        if !self.paused.is_empty() && ms > self.paused_interval * RETRY_STRETCH {
            tracing::info!(
                "映像フィルター: フレーム間隔が {:.1} → {ms:.1} ms に延びたので、停止したフィルターを再計測する",
                self.paused_interval
            );
            self.paused_interval = ms;
            self.key = None;
        }
    }

    pub fn output_size(&self) -> Option<[u32; 2]> {
        self.picture
            .last()
            .or(self.deinterlace.as_ref())
            .map(Stage::output_size)
    }

    pub fn spent_ms(&self) -> Option<f64> {
        self.deinterlace
            .iter()
            .chain(&self.picture)
            .map(|stage| stage.ms)
            .sum()
    }

    pub fn buffer_ms(&self) -> u32 {
        self.deinterlace
            .iter()
            .chain(&self.picture)
            .map(|stage| stage.graph.buffer_ms)
            .max()
            .unwrap_or(0)
    }

    pub fn set_program(&mut self, program: Option<zv::Program>) {
        self.program = program;
    }

    pub fn is_empty(&self) -> bool {
        self.deinterlace.is_none() && self.picture.is_empty()
    }

    pub fn retime(&self) -> Option<f64> {
        self.picture.iter().find_map(|stage| stage.graph.retime())
    }

    pub fn outputs(&self) -> u32 {
        self.deinterlace
            .iter()
            .chain(&self.picture)
            .map(Stage::outputs)
            .product::<u32>()
            .max(1)
    }

    pub fn reach(&self) -> (usize, usize) {
        self.deinterlace.as_ref().map_or((1, 1), |stage| {
            let [lo, hi] = stage.frames();
            ((-lo).max(1) as usize, hi.max(1) as usize)
        })
    }

    pub fn has_deinterlace(&self) -> bool {
        self.deinterlace.is_some()
    }

    pub fn names(&self) -> Vec<String> {
        self.deinterlace
            .iter()
            .chain(&self.picture)
            .map(|stage| stage.name.clone())
            .collect()
    }

    /// ビルドし直すべきならビルドし直す。ビルドし直したら真。
    pub fn refresh(&mut self, ctx: &Ctx<'_>) -> bool {
        let installed = self.install();
        let now = Instant::now();
        let same_key = self.key.as_ref() == Some(&ctx.key);
        let only_target = self.key.as_ref().is_some_and(|key| {
            Key {
                target: ctx.key.target,
                ..key.clone()
            } == ctx.key
        });
        if (same_key || only_target) && self.checked.is_some_and(|at| now - at < CHECK_EVERY) {
            return installed;
        }
        self.checked = Some(now);
        if self.device_name.is_empty() {
            let info = ctx.device.adapter_info();
            self.device_name = format!("{}・{:?}", info.name, info.backend);
        }
        let (loaded, generation) = crate::plugins::current();
        let mut dirty = !same_key;
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.load(loaded.as_deref());
            dirty = true;
        }
        for entry in &mut self.entries {
            let stamp = entry.instance.stamp();
            if stamp != entry.stamp {
                entry.stamp = stamp;
                dirty = true;
            }
        }
        // キーが同じならビルドし直さない（時間方向の履歴を捨てない）。
        if !dirty && self.program != self.program_seen {
            self.program_seen = self.program.clone();
            let size = [ctx.key.width, ctx.key.height];
            for i in 0..self.entries.len() {
                if !self.entries[i].program {
                    continue;
                }
                let env = self.env(ctx, self.entries[i].at, size, true);
                let entry = &mut self.entries[i];
                let key = entry.instance.key(&env);
                if key.is_none() || key != entry.key {
                    dirty = true;
                }
            }
        }
        if !dirty {
            return installed;
        }
        self.key = Some(ctx.key.clone());
        self.rebuild(ctx);
        true
    }

    fn load(&mut self, loaded: Option<&crate::plugins::Loaded>) {
        self.entries.clear();
        let Some(loaded) = loaded else {
            return;
        };
        for (plugin, io) in loaded.video() {
            let marker = marker_of(&plugin);
            if let Some(left) = Marker::left_behind(&marker) {
                tracing::warn!(
                    "映像フィルター: {} を読み込まない（前回の計測中にデバイスロスト。{} を消すと再試行）",
                    plugin.name(),
                    left.display()
                );
                continue;
            }
            let manifest = plugin.video_manifest();
            for &at in &manifest.at {
                match plugin.video(crate::plugins::logger(plugin.name().to_owned()), io.clone()) {
                    Ok(instance) => self.entries.push(Entry {
                        name: plugin.name().to_owned(),
                        at,
                        stamp: instance.stamp(),
                        program: manifest.program,
                        key: None,
                        instance,
                        marker: marker.clone(),
                    }),
                    Err(why) => tracing::warn!("映像フィルター: {why}"),
                }
            }
        }
        self.entries
            .sort_by_key(|entry| entry.at != At::Deinterlace);
    }

    fn env(&self, ctx: &Ctx<'_>, at: At, input: [u32; 2], program: bool) -> zv::Env {
        let key = &ctx.key;
        let limits = ctx.device.limits();
        zv::Env {
            at,
            format: zv::Format {
                width: key.width,
                height: key.height,
                depth: key.depth,
                interlaced: key.interlaced,
                aspect: key.aspect,
                transfer: key.transfer.to_owned(),
            },
            input,
            target: key.target,
            rate: key.rate,
            display: f64::from(key.display) / 1000.0,
            device: zv::Device {
                backend: format!("{:?}", ctx.device.adapter_info().backend).to_lowercase(),
                compute: limits.max_compute_workgroups_per_dimension > 0,
                float32_filterable: ctx
                    .device
                    .features()
                    .contains(wgpu::Features::FLOAT32_FILTERABLE),
                max_texture: limits.max_texture_dimension_2d,
                host: super::host::available(ctx.device),
                flows: super::host::flow_caps(ctx.device),
                shader_f16: ctx.device.features().contains(wgpu::Features::SHADER_F16),
                matrices: crate::output::video::gpu::matrices(ctx.device)
                    .iter()
                    .filter_map(|m| {
                        Some(zv::Matrix {
                            m: m.m_size,
                            n: m.n_size,
                            k: m.k_size,
                            ab: scalar_name(m.ab_type)?.to_owned(),
                            cr: scalar_name(m.cr_type)?.to_owned(),
                        })
                    })
                    .collect(),
                limits: zv::Limits {
                    buffer: limits.max_buffer_size,
                    storage_binding: limits.max_storage_buffer_binding_size,
                    storage_buffers: limits.max_storage_buffers_per_shader_stage,
                    uniform_align: limits.min_uniform_buffer_offset_alignment,
                    storage_align: limits.min_storage_buffer_offset_alignment,
                    workgroup_storage: limits.max_compute_workgroup_storage_size,
                    invocations: limits.max_compute_invocations_per_workgroup,
                    groups: limits.max_compute_workgroups_per_dimension,
                    subgroup: {
                        let info = ctx.device.adapter_info();
                        [info.subgroup_min_size, info.subgroup_max_size]
                    },
                },
            },
            settings: BTreeMap::new(),
            program: if program { self.program.clone() } else { None },
        }
    }

    fn rebuild(&mut self, ctx: &Ctx<'_>) {
        self.program_seen = self.program.clone();
        // 前にビルドさせていた物は捨てて止める（ビルドが並ばないように）。
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.building = None;
        self.progress = None;
        self.building_for.clear();
        for stage in self.deinterlace.iter_mut().chain(&mut self.picture) {
            if let Some(marker) = stage.marker.take() {
                marker.clear();
            }
        }
        self.deinterlace = None;
        self.picture.clear();
        self.paused.clear();
        self.failed.clear();
        if self.entries.is_empty() {
            return;
        }
        let device = ctx.device;
        self.host_jobs = super::host::available(device);
        self.samplers.get_or_insert_with(|| Samplers::new(device));
        let key = ctx.key.clone();
        let size = [key.width, key.height];

        let mut seen_deinterlace = false;
        let mut jobs: Vec<Job> = Vec::new();
        let mut input = size;
        let mut input_format = ctx.picture;
        let envs: Vec<(usize, zv::Env)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (i, self.env(ctx, entry.at, size, entry.program)))
            .collect();
        for (i, mut env) in envs {
            let at = self.entries[i].at;
            let name = self.entries[i].name.clone();
            // ビルドしないフィルターでもキーは覚える。覚えなければ番組が変わるたびにビルドし直す。
            if self.entries[i].program {
                let key = self.entries[i].instance.key(&env);
                self.entries[i].key = key;
            }
            match at {
                At::Deinterlace => {
                    if seen_deinterlace {
                        tracing::info!(
                            "映像フィルター: {name} は使わない（デインターレースのフィルターは一つだけ）"
                        );
                        continue;
                    }
                    seen_deinterlace = true;
                    if !key.interlaced {
                        continue;
                    }
                }
                At::Picture => env.input = input,
            }
            let graph = match self.entries[i].instance.build(&env) {
                Ok(graph) => graph,
                Err(why) => {
                    tracing::warn!("映像フィルター {name}: {why}");
                    self.failed.push((name.clone(), why.clone()));
                    self.tell(&name, Some(at), &zv::Event::Failed { at, reason: why });
                    continue;
                }
            };
            if graph.passes.is_empty() {
                continue;
            }
            let frames = (at == At::Deinterlace).then(|| {
                let frames = graph.frames;
                (
                    frames_layout(device, key.depth, key.shape, frames),
                    shader::frames_prelude(key.depth, key.shape, frames),
                )
            });
            let job_input = match at {
                At::Deinterlace => (size, ctx.picture),
                At::Picture => (input, input_format),
            };
            if at == At::Picture {
                input = graph.output;
                input_format = WORKING;
            }
            jobs.push(Job {
                name,
                at,
                graph,
                input: job_input,
                frames,
                marker: self.entries[i].marker.clone(),
            });
        }
        if jobs.is_empty() {
            return;
        }
        if jobs.iter().any(|job| job.at == At::Deinterlace) && self.copy.is_none() {
            self.copy = Some(copy_pipeline(device, ctx.picture));
        }
        // レンダリングスレッドの外でビルドする（FXC だと何十秒もかかる）。できあがるまでは本体の処理でレンダリングする。
        let (send, receive) = std::sync::mpsc::channel();
        let (device, queue, picture) = (ctx.device.clone(), ctx.queue.clone(), ctx.picture);
        let total = jobs.len() as u32;
        let done = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&done);
        let mut names: Vec<String> = jobs.iter().map(|job| job.name.clone()).collect();
        names.dedup();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name(String::from("zw-video-filter"))
            .spawn(move || {
                let ctx = Ctx {
                    device: &device,
                    queue: &queue,
                    key,
                    picture,
                };
                let began = Instant::now();
                let mut built = Vec::new();
                let mut broken = false;
                for job in jobs {
                    if cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    let result = if broken && job.at == At::Picture {
                        Err(String::from("チェーンの前のフィルターがビルドできなかった"))
                    } else {
                        Stage::build(
                            &ctx,
                            &job.name,
                            job.at,
                            job.graph,
                            job.input,
                            job.frames
                                .as_ref()
                                .map(|(layout, prelude)| (layout, prelude.as_str())),
                        )
                    };
                    broken |= result.is_err() && job.at == At::Picture;
                    built.push((job.name, job.at, job.frames, job.marker, result));
                    counter.fetch_add(1, Ordering::Release);
                }
                let _ = send.send((built, began.elapsed()));
            });
        match spawned {
            Ok(_) => {
                self.building = Some(receive);
                self.progress = Some((done, total, 0));
                for name in &names {
                    self.tell(name, None, &zv::Event::Building { total });
                }
                self.building_for = names;
                self.cancel = Some(cancel);
            }
            Err(error) => tracing::warn!("映像フィルター: ビルド用スレッドを作れない: {error}"),
        }
    }

    pub(super) fn tell(&mut self, name: &str, at: Option<At>, event: &zv::Event) {
        let wanted = at.unwrap_or(At::Picture);
        let index = self
            .entries
            .iter()
            .position(|entry| entry.name == name && entry.at == wanted)
            .or_else(|| {
                at.is_none()
                    .then(|| self.entries.iter().position(|entry| entry.name == name))
                    .flatten()
            });
        if let Some(index) = index {
            self.entries[index].instance.event(event);
        }
    }

    fn tell_progress(&mut self) {
        let Some((done, total, said)) = self.progress.as_mut() else {
            return;
        };
        let now = done.load(Ordering::Acquire);
        if now == *said || now >= *total {
            return;
        }
        *said = now;
        let event = zv::Event::Progress {
            done: now,
            total: *total,
        };
        for name in self.building_for.clone() {
            self.tell(&name, None, &event);
        }
    }

    /// 測った時刻の読み出しを要求する（`submit` のあとに呼ぶ）。
    pub fn after_submit(&self) {
        for stage in self.deinterlace.iter().chain(&self.picture) {
            if let Some(timing) = &stage.timing {
                timing.submitted();
            }
        }
    }

    fn install(&mut self) -> bool {
        self.tell_progress();
        let Some(receive) = self.building.as_ref() else {
            return false;
        };
        let (built, spent) = match receive.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.building = None;
                return false;
            }
        };
        self.building = None;
        self.progress = None;
        tracing::info!("映像フィルター: ビルド {:.1} 秒", spent.as_secs_f64());
        for name in std::mem::take(&mut self.building_for) {
            self.tell(
                &name,
                None,
                &zv::Event::Built {
                    seconds: spent.as_secs_f64(),
                },
            );
        }
        for (name, at, frames, marker, result) in built {
            match result {
                Ok(mut stage) => {
                    stage.marker = Marker::place(&marker, &name);
                    tracing::info!(
                        "映像フィルター {name}: {} パス、{}x{} → {}x{}{}",
                        stage.passes.len(),
                        stage.input_size[0],
                        stage.input_size[1],
                        stage.graph.output[0],
                        stage.graph.output[1],
                        if stage.outputs() > 1 {
                            format!("、1 フレームから {} フレーム", stage.outputs())
                        } else {
                            String::new()
                        }
                    );
                    match at {
                        At::Deinterlace => {
                            self.frames_layout = frames;
                            self.deinterlace = Some(stage);
                        }
                        At::Picture => self.picture.push(stage),
                    }
                }
                Err(why) => {
                    tracing::warn!("映像フィルター {name}: ビルドできない: {why}");
                    self.failed.push((name.clone(), why.clone()));
                    self.tell(&name, Some(at), &zv::Event::Failed { at, reason: why });
                }
            }
        }
        true
    }

    pub fn wait_built(&mut self) {
        while self.building.is_some() && !self.install() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn scalar_name(kind: wgpu::CooperativeScalarType) -> Option<&'static str> {
    match kind {
        wgpu::CooperativeScalarType::F16 => Some("f16"),
        wgpu::CooperativeScalarType::F32 => Some("f32"),
        _ => None,
    }
}
