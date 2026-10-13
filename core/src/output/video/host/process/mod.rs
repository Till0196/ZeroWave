// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 外部プロセスによる補間のバックエンド（`interpolate/process`）。利用者が入れた実行ファイルを起動し、
//! 前と今の 2 フレームと位相を渡して中間フレームを受け取る。プロトコルは [`zwsdk::video::process`]。
//! 転送方式は OPEN で決める（レンダリングする GPU の API の方式が先、メインメモリの共有 [`shm`] が最後）。
//! 準備（OPEN）は別のスレッドで待ち、チェーンを作り直しても捨てずに次のチェーンへ渡す（[`OPENINGS`]）。

mod child;
mod courier;
#[cfg(all(feature = "d3d12", windows))]
mod d3d12;
mod desk;
#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
mod metal;
mod planar;
mod shm;
#[cfg(all(any(feature = "nvdec", feature = "vaapi"), target_os = "linux"))]
mod vulkan;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zwsdk::video as zv;
use zwsdk::video::process::{self as wire, Format, Way as WayId};

use self::child::{Link, Said};
use self::courier::{Courier, Errand, Then};
use self::desk::Desk;
use self::planar::Planar;
use super::lag::{Frc, SLOTS, slot};
use super::{Convert, Gpu};

/// 処理が完了しないまま、プロセスの生存を確かめるまでの時間。
const STUCK: Duration = Duration::from_secs(2);
const PHASE: f32 = 0.5;

