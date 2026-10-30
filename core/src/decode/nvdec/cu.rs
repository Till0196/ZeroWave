// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! CUDA と NVDEC を実行時に読み込む（リンクしない）。
//! 構造体は `nv-codec-headers` から移したもの。レイアウトは `mod tests` が確かめる。

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::Arc;

use crate::decode::Error;
use crate::dl::Object;

mod context;
mod decoder;
#[cfg(test)]
mod tests;
mod types;

#[cfg(windows)]
pub use context::{Fence, Shared, Stream};
#[cfg(unix)]
pub use decoder::Linear;
pub use decoder::{Decoder, Mapped};
pub use types::*;

pub type CUresult = c_int;
pub type CUdevice = c_int;
pub type CUdeviceptr = u64;
pub type CUcontext = *mut c_void;
pub type CUstream = *mut c_void;
pub type CUvideodecoder = *mut c_void;

/// `tcu_ulong`（C の `unsigned long`）。LP64 で 8、Windows（LLP64）で 4 バイト。
/// 間違えると `cuvidCreateDecoder` が `CUDA_ERROR_NOT_SUPPORTED` を返す。
#[cfg(windows)]
pub type CuUlong = u32;
#[cfg(not(windows))]
pub type CuUlong = u64;

pub const CUDA_SUCCESS: CUresult = 0;

pub const CUDA_VIDEO_CODEC_MPEG2: c_int = 1;
pub const CUDA_VIDEO_CODEC_H264: c_int = 4;
pub const CUDA_VIDEO_CODEC_HEVC: c_int = 8;

pub const CUDA_VIDEO_CHROMA_420: c_int = 1;

pub const CUDA_VIDEO_SURFACE_NV12: c_int = 0;
pub const CUDA_VIDEO_SURFACE_P016: c_int = 1;

/// `cudaVideoDeinterlaceMode_Weave`。
pub const CUDA_VIDEO_DEINTERLACE_WEAVE: c_int = 0;
pub const CUDA_VIDEO_DEINTERLACE_BOB: c_int = 1;
pub const CUDA_VIDEO_DEINTERLACE_ADAPTIVE: c_int = 2;

pub const CUDA_VIDEO_CREATE_PREFER_CUVID: CuUlong = 0x04;

pub const CU_MEMORYTYPE_HOST: c_int = 1;
pub const CU_MEMORYTYPE_DEVICE: c_int = 2;

pub const CU_MEM_ALLOCATION_TYPE_PINNED: c_int = 1;
pub const CU_MEM_LOCATION_TYPE_DEVICE: c_int = 1;
pub const CU_MEM_HANDLE_TYPE_POSIX_FILE_DESCRIPTOR: c_int = 0x1;
pub const CU_MEM_ACCESS_FLAGS_PROT_READWRITE: c_int = 3;
pub const CU_MEM_ALLOC_GRANULARITY_RECOMMENDED: c_int = 1;
pub const CU_MEM_RANGE_HANDLE_TYPE_DMA_BUF_FD: c_int = 0x1;

/// `CU_DEVICE_ATTRIBUTE_DMA_BUF_SUPPORTED`。真のときだけ DMA-BUF でエクスポートできる。
pub const CU_DEVICE_ATTRIBUTE_DMA_BUF_SUPPORTED: c_int = 124;

