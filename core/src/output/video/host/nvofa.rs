// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! NVIDIA Optical Flow（NVOFA）を D3D12 で。構造体は `nvOpticalFlowCommon.h`・
//! `nvOpticalFlowD3D12.h`（MIT）から移した物。順序は一つのフェンスで GPU 側だけで守る。

use std::ffi::{c_char, c_void};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use super::bridge::{Lane, Worker};
use super::d3d12::{self, Fence};
use super::{Convert, Gpu, Run};

type Handle = *mut c_void;
type Status = i32;

const API_VERSION: u32 = 5 << 4;
const STATUS: [&str; 12] = [
    "SUCCESS",
    "OF_NOT_AVAILABLE",
    "UNSUPPORTED_DEVICE",
    "DEVICE_DOES_NOT_EXIST",
    "INVALID_PTR",
    "INVALID_PARAM",
    "INVALID_CALL",
    "INVALID_VERSION",
    "OUT_OF_MEMORY",
    "NOT_INITIALIZED",
    "UNSUPPORTED_FEATURE",
    "GENERIC",
];

const MODE_OPTICALFLOW: u32 = 1;
const FORMAT_GRAYSCALE8: u32 = 1;
const FORMAT_ABGR8: u32 = 3;
const CAPS_OUTPUT_GRID_SIZES: u32 = 0;
const USAGE_INPUT: u32 = 1;
const DIRECTION_BOTH: u32 = 2;
/// 5 遅い・10・20 速い。
const PERF_MEDIUM: u32 = 10;
const NVIDIA: u32 = 0x10de;

#[repr(C)]
#[derive(Clone, Copy)]
struct FencePoint {
    fence: *mut c_void,
    value: u64,
}

#[repr(C)]
struct RegisterParams {
    resource: *mut c_void,
    input_fence: FencePoint,
    buffer: *mut Handle,
    output_fence: FencePoint,
}

#[repr(C)]
struct UnregisterParams {
    buffer: Handle,
}

#[repr(C)]
struct InitParams {
    width: u32,
    height: u32,
    out_grid: u32,
    hint_grid: u32,
    mode: u32,
    perf: u32,
    external_hints: u32,
    output_cost: u32,
    private: *mut c_void,
    disparity_range: u32,
    roi: u32,
    direction: u32,
    global_flow: u32,
    input_format: u32,
}

#[repr(C)]
struct ExecuteIn {
    input: Handle,
    reference: Handle,
    hints: Handle,
    disable_temporal_hints: u32,
    padding: u32,
    private: *mut c_void,
    padding2: u32,
    rois: u32,
    roi_data: *mut c_void,
    padding3: u32,
    fence_points: u32,
    fence_point: *mut FencePoint,
}

#[repr(C)]
struct ExecuteOut {
    output: Handle,
    cost: Handle,
    private: *mut c_void,
    backward: Handle,
    backward_cost: Handle,
    global_flow: Handle,
    padding: [u32; 2],
    fence_point: *mut FencePoint,
}

type Fn<A> = Option<A>;

