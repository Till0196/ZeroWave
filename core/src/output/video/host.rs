// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 本体が受け持つ映像処理（[`zv::Kind::Host`] のパス：フレーム補間、オプティカルフロー）。バックエンドは wgpu の
//! 外で動くので、前後は GPU の同期オブジェクト（Metal は共有イベント、D3D12 はフェンス）で守る。
//! AMF と外部プロセスの補間は完了を待たずに要求し、1〜2 入力遅らせて出す（[`lag`]）。

use zwsdk::video as zv;

#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
mod videotoolbox;

#[cfg(all(feature = "d3d12", windows))]
mod amf;
#[cfg(all(feature = "d3d12", windows))]
mod bridge;
mod compute;
#[cfg(all(feature = "d3d12", windows))]
mod d3d12;
mod flow;
#[cfg_attr(target_os = "android", allow(dead_code))]
mod lag;
#[cfg(all(feature = "d3d12", windows))]
mod nvofa;
#[cfg(all(feature = "nvdec", target_os = "linux"))]
mod nvofa_cuda;
#[cfg(any(windows, target_os = "linux"))]
mod opencl;
#[cfg(not(target_os = "android"))]
mod process;
#[cfg(any(
    all(feature = "d3d12", windows),
    all(any(feature = "nvdec", feature = "vaapi"), target_os = "linux")
))]
mod staging;
#[cfg(all(feature = "d3d12", feature = "qsv", windows))]
mod vpl;
#[cfg(all(any(feature = "nvdec", feature = "vaapi"), target_os = "linux"))]
pub(crate) mod vulkan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    Interpolate,
    Flow,
    FlowConfidence,
}

impl Job {
    pub fn of(source: &str) -> Option<Self> {
        let job = source.split_once('/').map_or(source, |(job, _)| job);
        match job {
            zv::HOST_INTERPOLATE => Some(Self::Interpolate),
            zv::HOST_FLOW => Some(Self::Flow),
            zv::HOST_FLOW_CONFIDENCE => Some(Self::FlowConfidence),
            _ => None,
        }
    }

    pub fn who(source: &str) -> Option<&str> {
        source
            .split_once('/')
            .map(|(_, who)| who)
            .filter(|who| !who.is_empty())
    }

    fn name(self) -> &'static str {
        match self {
            Self::Interpolate => zv::HOST_INTERPOLATE,
            Self::Flow => zv::HOST_FLOW,
            Self::FlowConfidence => zv::HOST_FLOW_CONFIDENCE,
        }
    }
}

#[derive(Debug)]
pub struct Run<'a> {
    pub prev: &'a wgpu::Texture,
    pub cur: &'a wgpu::Texture,
    pub target: &'a wgpu::Texture,
    /// 0 が前、1 が今。
    pub phase: f32,
    pub new_input: bool,
    /// 無ければ本体が選ぶ。
    pub who: Option<&'a str>,
    pub process: Option<&'a zv::Launch>,
    /// 0 は本体の既定。
    pub held: u32,
}

/// バックエンドは wgpu の外で動くので、途中までをサブミットしてから命令を発行する（[`Gpu::flush`]）。
#[derive(Debug)]
pub struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
}

impl Gpu<'_> {
    pub fn flush(&mut self) -> wgpu::SubmissionIndex {
        let next = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zw host"),
            });
        let done = std::mem::replace(self.encoder, next);
        self.queue.submit([done.finish()])
    }
}

