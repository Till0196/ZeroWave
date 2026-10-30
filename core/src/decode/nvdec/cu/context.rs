// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! CUDA のコンテキストを開く、ケイパビリティを問い合わせる、デコーダーを作る。Windows では D3D12 との共有。

use super::*;

impl Context {
    pub fn open() -> Result<Self, Error> {
        Self::open_matching(None)
    }

    /// レンダラーと同じデバイス（D3D12 の LUID）を開く。無ければ最初のデバイスにフォールバックする。
    pub fn open_matching(luid: Option<[u8; 8]>) -> Result<Self, Error> {
        let library = Library::load()?;
        // SAFETY: `cuInit` はフラグを一つ取り、0 が定められた値。
        let status = unsafe { (library.symbols.init)(0) };
        library.check("cuInit", status)?;
        let mut count = 0;
        // SAFETY: 生きたローカル変数を一つ渡す。
        let status = unsafe { (library.symbols.device_get_count)(&mut count) };
        library.check("cuDeviceGetCount", status)?;
        if count < 1 {
            return Err(Error::Unavailable(String::from(
                "CUDA のデバイスが一つもありません",
            )));
        }
        // `0` はデバイスが一つ以上あるときに必ずある序数。
        let first = library.device(0).map_err(|status| {
            Error::Unavailable(format!("cuDeviceGet: {}", library.text(status)))
        })?;
        #[cfg(windows)]
        let device = luid
            .and_then(|wanted| {
                (0..count).find_map(|ordinal| {
                    let found = library.device(ordinal).ok()?;
                    (library.luid(found)? == wanted).then_some(found)
                })
            })
            .unwrap_or(first);
        #[cfg(not(windows))]
        let device = {
            let _ = luid;
            first
        };
        let mut name: [c_char; 128] = [0; 128];
        // SAFETY: `name` は 128 バイトの生きた配列で、その長さを渡す。
        unsafe {
            (library.symbols.device_get_name)(name.as_mut_ptr(), name.len() as c_int - 1, device)
        };
        // SAFETY: CUDA は NUL 終端で書き、最後の 1 バイトは 0 のまま残る。
        let name = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let mut raw: CUcontext = std::ptr::null_mut();
        // SAFETY: 生きたローカル変数とデバイス。プライマリーコンテキストは `Drop` で一度だけ解放する。
        let status = unsafe { (library.symbols.primary_ctx_retain)(&mut raw, device) };
        library.check("cuDevicePrimaryCtxRetain", status)?;
        Ok(Self {
            library,
            device,
            raw,
            name,
        })
    }

    pub fn attribute(&self, which: c_int) -> Option<i32> {
        let mut value: c_int = 0;
        // SAFETY: 生きたローカル変数と、`cuDeviceGet` が返したデバイス。
        let status =
            unsafe { (self.library.symbols.device_get_attribute)(&mut value, which, self.device) };
        (status == CUDA_SUCCESS).then_some(value)
    }

    pub fn memory(&self) -> Option<(usize, usize)> {
        let _current = Current::push(self).ok()?;
        let (mut free, mut total) = (0usize, 0usize);
        // SAFETY: コンテキストを有効にした間に、生きたローカル変数へ書かせる。
        let status = unsafe { (self.library.symbols.mem_get_info)(&mut free, &mut total) };
        (status == 0).then_some((free >> 20, total >> 20))
    }

    pub fn exports_dma_buf(&self) -> bool {
        self.attribute(CU_DEVICE_ATTRIBUTE_DMA_BUF_SUPPORTED)
            .is_some_and(|value| value != 0)
    }

    pub fn caps(&self, codec: c_int, depth: u32) -> Result<DecodeCaps, Error> {
        let _current = Current::push(self)?;
        let mut caps = DecodeCaps {
            codec,
            chroma_format: CUDA_VIDEO_CHROMA_420,
            bit_depth_minus8: depth.saturating_sub(8),
            ..DecodeCaps::default()
        };
        // SAFETY: 生きたローカル変数。入力の三つを埋めてあり、ドライバーは残りを書く。
        let status = unsafe { (self.library.symbols.get_decoder_caps)(&mut caps) };
        self.library.check("cuvidGetDecoderCaps", status)?;
        Ok(caps)
    }