#[allow(clippy::type_complexity)]
struct Symbols {
    init: unsafe extern "C" fn(c_uint) -> CUresult,
    get_error_name: unsafe extern "C" fn(CUresult, *mut *const c_char) -> CUresult,
    get_error_string: unsafe extern "C" fn(CUresult, *mut *const c_char) -> CUresult,
    device_get_count: unsafe extern "C" fn(*mut c_int) -> CUresult,
    device_get: unsafe extern "C" fn(*mut CUdevice, c_int) -> CUresult,
    device_get_name: unsafe extern "C" fn(*mut c_char, c_int, CUdevice) -> CUresult,
    device_get_attribute: unsafe extern "C" fn(*mut c_int, c_int, CUdevice) -> CUresult,
    primary_ctx_retain: unsafe extern "C" fn(*mut CUcontext, CUdevice) -> CUresult,
    primary_ctx_release: unsafe extern "C" fn(CUdevice) -> CUresult,
    ctx_push_current: unsafe extern "C" fn(CUcontext) -> CUresult,
    ctx_pop_current: unsafe extern "C" fn(*mut CUcontext) -> CUresult,
    memcpy_2d: unsafe extern "C" fn(*const Memcpy2D) -> CUresult,
    #[cfg(windows)]
    device_get_luid: unsafe extern "C" fn(*mut c_char, *mut c_uint, CUdevice) -> CUresult,
    mem_get_info: unsafe extern "C" fn(*mut usize, *mut usize) -> CUresult,
    #[cfg(windows)]
    mem_free: unsafe extern "C" fn(CUdeviceptr) -> CUresult,
    #[cfg(windows)]
    memcpy_2d_async: unsafe extern "C" fn(*const Memcpy2D, CUstream) -> CUresult,
    #[cfg(windows)]
    stream_create: unsafe extern "C" fn(*mut CUstream, c_uint) -> CUresult,
    #[cfg(windows)]
    stream_destroy: unsafe extern "C" fn(CUstream) -> CUresult,
    #[cfg(windows)]
    stream_synchronize: unsafe extern "C" fn(CUstream) -> CUresult,
    #[cfg(windows)]
    import_external_memory:
        unsafe extern "C" fn(*mut CUexternalMemory, *const ExternalMemoryHandleDesc) -> CUresult,
    #[cfg(windows)]
    destroy_external_memory: unsafe extern "C" fn(CUexternalMemory) -> CUresult,
    #[cfg(windows)]
    external_memory_get_mapped_buffer: unsafe extern "C" fn(
        *mut CUdeviceptr,
        CUexternalMemory,
        *const ExternalMemoryBufferDesc,
    ) -> CUresult,
    #[cfg(windows)]
    import_external_semaphore: unsafe extern "C" fn(
        *mut CUexternalSemaphore,
        *const ExternalSemaphoreHandleDesc,
    ) -> CUresult,
    #[cfg(windows)]
    destroy_external_semaphore: unsafe extern "C" fn(CUexternalSemaphore) -> CUresult,
    #[cfg(windows)]
    wait_external_semaphores: unsafe extern "C" fn(
        *const CUexternalSemaphore,
        *const ExternalSemaphoreParams,
        c_uint,
        CUstream,
    ) -> CUresult,
    #[cfg(unix)]
    mem_get_allocation_granularity:
        unsafe extern "C" fn(*mut usize, *const MemAllocationProp, c_int) -> CUresult,
    #[cfg(unix)]
    mem_create: unsafe extern "C" fn(*mut u64, usize, *const MemAllocationProp, u64) -> CUresult,
    #[cfg(unix)]
    mem_release: unsafe extern "C" fn(u64) -> CUresult,
    #[cfg(unix)]
    mem_address_reserve:
        unsafe extern "C" fn(*mut CUdeviceptr, usize, usize, CUdeviceptr, u64) -> CUresult,
    #[cfg(unix)]
    mem_address_free: unsafe extern "C" fn(CUdeviceptr, usize) -> CUresult,
    #[cfg(unix)]
    mem_map: unsafe extern "C" fn(CUdeviceptr, usize, usize, u64, u64) -> CUresult,
    #[cfg(unix)]
    mem_unmap: unsafe extern "C" fn(CUdeviceptr, usize) -> CUresult,
    #[cfg(unix)]
    mem_set_access:
        unsafe extern "C" fn(CUdeviceptr, usize, *const MemAccessDesc, usize) -> CUresult,
    #[cfg(unix)]
    mem_get_handle_for_address_range:
        unsafe extern "C" fn(*mut c_void, CUdeviceptr, usize, c_int, u64) -> CUresult,
    get_decoder_caps: unsafe extern "C" fn(*mut DecodeCaps) -> CUresult,
    create_decoder: unsafe extern "C" fn(*mut CUvideodecoder, *mut DecodeCreateInfo) -> CUresult,
    destroy_decoder: unsafe extern "C" fn(CUvideodecoder) -> CUresult,
    decode_picture: unsafe extern "C" fn(CUvideodecoder, *mut PicParams) -> CUresult,
    map_video_frame: unsafe extern "C" fn(
        CUvideodecoder,
        c_int,
        *mut u64,
        *mut c_uint,
        *mut ProcParams,
    ) -> CUresult,
    unmap_video_frame: unsafe extern "C" fn(CUvideodecoder, u64) -> CUresult,
    get_decode_status: unsafe extern "C" fn(CUvideodecoder, c_int, *mut DecodeStatus) -> CUresult,
}

