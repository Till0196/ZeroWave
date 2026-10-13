// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコーダー、マップしたサーフェス、Linux の線形の割り当て（DMA-BUF）。

use super::*;

pub struct Decoder {
    pub(super) context: Arc<Context>,
    pub(super) raw: CUvideodecoder,
}

// SAFETY: 不透明なハンドルでスレッドに紐づかない。一つのデコーダーが持つ。
unsafe impl Send for Decoder {}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Decoder")
    }
}

impl Decoder {
    pub fn context(&self) -> &Arc<Context> {
        &self.context
    }

    pub fn decode(&self, params: &mut PicParams) -> Result<(), Error> {
        let _current = Current::push(&self.context)?;
        // SAFETY: 生きた `CUVIDPICPARAMS`。中の配列は呼び出し側が生かしている。
        let status = unsafe { (self.context.library.symbols.decode_picture)(self.raw, params) };
        self.context.check("cuvidDecodePicture", status)
    }

    pub fn status(&self, pic_idx: i32) -> Option<c_int> {
        let _current = Current::push(&self.context).ok()?;
        let mut status = DecodeStatus::default();
        // SAFETY: 生きたローカル変数の `CUVIDGETDECODESTATUS` を一つ渡す。
        let result = unsafe {
            (self.context.library.symbols.get_decode_status)(self.raw, pic_idx, &mut status)
        };
        (result == CUDA_SUCCESS).then_some(status.status)
    }

    /// デコードし終えたサーフェスをマップする。NV12 / P016 の線形で、色差は `pitch * target_height` から。
    pub fn map(&self, pic_idx: i32, proc: &mut ProcParams) -> Result<Mapped<'_>, Error> {
        let _current = Current::push(&self.context)?;
        let mut pointer: u64 = 0;
        let mut pitch: c_uint = 0;
        // SAFETY: 三つとも生きたローカル変数。`pic_idx` はこのデコーダーへ渡した `CurrPicIdx`。
        let status = unsafe {
            (self.context.library.symbols.map_video_frame)(
                self.raw,
                pic_idx,
                &mut pointer,
                &mut pitch,
                proc,
            )
        };
        self.context.check("cuvidMapVideoFrame", status)?;
        Ok(Mapped {
            decoder: self,
            pointer,
            pitch: pitch as usize,
        })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.context) else {
            return;
        };
        // SAFETY: `cuvidCreateDecoder` が作ったハンドルを一度だけ破棄する。
        unsafe { (self.context.library.symbols.destroy_decoder)(self.raw) };
    }
}

#[derive(Debug)]
pub struct Mapped<'a> {
    decoder: &'a Decoder,
    pub pointer: CUdeviceptr,
    pub pitch: usize,
}

impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.decoder.context) else {
            return;
        };
        // SAFETY: `cuvidMapVideoFrame64` がマップしたものを一度だけアンマップする。
        unsafe {
            (self.decoder.context.library.symbols.unmap_video_frame)(self.decoder.raw, self.pointer)
        };
    }
}

/// NVDEC のサーフェスのコピー先の線形の割り当て。DMA-BUF としてエクスポートする（Linux だけ）。
#[cfg(unix)]
pub struct Linear {
    context: Arc<Context>,
    handle: u64,
    reserved: CUdeviceptr,
    size: usize,
    pub pointer: CUdeviceptr,
    pub pitch: usize,
    pub rows: usize,
}

// SAFETY: 中身はデバイスのアドレスと割り当てのハンドルで、どちらもスレッドに紐づかない。
#[cfg(unix)]
unsafe impl Send for Linear {}

#[cfg(unix)]
impl std::fmt::Debug for Linear {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Linear({} x {})", self.pitch, self.rows)
    }
}