    pub fn decoder(self: &Arc<Self>, mut info: DecodeCreateInfo) -> Result<Decoder, Error> {
        let _current = Current::push(self)?;
        let mut raw: CUvideodecoder = std::ptr::null_mut();
        // SAFETY: 生きたローカル変数を二つ渡す。ハンドルは `Drop` で一度だけ破棄する。
        let status = unsafe { (self.library.symbols.create_decoder)(&mut raw, &mut info) };
        self.library.check("cuvidCreateDecoder", status)?;
        Ok(Decoder {
            context: Arc::clone(self),
            raw,
        })
    }

    pub fn copy(&self, copy: &Memcpy2D) -> Result<(), Error> {
        let _current = Current::push(self)?;
        // SAFETY: 生きた `CUDA_MEMCPY2D`。両側は呼び出し側が揃えている。
        let status = unsafe { (self.library.symbols.memcpy_2d)(copy) };
        self.library.check("cuMemcpy2D", status)
    }

    pub(super) fn check(&self, what: &str, status: CUresult) -> Result<(), Error> {
        self.library.check(what, status)
    }

    #[cfg(windows)]
    pub fn luid(&self) -> Option<[u8; 8]> {
        let _current = Current::push(self).ok()?;
        self.library.luid(self.device)
    }

    #[cfg(windows)]
    pub fn stream(self: &Arc<Self>) -> Result<Stream, Error> {
        let _current = Current::push(self)?;
        let mut raw: CUstream = std::ptr::null_mut();
        // SAFETY: 生きたローカル変数。`CU_STREAM_NON_BLOCKING` は 1。
        let status = unsafe { (self.library.symbols.stream_create)(&mut raw, 1) };
        self.check("cuStreamCreate", status)?;
        Ok(Stream {
            context: Arc::clone(self),
            raw,
        })
    }

    /// 外部メモリー（NT のハンドル）を線形のバッファとしてインポートする。
    /// インポートが参照を持つので、ハンドルは呼び出し側がすぐ閉じてよい。
    #[cfg(windows)]
    pub fn import(self: &Arc<Self>, handle: *mut c_void, size: u64) -> Result<Shared, Error> {
        let _current = Current::push(self)?;
        let description = ExternalMemoryHandleDesc {
            kind: CU_EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE,
            handle: ExternalHandle {
                win32_handle: handle,
                win32_name: std::ptr::null(),
            },
            size,
            flags: CUDA_EXTERNAL_MEMORY_DEDICATED,
            reserved: [0; 16],
        };
        let mut memory: CUexternalMemory = std::ptr::null_mut();
        // SAFETY: 生きたローカル変数を二つ。ハンドルは `Drop` で一度だけ解放する。
        let status =
            unsafe { (self.library.symbols.import_external_memory)(&mut memory, &description) };
        self.check("cuImportExternalMemory", status)?;
        let buffer = ExternalMemoryBufferDesc {
            offset: 0,
            size,
            flags: 0,
            reserved: [0; 16],
        };
        let mut pointer: CUdeviceptr = 0;
        // SAFETY: `memory` は今インポートしたもの、残り二つは生きたローカル変数。
        let status = unsafe {
            (self.library.symbols.external_memory_get_mapped_buffer)(&mut pointer, memory, &buffer)
        };
        let made = Shared {
            context: Arc::clone(self),
            memory,
            pointer,
        };
        self.check("cuExternalMemoryGetMappedBuffer", status)?;
        Ok(made)
    }

    #[cfg(windows)]
    pub fn import_fence(self: &Arc<Self>, handle: *mut c_void) -> Result<Fence, Error> {
        let _current = Current::push(self)?;
        let description = ExternalSemaphoreHandleDesc {
            kind: CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE,
            handle: ExternalHandle {
                win32_handle: handle,
                win32_name: std::ptr::null(),
            },
            flags: 0,
            reserved: [0; 16],
        };
        let mut semaphore: CUexternalSemaphore = std::ptr::null_mut();
        // SAFETY: 生きたローカル変数を二つ。ハンドルは `Drop` で一度だけ解放する。
        let status = unsafe {
            (self.library.symbols.import_external_semaphore)(&mut semaphore, &description)
        };
        self.check("cuImportExternalSemaphore", status)?;
        Ok(Fence {
            context: Arc::clone(self),
            semaphore,
        })
    }
}

