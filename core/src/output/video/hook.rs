// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインの映像フィルター（`video` hook）を GPU で実行する。ビルドできないフィルターと重すぎるフィルターは外す。
//! 測っている間に GPU が失われたらマーカーのファイルが残り、次の起動でそのプラグインを読み込まない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use zwsdk::host::{Plugin, VideoInstance};
use zwsdk::video::{self as zv, At, Graph, Kind, Source, Zw};

use super::shader::{self, Shape};

mod build;
mod chain;
mod marker;
mod report;
mod run;
mod stage;
mod timing;

use super::host;
use marker::Marker;
pub use report::{Paused, Report, StageReport};

use super::{AFFORDABLE, FIELD_BUDGET_MS, HEADROOM};

const CHECK_EVERY: Duration = Duration::from_millis(500);

pub(super) const WORKING: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// 最初のフレームはドライバーがパイプラインをビルドするので捨てる。
const MEASURE: usize = 4;
/// 外したときよりフレーム間隔がこれだけ延びたら測り直す。
const RETRY_STRETCH: f64 = 1.4;

const SETTLE: usize = 12;

/// 全体を覆う三角形一つと、0..1 の座標（左上が 0）。
const VERTEX: &str = "
struct ZwVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn zw_vertex(@builtin(vertex_index) index: u32) -> ZwVertex {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let corner = corners[index];
    var out: ZwVertex;
    out.position = vec4<f32>(corner, 0.0, 1.0);
    out.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return out;
}
";

const COPY: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn zw_copy(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(clamp(textureSample(source, smp, uv).rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
";

fn format_of(name: &str) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    Some(match zv::format_name(name)? {
        "rgba16float" => F::Rgba16Float,
        "rgba32float" => F::Rgba32Float,
        "rgba8unorm" => F::Rgba8Unorm,
        "r16float" => F::R16Float,
        "r32float" => F::R32Float,
        "rg16float" => F::Rg16Float,
        "rg32float" => F::Rg32Float,
        "r8unorm" => F::R8Unorm,
        "rg8unorm" => F::Rg8Unorm,
        _ => return None,
    })
}

#[derive(Debug)]
pub struct Ctx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub key: Key,
    pub picture: wgpu::TextureFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub shape: Shape,
    pub interlaced: bool,
    pub aspect: [u32; 2],
    pub rate: [u32; 2],
    pub transfer: &'static str,
    pub target: [u32; 2],
    pub display: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// `picture:k` の履歴はこのときだけ進む。
    pub new_input: bool,
    pub zw: Zw,
}

#[derive(Debug)]
struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: [u32; 3],
    dimension: wgpu::TextureViewDimension,
    linear: bool,
}

#[derive(Debug)]
struct Ring {
    texs: Vec<Tex>,
    head: usize,
}

#[derive(Debug)]
struct Built {
    name: String,
    kind: Kind,
    job: Option<super::host::Job>,
    who: Option<String>,
    process: Option<zv::Launch>,
    held: u32,
    reads: Vec<String>,
    writes: String,
    block: [u32; 2],
    /// 0 でなければワークグループの数をそのまま。続くこれらは一つの compute パスに記録する。
    groups: [u32; 3],
    target: [u32; 2],
    layout: wgpu::BindGroupLayout,
    render: Option<wgpu::RenderPipeline>,
    compute: Option<wgpu::ComputePipeline>,
    zw: wgpu::Buffer,
    uses_zw: bool,
    fixed: Option<wgpu::BindGroup>,
    buffers: Option<wgpu::BindGroup>,
    /// タイムスタンプのスロット（続けて記録するパスは最後の一つだけが持つ）。
    slot: Option<usize>,
}

#[derive(Debug)]
pub struct Input<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    pub size: [u32; 2],
}

#[derive(Debug)]
pub struct Stage {
    name: String,
    graph: Graph,
    passes: Vec<Built>,
    textures: BTreeMap<String, Tex>,
    empty: Option<wgpu::BindGroup>,
    /// 新しい入力のたびに `名前:1` へコピーする。
    previous: Vec<String>,
    inputs: Option<Ring>,
    outputs: Ring,
    input_size: [u32; 2],
    judging: bool,
    /// GPU が時刻を測れないときに、送って待って測った時間（ミリ秒）。
    measured: Option<Vec<f64>>,
    ms: Option<f64>,
    marker: Option<Marker>,
    timing: Option<timing::Timing>,
    host_failed: bool,
    host_failure: Option<String>,
    /// 上限を越えているが、`keep` で外さずに掛けている。
    over: bool,
}

#[derive(Debug)]
pub struct Samplers {
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
}

/// （名前、スロット、`@group(1)`、マーカーのパス、ビルドできたか）
type Finished = (
    String,
    At,
    Option<(wgpu::BindGroupLayout, String)>,
    PathBuf,
    Result<Stage, String>,
);

struct Entry {
    name: String,
    at: At,
    instance: VideoInstance,
    stamp: u64,
    program: bool,
    key: Option<String>,
    marker: PathBuf,
}

pub struct Hooks {
    /// 直前のフィルターの出力（次のフィルターの計時の最初のパスがこれも読んで待つ）。
    after: Option<wgpu::TextureView>,
    generation: Option<u64>,
    entries: Vec<Entry>,
    deinterlace: Option<Stage>,
    picture: Vec<Stage>,
    key: Option<Key>,
    checked: Option<Instant>,
    frame: u32,
    seed: u32,
    samplers: Option<Samplers>,
    frames_layout: Option<(wgpu::BindGroupLayout, String)>,
    copy: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    building: Option<std::sync::mpsc::Receiver<(Vec<Finished>, Duration)>>,
    paused: Vec<Paused>,
    failed: Vec<(String, String)>,
    progress: Option<(Arc<AtomicU32>, u32, u32)>,
    building_for: Vec<String>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    device_name: String,
    program: Option<zv::Program>,
    program_seen: Option<zv::Program>,
    interval_ms: f64,
    paused_interval: f64,
    host: super::host::Host,
    host_jobs: Vec<String>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Hooks")
            .field(
                "entries",
                &self.entries.iter().map(|e| &e.name).collect::<Vec<_>>(),
            )
            .field("deinterlace", &self.deinterlace.as_ref().map(Stage::name))
            .field(
                "picture",
                &self.picture.iter().map(Stage::name).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Default for Hooks {
    fn default() -> Self {
        Self::new()
    }
}

/// デインタレースのスロットのプラグインがあるか（プールを深くするか）。
pub fn wants_frames() -> bool {
    crate::plugins::current().0.is_some_and(|loaded| {
        loaded
            .video()
            .iter()
            .any(|(plugin, _)| plugin.video_manifest().at.contains(&At::Deinterlace))
    })
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for stage in self.deinterlace.iter_mut().chain(&mut self.picture) {
            if let Some(marker) = stage.marker.take() {
                marker.clear();
            }
        }
    }
}

/// GPU の無い環境で WGSL の構文エラーを見つける。
#[cfg(test)]
mod tests;