#[cfg(unix)]
impl Linear {
    /// `rows` は輝度と色差の行の合計。ストライドをサーフェスと同じにすれば一回でコピーできる。
    pub fn new(context: &Arc<Context>, pitch: usize, rows: usize) -> Result<Self, Error> {
        let _current = Current::push(context)?;
        let symbols = &context.library.symbols;
        let prop = MemAllocationProp {
            kind: CU_MEM_ALLOCATION_TYPE_PINNED,
            // エクスポートできる割り当てだと最初に指定する。指定しないとエクスポートが拒否される。
            requested_handle_types: CU_MEM_HANDLE_TYPE_POSIX_FILE_DESCRIPTOR,
            location: MemLocation {
                kind: CU_MEM_LOCATION_TYPE_DEVICE,
                id: context.device,
            },
            ..MemAllocationProp::default()
        };
        let mut granularity = 0usize;
        // SAFETY: 生きたローカル変数と生きた `CUmemAllocationProp`。
        let status = unsafe {
            (symbols.mem_get_allocation_granularity)(
                &mut granularity,
                &prop,
                CU_MEM_ALLOC_GRANULARITY_RECOMMENDED,
            )
        };
        context.check("cuMemGetAllocationGranularity", status)?;
        let granularity = granularity.max(1);
        let size = (pitch * rows).next_multiple_of(granularity);
        let mut handle: u64 = 0;
        // SAFETY: 生きたローカル変数と生きたプロパティ。フラグは 0 が定められた値。
        let status = unsafe { (symbols.mem_create)(&mut handle, size, &prop, 0) };
        context.check("cuMemCreate", status)?;
        let mut reserved: CUdeviceptr = 0;
        // SAFETY: 生きたローカル変数。アラインメントもアドレスの指定も 0 で「任せる」。
        let status = unsafe { (symbols.mem_address_reserve)(&mut reserved, size, 0, 0, 0) };
        if let Err(error) = context.check("cuMemAddressReserve", status) {
            // SAFETY: 直前に作ったハンドルを一度だけ解放する。
            unsafe { (symbols.mem_release)(handle) };
            return Err(error);
        }
        // SAFETY: 今予約したアドレス範囲と、今作った割り当て。
        let status = unsafe { (symbols.mem_map)(reserved, size, 0, handle, 0) };
        if let Err(error) = context.check("cuMemMap", status) {
            // SAFETY: マップできなかったので、予約したアドレスとハンドルを戻す。
            unsafe {
                (symbols.mem_address_free)(reserved, size);
                (symbols.mem_release)(handle);
            }
            return Err(error);
        }
        let access = MemAccessDesc {
            location: prop.location,
            flags: CU_MEM_ACCESS_FLAGS_PROT_READWRITE,
        };
        // SAFETY: マップしたばかりの範囲と、生きたアクセス記述子を一つ。
        let status = unsafe { (symbols.mem_set_access)(reserved, size, &access, 1) };
        let made = Self {
            context: Arc::clone(context),
            handle,
            reserved,
            size,
            pointer: reserved,
            pitch,
            rows,
        };
        context.check("cuMemSetAccess", status)?;
        Ok(made)
    }

    /// DMA-BUF としてエクスポートする。レイアウトは線形（`DRM_FORMAT_MOD_LINEAR`）。
    pub fn export(&self) -> Result<std::os::fd::OwnedFd, Error> {
        use std::os::fd::FromRawFd;
        let _current = Current::push(&self.context)?;
        let mut fd: c_int = -1;
        // SAFETY: `fd` は生きたローカル変数（`int`）。範囲はこの割り当てのアドレスそのもの。
        let status = unsafe {
            (self
                .context
                .library
                .symbols
                .mem_get_handle_for_address_range)(
                (&raw mut fd).cast(),
                self.pointer,
                self.size,
                CU_MEM_RANGE_HANDLE_TYPE_DMA_BUF_FD,
                0,
            )
        };
        self.context
            .check("cuMemGetHandleForAddressRange", status)?;
        if fd < 0 {
            return Err(Error::Unavailable(String::from(
                "DMA-BUF のハンドルが返りませんでした",
            )));
        }
        // SAFETY: CUDA が今作った、誰も持っていないハンドル。
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
    }
}

#[cfg(unix)]
impl Drop for Linear {
    fn drop(&mut self) {
        let Ok(_current) = Current::push(&self.context) else {
            return;
        };
        let symbols = &self.context.library.symbols;
        // SAFETY: マップした順の逆に、一度ずつ。
        unsafe {
            (symbols.mem_unmap)(self.reserved, self.size);
            (symbols.mem_address_free)(self.reserved, self.size);
            (symbols.mem_release)(self.handle);
        }
    }
}