/// 転送方式ごとの、レンダリングする GPU の側の処理。入力のスロットへ書き込み、出力のスロットから読み出す。
trait Way: Send {
    fn stride(&self) -> u32;
    fn input(&self, slot: usize) -> &wgpu::Buffer;
    fn output(&self, slot: usize) -> &wgpu::Buffer;
    fn share(&mut self, link: &mut Link) -> Result<(), String>;
    /// 書き込み終えた後、相手へ渡すコマンドを記録してサブミットする。`packed` は今回書き込んだ入力のスロット。
    fn hand_over(&mut self, gpu: &mut Gpu<'_>, packed: &[usize]) -> Result<Handed, String>;
    fn done(&self, value: u64) -> Option<bool>;
    fn take_back(&mut self, gpu: &mut Gpu<'_>, value: u64, slot: usize) -> Result<(), String>;
}

struct Handed {
    value: u64,
    after: Option<(wgpu::SubmissionIndex, Option<Then>)>,
}

/// このデバイスで使える転送方式（優先順）。`allowed` が空でなければその名前だけ。
fn offers(device: &wgpu::Device, allowed: &[String]) -> Vec<wire::Offer> {
    let mut found = Vec::new();
    #[cfg(all(feature = "d3d12", windows))]
    if let Some(id) = d3d12::id(device) {
        found.push(wire::Offer::new(WayId::D3d12, id));
    }
    #[cfg(all(any(feature = "nvdec", feature = "vaapi"), target_os = "linux"))]
    if let Some(id) = vulkan::id(device) {
        found.push(wire::Offer::new(WayId::Vulkan, id));
    }
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    if let Some(id) = metal::id(device) {
        found.push(wire::Offer::new(WayId::Metal, id));
    }
    let _ = device;
    found.push(wire::Offer::new(WayId::Shm, Vec::new()));
    only(found, allowed)
}

fn only(mut offers: Vec<wire::Offer>, allowed: &[String]) -> Vec<wire::Offer> {
    offers.retain(|offer| {
        allowed.is_empty()
            || WayId::of(offer.way).is_some_and(|way| allowed.iter().any(|a| a == way.name()))
    });
    offers
}

pub(super) fn supported(device: &wgpu::Device) -> bool {
    let limits = device.limits();
    limits.max_storage_buffers_per_shader_stage >= 2
        && limits.max_compute_workgroups_per_dimension > 0
        && !offers(device, &[]).is_empty()
}

fn make_way(
    gpu: &Gpu<'_>,
    way: WayId,
    format: Format,
    size: [u32; 2],
    jobs: u32,
) -> Result<Box<dyn Way>, String> {
    let (inputs, middles) = (wire::inputs(jobs) as usize, jobs as usize);
    let device = gpu.device;
    match way {
        #[cfg(all(feature = "d3d12", windows))]
        WayId::D3d12 => Ok(Box::new(d3d12::D3d12::new(
            device, format, size, inputs, middles,
        )?)),
        #[cfg(all(any(feature = "nvdec", feature = "vaapi"), target_os = "linux"))]
        WayId::Vulkan => Ok(Box::new(vulkan::Vulkan::new(
            device, format, size, inputs, middles,
        )?)),
        #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
        WayId::Metal => Ok(Box::new(metal::Metal::new(
            device, format, size, inputs, middles,
        )?)),
        WayId::Shm => Ok(Box::new(shm::Shm::new(
            device, format, size, inputs, middles,
        )?)),
        #[allow(unreachable_patterns)]
        other => Err(format!(
            "このホストは転送方式 {} に対応しない",
            other.name()
        )),
    }
}

type Opened = (Link, WayId, Format, u32);
type Outcome = Arc<Mutex<Option<Result<Opened, String>>>>;

/// 進行中の準備。数分かかることがあるので、チェーンが作り直されても捨てずに次のチェーンへ渡す。
struct Opening {
    key: (zv::Launch, [u32; 2], Vec<wire::Offer>),
    said: Said,
    outcome: Outcome,
    cancel: Arc<AtomicBool>,
}

/// OPEN の応答を待つ長さ（TensorRT のエンジンを作るのは何分もかかる）。
const OPEN_PATIENCE: Duration = Duration::from_secs(30 * 60);

static OPENINGS: Mutex<Vec<Opening>> = Mutex::new(Vec::new());

fn open_link(
    spec: &zv::Launch,
    size: [u32; 2],
    mut offers: Vec<wire::Offer>,
    said: &Said,
    cancel: &AtomicBool,
) -> Result<Opened, String> {
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    let service = metal::service_name();
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    for offer in &mut offers {
        if offer.way == WayId::Metal as u32 {
            offer.id.extend_from_slice(service.as_bytes());
        }
    }
    let mut link = Link::spawn(spec, said, &[])?;
    let open = wire::Open {
        width: size[0],
        height: size[1],
        jobs: wire::JOBS,
        offers: std::mem::take(&mut offers),
        formats: vec![Format::F32 as u32, Format::F16 as u32],
    };
    link.send(wire::OPEN, &open.encode())?;
    let reply = link
        .wait_cancellable(OPEN_PATIENCE, cancel)
        .map_err(|why| match link.dead() {
            Some(dead) => format!("{why}（{dead}）"),
            None => why,
        })?;
    let chosen = wire::Opened::decode(&reply)?;
    let way = WayId::of(chosen.way)
        .filter(|way| open.offers.iter().any(|offer| offer.way == *way as u32))
        .ok_or_else(|| {
            format!(
                "{} が提示していない転送方式（{}）を選んだ",
                link.name, chosen.way
            )
        })?;
    let format = Format::of(chosen.format).ok_or_else(|| {
        format!(
            "{} が未対応のフォーマット（{}）を選んだ",
            link.name, chosen.format
        )
    })?;
    if !(1..=open.jobs).contains(&chosen.jobs) {
        return Err(format!(
            "{} が返した並列数（{}）が 1〜{} にない",
            link.name, chosen.jobs, open.jobs
        ));
    }
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    if way == WayId::Metal {
        link.port = Some(metal::look_up(&service)?);
    }
    Ok((link, way, format, chosen.jobs))
}

/// 同じ準備が進行中ならそれを使い、無ければ始める。
fn join_opening(
    spec: &zv::Launch,
    size: [u32; 2],
    offers: Vec<wire::Offer>,
) -> Result<(Said, Outcome), String> {
    let key = (spec.clone(), size, offers.clone());
    let mut openings = OPENINGS.lock().unwrap_or_else(|e| e.into_inner());
    // 別の準備は完了していれば解放し、進行中なら取りやめる（スレッドと相手のプロセスが残らないように）。
    openings.retain(|opening| {
        let failed = opening
            .outcome
            .lock()
            .map_or(true, |done| matches!(*done, Some(Err(_))));
        let other = opening.key != key;
        if other {
            opening.cancel.store(true, Ordering::Relaxed);
        }
        !(failed || other)
    });
    if let Some(opening) = openings.iter().find(|opening| opening.key == key) {
        return Ok((opening.said.clone(), opening.outcome.clone()));
    }
    let said: Said = Arc::default();
    let outcome: Outcome = Arc::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let (theirs, written, stop) = (said.clone(), outcome.clone(), cancel.clone());
    let owned = spec.clone();
    std::thread::Builder::new()
        .name(String::from("zw process open"))
        .spawn(move || {
            let opened = open_link(&owned, size, offers, &theirs, &stop);
            if let Ok(mut slot) = written.lock() {
                *slot = Some(opened);
            }
        })
        .map_err(|e| format!("スレッドを作れない：{e}"))?;
    tracing::info!(
        "外部プロセスの補間: {} を起動（準備ができるまで補間なし）",
        spec.program
    );
    openings.push(Opening {
        key,
        said: said.clone(),
        outcome: outcome.clone(),
        cancel,
    });
    Ok((said, outcome))
}

fn take_opening(outcome: &Outcome) -> Option<Result<Opened, String>> {
    let taken = outcome.lock().ok()?.take()?;
    let mut openings = OPENINGS.lock().unwrap_or_else(|e| e.into_inner());
    openings.retain(|opening| !Arc::ptr_eq(&opening.outcome, outcome));
    Some(taken)
}

struct Running {
    /// RUN を送るスレッド。接続より先に止める。
    courier: Option<Courier>,
    link: Link,
    way: Box<dyn Way>,
    sync: wire::Sync,
    planar: Planar,
    outs: Vec<wgpu::Texture>,
    landed: [Option<u64>; SLOTS],
    desk: Desk,
    work_ms: f64,
    way_id: WayId,
    format: Format,
    jobs: u32,
}

enum State {
    Opening(Outcome),
    Running(Box<Running>),
    Failed(String),
}

pub struct Process {
    spec: zv::Launch,
    size: [u32; 2],
    state: State,
    began: Instant,
    said: Said,
    told: Option<(Option<u32>, String)>,
    news: Option<zv::Event>,
}

impl Process {
    pub fn open(device: &wgpu::Device, spec: &zv::Launch, size: [u32; 2]) -> Result<Self, String> {
        if spec.program.is_empty() {
            return Err(String::from("起動する実行ファイルの指定が無い"));
        }
        let offers = offers(device, &spec.ways);
        if offers.is_empty() {
            return Err(format!(
                "このホストは転送方式 {} に対応しない",
                spec.ways.join("・")
            ));
        }
        let (said, outcome) = join_opening(spec, size, offers)?;
        Ok(Self {
            spec: spec.clone(),
            size,
            state: State::Opening(outcome),
            began: Instant::now(),
            said,
            told: None,
            news: None,
        })
    }

    pub fn spec(&self) -> &zv::Launch {
        &self.spec
    }

    fn take_news(&mut self) -> Option<zv::Event> {
        if let Some(news) = self.news.take() {
            return Some(news);
        }
        if !matches!(self.state, State::Opening(_)) {
            return None;
        }
        let (progress, said) = self.said.lock().ok()?.clone()?;
        let key = (progress.map(|p| (p * 100.0) as u32), said.clone());
        if self.told.as_ref() == Some(&key) {
            return None;
        }
        self.told = Some(key);
        Some(zv::Event::Preparing { progress, said })
    }

    /// 開き終えていれば共有リソースを渡して開始する。動いていれば true。
    fn advance(&mut self, gpu: &Gpu<'_>) -> Result<bool, String> {
        if let State::Opening(outcome) = &self.state {
            let Some(opened) = take_opening(outcome) else {
                return Ok(false);
            };
            self.state = match opened.and_then(|opened| start(gpu, self.size, opened)) {
                Ok(running) => {
                    tracing::info!(
                        "外部プロセスの補間: {}x{}（転送方式 {}、形式 {:?}、並列数 {}、準備 {:.1} 秒）",
                        self.size[0],
                        self.size[1],
                        running.way_id.name(),
                        running.format,
                        running.jobs,
                        self.began.elapsed().as_secs_f64()
                    );
                    if self.told.is_some() {
                        self.news = Some(zv::Event::Prepared);
                    }
                    State::Running(running)
                }
                Err(why) => State::Failed(why),
            };
        }
        match &self.state {
            State::Opening(_) => Ok(false),
            State::Running(_) => Ok(true),
            State::Failed(why) => Err(format!("外部プロセスの補間：{why}")),
        }
    }

    fn fail_on<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        result.map_err(|why| {
            self.state = State::Failed(why.clone());
            format!("外部プロセスの補間：{why}")
        })
    }

    fn pump(&mut self) -> Result<(), String> {
        let State::Running(running) = &mut self.state else {
            return Ok(());
        };
        let result = running.pump();
        self.fail_on(result)
    }
}

fn start(
    gpu: &Gpu<'_>,
    size: [u32; 2],
    (mut link, way_id, format, jobs): Opened,
) -> Result<Box<Running>, String> {
    let device = gpu.device;
    let mut way = make_way(gpu, way_id, format, size, jobs)?;
    way.share(&mut link)?;
    let sync = way_id.sync();
    let courier = match sync {
        wire::Sync::Notify => Some(Courier::new(device.clone(), link.writer(), &link.name)?),
        wire::Sync::Fence => None,
    };
    let planar = Planar::new(device, size, way.stride(), format);
    let outs = (0..SLOTS)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("zw process mid"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect();
    Ok(Box::new(Running {
        courier,
        link,
        way,
        sync,
        planar,
        outs,
        landed: [None; SLOTS],
        desk: Desk::new(jobs),
        work_ms: 0.0,
        way_id,
        format,
        jobs,
    }))
}

impl Running {
    fn pump(&mut self) -> Result<(), String> {
        if let Some(courier) = &self.courier {
            courier.failed()?;
        }
        while let Some(reply) = self.link.try_reply() {
            let ran = wire::Ran::decode(&reply?)?;
            if let Some(ms) = ran.ms() {
                self.work_ms = f64::from(ms);
            }
            if self.sync == wire::Sync::Notify && !self.desk.finish(ran.value) {
                return Err(format!(
                    "{} が要求していない応答（{}）を返した",
                    self.link.name, ran.value
                ));
            }
        }
        if self.sync == wire::Sync::Fence {
            let way = &self.way;
            self.desk.refresh(|value| way.done(value).unwrap_or(false));
        }
        Ok(())
    }

    fn land(
        &mut self,
        gpu: &mut Gpu<'_>,
        middle: usize,
        index: u64,
        value: u64,
    ) -> Result<(), String> {
        let at = slot(index);
        if self.landed[at] == Some(index) {
            return Ok(());
        }
        self.way.take_back(gpu, value, middle)?;
        self.planar
            .unpack(gpu, self.way.output(middle), &self.outs[at]);
        self.landed[at] = Some(index);
        Ok(())
    }

    fn check(&mut self) -> Result<(), String> {
        if self.desk.pending().any(|job| job.since.elapsed() > STUCK) {
            return Err(match self.link.dead() {
                Some(dead) => dead,
                None => format!(
                    "{} が {} 秒たっても処理を終えない",
                    self.link.name,
                    STUCK.as_secs()
                ),
            });
        }
        Ok(())
    }

    /// 入力 `index` の処理を要求する。空きが無ければ false。
    fn submit(
        &mut self,
        gpu: &mut Gpu<'_>,
        index: u64,
        prev: &wgpu::Texture,
        cur: &wgpu::Texture,
    ) -> Result<bool, String> {
        self.check()?;
        let Some(plan) = self.desk.plan(index) else {
            return Ok(false);
        };
        if let Some(old) = self.desk.held(plan.middle) {
            let (old_index, old_value) = (old.index, old.value);
            self.land(gpu, plan.middle, old_index, old_value)?;
        }
        for &(at, input) in &plan.pack {
            let picture = if input == index { cur } else { prev };
            self.planar.pack(gpu, picture, self.way.input(at));
        }
        let packed: Vec<usize> = plan.pack.iter().map(|&(at, _)| at).collect();
        let handed = self.way.hand_over(gpu, &packed)?;
        self.desk.start(&plan, index, handed.value, Instant::now());
        let run = wire::Run {
            value: handed.value,
            phase: PHASE,
            previous: plan.previous as u32,
            current: plan.current as u32,
            middle: plan.middle as u32,
        };
        match (&self.courier, handed.after) {
            (Some(courier), Some((after, then))) => courier.send(Errand { after, then, run })?,
            _ => self.link.send(wire::RUN, &run.encode())?,
        }
        Ok(true)
    }

    fn ready(&mut self, index: u64) -> Result<bool, String> {
        if self.landed[slot(index)] == Some(index) {
            return Ok(true);
        }
        self.pump()?;
        match self.desk.find(index) {
            Some((_, job)) if job.done => Ok(true),
            Some(_) => self.check().map(|()| false),
            None => Ok(false),
        }
    }

    fn put(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        target: &wgpu::Texture,
    ) -> Result<(), String> {
        let at = slot(index);
        if self.landed[at] != Some(index) {
            let (middle, value) = match self.desk.find(index) {
                Some((middle, job)) if job.done => (middle, job.value),
                _ => return Err(String::from("外部プロセス：中間フレームが完了していない")),
            };
            self.land(gpu, middle, index, value)?;
        }
        convert.put(gpu, &self.outs[at], target);
        Ok(())
    }
}

impl Frc for Process {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn chained(&self) -> bool {
        false
    }

    fn poll(&mut self, _gpu: &mut Gpu<'_>) -> Result<(), String> {
        self.pump()
    }

    fn submit(
        &mut self,
        gpu: &mut Gpu<'_>,
        _convert: &mut Convert,
        index: u64,
        prev: &wgpu::Texture,
        cur: &wgpu::Texture,
    ) -> Result<bool, String> {
        if !self.advance(gpu)? {
            return Ok(false);
        }
        self.pump()?;
        let State::Running(running) = &mut self.state else {
            return Ok(false);
        };
        let result = running.submit(gpu, index, prev, cur);
        self.fail_on(result)
    }

    fn ready(&mut self, index: u64) -> Result<bool, String> {
        let State::Running(running) = &mut self.state else {
            return Ok(false);
        };
        let result = running.ready(index);
        self.fail_on(result)
    }

    fn put(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        target: &wgpu::Texture,
    ) -> Result<(), String> {
        let State::Running(running) = &mut self.state else {
            return Err(String::from("外部プロセスを開いていない"));
        };
        let result = running.put(gpu, convert, index, target);
        self.fail_on(result)
    }

    fn news(&mut self, gpu: &mut Gpu<'_>) -> Option<zv::Event> {
        if matches!(self.state, State::Opening(_)) {
            let _ = self.advance(gpu);
        }
        self.take_news()
    }

    fn work_ms(&self) -> f64 {
        match &self.state {
            State::Running(running) => running.work_ms,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests;