/// 使わないエントリーも、並びを合わせるために置く。
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct Functions {
    create: Fn<unsafe extern "system" fn(*mut c_void, *mut Handle) -> Status>,
    init: Fn<unsafe extern "system" fn(Handle, *const InitParams) -> Status>,
    format_count: Fn<unsafe extern "system" fn(Handle, u32, u32, *mut u32) -> Status>,
    format: Fn<unsafe extern "system" fn(Handle, u32, u32, *mut DXGI_FORMAT) -> Status>,
    register: Fn<unsafe extern "system" fn(Handle, *mut RegisterParams) -> Status>,
    unregister: Fn<unsafe extern "system" fn(*mut UnregisterParams) -> Status>,
    execute: Fn<unsafe extern "system" fn(Handle, *const ExecuteIn, *mut ExecuteOut) -> Status>,
    destroy: Fn<unsafe extern "system" fn(Handle) -> Status>,
    last_error: Fn<unsafe extern "system" fn(Handle, *mut c_char, *mut u32) -> Status>,
    caps: Fn<unsafe extern "system" fn(Handle, u32, *mut u32, *mut u32) -> Status>,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn library() -> *mut c_void {
    // 名前だけで探すのは実行ファイルのフォルダーと System32 だけ。
    crate::dl::load_library("nvofapi64.dll")
}

pub fn supported() -> bool {
    let module = library();
    // SAFETY: 開けた module と NUL 終端の名前。
    !module.is_null()
        && !unsafe { GetProcAddress(module, c"NvOFAPICreateInstanceD3D12".as_ptr()) }.is_null()
}

fn status(code: Status) -> String {
    STATUS
        .get(code as usize)
        .map_or_else(|| format!("{code}"), |s| (*s).to_owned())
}

pub struct Flow {
    functions: Functions,
    handle: Handle,
    /// レンダリング用 GPU と別なら [`Worker`] が転送する。
    worker: Worker,
    raw: ID3D12Device,
    /// 保持しておいたインスタンスを使い回すときに見分ける。
    render: ID3D12Device,
    fence: Fence,
    size: [u32; 2],
    /// NVOFA が出すのは、GPU が対応するグリッドのうち一番近い物。
    wanted: u32,
    /// 入力二つ、前→今と今→前のフロー、その信頼度（`handles` と同じ順）。
    lanes: Vec<Lane>,
    /// レンダリングしてから入力へコピーする。
    staging: [wgpu::Texture; 2],
    color: bool,
    handles: Vec<Handle>,
    has_prev: bool,
    /// 閉じる前に、そこへ送ったコマンドが終わるのを待つ。
    queue: Option<ID3D12CommandQueue>,
}

impl Flow {
    pub fn open(device: &wgpu::Device, size: [u32; 2], wanted: u32) -> Result<Self, String> {
        let module = library();
        if module.is_null() {
            return Err(String::from(
                "nvofapi64.dll を開けない（NVIDIA のドライバーが無い）",
            ));
        }
        // SAFETY: 開けた module。
        let create = unsafe { GetProcAddress(module, c"NvOFAPICreateInstanceD3D12".as_ptr()) };
        if create.is_null() {
            return Err(String::from("NvOFAPICreateInstanceD3D12 が無い"));
        }
        // SAFETY: ヘッダーの署名。
        let create: unsafe extern "system" fn(u32, *mut Functions) -> Status =
            unsafe { std::mem::transmute(create) };
        let mut functions = Functions::default();
        // SAFETY: 関数テーブルは有効なローカル変数。
        let code = unsafe { create(API_VERSION, &mut functions) };
        if code != 0 {
            return Err(format!("NvOFAPICreateInstanceD3D12：{}", status(code)));
        }
        let render = d3d12::device(device)?;
        let worker = Worker::open(device, NVIDIA)?;
        let raw = worker.device.clone();
        if let Some(flow) = reuse(&raw, &render, size, wanted) {
            return Ok(flow);
        }
        let mut handle: Handle = std::ptr::null_mut();
        let make = functions.create.ok_or("nvCreateOpticalFlowD3D12 が無い")?;
        // SAFETY: デバイスは有効、handle は書き込まれるローカル変数。
        let code = unsafe { make(raw.as_raw(), &mut handle) };
        if code != 0 {
            return Err(format!("nvCreateOpticalFlowD3D12：{}", status(code)));
        }
        // グリッドと入力フォーマットは `nvOFInit` の前にインスタンスに問い合わせられる。
        let caps = |which: u32| -> Vec<u32> {
            let Some(f) = functions.caps else {
                return Vec::new();
            };
            let mut count = 0u32;
            // SAFETY: 数だけ先に問い合わせる。
            if unsafe { f(handle, which, std::ptr::null_mut(), &mut count) } != 0 || count == 0 {
                return Vec::new();
            }
            let mut values = vec![0u32; count as usize];
            // SAFETY: 問い合わせた数のバッファ。
            if unsafe { f(handle, which, values.as_mut_ptr(), &mut count) } != 0 {
                return Vec::new();
            }
            values.truncate(count as usize);
            values
        };
        let have = caps(CAPS_OUTPUT_GRID_SIZES);
        let grid = super::flow::nearest_grid(&have, wanted).unwrap_or(zwsdk::video::HOST_FLOW_GRID);
        let inputs_have: Vec<DXGI_FORMAT> = (|| {
            let (Some(count), Some(format)) = (functions.format_count, functions.format) else {
                return Vec::new();
            };
            let mut n = 0;
            // SAFETY: 数を書くローカル変数。
            if unsafe { count(handle, USAGE_INPUT, MODE_OPTICALFLOW, &mut n) } != 0 || n == 0 {
                return Vec::new();
            }
            let mut list = vec![DXGI_FORMAT(0); n as usize];
            // SAFETY: 問い合わせた数のバッファ。
            if unsafe { format(handle, USAGE_INPUT, MODE_OPTICALFLOW, list.as_mut_ptr()) } != 0 {
                return Vec::new();
            }
            list
        })();
        // カラーの入力は、対応する 4 成分フォーマットのどれか。無ければ輝度。
        let four = inputs_have.iter().find_map(|format| match *format {
            DXGI_FORMAT_R8G8B8A8_UNORM => Some((*format, wgpu::TextureFormat::Rgba8Unorm)),
            DXGI_FORMAT_B8G8R8A8_UNORM => Some((*format, wgpu::TextureFormat::Bgra8Unorm)),
            _ => None,
        });
        let color = four.is_some();
        let (input_dxgi, input_wgpu, input_format) = match four {
            Some((dxgi, wgpu_format)) => (dxgi, wgpu_format, FORMAT_ABGR8),
            None => (
                DXGI_FORMAT_R8_UNORM,
                wgpu::TextureFormat::R8Unorm,
                FORMAT_GRAYSCALE8,
            ),
        };
        let init = InitParams {
            width: size[0],
            height: size[1],
            out_grid: grid,
            hint_grid: 0,
            mode: MODE_OPTICALFLOW,
            perf: PERF_MEDIUM,
            external_hints: 0,
            output_cost: 1,
            private: std::ptr::null_mut(),
            disparity_range: 0,
            roi: 0,
            direction: DIRECTION_BOTH,
            global_flow: 0,
            input_format,
        };
        let fence = Fence::new(&raw, false)?;
        let grid_size = [size[0].div_ceil(grid), size[1].div_ceil(grid)];
        let staging = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: input_wgpu,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        tracing::info!(
            "NVOFA: {}{}x{}、グリッド {grid}（要求 {wanted}、対応 {have:?}）、入力 {}（対応形式 {:?}）",
            if worker.manner().is_empty() {
                String::new()
            } else {
                format!("{}{}、", worker.name, worker.manner())
            },
            size[0],
            size[1],
            if color {
                "カラー（ABGR8）"
            } else {
                "輝度"
            },
            inputs_have.iter().map(|f| f.0).collect::<Vec<_>>()
        );
        let mut flow = Self {
            functions,
            handle,
            worker,
            raw,
            render,
            fence,
            size,
            wanted,
            lanes: Vec::new(),
            staging: [staging("zw nvofa in"), staging("zw nvofa in")],
            color,
            handles: Vec::new(),
            has_prev: false,
            queue: None,
        };
        let init_fn = flow.functions.init.ok_or("nvOFInit が無い")?;
        // SAFETY: 有効なローカル変数。
        flow.check("nvOFInit", unsafe { init_fn(flow.handle, &init) })?;
        let input_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let output_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        for _ in 0..2 {
            flow.register(
                device,
                size,
                input_dxgi,
                input_wgpu,
                D3D12_RESOURCE_FLAG_NONE,
                input_usage,
            )?;
        }
        for (dxgi, format) in [
            (DXGI_FORMAT_R16G16_SINT, wgpu::TextureFormat::Rg16Sint),
            (DXGI_FORMAT_R16G16_SINT, wgpu::TextureFormat::Rg16Sint),
            (DXGI_FORMAT_R8_UINT, wgpu::TextureFormat::R8Uint),
            (DXGI_FORMAT_R8_UINT, wgpu::TextureFormat::R8Uint),
        ] {
            flow.register(device, grid_size, dxgi, format, uav, output_usage)?;
        }
        Ok(flow)
    }

    fn check(&self, what: &str, code: Status) -> Result<(), String> {
        if code == 0 {
            return Ok(());
        }
        let mut text = [0 as c_char; 256];
        let mut size = text.len() as u32;
        if let Some(f) = self.functions.last_error {
            // SAFETY: 大きさを指定したローカルのバッファ。
            unsafe { f(self.handle, text.as_mut_ptr(), &mut size) };
        }
        // バッファの中だけを見る（NUL が無ければ全部）。
        let bytes: Vec<u8> = text
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        let detail = String::from_utf8_lossy(&bytes).into_owned();
        Err(format!(
            "{what}：{}{}",
            status(code),
            if detail.is_empty() {
                String::new()
            } else {
                format!("（{detail}）")
            }
        ))
    }

    fn register(
        &mut self,
        device: &wgpu::Device,
        size: [u32; 2],
        format: DXGI_FORMAT,
        wgpu_format: wgpu::TextureFormat,
        flags: D3D12_RESOURCE_FLAGS,
        usage: wgpu::TextureUsages,
    ) -> Result<(), String> {
        let lane = self.worker.lane(
            device,
            size,
            format,
            &[(wgpu_format, size)],
            flags,
            usage,
            "zw nvofa",
        )?;
        let resource = &lane.local;
        let mut handle: Handle = std::ptr::null_mut();
        let before = self.fence.value;
        let after = self.fence.next();
        let mut params = RegisterParams {
            resource: resource.as_raw(),
            input_fence: FencePoint {
                fence: self.fence.fence.as_raw(),
                value: before,
            },
            buffer: &mut handle,
            output_fence: FencePoint {
                fence: self.fence.fence.as_raw(),
                value: after,
            },
        };
        let register = self
            .functions
            .register
            .ok_or("nvOFRegisterResourceD3D12 が無い")?;
        // SAFETY: 有効なローカル変数。
        self.check("nvOFRegisterResourceD3D12", unsafe {
            register(self.handle, &mut params)
        })?;
        self.fence.wait(after)?;
        self.handles.push(handle);
        self.lanes.push(lane);
        Ok(())
    }

    pub fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        if !run.new_input {
            return Ok(());
        }
        if !self.has_prev {
            self.has_prev = true;
            convert.clear(gpu, run.target);
            return Ok(());
        }
        for (source, k) in [(run.prev, 0), (run.cur, 1)] {
            if self.color {
                convert.put(gpu, source, &self.staging[k]);
            } else {
                convert.luma(gpu, source, &self.staging[k]);
            }
            gpu.encoder.copy_texture_to_texture(
                self.staging[k].as_image_copy(),
                self.lanes[k].render().as_image_copy(),
                self.staging[k].size(),
            );
        }
        self.worker.send(gpu, &[&self.lanes[0], &self.lanes[1]])?;
        gpu.flush();
        let queue = self.worker.queue(gpu)?;
        self.queue = Some(queue.clone());
        let ready = self.fence.next();
        let done = self.fence.next();
        // wgpu のサブミットの後にシグナルする。
        d3d12::signal(&queue, &self.fence.fence, ready)?;
        let execute = self.functions.execute.ok_or("nvOFExecuteD3D12 が無い")?;
        let mut wait = FencePoint {
            fence: self.fence.fence.as_raw(),
            value: ready,
        };
        let mut signal = FencePoint {
            fence: self.fence.fence.as_raw(),
            value: done,
        };
        let input = ExecuteIn {
            input: self.handles[0],
            reference: self.handles[1],
            hints: std::ptr::null_mut(),
            disable_temporal_hints: 0,
            padding: 0,
            private: std::ptr::null_mut(),
            padding2: 0,
            rois: 0,
            roi_data: std::ptr::null_mut(),
            padding3: 0,
            fence_points: 1,
            fence_point: &mut wait,
        };
        let mut output = ExecuteOut {
            output: self.handles[2],
            cost: self.handles[4],
            private: std::ptr::null_mut(),
            backward: self.handles[3],
            backward_cost: self.handles[5],
            global_flow: std::ptr::null_mut(),
            padding: [0; 2],
            fence_point: &mut signal,
        };
        // SAFETY: 有効なローカル変数、登録した handle。
        self.check("nvOFExecuteD3D12", unsafe {
            execute(self.handle, &input, &mut output)
        })?;
        d3d12::wait(&queue, &self.fence.fence, done)?;
        let outputs: Vec<&Lane> = self.lanes[2..].iter().collect();
        self.worker.receive(gpu, &outputs)?;
        convert.flow(
            gpu,
            self.lanes[2].render(),
            self.lanes[3].render(),
            run.target,
        );
        Ok(())
    }
}