#[cfg(windows)]
pub const CUDA: &str = "nvcuda.dll";
#[cfg(windows)]
pub const NVCUVID: &str = "nvcuvid.dll";
#[cfg(not(windows))]
pub const CUDA: &str = "libcuda.so.1";
#[cfg(not(windows))]
pub const NVCUVID: &str = "libnvcuvid.so.1";

pub struct Library {
    symbols: Symbols,
    _cuda: Object,
    _cuvid: Object,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Library")
    }
}

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: CUDA / NVDEC のエントリーポイントで、型はヘッダーが宣言しているシグネチャー。
        unsafe { $object.symbol($name) }.map_err(|missing| Error::Unavailable(missing.0))?
    };
}

impl Library {
    pub fn load() -> Result<Self, Error> {
        let cuda = Object::open(CUDA).map_err(|missing| Error::Unavailable(missing.0))?;
        let cuvid = Object::open(NVCUVID).map_err(|missing| Error::Unavailable(missing.0))?;
        let symbols = Symbols {
            init: symbol!(cuda, b"cuInit\0"),
            get_error_name: symbol!(cuda, b"cuGetErrorName\0"),
            get_error_string: symbol!(cuda, b"cuGetErrorString\0"),
            device_get_count: symbol!(cuda, b"cuDeviceGetCount\0"),
            device_get: symbol!(cuda, b"cuDeviceGet\0"),
            device_get_name: symbol!(cuda, b"cuDeviceGetName\0"),
            device_get_attribute: symbol!(cuda, b"cuDeviceGetAttribute\0"),
            primary_ctx_retain: symbol!(cuda, b"cuDevicePrimaryCtxRetain\0"),
            primary_ctx_release: symbol!(cuda, b"cuDevicePrimaryCtxRelease\0"),
            ctx_push_current: symbol!(cuda, b"cuCtxPushCurrent_v2\0"),
            ctx_pop_current: symbol!(cuda, b"cuCtxPopCurrent_v2\0"),
            memcpy_2d: symbol!(cuda, b"cuMemcpy2D_v2\0"),
            #[cfg(windows)]
            device_get_luid: symbol!(cuda, b"cuDeviceGetLuid\0"),
            #[cfg(windows)]
            memcpy_2d_async: symbol!(cuda, b"cuMemcpy2DAsync_v2\0"),
            mem_get_info: symbol!(cuda, b"cuMemGetInfo_v2\0"),
            #[cfg(windows)]
            mem_free: symbol!(cuda, b"cuMemFree_v2\0"),
            #[cfg(windows)]
            stream_create: symbol!(cuda, b"cuStreamCreate\0"),
            #[cfg(windows)]
            stream_destroy: symbol!(cuda, b"cuStreamDestroy_v2\0"),
            #[cfg(windows)]
            stream_synchronize: symbol!(cuda, b"cuStreamSynchronize\0"),
            #[cfg(windows)]
            import_external_memory: symbol!(cuda, b"cuImportExternalMemory\0"),
            #[cfg(windows)]
            destroy_external_memory: symbol!(cuda, b"cuDestroyExternalMemory\0"),
            #[cfg(windows)]
            external_memory_get_mapped_buffer: symbol!(cuda, b"cuExternalMemoryGetMappedBuffer\0"),
            #[cfg(windows)]
            import_external_semaphore: symbol!(cuda, b"cuImportExternalSemaphore\0"),
            #[cfg(windows)]
            destroy_external_semaphore: symbol!(cuda, b"cuDestroyExternalSemaphore\0"),
            #[cfg(windows)]
            wait_external_semaphores: symbol!(cuda, b"cuWaitExternalSemaphoresAsync\0"),
            #[cfg(unix)]
            mem_get_allocation_granularity: symbol!(cuda, b"cuMemGetAllocationGranularity\0"),
            #[cfg(unix)]
            mem_create: symbol!(cuda, b"cuMemCreate\0"),
            #[cfg(unix)]
            mem_release: symbol!(cuda, b"cuMemRelease\0"),
            #[cfg(unix)]
            mem_address_reserve: symbol!(cuda, b"cuMemAddressReserve\0"),
            #[cfg(unix)]
            mem_address_free: symbol!(cuda, b"cuMemAddressFree\0"),
            #[cfg(unix)]
            mem_map: symbol!(cuda, b"cuMemMap\0"),
            #[cfg(unix)]
            mem_unmap: symbol!(cuda, b"cuMemUnmap\0"),
            #[cfg(unix)]
            mem_set_access: symbol!(cuda, b"cuMemSetAccess\0"),
            #[cfg(unix)]
            mem_get_handle_for_address_range: symbol!(cuda, b"cuMemGetHandleForAddressRange\0"),
            get_decoder_caps: symbol!(cuvid, b"cuvidGetDecoderCaps\0"),
            create_decoder: symbol!(cuvid, b"cuvidCreateDecoder\0"),
            destroy_decoder: symbol!(cuvid, b"cuvidDestroyDecoder\0"),
            decode_picture: symbol!(cuvid, b"cuvidDecodePicture\0"),
            map_video_frame: symbol!(cuvid, b"cuvidMapVideoFrame64\0"),
            unmap_video_frame: symbol!(cuvid, b"cuvidUnmapVideoFrame64\0"),
            get_decode_status: symbol!(cuvid, b"cuvidGetDecodeStatus\0"),
        };
        Ok(Self {
            symbols,
            _cuda: cuda,
            _cuvid: cuvid,
        })
    }

