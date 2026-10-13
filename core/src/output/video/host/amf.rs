// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! AMD AMF の FRC：前と今の中間フレーム（[`zwsdk::video::HOST_INTERPOLATE`]、x2 だけ）。
//! AMD の GPU の D3D11 で処理させ、共有テクスチャと共有フェンス二つで受け渡す。
//! k フレーム目を渡した出力は k−1 と k の真ん中で、時刻は k フレーム目の物。

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::Interface;

use super::bridge::{Lane, Ticket, Worker, adapter_of};
use super::d3d12::{self, Fence};
use super::lag::{Frc as Lagging, SLOTS, slot};
use super::{Convert, Gpu};

type Status = i32;
const AMF_OK: Status = 0;
const AMF_EOF: Status = 23;
const AMF_REPEAT: Status = 24;
const AMF_INPUT_FULL: Status = 25;
const AMF_NEED_MORE_INPUT: Status = 44;

const AMD: u32 = 0x1002;
const HEADER_VERSION: u64 = (1 << 48) | (5 << 32) | (2 << 16);
const AMF_SURFACE_BGRA: i32 = 3;
const AMF_DX11_0: i32 = 110;
const AMF_VARIANT_BOOL: i32 = 1;
const AMF_VARIANT_INT64: i32 = 2;

// vtable のインデックス（AMF の `amf/public/include` から）。
const FACTORY_CREATE_CONTEXT: usize = 0;
const FACTORY_CREATE_COMPONENT: usize = 1;
const RELEASE: usize = 1;
const SET_PROPERTY: usize = 3;
const CONTEXT_TERMINATE: usize = 13;
const CONTEXT_INIT_DX11: usize = 18;
const CONTEXT_SURFACE_FROM_DX11: usize = 49;
const COMPONENT_INIT: usize = 17;
const COMPONENT_TERMINATE: usize = 19;
const COMPONENT_SUBMIT_INPUT: usize = 22;
const COMPONENT_QUERY_OUTPUT: usize = 23;
const DATA_SET_PTS: usize = 19;
const DATA_GET_PTS: usize = 20;
/// 引数は dstX, dstY, srcX, srcY, width, height。
const SURFACE_COPY_REGION: usize = 30;

const FRC_ENGINE_DX11: i64 = 3;
const FRC_ONLY_INTERPOLATED: i64 = 2;
const FRC_PROFILE_HIGH: i64 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    kind: i32,
    value: [u64; 2],
}

fn int(value: i64) -> Variant {
    Variant {
        kind: AMF_VARIANT_INT64,
        value: [value as u64, 0],
    }
}

fn boolean(value: bool) -> Variant {
    Variant {
        kind: AMF_VARIANT_BOOL,
        value: [u64::from(value), 0],
    }
}

/// # Safety
/// `this` は有効な AMF のオブジェクト、`F` はそのインデックスの実際の型。
unsafe fn call<F: Copy>(this: *mut c_void, index: usize) -> F {
    unsafe {
        let table = *this.cast::<*const *const c_void>();
        std::mem::transmute_copy(&*table.add(index))
    }
}

unsafe fn release(this: *mut c_void) {
    if !this.is_null() {
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void) -> i32 = call(this, RELEASE);
            f(this);
        }
    }
}

unsafe fn set_pts(data: *mut c_void, pts: i64) {
    unsafe {
        let f: unsafe extern "system" fn(*mut c_void, i64) = call(data, DATA_SET_PTS);
        f(data, pts);
    }
}