#[derive(Default)]
pub struct Host {
    convert: Option<Convert>,
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    vt: Option<Result<videotoolbox::Frc, String>>,
    flow: Option<Result<Box<dyn flow::Provider>, String>>,
    flow_calls: f64,
    flow_inputs: f64,
    #[cfg(all(feature = "d3d12", windows))]
    amf: Option<Result<lag::Lagged<amf::Frc>, String>>,
    #[cfg(not(target_os = "android"))]
    process: Option<Result<lag::Lagged<process::Process>, String>>,
    news: Vec<zv::Event>,
    #[cfg(not(target_os = "android"))]
    process_spec: Option<zv::Launch>,
    #[cfg(all(feature = "d3d12", feature = "qsv", windows))]
    vpl: Option<Result<vpl::Frc, String>>,
    /// 最後に動いたバックエンド。前のバックエンドも残るので、決まった順で探すと古い時間を返してしまう。
    last: Option<Worked>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Worked {
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    Vt,
    #[cfg(all(feature = "d3d12", feature = "qsv", windows))]
    Vpl,
    #[cfg(all(feature = "d3d12", windows))]
    Amf,
    #[cfg(not(target_os = "android"))]
    Process,
    Flow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Lag {
    /// バックエンドの準備が済んでから数える。
    pub made: u64,
    pub missed: u64,
    /// ビジーのまま次の入力が来た入力は要求しない。
    pub requests: u64,
    /// 要求してから完了を CPU で確認するまでの平均（ミリ秒、レンダリング 1 回単位の精度）。
    pub turnaround_ms: f64,
    pub work_ms: f64,
}

/// オプティカルフローのバックエンドは、破棄すると困る物（NVOFA）があるので `retire` で解放する。
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(Ok(flow)) = self.flow.take() {
            flow.retire();
        }
    }
}

fn open_flow(
    device: &wgpu::Device,
    size: [u32; 2],
    grid: u32,
    named: Option<&str>,
) -> Result<Box<dyn flow::Provider>, String> {
    let prefix = format!("{}/", zv::HOST_FLOW);
    let who = named.map(str::to_owned).unwrap_or_else(|| {
        available(device)
            .into_iter()
            .find_map(|entry| entry.strip_prefix(&prefix).map(str::to_owned))
            .unwrap_or_default()
    });
    match who.as_str() {
        #[cfg(all(feature = "d3d12", windows))]
        "nvofa" => {
            nvofa::Flow::open(device, size, grid).map(|f| Box::new(f) as Box<dyn flow::Provider>)
        }
        #[cfg(all(feature = "nvdec", target_os = "linux"))]
        "nvofa" => nvofa_cuda::Flow::open(device, size, grid)
            .map(|f| Box::new(f) as Box<dyn flow::Provider>),
        #[cfg(any(windows, target_os = "linux"))]
        "intel" => {
            opencl::Flow::open(device, size, grid).map(|f| Box::new(f) as Box<dyn flow::Provider>)
        }
        _ => {
            compute::Flow::open(device, size, grid).map(|f| Box::new(f) as Box<dyn flow::Provider>)
        }
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Host").finish_non_exhaustive()
    }
}