    fn text(&self, status: CUresult) -> String {
        let mut name: *const c_char = std::ptr::null();
        let mut message: *const c_char = std::ptr::null();
        // SAFETY: 生きたローカル変数。CUDA は静的な文字列へのポインターを書くか、何も書かない。
        unsafe {
            (self.symbols.get_error_name)(status, &mut name);
            (self.symbols.get_error_string)(status, &mut message);
        }
        let read = |pointer: *const c_char| -> String {
            if pointer.is_null() {
                String::new()
            } else {
                // SAFETY: null でなければ CUDA が持つ静的な NUL 終端文字列。
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        format!("{} {} ({status})", read(name), read(message))
    }

    fn device(&self, ordinal: c_int) -> Result<CUdevice, CUresult> {
        let mut device: CUdevice = 0;
        // SAFETY: 生きたローカル変数。序数がデバイスの数を越えれば失敗を返すだけ。
        let status = unsafe { (self.symbols.device_get)(&mut device, ordinal) };
        if status == CUDA_SUCCESS {
            Ok(device)
        } else {
            Err(status)
        }
    }

    #[cfg(windows)]
    fn luid(&self, device: CUdevice) -> Option<[u8; 8]> {
        let mut bytes = [0i8; 8];
        let mut mask: c_uint = 0;
        // SAFETY: `cuDeviceGetLuid` は 8 バイト書く。どちらも生きたローカル変数。
        let got = unsafe { (self.symbols.device_get_luid)(bytes.as_mut_ptr(), &mut mask, device) };
        (got == CUDA_SUCCESS).then(|| bytes.map(|byte| byte as u8))
    }

    fn check(&self, what: &str, status: CUresult) -> Result<(), Error> {
        if status == CUDA_SUCCESS {
            Ok(())
        } else {
            Err(Error::Unavailable(format!("{what}: {}", self.text(status))))
        }
    }
}

pub struct Context {
    library: Library,
    device: CUdevice,
    raw: CUcontext,
    pub name: String,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Context({})", self.name)
    }
}

// SAFETY: 一つのデコーダーが持ちスレッドを移る。`Sync` は `Arc` で持つため。
unsafe impl Send for Context {}
// SAFETY: 上のとおり。
unsafe impl Sync for Context {}

/// CUDA のコンテキストをこのスレッドで有効にしている間だけ生きるガード。有効なコンテキストはスレッドごとなので呼び出しごとに push する。
struct Current<'a>(&'a Context);

impl<'a> Current<'a> {
    fn push(context: &'a Context) -> Result<Self, Error> {
        // SAFETY: `raw` は `cuDevicePrimaryCtxRetain` が返した生きたコンテキスト。
        let status = unsafe { (context.library.symbols.ctx_push_current)(context.raw) };
        context.library.check("cuCtxPushCurrent", status)?;
        Ok(Self(context))
    }
}

impl Drop for Current<'_> {
    fn drop(&mut self) {
        let mut out: CUcontext = std::ptr::null_mut();
        // SAFETY: push したのと同じスレッドで、一度だけ pop する。
        unsafe { (self.0.library.symbols.ctx_pop_current)(&mut out) };
    }
}