unsafe fn pts(data: *mut c_void) -> i64 {
    unsafe {
        let f: unsafe extern "system" fn(*mut c_void) -> i64 = call(data, DATA_GET_PTS);
        f(data)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn library() -> *mut c_void {
    crate::dl::load_library("amfrt64.dll")
}

pub fn supported() -> bool {
    !library().is_null()
}

/// 共有するフレーム 1 つ分（AMF のサーフェスはコンテキストを閉じる前に解放する）。
struct Shared {
    lane: Lane,
    _d3d11: ID3D11Texture2D,
    surface: *mut c_void,
}

pub struct Frc {
    worker: Worker,
    factory: *mut c_void,
    context: *mut c_void,
    _device: ID3D11Device,
    size: [u32; 2],
    inputs: Vec<Shared>,
    outs: Vec<Shared>,
    /// D3D12 → D3D11（入力を書き終えた）と、D3D11 → D3D12（コピーし終えた。D3D11 の側は [`Amf`]）。
    to_amf: (Fence, ID3D11Fence),
    from_amf: Fence,
    jobs: [Option<Job>; SLOTS],
    amf: Arc<Mutex<Amf>>,
    stop: Arc<AtomicBool>,
    picker: Option<std::thread::JoinHandle<()>>,
}

/// AMF のコンポーネントと、入力したがまだ出力されていない物。出力を受け取るスレッドとレンダリングスレッドがロックを取って使う。
struct Amf {
    component: *mut c_void,
    immediate: ID3D11DeviceContext4,
    from_amf: ID3D11Fence,
    outs: Vec<*mut c_void>,
    size: [u32; 2],
    pending: VecDeque<Pending>,
    made: [Option<u64>; SLOTS],
    frames: u64,
    gave_up: u64,
    failed: Option<String>,
}

// SAFETY: 生のポインターと D3D11 のオブジェクトはこの `Amf` だけが持ち、使うのはロックを取った一つのスレッド。
unsafe impl Send for Amf {}

struct Pending {
    index: u64,
    pts: i64,
    /// コピーし終えたら D3D11 がシグナルする `from_amf` の値。
    done: u64,
    since: Instant,
}

struct Job {
    index: u64,
    ticket: Ticket,
    landed: bool,
}

const PICK_EVERY: Duration = Duration::from_micros(500);

const GIVE_UP: Duration = Duration::from_millis(500);

impl Frc {
    pub fn open(device: &wgpu::Device, size: [u32; 2]) -> Result<Self, String> {
        let module = library();
        if module.is_null() {
            return Err(String::from(
                "amfrt64.dll を開けない（AMD のドライバーが無い）",
            ));
        }
        let symbol = |name: &std::ffi::CStr| {
            // SAFETY: 開けた module。
            let p = unsafe { GetProcAddress(module, name.as_ptr()) };
            (!p.is_null()).then_some(p)
        };
        let mut runtime = 0u64;
        if let Some(p) = symbol(c"AMFQueryVersion") {
            // SAFETY: `AMFQueryVersion_Fn(amf_uint64*)` に、書けるローカル変数を渡す。
            unsafe {
                let f: unsafe extern "C" fn(*mut u64) -> Status = std::mem::transmute(p);
                f(&mut runtime);
            }
        }
        let init = symbol(c"AMFInit").ok_or("AMFInit が無い")?;
        // SAFETY: `AMFInit_Fn(amf_uint64, AMFFactory**)`。
        let init: unsafe extern "C" fn(u64, *mut *mut c_void) -> Status =
            unsafe { std::mem::transmute(init) };
        let version = if runtime == 0 {
            HEADER_VERSION
        } else {
            runtime.min(HEADER_VERSION)
        };
        let mut factory = std::ptr::null_mut();
        // SAFETY: 出力は書けるローカル変数。
        let status = unsafe { init(version, &mut factory) };
        if status != AMF_OK || factory.is_null() {
            return Err(format!("AMFInit：AMF_RESULT {status}"));
        }

        let worker = Worker::open(device, AMD)?;
        tracing::info!("AMF: {} で補間{}", worker.name, worker.manner());
        let raw = worker.device.clone();
        let adapter = adapter_of(&raw)?;
        let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut made = None;
        // SAFETY: 出力先は有効なローカル変数。
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut made),
                None,
                None,
            )
        }
        .map_err(|e| format!("D3D11CreateDevice：{e}"))?;
        let d3d11: ID3D11Device = made.ok_or("D3D11 デバイスが無い")?;
        if let Ok(threads) = d3d11.cast::<ID3D11Multithread>() {
            // SAFETY: デバイスは有効。AMF は自分のスレッドからもコンテキストを使う。
            unsafe {
                let _ = threads.SetMultithreadProtected(true);
            }
        }
        let device1: ID3D11Device1 = d3d11
            .cast()
            .map_err(|e| format!("ID3D11Device1 が無い：{e}"))?;
        let device5: ID3D11Device5 = d3d11
            .cast()
            .map_err(|e| format!("ID3D11Device5 が無い：{e}"))?;
        // SAFETY: デバイスは有効。
        let immediate: ID3D11DeviceContext4 = unsafe { d3d11.GetImmediateContext() }
            .map_err(|e| e.to_string())?
            .cast()
            .map_err(|e| format!("ID3D11DeviceContext4 が無い：{e}"))?;

        let mut context = std::ptr::null_mut();
        // SAFETY: `CreateContext(AMFFactory*, AMFContext**)`。
        let status = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Status =
                call(factory, FACTORY_CREATE_CONTEXT);
            f(factory, &mut context)
        };
        if status != AMF_OK || context.is_null() {
            return Err(format!("CreateContext：AMF_RESULT {status}"));
        }
        // ここから先で失敗してもコンテキストは解放する（`Drop` は作り終えた物だけ）。
        let guard = ContextGuard(context);
        // SAFETY: `InitDX11(AMFContext*, void*, AMF_DX_VERSION)`。
        let status = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, i32) -> Status =
                call(context, CONTEXT_INIT_DX11);
            f(context, d3d11.as_raw(), AMF_DX11_0)
        };
        if status != AMF_OK {
            return Err(format!("InitDX11：AMF_RESULT {status}"));
        }
        let mut component = std::ptr::null_mut();
        let id = wide("AMFFRC");
        // SAFETY: `CreateComponent(AMFFactory*, AMFContext*, const wchar_t*, AMFComponent**)`。
        let status = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const u16,
                *mut *mut c_void,
            ) -> Status = call(factory, FACTORY_CREATE_COMPONENT);
            f(factory, context, id.as_ptr(), &mut component)
        };
        if status != AMF_OK || component.is_null() {
            return Err(format!(
                "CreateComponent(AMFFRC)：AMF_RESULT {status}（この GPU やドライバーに FRC が無い）"
            ));
        }
        let set = |name: &str, value: Variant| {
            let name = wide(name);
            // SAFETY: `SetProperty(AMFPropertyStorage*, const wchar_t*, AMFVariantStruct)`。
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *const u16, Variant) -> Status =
                    call(component, SET_PROPERTY);
                f(component, name.as_ptr(), value)
            }
        };
        for (name, value) in [
            ("FRCEngineType", int(FRC_ENGINE_DX11)),
            ("FRCMode", int(FRC_ONLY_INTERPOLATED)),
            ("FRCIndicator", boolean(false)),
            ("FRCEnableFallback", boolean(true)),
            ("FRCProfile", int(FRC_PROFILE_HIGH)),
            ("FRCMVSEARCHMODE", int(0)),
            ("FRCUseFutureFrame", boolean(false)),
        ] {
            let status = set(name, value);
            if status != AMF_OK {
                // SAFETY: 作ったコンポーネントを解放する。
                unsafe { release(component) };
                return Err(format!("SetProperty({name})：AMF_RESULT {status}"));
            }
        }
        // SAFETY: `Init(AMFComponent*, AMF_SURFACE_FORMAT, amf_int32, amf_int32)`。
        let status = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, i32, i32, i32) -> Status =
                call(component, COMPONENT_INIT);
            f(component, AMF_SURFACE_BGRA, size[0] as i32, size[1] as i32)
        };
        if status != AMF_OK {
            // SAFETY: 作ったコンポーネントを解放する。
            unsafe { release(component) };
            return Err(format!("FRC の Init：AMF_RESULT {status}"));
        }

        // ここから先で失敗しても、作ったサーフェスとコンポーネントは解放する（コンテキストは `guard` が後で）。
        let mut building = Building {
            component,
            surfaces: Vec::new(),
        };
        let mut shared = |label: &str| {
            let made = Self::shared(device, &worker, &device1, context, size, label)?;
            building.surfaces.push(made.surface);
            Ok::<_, String>(made)
        };
        let inputs = (0..SLOTS)
            .map(|_| shared("zw amf in"))
            .collect::<Result<Vec<_>, _>>()?;
        let outs = (0..SLOTS)
            .map(|_| shared("zw amf out"))
            .collect::<Result<Vec<_>, _>>()?;
        let fences = |raw: &ID3D12Device| -> Result<(Fence, ID3D11Fence), String> {
            let fence = Fence::new(raw, true)?;
            let opened = d3d12::d3d11_fence(raw, &fence.fence, &device5)?;
            Ok((fence, opened))
        };
        let to_amf = fences(&raw)?;
        let from_amf = fences(&raw)?;
        let amf = Arc::new(Mutex::new(Amf {
            component,
            immediate,
            from_amf: from_amf.1,
            outs: outs.iter().map(|shared| shared.surface).collect(),
            size,
            pending: VecDeque::new(),
            made: [None; SLOTS],
            frames: 0,
            gave_up: 0,
            failed: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let picker = {
            let (amf, stop) = (amf.clone(), stop.clone());
            std::thread::Builder::new()
                .name(String::from("zw amf pick"))
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if let Ok(mut amf) = amf.lock() {
                            amf.pick();
                        }
                        std::thread::sleep(PICK_EVERY);
                    }
                })
                .map_err(|e| format!("スレッドを作れない：{e}"))?
        };
        std::mem::forget(building);
        std::mem::forget(guard);
        Ok(Self {
            worker,
            factory,
            context,
            _device: d3d11,
            size,
            inputs,
            outs,
            to_amf,
            from_amf: from_amf.0,
            jobs: Default::default(),
            amf,
            stop,
            picker: Some(picker),
        })
    }

    fn shared(
        device: &wgpu::Device,
        worker: &Worker,
        device1: &ID3D11Device1,
        context: *mut c_void,
        size: [u32; 2],
        label: &str,
    ) -> Result<Shared, String> {
        let lane = worker.lane(
            device,
            size,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[(wgpu::TextureFormat::Bgra8Unorm, size)],
            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            label,
        )?;
        let raw = &worker.device;
        let resource = &lane.local;
        let d3d11 = d3d12::d3d11_texture(raw, resource, device1)?;
        let mut surface = std::ptr::null_mut();
        // SAFETY: `CreateSurfaceFromDX11Native`（コンテキスト、D3D11 のテクスチャ、出力、オブザーバー）。
        let status = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut *mut c_void,
                *mut c_void,
            ) -> Status = call(context, CONTEXT_SURFACE_FROM_DX11);
            f(context, d3d11.as_raw(), &mut surface, std::ptr::null_mut())
        };
        if status != AMF_OK || surface.is_null() {
            return Err(format!("CreateSurfaceFromDX11Native：AMF_RESULT {status}"));
        }
        Ok(Shared {
            lane,
            _d3d11: d3d11,
            surface,
        })
    }

    /// 入力 `index` の処理が、転送まで終わったか（依頼していなければ終わったとみなす）。
    fn settled(&self, index: u64) -> bool {
        match &self.jobs[slot(index)] {
            Some(job) if job.index >= index => {
                job.index > index || self.worker.fetched(&job.ticket)
            }
            _ => true,
        }
    }

    fn amf(&self) -> Result<std::sync::MutexGuard<'_, Amf>, String> {
        let amf = self
            .amf
            .lock()
            .map_err(|_| String::from("AMF のロックが壊れた"))?;
        match &amf.failed {
            Some(why) => Err(why.clone()),
            None => Ok(amf),
        }
    }
}