impl super::flow::Provider for Flow {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn wanted_grid(&self) -> u32 {
        self.wanted
    }

    fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        Flow::run(self, gpu, convert, run)
    }

    fn confidence(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        target: &wgpu::Texture,
    ) -> bool {
        convert.cost(gpu, self.lanes[4].render(), self.lanes[5].render(), target);
        true
    }

    fn retire(self: Box<Self>) {
        park(*self);
    }
}

// SAFETY: NVOFA のオブジェクトはこの `Flow` だけが持ち、`&mut self` の一つのスレッドで使う。
unsafe impl Send for Flow {}

/// NVOFA は破棄せずに保持して使い回す。`nvOFDestroy` のあと同じデバイスで D3D12 のデコーダーを作ると
/// クラッシュするため。
static SPARE: std::sync::Mutex<Vec<Flow>> = std::sync::Mutex::new(Vec::new());

/// 依頼した処理が終わるのを待ってから保持する。
pub fn park(mut flow: Flow) {
    flow.settle();
    flow.has_prev = false;
    if let Ok(mut spare) = SPARE.lock() {
        spare.push(flow);
    } else {
        std::mem::forget(flow);
    }
}

fn reuse(raw: &ID3D12Device, render: &ID3D12Device, size: [u32; 2], wanted: u32) -> Option<Flow> {
    let mut spare = SPARE.lock().ok()?;
    let at = spare.iter().position(|flow| {
        flow.raw == *raw && flow.render == *render && flow.size == size && flow.wanted == wanted
    })?;
    Some(spare.swap_remove(at))
}

impl Flow {
    fn settle(&mut self) {
        if let Some(queue) = self.queue.take() {
            let idle = self.fence.next();
            if let Err(why) = d3d12::signal(&queue, &self.fence.fence, idle) {
                tracing::warn!("NVOFA: 終了前の Signal に失敗: {why}");
            }
        }
        if let Err(why) = self.fence.wait(self.fence.value) {
            tracing::warn!("NVOFA: 終了前の待機でエラー（{why}）");
        }
    }
}

/// [`park`] を通らずに破棄した物だけがここへ来る。
impl Drop for Flow {
    fn drop(&mut self) {
        self.settle();
        if let Some(f) = self.functions.unregister {
            for &buffer in &self.handles {
                let mut params = UnregisterParams { buffer };
                // SAFETY: 登録した handle を一度ずつ。
                unsafe { f(&mut params) };
            }
        }
        if let Some(f) = self.functions.destroy {
            // SAFETY: 作った handle を一度だけ。
            unsafe { f(self.handle) };
        }
    }
}
