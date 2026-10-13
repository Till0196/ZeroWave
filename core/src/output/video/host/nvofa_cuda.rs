// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! NVIDIA の NVOFA を CUDA で使う（Linux）：前と今のフレーム間のオプティカルフロー（補間方式の名前は `nvofa`）。
//! 構造体は `nvOpticalFlowCommon.h`・`nvOpticalFlowCuda.h`（MIT）から移した物。方向ごとに一つずつ開く。
//! 同じ NVIDIA の GPU なら Vulkan のバッファとセマフォを CUDA へインポートし、別ならメインメモリを通す。

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::os::fd::{AsRawFd, IntoRawFd};
use std::sync::OnceLock;

use ash::vk;

use super::staging::{self, Layout, Staging};
use super::vulkan::{Context, Exported, Timeline};
use super::{Convert, Gpu, Run};
use crate::dl::Object;

type CUresult = c_int;
type CUdevice = c_int;
type CUdeviceptr = u64;
type Ptr = *mut c_void;
type Handle = *mut c_void;
type Status = c_int;

const CUDA_SUCCESS: CUresult = 0;
const CU_MEMORYTYPE_HOST: c_int = 1;
const CU_MEMORYTYPE_DEVICE: c_int = 2;
const CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD: c_int = 1;
const CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_TIMELINE_SEMAPHORE_FD: c_int = 9;
const CUDA_EXTERNAL_MEMORY_DEDICATED: c_uint = 1;

const API_VERSION: u32 = 2 << 4;
const MODE_OPTICALFLOW: u32 = 1;
const PERF_MEDIUM: u32 = 10;
const USAGE_INPUT: u32 = 1;
const USAGE_OUTPUT: u32 = 2;
const USAGE_COST: u32 = 4;
const FORMAT_GRAYSCALE8: u32 = 1;
const FORMAT_ABGR8: u32 = 3;
const FORMAT_SHORT2: u32 = 5;
const FORMAT_UINT8: u32 = 7;
const BUFFER_CUDEVICEPTR: u32 = 2;
const CAPS_OUTPUT_GRID_SIZES: u32 = 0;
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