impl Amf {
    /// 出力を受け取る（受け取り用のスレッドから）。エラーは覚えておき、レンダリングスレッドが報告する。
    fn pick(&mut self) {
        if self.pending.is_empty() || self.failed.is_some() {
            return;
        }
        let result = self.query().and_then(|outs| self.take(outs));
        let result = result.and_then(|()| {
            while self
                .pending
                .front()
                .is_some_and(|pending| pending.since.elapsed() > GIVE_UP)
            {
                self.give_up_front()?;
            }
            Ok(())
        });
        if let Err(why) = result {
            self.failed = Some(why);
        }
    }

    /// AMF の出力を、待っている入力に対応させてコピーする。飛ばされた入力は諦める。
    fn take(&mut self, outs: Vec<*mut c_void>) -> Result<(), String> {
        let mut result = Ok(());
        for data in outs {
            // SAFETY: QueryOutput が返した有効なオブジェクト。
            let at = unsafe { pts(data) };
            if let Some(found) = self.pending.iter().position(|p| p.pts == at) {
                for _ in 0..found {
                    self.give_up_front()?;
                }
                if let Some(pending) = self.pending.pop_front() {
                    let copied = self.copy_out(data, pending.index);
                    if copied.is_err() && result.is_ok() {
                        result = copied.clone();
                    }
                    self.finish(&pending, copied.is_ok())?;
                }
            }
            // SAFETY: 受け取ったオブジェクトを一度だけ解放する。
            unsafe { release(data) };
        }
        result
    }

