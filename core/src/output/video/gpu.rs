// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The GPU: the texture pool, the passes that filter and convert, and the surface.
//! Textures are allocated once; the cheaper upload path is measured at startup.

use std::sync::Arc;

use std::sync::atomic::{AtomicBool, Ordering};

use std::time::Instant;

use mediadecoder::es::Format;

use mediadecoder::video::{Frame, Plane};

use crate::decode::Decoded;

use super::colour::{Conversion, Mapping, Primaries, Space, Transfer};

use super::field::{self, Cadence, Parity};

use super::filter;

use super::schedule::Schedule;

use super::shader::{self, Params, Shape};

use super::{AFFORDABLE, Deinterlace, Error, FIELD_BUDGET_MS, Settings};

use super::{Scaler, Scaling, pan_scan};

#[cfg(feature = "plugins")]
use super::{HEADROOM, colour, hook, schedule};

#[cfg(any(
    feature = "vaapi",
    all(feature = "android", target_os = "android"),
    all(feature = "d3d12", windows),
    all(feature = "videotoolbox", target_vendor = "apple")
))]
use super::import;

mod device;
mod direct;
mod draw;
mod encode;
mod matcher;
mod pool;
mod push;
mod reduce;
mod setup;
mod sheet;
mod split;

pub use device::{lose_for_test, lost, mark_removed, matrices, removed};
use pool::padded;
pub use pool::{Capability, Pool, Upload};
pub use sheet::{Onto, Over, Sheet};
use sheet::{attachment, blit_group, blit_layout, build, display_sampler, fit_buffer};

#[derive(Debug)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub field: Option<Parity>,
    pub at: u8,
    pub of: u8,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Spent {
    pub upload: std::time::Duration,
    /// Under `Fifo` this is the vertical blank wait.
    pub acquire: std::time::Duration,
    pub draw: std::time::Duration,
    pub show: std::time::Duration,
    pub pushed: u64,
    pub shown: u64,
    /// 出力がビルドできずに前のフレームをコピーし直した数。
    pub shown_again: u64,
    /// 周期の検出が 5 フレームに 1 フレームの重複として抜いた数。0 が続くなら周期が合っていない。
    pub woven: u64,
    pub locked: bool,
    pub whole_frames: bool,
    /// ソフトテレシネか。フィールドマッチに通さない。
    pub soft: bool,
    pub matched: u64,
    pub film: bool,
    pub overlay: bool,
    pub film_changes: u64,
    pub combed: u64,
    pub stripes: u32,
}

fn counts(output: &field::Output) -> bool {
    output.at == 0 && output.field.is_some()
}