#[repr(C)]
#[derive(Clone, Copy)]
struct Memcpy2D {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: c_int,
    src_host: *const c_void,
    src_device: CUdeviceptr,
    src_array: Ptr,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: c_int,
    dst_host: Ptr,
    dst_device: CUdeviceptr,
    dst_array: Ptr,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

impl Default for Memcpy2D {
    fn default() -> Self {
        Self {
            src_x_in_bytes: 0,
            src_y: 0,
            src_memory_type: 0,
            src_host: std::ptr::null(),
            src_device: 0,
            src_array: std::ptr::null_mut(),
            src_pitch: 0,
            dst_x_in_bytes: 0,
            dst_y: 0,
            dst_memory_type: 0,
            dst_host: std::ptr::null_mut(),
            dst_device: 0,
            dst_array: std::ptr::null_mut(),
            dst_pitch: 0,
            width_in_bytes: 0,
            height: 0,
        }
    }
}

/// ハンドルの共用体（fd は先頭の 4 バイト、サイズとアライメントはポインター二つ分）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FdHandle {
    fd: c_int,
    _pad: u32,
    _rest: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ExternalMemoryHandleDesc {
    kind: c_int,
    handle: FdHandle,
    size: u64,
    flags: c_uint,
    reserved: [c_uint; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ExternalMemoryBufferDesc {
    offset: u64,
    size: u64,
    flags: c_uint,
    reserved: [c_uint; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ExternalSemaphoreHandleDesc {
    kind: c_int,
    handle: FdHandle,
    flags: c_uint,
    reserved: [c_uint; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ExternalSemaphoreParams {
    value: u64,
    params_reserved: [c_uint; 16],
    flags: c_uint,
    reserved: [c_uint; 16],
}

#[allow(clippy::type_complexity)]
struct Cuda {
    _object: Object,
    init: unsafe extern "C" fn(c_uint) -> CUresult,
    get_error_name: unsafe extern "C" fn(CUresult, *mut *const c_char) -> CUresult,
    device_get_count: unsafe extern "C" fn(*mut c_int) -> CUresult,
    device_get: unsafe extern "C" fn(*mut CUdevice, c_int) -> CUresult,
    device_get_name: unsafe extern "C" fn(*mut c_char, c_int, CUdevice) -> CUresult,
    device_get_uuid: unsafe extern "C" fn(*mut [u8; 16], CUdevice) -> CUresult,
    primary_ctx_retain: unsafe extern "C" fn(*mut Ptr, CUdevice) -> CUresult,
    primary_ctx_release: unsafe extern "C" fn(CUdevice) -> CUresult,
    ctx_push: unsafe extern "C" fn(Ptr) -> CUresult,
    ctx_pop: unsafe extern "C" fn(*mut Ptr) -> CUresult,
    stream_create: unsafe extern "C" fn(*mut Ptr, c_uint) -> CUresult,
    stream_destroy: unsafe extern "C" fn(Ptr) -> CUresult,
    stream_synchronize: unsafe extern "C" fn(Ptr) -> CUresult,
    memcpy_2d_async: unsafe extern "C" fn(*const Memcpy2D, Ptr) -> CUresult,
    mem_free: unsafe extern "C" fn(CUdeviceptr) -> CUresult,
    import_external_memory:
        unsafe extern "C" fn(*mut Ptr, *const ExternalMemoryHandleDesc) -> CUresult,
    external_memory_get_mapped_buffer:
        unsafe extern "C" fn(*mut CUdeviceptr, Ptr, *const ExternalMemoryBufferDesc) -> CUresult,
    destroy_external_memory: unsafe extern "C" fn(Ptr) -> CUresult,
    import_external_semaphore:
        unsafe extern "C" fn(*mut Ptr, *const ExternalSemaphoreHandleDesc) -> CUresult,
    signal_external_semaphores:
        unsafe extern "C" fn(*const Ptr, *const ExternalSemaphoreParams, c_uint, Ptr) -> CUresult,
    wait_external_semaphores:
        unsafe extern "C" fn(*const Ptr, *const ExternalSemaphoreParams, c_uint, Ptr) -> CUresult,
    destroy_external_semaphore: unsafe extern "C" fn(Ptr) -> CUresult,
}

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: CUDA のエントリーポイントで、型はヘッダーが宣言している署名。
        unsafe { $object.symbol($name) }.map_err(|missing| missing.0)?
    };
}

impl Cuda {
    fn get() -> Result<&'static Self, String> {
        static CUDA: OnceLock<Result<Cuda, String>> = OnceLock::new();
        CUDA.get_or_init(Self::load).as_ref().map_err(Clone::clone)
    }

    fn load() -> Result<Self, String> {
        let object = Object::open("libcuda.so.1").map_err(|missing| missing.0)?;
        let cuda = Self {
            init: symbol!(object, b"cuInit\0"),
            get_error_name: symbol!(object, b"cuGetErrorName\0"),
            device_get_count: symbol!(object, b"cuDeviceGetCount\0"),
            device_get: symbol!(object, b"cuDeviceGet\0"),
            device_get_name: symbol!(object, b"cuDeviceGetName\0"),
            device_get_uuid: symbol!(object, b"cuDeviceGetUuid\0"),
            primary_ctx_retain: symbol!(object, b"cuDevicePrimaryCtxRetain\0"),
            primary_ctx_release: symbol!(object, b"cuDevicePrimaryCtxRelease_v2\0"),
            ctx_push: symbol!(object, b"cuCtxPushCurrent_v2\0"),
            ctx_pop: symbol!(object, b"cuCtxPopCurrent_v2\0"),
            stream_create: symbol!(object, b"cuStreamCreate\0"),
            stream_destroy: symbol!(object, b"cuStreamDestroy_v2\0"),
            stream_synchronize: symbol!(object, b"cuStreamSynchronize\0"),
            memcpy_2d_async: symbol!(object, b"cuMemcpy2DAsync_v2\0"),
            mem_free: symbol!(object, b"cuMemFree_v2\0"),
            import_external_memory: symbol!(object, b"cuImportExternalMemory\0"),
            external_memory_get_mapped_buffer: symbol!(
                object,
                b"cuExternalMemoryGetMappedBuffer\0"
            ),
            destroy_external_memory: symbol!(object, b"cuDestroyExternalMemory\0"),
            import_external_semaphore: symbol!(object, b"cuImportExternalSemaphore\0"),
            signal_external_semaphores: symbol!(object, b"cuSignalExternalSemaphoresAsync\0"),
            wait_external_semaphores: symbol!(object, b"cuWaitExternalSemaphoresAsync\0"),
            destroy_external_semaphore: symbol!(object, b"cuDestroyExternalSemaphore\0"),
            _object: object,
        };
        // SAFETY: フラグは 0 だけが正しい。
        cuda.check("cuInit", unsafe { (cuda.init)(0) })?;
        Ok(cuda)
    }

    fn check(&self, what: &str, status: CUresult) -> Result<(), String> {
        if status == CUDA_SUCCESS {
            return Ok(());
        }
        let mut name: *const c_char = std::ptr::null();
        // SAFETY: CUDA が持つ静的な文字列を指すだけ。
        unsafe { (self.get_error_name)(status, &mut name) };
        let name = if name.is_null() {
            format!("{status}")
        } else {
            // SAFETY: NUL 終端の静的な文字列。
            unsafe { std::ffi::CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        };
        Err(format!("{what}：{name}"))
    }

    fn devices(&self) -> Vec<(CUdevice, String, [u8; 16])> {
        let mut count = 0;
        // SAFETY: ローカル変数に書くだけ。
        if unsafe { (self.device_get_count)(&mut count) } != CUDA_SUCCESS {
            return Vec::new();
        }
        (0..count)
            .filter_map(|ordinal| {
                let mut device = 0;
                let mut name = [0 as c_char; 128];
                let mut uuid = [0u8; 16];
                // SAFETY: ローカル変数に書くだけ。大きさを指定したバッファ。
                unsafe {
                    if (self.device_get)(&mut device, ordinal) != CUDA_SUCCESS
                        || (self.device_get_name)(name.as_mut_ptr(), 128, device) != CUDA_SUCCESS
                        || (self.device_get_uuid)(&mut uuid, device) != CUDA_SUCCESS
                    {
                        return None;
                    }
                    let name = std::ffi::CStr::from_ptr(name.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    Some((device, name, uuid))
                }
            })
            .collect()
    }
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
    private: Ptr,
    disparity_range: u32,
    roi: u32,
}

#[repr(C)]
struct BufferDesc {
    width: u32,
    height: u32,
    usage: u32,
    format: u32,
}

#[repr(C)]
#[derive(Default)]
struct StrideInfo {
    strides: [[u32; 2]; 3],
    planes: u32,
}

#[repr(C)]
struct ExecuteIn {
    input: Handle,
    reference: Handle,
    hints: Handle,
    disable_temporal_hints: u32,
    padding: u32,
    private: Ptr,
    padding2: u32,
    rois: u32,
    roi_data: Ptr,
}

#[repr(C)]
struct ExecuteOut {
    output: Handle,
    cost: Handle,
    private: Ptr,
}

type Fn<A> = Option<A>;

/// 関数テーブル（`NV_OF_CUDA_API_FUNCTION_LIST`）。使わないエントリーも並びを合わせるために置く。
#[repr(C)]
#[derive(Default, Clone, Copy)]
#[allow(dead_code)]
struct Functions {
    create: Fn<unsafe extern "C" fn(Ptr, *mut Handle) -> Status>,
    init: Fn<unsafe extern "C" fn(Handle, *const InitParams) -> Status>,
    create_buffer: Fn<unsafe extern "C" fn(Handle, *const BufferDesc, u32, *mut Handle) -> Status>,
    buffer_array: Fn<unsafe extern "C" fn(Handle) -> Ptr>,
    buffer_pointer: Fn<unsafe extern "C" fn(Handle) -> CUdeviceptr>,
    buffer_stride: Fn<unsafe extern "C" fn(Handle, *mut StrideInfo) -> Status>,
    set_streams: Fn<unsafe extern "C" fn(Handle, Ptr, Ptr) -> Status>,
    execute: Fn<unsafe extern "C" fn(Handle, *const ExecuteIn, *mut ExecuteOut) -> Status>,
    destroy_buffer: Fn<unsafe extern "C" fn(Handle) -> Status>,
    destroy: Fn<unsafe extern "C" fn(Handle) -> Status>,
    last_error: Fn<unsafe extern "C" fn(Handle, *mut c_char, *mut u32) -> Status>,
    caps: Fn<unsafe extern "C" fn(Handle, u32, *mut u32, *mut u32) -> Status>,
}

struct OpticalFlow {
    _object: Object,
    functions: Functions,
}

impl OpticalFlow {
    fn get() -> Result<&'static Self, String> {
        static OF: OnceLock<Result<OpticalFlow, String>> = OnceLock::new();
        OF.get_or_init(|| {
            let object = Object::open("libnvidia-opticalflow.so.1")
                .map_err(|_| String::from("libnvidia-opticalflow.so.1 を開けない"))?;
            let create: unsafe extern "C" fn(u32, *mut Functions) -> Status =
                symbol!(object, b"NvOFAPICreateInstanceCuda\0");
            let mut functions = Functions::default();
            // SAFETY: 関数テーブルは有効なローカル変数。
            let code = unsafe { create(API_VERSION, &mut functions) };
            if code != 0 {
                return Err(format!("NvOFAPICreateInstanceCuda：{}", status(code)));
            }
            Ok(Self {
                _object: object,
                functions,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
    }
}

fn status(code: Status) -> String {
    STATUS
        .get(code as usize)
        .map_or_else(|| format!("{code}"), |name| (*name).to_owned())
}

pub fn supported() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        Cuda::get().is_ok_and(|cuda| !cuda.devices().is_empty()) && OpticalFlow::get().is_ok()
    })
}

struct Buffer {
    handle: Handle,
    pointer: CUdeviceptr,
    stride: u32,
}

struct Session {
    handle: Handle,
    inputs: [Buffer; 2],
    output: Buffer,
    cost: Buffer,
}

struct Shared {
    ins: Exported,
    outs: Exported,
    timeline: Timeline,
    memories: [Ptr; 2],
    pointers: [CUdeviceptr; 2],
    semaphore: Ptr,
}

struct Host {
    ins: Staging,
    outs: Staging,
    bytes: Vec<u8>,
}

pub struct Flow {
    of: &'static OpticalFlow,
    cuda: &'static Cuda,
    device: CUdevice,
    context: Ptr,
    stream: Ptr,
    /// 前→今（入力が前、参照が今）と、今→前。
    sessions: Vec<Session>,
    shared: Option<Shared>,
    host: Option<Host>,
    textures: Vec<wgpu::Texture>,
    ins: Layout,
    outs: Layout,
    size: [u32; 2],
    grid_size: [u32; 2],
    wanted: u32,
    color: bool,
    has_prev: bool,
    outside: f64,
}

struct Pushed<'a>(&'a Cuda);

impl<'a> Pushed<'a> {
    fn new(cuda: &'a Cuda, context: Ptr) -> Result<Self, String> {
        // SAFETY: 有効なコンテキスト。
        cuda.check("cuCtxPushCurrent", unsafe { (cuda.ctx_push)(context) })?;
        Ok(Self(cuda))
    }
}

impl Drop for Pushed<'_> {
    fn drop(&mut self) {
        let mut popped = std::ptr::null_mut();
        // SAFETY: プッシュしたコンテキストをポップする。
        unsafe { (self.0.ctx_pop)(&mut popped) };
    }
}

impl Flow {
    pub fn open(device: &wgpu::Device, size: [u32; 2], wanted: u32) -> Result<Self, String> {
        let cuda = Cuda::get()?;
        let of = OpticalFlow::get()?;
        let forced = std::env::var("ZW_BRIDGE").is_ok_and(|way| way == "host");
        let vulkan = Context::new(device);
        let devices = cuda.devices();
        // レンダリング用 GPU と同じデバイスがあればそれ、無ければ最初のデバイス（メインメモリを通す）。
        let same = vulkan.as_ref().ok().and_then(|context| {
            let uuid = context.uuid();
            devices.iter().position(|(_, _, found)| *found == uuid)
        });
        let (cu_device, name, _) = devices
            .get(same.unwrap_or(0))
            .cloned()
            .ok_or("CUDA デバイスが無い")?;
        let mut context = std::ptr::null_mut();
        // SAFETY: ローカル変数に書く。
        cuda.check("cuDevicePrimaryCtxRetain", unsafe {
            (cuda.primary_ctx_retain)(&mut context, cu_device)
        })?;
        let mut flow = Self {
            of,
            cuda,
            device: cu_device,
            context,
            stream: std::ptr::null_mut(),
            sessions: Vec::new(),
            shared: None,
            host: None,
            textures: Vec::new(),
            ins: Layout {
                planes: Vec::new(),
                total: 0,
            },
            outs: Layout {
                planes: Vec::new(),
                total: 0,
            },
            size,
            grid_size: [0, 0],
            wanted,
            color: false,
            has_prev: false,
            outside: 0.0,
        };
        let have = flow.start(wanted)?;
        let grid = flow.grid();
        flow.textures = flow.make_textures(device);
        let manner = match (same, &vulkan) {
            (Some(_), Ok(context)) if !forced => {
                flow.shared = Some(flow.share(device, context)?);
                ""
            }
            _ => {
                flow.host = Some(Host {
                    ins: Staging::new(device, flow.ins.clone(), "zw nvofa in"),
                    outs: Staging::new(device, flow.outs.clone(), "zw nvofa out"),
                    bytes: vec![0; flow.outs.total as usize],
                });
                match (&vulkan, same) {
                    (Err(why), _) => {
                        tracing::info!("NVOFA: Vulkan との共有を使えない（{why}）");
                        "（メインメモリ経由）"
                    }
                    (_, None) => "（レンダリング用 GPU とは別、メインメモリ経由）",
                    _ => "（メインメモリ経由）",
                }
            }
        };
        tracing::info!(
            "NVOFA（CUDA）: {name}{manner}、{}x{}、グリッド {grid}（要求 {wanted}、対応 {have:?}）、入力 {}",
            size[0],
            size[1],
            if flow.color {
                "カラー（ABGR8）"
            } else {
                "輝度"
            },
        );
        Ok(flow)
    }

    fn grid(&self) -> u32 {
        self.size[0].div_ceil(self.grid_size[0].max(1))
    }

    fn start(&mut self, wanted: u32) -> Result<Vec<u32>, String> {
        let _pushed = Pushed::new(self.cuda, self.context)?;
        let mut stream = std::ptr::null_mut();
        // SAFETY: ローカル変数に書く。
        self.cuda.check("cuStreamCreate", unsafe {
            (self.cuda.stream_create)(&mut stream, 0)
        })?;
        self.stream = stream;
        let f = self.of.functions;
        let mut have = Vec::new();
        for direction in 0..2 {
            let create = f.create.ok_or("nvCreateOpticalFlowCuda が無い")?;
            let mut handle: Handle = std::ptr::null_mut();
            // SAFETY: プッシュしたコンテキストとローカル変数。
            let code = unsafe { create(self.context, &mut handle) };
            if code != 0 {
                return Err(format!("nvCreateOpticalFlowCuda：{}", status(code)));
            }
            if direction == 0 {
                have = caps(&f, handle);
            }
            let grid = super::flow::nearest_grid(&have, wanted).unwrap_or(4);
            self.grid_size = [self.size[0].div_ceil(grid), self.size[1].div_ceil(grid)];
            let init = InitParams {
                width: self.size[0],
                height: self.size[1],
                out_grid: grid,
                hint_grid: 0,
                mode: MODE_OPTICALFLOW,
                perf: PERF_MEDIUM,
                external_hints: 0,
                output_cost: 1,
                private: std::ptr::null_mut(),
                disparity_range: 0,
                roi: 0,
            };
            let mut session = Session {
                handle,
                inputs: [Buffer::empty(), Buffer::empty()],
                output: Buffer::empty(),
                cost: Buffer::empty(),
            };
            let result = (|| {
                let init_fn = f.init.ok_or("nvOFInit が無い")?;
                // SAFETY: 有効なローカル変数。
                self.check(handle, "nvOFInit", unsafe { init_fn(handle, &init) })?;
                if let Some(set) = f.set_streams {
                    // SAFETY: 有効なハンドルとストリーム。
                    self.check(handle, "nvOFSetIOCudaStreams", unsafe {
                        set(handle, self.stream, self.stream)
                    })?;
                }
                // カラーを試し、拒否されたら輝度（二つ目の方向は一つ目に合わせる）。
                let color = if direction == 0 {
                    self.buffer(handle, self.size, USAGE_INPUT, FORMAT_ABGR8)
                        .map(|first| {
                            session.inputs[0] = first;
                            true
                        })
                        .unwrap_or(false)
                } else {
                    self.color
                };
                self.color = color;
                let format = if color {
                    FORMAT_ABGR8
                } else {
                    FORMAT_GRAYSCALE8
                };
                for k in 0..2 {
                    if session.inputs[k].handle.is_null() {
                        session.inputs[k] = self.buffer(handle, self.size, USAGE_INPUT, format)?;
                    }
                }
                session.output =
                    self.buffer(handle, self.grid_size, USAGE_OUTPUT, FORMAT_SHORT2)?;
                session.cost = self.buffer(handle, self.grid_size, USAGE_COST, FORMAT_UINT8)?;
                Ok::<(), String>(())
            })();
            self.sessions.push(session);
            result?;
        }
        let input_format = if self.color {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::R8Unorm
        };
        self.ins = Layout::of(&[(input_format, self.size), (input_format, self.size)]);
        self.outs = Layout::of(&[
            (wgpu::TextureFormat::Rg16Sint, self.grid_size),
            (wgpu::TextureFormat::Rg16Sint, self.grid_size),
            (wgpu::TextureFormat::R8Uint, self.grid_size),
            (wgpu::TextureFormat::R8Uint, self.grid_size),
        ]);
        Ok(have)
    }

    fn buffer(
        &self,
        of: Handle,
        size: [u32; 2],
        usage: u32,
        format: u32,
    ) -> Result<Buffer, String> {
        let f = self.of.functions;
        let description = BufferDesc {
            width: size[0],
            height: size[1],
            usage,
            format,
        };
        let mut handle: Handle = std::ptr::null_mut();
        let create = f.create_buffer.ok_or("nvOFCreateGPUBufferCuda が無い")?;
        // SAFETY: 有効なローカル変数。
        self.check(of, "nvOFCreateGPUBufferCuda", unsafe {
            create(of, &description, BUFFER_CUDEVICEPTR, &mut handle)
        })?;
        let mut stride = StrideInfo::default();
        // SAFETY: 作ったバッファ。
        unsafe {
            if let Some(get) = f.buffer_stride {
                get(handle, &mut stride);
            }
            let pointer = f.buffer_pointer.map_or(0, |get| get(handle));
            Ok(Buffer {
                handle,
                pointer,
                stride: stride.strides[0][0],
            })
        }
    }

    fn check(&self, of: Handle, what: &str, code: Status) -> Result<(), String> {
        if code == 0 {
            return Ok(());
        }
        let mut text = [0 as c_char; 256];
        let mut size = text.len() as u32;
        if let Some(f) = self.of.functions.last_error {
            // SAFETY: 大きさを指定したローカルのバッファ。
            unsafe { f(of, text.as_mut_ptr(), &mut size) };
        }
        // SAFETY: ドライバーが NUL 終端で書く（書かなければ 0 のまま）。
        let detail = unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }.to_string_lossy();
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

    fn make_textures(&self, device: &wgpu::Device) -> Vec<wgpu::Texture> {
        let input_format = if self.color {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::R8Unorm
        };
        let make = |format, size: [u32; 2], usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("zw nvofa"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let input = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        let output = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        vec![
            make(input_format, self.size, input),
            make(input_format, self.size, input),
            make(wgpu::TextureFormat::Rg16Sint, self.grid_size, output),
            make(wgpu::TextureFormat::Rg16Sint, self.grid_size, output),
            make(wgpu::TextureFormat::R8Uint, self.grid_size, output),
            make(wgpu::TextureFormat::R8Uint, self.grid_size, output),
        ]
    }

    fn share(&self, device: &wgpu::Device, context: &Context) -> Result<Shared, String> {
        let kind = vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD;
        let ins = context.export_buffer(device, self.ins.total, kind, "zw nvofa in")?;
        let outs = context.export_buffer(device, self.outs.total, kind, "zw nvofa out")?;
        let timeline = context.timeline()?;
        let _pushed = Pushed::new(self.cuda, self.context)?;
        let mut memories = [std::ptr::null_mut(); 2];
        let mut pointers = [0; 2];
        for (k, exported) in [&ins, &outs].into_iter().enumerate() {
            let fd = exported.fd(kind)?;
            let description = ExternalMemoryHandleDesc {
                kind: CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD,
                handle: FdHandle {
                    fd: fd.as_raw_fd(),
                    ..Default::default()
                },
                size: exported.allocated,
                flags: CUDA_EXTERNAL_MEMORY_DEDICATED,
                ..Default::default()
            };
            // SAFETY: ローカルの記述子。成功すれば CUDA がファイル記述子を引き取る（失敗なら `fd` が破棄されて閉じる）。
            let code =
                unsafe { (self.cuda.import_external_memory)(&mut memories[k], &description) };
            self.cuda.check("cuImportExternalMemory", code)?;
            let _ = fd.into_raw_fd();
            let mapped = ExternalMemoryBufferDesc {
                offset: 0,
                size: exported.buffer.size(),
                ..Default::default()
            };
            // SAFETY: インポートしたメモリとローカル変数。
            self.cuda.check("cuExternalMemoryGetMappedBuffer", unsafe {
                (self.cuda.external_memory_get_mapped_buffer)(
                    &mut pointers[k],
                    memories[k],
                    &mapped,
                )
            })?;
        }
        let fd = timeline.fd()?;
        let description = ExternalSemaphoreHandleDesc {
            kind: CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_TIMELINE_SEMAPHORE_FD,
            handle: FdHandle {
                fd: fd.as_raw_fd(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut semaphore = std::ptr::null_mut();
        // SAFETY: ローカルの記述子。成功すれば CUDA がファイル記述子を引き取る（失敗なら `fd` が破棄されて閉じる）。
        let code = unsafe { (self.cuda.import_external_semaphore)(&mut semaphore, &description) };
        self.cuda.check("cuImportExternalSemaphore", code)?;
        let _ = fd.into_raw_fd();
        Ok(Shared {
            ins,
            outs,
            timeline,
            memories,
            pointers,
            semaphore,
        })
    }

    fn input_row(&self) -> usize {
        self.size[0] as usize * if self.color { 4 } else { 1 }
    }

    /// 入力を NVOFA のバッファへコピーする。`from` は入力の先頭、`Host` ならメインメモリ。
    fn put_inputs(&self, from: Source) -> Result<(), String> {
        for (k, session) in self.sessions.iter().enumerate() {
            // 前→今は入力が前（プレーン 0）、今→前は入力が今（プレーン 1）。
            let order = if k == 0 { [0, 1] } else { [1, 0] };
            for (slot, plane) in order.into_iter().enumerate() {
                let at = &self.ins.planes[plane];
                let mut copy = Memcpy2D {
                    src_pitch: at.row_pitch as usize,
                    dst_memory_type: CU_MEMORYTYPE_DEVICE,
                    dst_device: session.inputs[slot].pointer,
                    dst_pitch: session.inputs[slot].stride as usize,
                    width_in_bytes: self.input_row(),
                    height: self.size[1] as usize,
                    ..Default::default()
                };
                from.fill(&mut copy, at.offset);
                // SAFETY: 二つの範囲はどちらも有効で、大きさは記述どおり。
                self.cuda.check("cuMemcpy2DAsync（入力）", unsafe {
                    (self.cuda.memcpy_2d_async)(&copy, self.stream)
                })?;
            }
        }
        Ok(())
    }

    fn execute(&self) -> Result<(), String> {
        let execute = self.of.functions.execute.ok_or("nvOFExecute が無い")?;
        for session in &self.sessions {
            let input = ExecuteIn {
                input: session.inputs[0].handle,
                reference: session.inputs[1].handle,
                hints: std::ptr::null_mut(),
                disable_temporal_hints: 0,
                padding: 0,
                private: std::ptr::null_mut(),
                padding2: 0,
                rois: 0,
                roi_data: std::ptr::null_mut(),
            };
            let mut output = ExecuteOut {
                output: session.output.handle,
                cost: session.cost.handle,
                private: std::ptr::null_mut(),
            };
            // SAFETY: 有効なローカル変数と作ったバッファ。
            self.check(session.handle, "nvOFExecute", unsafe {
                execute(session.handle, &input, &mut output)
            })?;
        }
        Ok(())
    }

    fn take_outputs(&self, to: Target) -> Result<(), String> {
        let buffers = [
            (&self.sessions[0].output, 4),
            (&self.sessions[1].output, 4),
            (&self.sessions[0].cost, 1),
            (&self.sessions[1].cost, 1),
        ];
        for ((buffer, bytes), at) in buffers.into_iter().zip(&self.outs.planes) {
            let mut copy = Memcpy2D {
                src_memory_type: CU_MEMORYTYPE_DEVICE,
                src_device: buffer.pointer,
                src_pitch: buffer.stride as usize,
                dst_pitch: at.row_pitch as usize,
                width_in_bytes: self.grid_size[0] as usize * bytes,
                height: self.grid_size[1] as usize,
                ..Default::default()
            };
            to.fill(&mut copy, at.offset);
            // SAFETY: 二つの範囲はどちらも有効で、大きさは記述どおり。
            self.cuda.check("cuMemcpy2DAsync（フロー）", unsafe {
                (self.cuda.memcpy_2d_async)(&copy, self.stream)
            })?;
        }
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
                convert.put(gpu, source, &self.textures[k]);
            } else {
                convert.luma(gpu, source, &self.textures[k]);
            }
        }
        if let Some(shared) = self.shared.as_mut() {
            for (texture, plane) in self.textures[..2].iter().zip(&self.ins.planes) {
                gpu.encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &shared.ins.buffer,
                        layout: copy_layout(plane),
                    },
                    texture.size(),
                );
            }
            let ready = shared.timeline.signal_after_next_submit(gpu.queue)?;
            gpu.flush();
            shared.timeline.value += 1;
            let done = shared.timeline.value;
            let (pointers, semaphore) = (shared.pointers, shared.semaphore);
            {
                let _pushed = Pushed::new(self.cuda, self.context)?;
                let wait = ExternalSemaphoreParams {
                    value: ready,
                    ..Default::default()
                };
                // SAFETY: インポートしたセマフォと有効なストリーム。
                self.cuda.check("cuWaitExternalSemaphoresAsync", unsafe {
                    (self.cuda.wait_external_semaphores)(&semaphore, &wait, 1, self.stream)
                })?;
                self.put_inputs(Source::Device(pointers[0]))?;
                self.execute()?;
                self.take_outputs(Target::Device(pointers[1]))?;
                let signal = ExternalSemaphoreParams {
                    value: done,
                    ..Default::default()
                };
                // SAFETY: 同上。
                self.cuda.check("cuSignalExternalSemaphoresAsync", unsafe {
                    (self.cuda.signal_external_semaphores)(&semaphore, &signal, 1, self.stream)
                })?;
            }
            let shared = self.shared.as_ref().ok_or("インポートしたリソースが無い")?;
            shared.timeline.wait_before_next_submit(gpu.queue, done)?;
            for (texture, plane) in self.textures[2..].iter().zip(&self.outs.planes) {
                gpu.encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &shared.outs.buffer,
                        layout: copy_layout(plane),
                    },
                    texture.as_image_copy(),
                    texture.size(),
                );
            }
        } else if let Some(mut host) = self.host.take() {
            let began = std::time::Instant::now();
            let result = (|| {
                let inputs = &self.textures[..2];
                staging::download(gpu, &[(&host.ins, inputs)], |_, bytes| {
                    let _pushed = Pushed::new(self.cuda, self.context)?;
                    self.put_inputs(Source::Host(bytes.as_ptr()))?;
                    self.execute()?;
                    self.take_outputs(Target::Host(host.bytes.as_mut_ptr()))?;
                    // SAFETY: 有効なストリーム。メインメモリへのコピーが終わるまで待つ。
                    self.cuda.check("cuStreamSynchronize", unsafe {
                        (self.cuda.stream_synchronize)(self.stream)
                    })
                })?;
                host.outs.upload(gpu, &self.textures[2..], &host.bytes);
                Ok::<(), String>(())
            })();
            let ms = began.elapsed().as_secs_f64() * 1000.0;
            self.outside = if self.outside > 0.0 {
                self.outside * 0.8 + ms * 0.2
            } else {
                ms
            };
            self.host = Some(host);
            result?;
        }
        convert.flow(gpu, &self.textures[2], &self.textures[3], run.target);
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Source {
    Device(CUdeviceptr),
    Host(*const u8),
}

impl Source {
    fn fill(self, copy: &mut Memcpy2D, offset: u64) {
        match self {
            Self::Device(base) => {
                copy.src_memory_type = CU_MEMORYTYPE_DEVICE;
                copy.src_device = base + offset;
            }
            Self::Host(base) => {
                copy.src_memory_type = CU_MEMORYTYPE_HOST;
                // アドレスを CUDA へ渡すだけで、こちらでは読まない。
                copy.src_host = base.wrapping_add(offset as usize).cast();
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Target {
    Device(CUdeviceptr),
    Host(*mut u8),
}

impl Target {
    fn fill(self, copy: &mut Memcpy2D, offset: u64) {
        match self {
            Self::Device(base) => {
                copy.dst_memory_type = CU_MEMORYTYPE_DEVICE;
                copy.dst_device = base + offset;
            }
            Self::Host(base) => {
                copy.dst_memory_type = CU_MEMORYTYPE_HOST;
                // アドレスを CUDA へ渡すだけで、こちらでは読まない。
                copy.dst_host = base.wrapping_add(offset as usize).cast();
            }
        }
    }
}

fn copy_layout(plane: &staging::Plane) -> wgpu::TexelCopyBufferLayout {
    wgpu::TexelCopyBufferLayout {
        offset: plane.offset,
        bytes_per_row: Some(plane.row_pitch),
        rows_per_image: Some(plane.rows),
    }
}

fn caps(f: &Functions, handle: Handle) -> Vec<u32> {
    let Some(get) = f.caps else {
        return Vec::new();
    };
    let mut count = 0u32;
    // SAFETY: 数だけ先に問い合わせる。
    if unsafe {
        get(
            handle,
            CAPS_OUTPUT_GRID_SIZES,
            std::ptr::null_mut(),
            &mut count,
        )
    } != 0
        || count == 0
    {
        return Vec::new();
    }
    let mut values = vec![0u32; count as usize];
    // SAFETY: 問い合わせた数のバッファ。
    if unsafe {
        get(
            handle,
            CAPS_OUTPUT_GRID_SIZES,
            values.as_mut_ptr(),
            &mut count,
        )
    } != 0
    {
        return Vec::new();
    }
    values.truncate(count as usize);
    values
}

impl Buffer {
    fn empty() -> Self {
        Self {
            handle: std::ptr::null_mut(),
            pointer: 0,
            stride: 0,
        }
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
        convert.cost(gpu, &self.textures[4], &self.textures[5], target);
        true
    }

    fn outside_ms(&self) -> f64 {
        self.outside
    }
}

// SAFETY: 生のポインターはこの `Flow` だけが持つ CUDA と NVOFA のオブジェクトで、使うのは `&mut self` の一つのスレッドだけ。
unsafe impl Send for Flow {}

impl Drop for Flow {
    fn drop(&mut self) {
        let f = self.of.functions;
        if let Ok(_pushed) = Pushed::new(self.cuda, self.context) {
            // SAFETY: 作ったオブジェクトを一度ずつ、ストリームが終わってから解放する。
            unsafe {
                if !self.stream.is_null() {
                    (self.cuda.stream_synchronize)(self.stream);
                }
                for session in &self.sessions {
                    if let Some(destroy) = f.destroy_buffer {
                        for buffer in session
                            .inputs
                            .iter()
                            .chain([&session.output, &session.cost])
                        {
                            if !buffer.handle.is_null() {
                                destroy(buffer.handle);
                            }
                        }
                    }
                    if let Some(destroy) = f.destroy {
                        destroy(session.handle);
                    }
                }
                if let Some(shared) = &self.shared {
                    (self.cuda.destroy_external_semaphore)(shared.semaphore);
                    for (memory, pointer) in shared.memories.iter().zip(shared.pointers) {
                        if pointer != 0 {
                            (self.cuda.mem_free)(pointer);
                        }
                        if !memory.is_null() {
                            (self.cuda.destroy_external_memory)(*memory);
                        }
                    }
                }
                if !self.stream.is_null() {
                    (self.cuda.stream_destroy)(self.stream);
                }
            }
        }
        // SAFETY: 開くときに一度だけ取得したもの。
        unsafe { (self.cuda.primary_ctx_release)(self.device) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_external_descriptions_have_the_header_layout() {
        assert_eq!(std::mem::offset_of!(ExternalMemoryHandleDesc, handle), 8);
        assert_eq!(std::mem::offset_of!(ExternalMemoryHandleDesc, size), 24);
        assert_eq!(size_of::<ExternalMemoryHandleDesc>(), 104);
        assert_eq!(size_of::<ExternalSemaphoreHandleDesc>(), 96);
        assert_eq!(size_of::<ExternalSemaphoreParams>(), 144);
        assert_eq!(size_of::<Memcpy2D>(), 128);
        assert_eq!(size_of::<InitParams>(), 48);
        assert_eq!(size_of::<ExecuteIn>(), 56);
    }
}