    fn copy_out(&self, data: *mut c_void, index: u64) -> Result<(), String> {
        let out = self.outs[slot(index)];
        // SAFETY: AMF のサーフェスから同じコンテキストの共有の出力へ。
        let status = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
            ) -> Status = call(data, SURFACE_COPY_REGION);
            f(
                data,
                out,
                0,
                0,
                0,
                0,
                self.size[0] as i32,
                self.size[1] as i32,
            )
        };
        if status != AMF_OK {
            return Err(format!("CopySurfaceRegion：AMF_RESULT {status}"));
        }
        Ok(())
    }

    /// コピーし終えたら D3D11 がシグナルする（出力されなかった物もシグナルする。しないと補間用 GPU のキューが止まる）。
    fn finish(&mut self, pending: &Pending, made: bool) -> Result<(), String> {
        d3d12::d3d11_signal(&self.immediate, &self.from_amf, pending.done)?;
        if made {
            self.made[slot(pending.index)] = Some(pending.index);
        }
        Ok(())
    }

    fn give_up_front(&mut self) -> Result<(), String> {
        let Some(pending) = self.pending.pop_front() else {
            return Ok(());
        };
        self.gave_up += 1;
        if self.gave_up == 1 || self.gave_up.is_power_of_two() {
            tracing::warn!(
                "AMF: 入力 {} の中間フレームが出ない（{} 回目）",
                pending.index,
                self.gave_up
            );
        }
        self.finish(&pending, false)
    }

    /// 入力する。受け付けられなければ出力を受け取ってから入力し直す。
    fn give(&mut self, surface: *mut c_void) -> Result<(), String> {
        for _ in 0..100 {
            // SAFETY: `SubmitInput(AMFComponent*, AMFData*)`。
            let status = unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Status =
                    call(self.component, COMPONENT_SUBMIT_INPUT);
                f(self.component, surface)
            };
            match status {
                AMF_OK => return Ok(()),
                AMF_INPUT_FULL => {
                    let outs = self.query()?;
                    self.take(outs)?;
                    std::thread::sleep(Duration::from_micros(200));
                }
                other => return Err(format!("SubmitInput：AMF_RESULT {other}")),
            }
        }
        Err(String::from("SubmitInput が受け取らない"))
    }

    fn query(&self) -> Result<Vec<*mut c_void>, String> {
        let mut out = Vec::new();
        loop {
            let mut data = std::ptr::null_mut();
            // SAFETY: `QueryOutput(AMFComponent*, AMFData**)`。
            let status = unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Status =
                    call(self.component, COMPONENT_QUERY_OUTPUT);
                f(self.component, &mut data)
            };
            if !data.is_null() {
                out.push(data);
            }
            match status {
                AMF_OK if !data.is_null() => continue,
                AMF_OK | AMF_REPEAT | AMF_NEED_MORE_INPUT | AMF_EOF => return Ok(out),
                other => return Err(format!("QueryOutput：AMF_RESULT {other}")),
            }
        }
    }
}