impl Host {
    /// 失敗なら理由。
    pub fn run(&mut self, gpu: &mut Gpu<'_>, job: Job, run: &Run<'_>) -> Result<(), String> {
        let size = [run.cur.width(), run.cur.height()];
        let convert = self.convert.get_or_insert_with(|| Convert::new(gpu.device));
        match job {
            #[cfg(not(target_os = "android"))]
            Job::Interpolate if run.who == Some(zv::HOST_PROCESS) => {
                let spec = run.process.ok_or("起動する外部プロセスの指定が無い")?;
                // 指定が変われば開き直す（前に開けなかった物でも）。
                let same = |frc: &lag::Lagged<process::Process>| frc.frc.spec() == spec;
                let reopen = match &self.process {
                    Some(Ok(frc)) => !same(frc) || frc.size() != size,
                    Some(Err(_)) => self.process_spec.as_ref() != Some(spec),
                    None => true,
                };
                if reopen {
                    self.process = None;
                    self.process_spec = Some(spec.clone());
                    self.process =
                        Some(process::Process::open(gpu.device, spec, size).map(lag::Lagged::new));
                }
                self.last = Some(Worked::Process);
                match self.process.as_mut() {
                    Some(Ok(frc)) => {
                        let result = frc.run(gpu, convert, run);
                        self.news.extend(frc.news(gpu));
                        result
                    }
                    Some(Err(why)) => Err(why.clone()),
                    None => Err(String::from("外部プロセスを開いていない")),
                }
            }
            #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
            Job::Interpolate => {
                let reopen = !matches!(&self.vt, Some(Ok(frc)) if frc.size() == size);
                if reopen && !matches!(&self.vt, Some(Err(_))) {
                    self.vt = Some(videotoolbox::Frc::open(gpu.device, gpu.queue, size));
                }
                self.last = Some(Worked::Vt);
                match self.vt.as_mut() {
                    Some(Ok(frc)) => frc.run(gpu, convert, run),
                    Some(Err(why)) => Err(why.clone()),
                    None => Err(String::from("VideoToolbox を開いていない")),
                }
            }
            // 指定が無ければ、Intel の GPU でレンダリングしていれば oneVPL、でなければ AMF。
            #[cfg(all(feature = "d3d12", feature = "qsv", windows))]
            Job::Interpolate
                if run.who == Some("vpl")
                    || (run.who.is_none() && gpu.device.adapter_info().vendor == INTEL) =>
            {
                let reopen = !matches!(&self.vpl, Some(Ok(frc)) if frc.size() == size);
                if reopen && !matches!(&self.vpl, Some(Err(_))) {
                    self.vpl = None;
                    self.vpl = Some(vpl::Frc::open(gpu.device, size));
                }
                self.last = Some(Worked::Vpl);
                match self.vpl.as_mut() {
                    Some(Ok(frc)) => frc.run(gpu, convert, run),
                    Some(Err(why)) => Err(why.clone()),
                    None => Err(String::from("oneVPL を開いていない")),
                }
            }
            #[cfg(all(feature = "d3d12", windows))]
            Job::Interpolate if run.who.is_none_or(|who| who == "amf") => {
                let reopen = !matches!(&self.amf, Some(Ok(frc)) if frc.size() == size);
                if reopen && !matches!(&self.amf, Some(Err(_))) {
                    self.amf = None;
                    self.amf = Some(amf::Frc::open(gpu.device, size).map(lag::Lagged::new));
                }
                self.last = Some(Worked::Amf);
                match self.amf.as_mut() {
                    Some(Ok(frc)) => frc.run(gpu, convert, run),
                    Some(Err(why)) => Err(why.clone()),
                    None => Err(String::from("AMF を開いていない")),
                }
            }
            Job::Flow => {
                self.flow_calls = self.flow_calls * 0.98 + 1.0;
                self.flow_inputs = self.flow_inputs * 0.98 + f64::from(u8::from(run.new_input));
                let grid = flow::grid_of(size, run.target);
                let reopen = !matches!(&self.flow,
                    Some(Ok(flow)) if flow.size() == size && flow.wanted_grid() == grid);
                if reopen && !matches!(&self.flow, Some(Err(_))) {
                    if let Some(Ok(old)) = self.flow.take() {
                        old.retire();
                    }
                    self.flow = Some(open_flow(gpu.device, size, grid, run.who));
                }
                self.last = Some(Worked::Flow);
                match self.flow.as_mut() {
                    Some(Ok(flow)) => flow.run(gpu, convert, run),
                    Some(Err(why)) => Err(why.clone()),
                    None => Err(String::from(
                        "オプティカルフローのバックエンドを開いていない",
                    )),
                }
            }
            // バックエンドが出さなければ 1。
            Job::FlowConfidence => {
                if run.new_input {
                    let given = match self.flow.as_mut() {
                        Some(Ok(flow)) => flow.confidence(gpu, convert, run.target),
                        _ => false,
                    };
                    if !given {
                        convert.fill(gpu, run.target, wgpu::Color::WHITE);
                    }
                }
                Ok(())
            }
            #[allow(unreachable_patterns)]
            other => {
                let _ = (size, convert);
                Err(format!("このホストは {} に対応しない", other.name()))
            }
        }
    }