static LIVE_VIDEOS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Drop for Video {
    fn drop(&mut self) {
        let left = LIVE_VIDEOS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
        tracing::debug!("レンダリング: 映像レンダラーを破棄（使用中 {left} 個）");
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Blit {
    up: u32,
    down: u32,
    pass: u32,
}

#[derive(Debug)]
pub struct Video {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: wgpu::TextureFormat,
    space: Space,
    sdr_white: f32,
    hlg_peak: f32,
    /// Where within its letterbox the picture is drawn (`[left, top, width, height, aspect]`).
    inset: Option<[f32; 5]>,
    format: Format,
    settings: Settings,
    shape: Shape,
    upload: Upload,
    pool: Pool,
    /// The picture at its coded resolution. The filter has to run there.
    picture: wgpu::Texture,
    picture_view: wgpu::TextureView,
    planes: wgpu::BindGroupLayout,
    /// デインタレースのパイプライン。初めて要るときに作る（ビデオプロセスで処理するなら作らない）。
    /// 遅い PC ではコンパイルに数秒かかる。
    pipeline: Option<wgpu::RenderPipeline>,
    params: wgpu::Buffer,
    blits: Vec<(Blit, wgpu::RenderPipeline)>,
    blits_coming: Vec<(Blit, std::sync::mpsc::Receiver<wgpu::RenderPipeline>)>,
    pan_scan: super::pan_scan::PanScan,
    fit: wgpu::Buffer,
    #[cfg(feature = "vaapi")]
    import: Option<super::import::dmabuf::Import>,
    #[cfg(all(feature = "android", target_os = "android"))]
    import: Option<super::import::android::Import>,
    #[cfg(all(feature = "d3d12", windows))]
    import: Option<super::import::d3d12::Import>,
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    import: Option<super::import::videotoolbox::Import>,
    pushed: u64,
    slot_index: Vec<u64>,
    /// スロットのフレームが一枚で一フィールドなら、そのパリティー（HEVC の `field_seq_flag`）。
    lone: Vec<Option<Parity>>,
    /// 今見せているフレームの [`Video::lone`]。縦に伸ばすとき半行ずらす。
    lone_shown: Option<Parity>,
    /// 変換のパスを飛ばし、表示のパスで直に読むフレーム（[`direct`]）。
    direct: Option<direct::Direct>,
    direct_layout: Option<wgpu::BindGroupLayout>,
    direct_pipeline: Option<wgpu::RenderPipeline>,
    /// 前の表示で直に読めたか（画面へ出す大きさで決まる。初めは RGB を作って描く）。
    direct_fits: bool,
    last_progressive: bool,
    last_slot: Option<usize>,
    /// c/p を選ぶのは GPU、film か 60i かはこちら。
    matcher: Option<matcher::Matcher>,
    planes_entries: Vec<wgpu::BindGroupLayoutEntry>,
    drawn_once: bool,
    schedule: Schedule,
    scratch: Vec<u8>,
    spent: Spent,
    #[cfg(feature = "plugins")]
    hooks: super::hook::Hooks,
    sub: u32,
    /// スケジュールに入れた出力ごとに何回レンダリングするか（`push` したときの数）。レンダリング時の数だとフィルターが
    /// ビルドし直されて `push` の返り値と一致しなくなり、映像が止まる。
    pers: std::collections::VecDeque<u32>,
    phases: std::collections::VecDeque<Vec<f32>>,
    last_phases: Vec<f32>,
    retime_at: f64,
    display_mhz: u32,
    lost: usize,
    /// 使う拡大縮小（自動を決めた後）。
    scaling: super::Scaling,
    /// 頼まれた拡大縮小（自動のまま）と、測った GPU の強さ（[`super::Scaling::resolved`]）。
    asked_scaling: super::Scaling,
    strength: Option<Deinterlace>,
    adjust: super::Adjust,
    shown: Option<(wgpu::TextureView, [u32; 2], wgpu::TextureFormat)>,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    reduce: reduce::Reduce,
    split: split::Split,
    content: (f64, f64),
    #[cfg(feature = "plugins")]
    ahead: Ahead,
    /// 方式を自動のまま、シェーダーでデインタレースした・拡大縮小を自動で決めた（GPU を測ってよい）。
    shaded: bool,
}

struct Encoded {
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    chained: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    read: Vec<usize>,
    sub: u32,
    per: u32,
    shown: Shown,
}

/// GPU は一つのキューで進むので、1 回のレンダリングが一フレームの間隔を超えるものは間に合わない。
#[cfg(feature = "plugins")]
#[derive(Debug, Default)]
struct Ahead {
    ring: std::collections::VecDeque<Made>,
    spare: Vec<Made>,
    showing: Option<Made>,
}

#[cfg(feature = "plugins")]
#[derive(Debug)]
struct Made {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    shown: Shown,
}

#[cfg(feature = "plugins")]
const AHEAD_MEMORY: u64 = 512 << 20;

/// Eight or ten bits, each keeping its broadcast curve; four bytes a pixel either way.
pub fn picture_format(depth: u8) -> wgpu::TextureFormat {
    if depth > 8 {
        wgpu::TextureFormat::Rgb10a2Unorm
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

/// A memory budget (6 to 16): every waiting output pins three slots. An imported picture
/// gets [`crate::output::video::HELD_BY_FILTER`], which the decoders size their rings against.
fn slots(format: Format, shape: Shape) -> usize {
    if shape != Shape::Planar {
        return crate::output::video::HELD_BY_FILTER;
    }
    const BUDGET: u64 = 48 << 20;
    let bytes = u64::from(format.width)
        * u64::from(format.height)
        * if format.depth > 8 { 2 } else { 1 }
        * 3
        / 2;
    (BUDGET / bytes.max(1)).clamp(6, 16) as usize
}

/// フレームレート変換：長さ `step` の間に来る目標の時刻の位相。一つも来なければ 0 を一つ。
fn retime_phases(at: &mut f64, step: f64) -> Vec<f32> {
    let start = *at;
    let end = start + step;
    let mut phases: Vec<f32> = Vec::new();
    let mut tick = start.ceil();
    while tick < end {
        phases.push(((tick - start) / step) as f32);
        tick += 1.0;
    }
    // 端数だけを持ち続ける（長く回しても桁を失わない）。
    *at = end - start.floor();
    if phases.is_empty() {
        phases.push(0.0);
    }
    phases
}

#[cfg(test)]
mod retime_tests {
    use super::retime_phases;

    #[test]
    fn film_becomes_two_and_three_at_sixty() {
        let mut at = 0.0;
        let step = 60.0 / 24.0;
        let counts: Vec<usize> = (0..4).map(|_| retime_phases(&mut at, step).len()).collect();
        assert_eq!(counts.iter().sum::<usize>(), 10, "四フレームで十回の出力");
        assert!(counts.iter().all(|c| *c == 2 || *c == 3), "{counts:?}");
        let mut at = 0.0;
        assert_eq!(retime_phases(&mut at, step), vec![0.0, 0.4, 0.8]);
        assert_eq!(retime_phases(&mut at, step), vec![0.2, 0.6]);
    }

    #[test]
    fn a_slower_target_still_draws_each_input_once() {
        let mut at = 0.0;
        let all: usize = (0..120)
            .map(|_| retime_phases(&mut at, 59.94 / 60.0).len())
            .sum();
        assert_eq!(all, 120, "一回も来ない出力も一回はレンダリングする");
        assert!(at < 2.0, "端数だけ持つ");
    }
}