impl Lagging for Frc {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn chained(&self) -> bool {
        true
    }

    fn poll(&mut self, _gpu: &mut Gpu<'_>) -> Result<(), String> {
        self.amf().map(drop)
    }

    fn submit(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        _prev: &wgpu::Texture,
        cur: &wgpu::Texture,
    ) -> Result<bool, String> {
        // スロットを使い回す前に、同じスロットの処理と、それを前の入力として読む処理（index − 2）が終わっていること。
        if !self.settled(index.saturating_sub(SLOTS as u64 - 1)) {
            return Ok(false);
        }
        let at = slot(index);
        convert.put(gpu, cur, self.inputs[at].lane.render());
        self.worker.send(gpu, &[&self.inputs[at].lane])?;
        gpu.flush();
        let queue = self.worker.queue(gpu)?;
        let ready = self.to_amf.0.next();
        // wgpu のサブミットの後にシグナルし、D3D11 はそれを待つ。
        d3d12::signal(&queue, &self.to_amf.0.fence, ready)?;
        let done = self.from_amf.next();
        let ticket = self
            .worker
            .fetch(&[&self.outs[at].lane], &self.from_amf.fence, done)?;
        self.jobs[at] = Some(Job {
            index,
            ticket,
            landed: false,
        });
        let input = self.inputs[at].surface;
        let mut amf = self.amf()?;
        d3d12::d3d11_wait(&amf.immediate, &self.to_amf.1, ready)?;
        amf.frames += 1;
        let pts = amf.frames as i64 * 1000;
        // SAFETY: 有効なサーフェス。
        unsafe { set_pts(input, pts) };
        amf.made[at] = None;
        amf.pending.push_back(Pending {
            index,
            pts,
            done,
            since: Instant::now(),
        });
        amf.give(input)?;
        Ok(true)
    }