    /// フィルターのパスの GPU タイマーに入らないので負荷の判定と遅延の見積もりに足す（NVOFA は GPU タイマーに入る）。
    pub fn outside_ms(&self) -> f64 {
        match self.last {
            #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
            Some(Worked::Vt) => match &self.vt {
                Some(Ok(frc)) => frc.outside_ms(),
                _ => 0.0,
            },
            #[cfg(all(feature = "d3d12", feature = "qsv", windows))]
            Some(Worked::Vpl) => match &self.vpl {
                Some(Ok(frc)) => frc.outside_ms(),
                _ => 0.0,
            },
            #[cfg(all(feature = "d3d12", windows))]
            Some(Worked::Amf) => match &self.amf {
                Some(Ok(frc)) => frc.outside_ms(),
                _ => 0.0,
            },
            #[cfg(not(target_os = "android"))]
            Some(Worked::Process) => match &self.process {
                Some(Ok(frc)) => frc.outside_ms(),
                _ => 0.0,
            },
            // 新しい入力のときだけ動くので、その割合を掛ける。
            Some(Worked::Flow) => match &self.flow {
                Some(Ok(flow)) => {
                    let share = if self.flow_calls > 0.0 {
                        (self.flow_inputs / self.flow_calls).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    flow.outside_ms() * share
                }
                _ => 0.0,
            },
            None => 0.0,
        }
    }

    pub fn take_news(&mut self) -> Vec<zv::Event> {
        std::mem::take(&mut self.news)
    }

    pub fn held_inputs(&self) -> u32 {
        match self.last {
            #[cfg(all(feature = "d3d12", windows))]
            Some(Worked::Amf) => match &self.amf {
                Some(Ok(frc)) => frc.held(),
                _ => 0,
            },
            #[cfg(not(target_os = "android"))]
            Some(Worked::Process) => match &self.process {
                Some(Ok(frc)) => frc.held(),
                _ => 0,
            },
            _ => 0,
        }
    }

    pub fn lag(&self) -> Option<Lag> {
        match self.last {
            #[cfg(all(feature = "d3d12", windows))]
            Some(Worked::Amf) => self.amf.as_ref()?.as_ref().ok().map(|frc| frc.report()),
            #[cfg(not(target_os = "android"))]
            Some(Worked::Process) => self.process.as_ref()?.as_ref().ok().map(|frc| frc.report()),
            _ => None,
        }
    }

    /// バックエンドが使えないとき：今のフレーム（オプティカルフローなら 0）を出力先へ。
    pub fn fallback(&mut self, gpu: &mut Gpu<'_>, job: Job, run: &Run<'_>) {
        let convert = self.convert.get_or_insert_with(|| Convert::new(gpu.device));
        match job {
            Job::Interpolate => convert.put(gpu, run.cur, run.target),
            Job::Flow => convert.clear(gpu, run.target),
            Job::FlowConfidence => convert.clear(gpu, run.target),
        }
    }
}

const INTEL: u32 = 0x8086;
#[cfg(all(feature = "d3d12", windows))]
const NVIDIA: u32 = 0x10de;
#[cfg(all(feature = "d3d12", windows))]
const AMD: u32 = 0x1002;

/// グリッドは見込みで、対応しなければ開くときに一番近いグリッドで求めてリサンプリングする。
pub fn flow_caps(device: &wgpu::Device) -> Vec<zv::FlowCaps> {
    available(device)
        .iter()
        .filter_map(|entry| entry.strip_prefix(&format!("{}/", zv::HOST_FLOW)))
        .map(|who| match who {
            "nvofa" => zv::FlowCaps {
                who: who.to_owned(),
                grids: vec![1, 2, 4],
                confidence: true,
            },
            "intel" | "compute" => zv::FlowCaps {
                who: who.to_owned(),
                grids: vec![4],
                confidence: true,
            },
            other => zv::FlowCaps {
                who: other.to_owned(),
                grids: vec![zv::HOST_FLOW_GRID],
                confidence: false,
            },
        })
        .collect()
}

/// 開けそうかを軽く確かめるだけ。
pub fn available(device: &wgpu::Device) -> Vec<String> {
    let _ = device;
    let mut found: Vec<String> = Vec::new();
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    if videotoolbox::supported() {
        found.push(format!("{}/videotoolbox", zv::HOST_INTERPOLATE));
    }
    #[cfg(all(feature = "d3d12", windows))]
    {
        let vendor = device.adapter_info().vendor;
        let dx12 = device.adapter_info().backend == wgpu::Backend::Dx12;
        // バックエンドの GPU はレンダリングする GPU と別でもよい（[`bridge`] が転送する）。
        let present = |wanted: u32| vendor == wanted || bridge::has_vendor(wanted);
        if dx12 && nvofa::supported() && present(NVIDIA) {
            found.push(format!("{}/nvofa", zv::HOST_FLOW));
        }
        if dx12 && amf::supported() && present(AMD) {
            found.push(format!("{}/amf", zv::HOST_INTERPOLATE));
        }
        #[cfg(feature = "qsv")]
        if dx12 && vpl::supported() && present(INTEL) {
            found.push(format!("{}/vpl", zv::HOST_INTERPOLATE));
        }
    }
    #[cfg(not(target_os = "android"))]
    if process::supported(device) {
        found.push(format!("{}/{}", zv::HOST_INTERPOLATE, zv::HOST_PROCESS));
    }
    // Linux の NVOFA は CUDA で（同じ GPU なら Vulkan からインポートし、別ならメインメモリを通す）。
    #[cfg(all(feature = "nvdec", target_os = "linux"))]
    if nvofa_cuda::supported() {
        found.push(format!("{}/nvofa", zv::HOST_FLOW));
    }
    // Intel の VME はメインメモリを通すのでレンダリングする GPU を問わないが、それが Intel でなければ
    // シェーダーの実装の後ろ。
    #[cfg(any(windows, target_os = "linux"))]
    let vme = opencl::supported();
    #[cfg(not(any(windows, target_os = "linux")))]
    let vme = false;
    let on_intel = device.adapter_info().vendor == INTEL;
    if vme && on_intel {
        found.push(format!("{}/intel", zv::HOST_FLOW));
    }
    found.push(format!("{}/compute", zv::HOST_FLOW));
    if vme && !on_intel {
        found.push(format!("{}/intel", zv::HOST_FLOW));
    }
    found
}

pub(crate) struct Convert {
    module: wgpu::ShaderModule,
    flow: wgpu::ShaderModule,
    nv12: wgpu::ShaderModule,
    cost: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    flow_layout: wgpu::BindGroupLayout,
    nv12_layout: wgpu::BindGroupLayout,
    cost_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipelines: Vec<((&'static str, wgpu::TextureFormat), wgpu::RenderPipeline)>,
}

impl Convert {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw host convert"),
            source: wgpu::ShaderSource::Wgsl(include_str!("host/convert.wgsl").into()),
        });
        let flow = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw host flow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("host/flow.wgsl").into()),
        });
        let nv12 = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw host nv12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("host/nv12.wgsl").into()),
        });
        let cost = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw host cost"),
            source: wgpu::ShaderSource::Wgsl(include_str!("host/cost.wgsl").into()),
        });
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw host convert"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let flow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw host flow"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Sint),
                texture(1, wgpu::TextureSampleType::Sint),
            ],
        });
        let cost_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw host cost"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Uint),
                texture(1, wgpu::TextureSampleType::Uint),
            ],
        });
        let nv12_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw host nv12"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: true }),
                texture(1, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("zw host convert"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            module,
            flow,
            nv12,
            cost,
            layout,
            flow_layout,
            nv12_layout,
            cost_layout,
            sampler,
            pipelines: Vec::new(),
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        entry: &'static str,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        if let Some((_, found)) = self
            .pipelines
            .iter()
            .find(|(key, _)| *key == (entry, format))
        {
            return found.clone();
        }
        let own = matches!(entry, "flow" | "nv12" | "cost");
        let (module, layout) = match entry {
            "flow" => (&self.flow, &self.flow_layout),
            "nv12" => (&self.nv12, &self.nv12_layout),
            "cost" => (&self.cost, &self.cost_layout),
            _ => (&self.module, &self.layout),
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("zw host convert"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("zw host convert"),
                    bind_group_layouts: &[Some(layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(if own { "fs" } else { entry }),
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
        self.pipelines.push(((entry, format), pipeline.clone()));
        pipeline
    }

    fn draw(
        &mut self,
        gpu: &mut Gpu<'_>,
        entry: &'static str,
        group: &wgpu::BindGroup,
        target: &wgpu::Texture,
    ) {
        let pipeline = self.pipeline(gpu.device, entry, target.format());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zw host convert"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn sampled(&self, device: &wgpu::Device, source: &wgpu::Texture) -> wgpu::BindGroup {
        let view = source.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw host convert"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    pub(crate) fn put(
        &mut self,
        gpu: &mut Gpu<'_>,
        source: &wgpu::Texture,
        target: &wgpu::Texture,
    ) {
        // コピーできるのは COPY_SRC・COPY_DST があるときだけ（無いとコマンドごと捨てられ、フレームが前のまま
        // 残る）。フィルターの「前のフレーム」は COPY_SRC を持たない。
        if source.format() == target.format()
            && source.size() == target.size()
            && source.usage().contains(wgpu::TextureUsages::COPY_SRC)
            && target.usage().contains(wgpu::TextureUsages::COPY_DST)
        {
            gpu.encoder.copy_texture_to_texture(
                source.as_image_copy(),
                target.as_image_copy(),
                source.size(),
            );
            return;
        }
        let group = self.sampled(gpu.device, source);
        self.draw(gpu, "fs_copy", &group, target);
    }

    #[cfg_attr(not(all(feature = "d3d12", windows)), allow(dead_code))]
    pub(crate) fn luma(
        &mut self,
        gpu: &mut Gpu<'_>,
        source: &wgpu::Texture,
        target: &wgpu::Texture,
    ) {
        let group = self.sampled(gpu.device, source);
        self.draw(gpu, "fs_luma", &group, target);
    }

    /// グリッドが違えばリサンプリングする。
    #[cfg_attr(not(all(feature = "d3d12", windows)), allow(dead_code))]
    pub(crate) fn flow(
        &mut self,
        gpu: &mut Gpu<'_>,
        forward: &wgpu::Texture,
        backward: &wgpu::Texture,
        target: &wgpu::Texture,
    ) {
        let (f, b) = (
            forward.create_view(&Default::default()),
            backward.create_view(&Default::default()),
        );
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw host flow"),
            layout: &self.flow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&f),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&b),
                },
            ],
        });
        self.draw(gpu, "flow", &group, target);
    }

    #[cfg_attr(
        not(all(feature = "d3d12", feature = "qsv", windows)),
        allow(dead_code)
    )]
    pub(crate) fn nv12_in(
        &mut self,
        gpu: &mut Gpu<'_>,
        source: &wgpu::Texture,
        y: &wgpu::Texture,
        uv: &wgpu::Texture,
    ) {
        let group = self.sampled(gpu.device, source);
        self.draw(gpu, "fs_y", &group, y);
        self.draw(gpu, "fs_uv", &group, uv);
    }

    #[cfg_attr(
        not(all(feature = "d3d12", feature = "qsv", windows)),
        allow(dead_code)
    )]
    pub(crate) fn nv12_out(
        &mut self,
        gpu: &mut Gpu<'_>,
        y: &wgpu::Texture,
        uv: &wgpu::Texture,
        target: &wgpu::Texture,
    ) {
        let (luma, chroma) = (
            y.create_view(&Default::default()),
            uv.create_view(&Default::default()),
        );
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw host nv12"),
            layout: &self.nv12_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&luma),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&chroma),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.draw(gpu, "nv12", &group, target);
    }

    /// グリッドが違えばリサンプリングする。
    #[cfg_attr(not(all(feature = "d3d12", windows)), allow(dead_code))]
    pub(crate) fn cost(
        &mut self,
        gpu: &mut Gpu<'_>,
        forward: &wgpu::Texture,
        backward: &wgpu::Texture,
        target: &wgpu::Texture,
    ) {
        let (f, b) = (
            forward.create_view(&Default::default()),
            backward.create_view(&Default::default()),
        );
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw host cost"),
            layout: &self.cost_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&f),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&b),
                },
            ],
        });
        self.draw(gpu, "cost", &group, target);
    }

    pub(crate) fn clear(&mut self, gpu: &mut Gpu<'_>, target: &wgpu::Texture) {
        self.fill(gpu, target, wgpu::Color::TRANSPARENT);
    }

    pub(crate) fn fill(&mut self, gpu: &mut Gpu<'_>, target: &wgpu::Texture, color: wgpu::Color) {
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zw host clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(device: &wgpu::Device, usage: wgpu::TextureUsages) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("試験"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage,
            view_formats: &[],
        })
    }

    #[test]
    fn the_planar_shader_is_valid() {
        let module =
            naga::front::wgsl::parse_str(include_str!("host/process/planar.wgsl")).expect("読める");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("正しい");
    }

    /// COPY_SRC を持たないコピー元はレンダリングで移す（コピーするとコマンドごと捨てられ、前のフレームが残る）。
    #[test]
    fn a_source_that_cannot_be_copied_is_drawn() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("GPU が無いのでスキップ");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("GPU が開けないのでスキップ");
            return;
        };
        let source = texture(
            &device,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let target = texture(
            &device,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST,
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("zw host"),
        });
        let mut convert = Convert::new(&device);
        convert.put(
            &mut Gpu {
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
            },
            &source,
            &target,
        );
        queue.submit([encoder.finish()]);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }
}