#[cfg(windows)]
pub struct Stream {
    context: Arc<Context>,
    raw: CUstream,
}

// SAFETY: ストリームのハンドルはスレッドに紐づかない。一つのデコーダーが持つ。
#[cfg(windows)]
unsafe impl Send for Stream {}

#[cfg(windows)]
impl std::fmt::Debug for Stream {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Stream")
    }
}

#[cfg(windows)]
impl Stream {
    pub fn copy(&self, copy: &Memcpy2D) -> Result<(), Error> {
        let _current = Current::push(&self.context)?;
        // SAFETY: 生きた `CUDA_MEMCPY2D`。完了はフェンスか [`Stream::wait`] が待つ。
        let status = unsafe { (self.context.library.symbols.memcpy_2d_async)(copy, self.raw) };
        self.context.check("cuMemcpy2DAsync", status)
    }

    pub fn wait(&self) -> Result<(), Error> {
        let _current = Current::push(&self.context)?;
        // SAFETY: `raw` はこのコンテキストで作ったストリーム。
        let status = unsafe { (self.context.library.symbols.stream_synchronize)(self.raw) };
        self.context.check("cuStreamSynchronize", status)
    }
}

#[cfg(windows)]
impl Drop for Stream {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.context) else {
            return;
        };
        // SAFETY: `cuStreamCreate` が作ったものを一度だけ破棄する。
        unsafe { (self.context.library.symbols.stream_destroy)(self.raw) };
    }
}

#[cfg(windows)]
pub struct Shared {
    context: Arc<Context>,
    memory: CUexternalMemory,
    pub pointer: CUdeviceptr,
}

// SAFETY: インポートのハンドルもデバイスのアドレスも、スレッドに紐づかない。
#[cfg(windows)]
unsafe impl Send for Shared {}

#[cfg(windows)]
impl std::fmt::Debug for Shared {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Shared(0x{:x})", self.pointer)
    }
}

#[cfg(windows)]
impl Drop for Shared {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.context) else {
            return;
        };
        // SAFETY: マップしたアドレスを外してから、インポートしたものを一度だけ解放する。
        // アドレスはインポートと一緒には消えないので `cuMemFree` で解放する。
        unsafe {
            (self.context.library.symbols.mem_free)(self.pointer);
            (self.context.library.symbols.destroy_external_memory)(self.memory);
        }
    }
}

#[cfg(windows)]
pub struct Fence {
    context: Arc<Context>,
    semaphore: CUexternalSemaphore,
}

// SAFETY: フェンスのハンドルはスレッドに紐づかない。
#[cfg(windows)]
unsafe impl Send for Fence {}

#[cfg(windows)]
impl std::fmt::Debug for Fence {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Fence")
    }
}

#[cfg(windows)]
impl Fence {
    /// ストリームに、フェンスが `value` に届くまで待たせる。
    pub fn wait(&self, stream: &Stream, value: u64) -> Result<(), Error> {
        let _current = Current::push(&self.context)?;
        let params = ExternalSemaphoreParams {
            fence_value: value,
            ..ExternalSemaphoreParams::default()
        };
        // SAFETY: フェンスもストリームもこのコンテキストのもの、引数は生きたローカル変数。
        let status = unsafe {
            (self.context.library.symbols.wait_external_semaphores)(
                &self.semaphore,
                &params,
                1,
                stream.raw,
            )
        };
        self.context.check("cuWaitExternalSemaphoresAsync", status)
    }
}

#[cfg(windows)]
impl Drop for Fence {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.context) else {
            return;
        };
        // SAFETY: インポートしたものを一度だけ解放する。
        unsafe { (self.context.library.symbols.destroy_external_semaphore)(self.semaphore) };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: `open` で取ったものを一度だけ解放する。
        unsafe { (self.library.symbols.primary_ctx_release)(self.device) };
    }
}