    fn ready(&mut self, index: u64) -> Result<bool, String> {
        let at = slot(index);
        let Some(job) = self.jobs[at].as_ref().filter(|job| job.index == index) else {
            return Ok(false);
        };
        let made = self.amf()?.made[at] == Some(index);
        Ok(made && self.worker.fetched(&job.ticket))
    }

    fn put(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        index: u64,
        target: &wgpu::Texture,
    ) -> Result<(), String> {
        let at = slot(index);
        let job = self.jobs[at]
            .as_mut()
            .filter(|job| job.index == index)
            .ok_or("AMF：依頼していない中間フレーム")?;
        if !job.landed {
            self.worker.land(gpu, &[&self.outs[at].lane], &job.ticket)?;
            job.landed = true;
        }
        convert.put(gpu, self.outs[at].lane.render(), target);
        Ok(())
    }
}

struct ContextGuard(*mut c_void);

struct Building {
    component: *mut c_void,
    surfaces: Vec<*mut c_void>,
}

impl Drop for Building {
    fn drop(&mut self) {
        // SAFETY: 作ったオブジェクトを、サーフェス・コンポーネントの順に一度ずつ解放する（コンテキストより先）。
        unsafe {
            for surface in self.surfaces.drain(..) {
                release(surface);
            }
            let f: unsafe extern "system" fn(*mut c_void) -> Status =
                call(self.component, COMPONENT_TERMINATE);
            f(self.component);
            release(self.component);
        }
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        // SAFETY: CreateContext が返したオブジェクトを、閉じてから一度だけ解放する。
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void) -> Status =
                call(self.0, CONTEXT_TERMINATE);
            f(self.0);
            release(self.0);
        }
    }
}

// SAFETY: 生のポインターはこの `Frc` だけが持つ AMF のオブジェクトで、使うのは `&mut self` の一つのスレッドだけ。
unsafe impl Send for Frc {}

impl Drop for Frc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(picker) = self.picker.take() {
            let _ = picker.join();
        }
        // 出力されていない分もシグナルしてから閉じる（補間用 GPU のキューがその値を待っている）。
        let component = match self.amf.lock() {
            Ok(mut amf) => {
                while !amf.pending.is_empty() {
                    if amf.give_up_front().is_err() {
                        break;
                    }
                }
                amf.component
            }
            Err(_) => return,
        };
        if let Err(why) = self.from_amf.wait(self.from_amf.value) {
            tracing::warn!("AMF: 終了前の待機でエラー（{why}）");
        }
        // SAFETY: 作ったオブジェクトを、サーフェス・コンポーネント・コンテキストの順に一度ずつ解放する。
        unsafe {
            for shared in self.inputs.iter_mut().chain(self.outs.iter_mut()) {
                release(shared.surface);
                shared.surface = std::ptr::null_mut();
            }
            let f: unsafe extern "system" fn(*mut c_void) -> Status =
                call(component, COMPONENT_TERMINATE);
            f(component);
            release(component);
            let f: unsafe extern "system" fn(*mut c_void) -> Status =
                call(self.context, CONTEXT_TERMINATE);
            f(self.context);
            release(self.context);
        }
        let _ = self.factory;
    }
}
