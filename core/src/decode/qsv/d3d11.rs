// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! QSV のサーフェスを GPU のメモリ（D3D11 のテクスチャ）に置き、D3D12 のレンダラーへ渡す。
//! メインメモリで受け渡すと、デコード・VPP・レンダラーのあいだで 1 フレームに GPU と CPU を 3 回往復する。
//! サーフェスの確保は `mfxFrameAllocator` で受け持つ。VPP の出力は初めから D3D12 と共有するテクスチャに作り、
//! そのまま渡す。デコーダーの出力（配列のスライス）は GPU の上で共有テクスチャへ写す。どちらも共有フェンスで
//! 待ち合わせる。

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use super::mfx;
use crate::decode::Error;
use crate::decode::d3d12::Frame;
use crate::decode::d3d12::d3d11::shared_texture;
use crate::decode::d3d12::share::Sharer;

/// 一枚のサーフェス。`mfxMemId` はこれを指す。
struct Mid {
    texture: ID3D11Texture2D,
    index: u32,
    /// D3D12 で開いた同じテクスチャ（VPP の出力。写さずに渡す）。
    opened: Option<ID3D12Resource>,
    /// レンダラーが持っている間は 2 以上（その間は VPP の出力先にしない）。
    lease: Arc<()>,
}

/// 一回の `Alloc` で作った物。`mids` の先頭のアドレスで `Free` と突き合わせる。
struct Allocation {
    /// ランタイムが要素のアドレスを持つので、作った後は増やさない。
    _mids: Vec<Mid>,
    pointers: Vec<*mut c_void>,
    /// デコーダーの外部のサーフェス。ランタイムが初期化の中でもう一度求めたら、同じ物を返す。
    decoder: Option<(u16, u16, u32)>,
    /// 返した回数（`Free` の回数と合わせて解放する）。
    users: u32,
    /// アプリケーションが [`Gpu::surfaces`] で確保した（ストリームを閉じたら [`Gpu::forget`] で手放す）。
    ours: bool,
}

struct Frames {
    device: ID3D11Device,
    raw12: ID3D12Device,
    allocations: Mutex<Vec<Allocation>>,
    /// いまの `Alloc` はアプリケーションから。
    ours: std::sync::atomic::AtomicBool,
}

/// GPU のメモリで受け渡すための物。ランタイムが `allocator` を指すので、箱に入れて動かさない。
pub(super) struct Gpu {
    device: ID3D11Device,
    frames: Box<Frames>,
    allocator: Box<mfx::FrameAllocator>,
    sharer: Sharer,
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "QsvGpu({:?})", self.sharer)
    }
}

// SAFETY: デコードのスレッドだけが触り、D3D11 のデバイスはマルチスレッド保護を有効にしてある。
unsafe impl Send for Gpu {}

impl Gpu {
    /// 描画に使う D3D12 のデバイスの GPU に作る（QSV のセッションも同じ GPU で開いてあること）。
    pub fn open() -> Result<Self, Error> {
        let raw12 = crate::decode::d3d12::dx::adopted()
            .ok_or_else(|| Error::Unavailable(String::from("レンダラーのデバイスが無い")))?;
        let (device, context) = crate::decode::d3d12::d3d11::device_on(&raw12)?;
        let sharer = Sharer::open(&raw12, &device, &context)?;
        let mut frames = Box::new(Frames {
            device: device.clone(),
            raw12: raw12.clone(),
            allocations: Mutex::new(Vec::new()),
            ours: std::sync::atomic::AtomicBool::new(false),
        });
        let allocator = Box::new(mfx::FrameAllocator::new(
            std::ptr::from_mut(frames.as_mut()).cast(),
            alloc,
            lock,
            get_hdl,
            free,
        ));
        Ok(Self {
            device,
            frames,
            allocator,
            sharer,
        })
    }

    /// session に D3D11 のデバイスとサーフェスの確保を渡す（デコーダーを作る前に）。
    pub fn attach(&mut self, entries: &mfx::Entries, session: mfx::Session) -> Result<(), Error> {
        // SAFETY: 生きた session とデバイス。デバイスとアロケーターは session より長生きする（`Qsv` が持つ）。
        unsafe {
            let status =
                (entries.set_handle)(session, mfx::HANDLE_D3D11_DEVICE, self.device.as_raw());
            if status < mfx::ERR_NONE {
                return Err(Error::Unavailable(format!(
                    "MFXVideoCORE_SetHandle が {status} を返しました"
                )));
            }
            let status = (entries.set_frame_allocator)(session, self.allocator.as_mut());
            if status < mfx::ERR_NONE {
                return Err(Error::Unavailable(format!(
                    "MFXVideoCORE_SetFrameAllocator が {status} を返しました"
                )));
            }
        }
        Ok(())
    }

    /// `request` のとおりにビデオメモリのサーフェスを作り、`mfxMemId` を返す。
    pub fn surfaces(
        &mut self,
        request: &mut mfx::FrameAllocRequest,
    ) -> Result<Vec<*mut c_void>, Error> {
        let mut response = mfx::FrameAllocResponse::default();
        self.frames
            .ours
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: アロケーターは self の物で、二つの生きたローカル変数を渡す。
        let status = unsafe {
            alloc(
                std::ptr::from_mut(self.frames.as_mut()).cast(),
                request,
                &mut response,
            )
        };
        self.frames
            .ours
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if status < mfx::ERR_NONE {
            return Err(Error::Unavailable(format!(
                "サーフェスを確保できません（{status}）"
            )));
        }
        // SAFETY: `alloc` が `num_frame_actual` 個の配列を指して返した。
        Ok(unsafe {
            std::slice::from_raw_parts(response.mids, usize::from(response.num_frame_actual))
        }
        .to_vec())
    }

    /// レンダラーがまだ持っているサーフェスか（VPP の出力先にしない）。
    pub fn busy(&self, mem_id: *mut c_void) -> bool {
        if mem_id.is_null() {
            return false;
        }
        // SAFETY: このアロケーターが作った `Mid` を指す。
        let mid = unsafe { &*mem_id.cast::<Mid>() };
        Arc::strong_count(&mid.lease) > 1
    }

    /// 出来たサーフェスを渡す（書き終わりはフェンスで知らせる）。D3D12 で開いてあればそのまま、
    /// 無ければ GPU の上で共有テクスチャへ写す。
    pub fn hand(
        &mut self,
        mem_id: *mut c_void,
        wide: bool,
        width: u32,
        height: u32,
    ) -> Result<Option<Frame>, Error> {
        if mem_id.is_null() {
            return Ok(None);
        }
        // SAFETY: このアロケーターが作った `Mid` を指す（ランタイムはそのまま返す）。
        let mid = unsafe { &*mem_id.cast::<Mid>() };
        if let Some(opened) = mid.opened.as_ref() {
            let ready = self.sharer.signal()?;
            return Ok(Some(Frame {
                device: self.sharer.raw12().clone(),
                texture: opened.clone(),
                width,
                height,
                ten_bit: wide,
                lease: Some(mid.lease.clone()),
                ready: Some(ready),
            }));
        }
        self.sharer
            .hand(&mid.texture, mid.index, width, height, wide)
            .map(Some)
    }

    /// アプリケーションが確保したサーフェスを手放す（デコーダーと VPP を閉じた後に）。
    pub fn forget(&mut self) {
        if let Ok(mut allocations) = self.frames.allocations.lock() {
            for allocation in allocations.iter_mut().filter(|allocation| allocation.ours) {
                allocation.ours = false;
                allocation.users -= 1;
            }
            allocations.retain(|allocation| allocation.users > 0);
        }
    }
}

unsafe extern "C" fn alloc(
    pthis: *mut c_void,
    request: *mut mfx::FrameAllocRequest,
    response: *mut mfx::FrameAllocResponse,
) -> mfx::Status {
    // SAFETY: `pthis` は `Gpu` が箱に入れて持つ `Frames`、ほかはランタイムが渡した生きた構造体。
    let (frames, request, response) =
        unsafe { (&*pthis.cast::<Frames>(), &*request, &mut *response) };
    if request.kind & mfx::MEMTYPE_SYSTEM_MEMORY != 0 {
        return mfx::ERR_UNSUPPORTED;
    }
    let info = request.info;
    // アプリケーションが先に確保したデコーダーのサーフェスは、ランタイムが `DECODE_Init` の中で同じ要求をして
    // 受け取る（違う物を返すと、渡したサーフェスと食い違い、デコードが未定義の動作を返す）。
    let external_decoder = request.kind & mfx::MEMTYPE_EXTERNAL_FRAME != 0
        && request.kind & mfx::MEMTYPE_FROM_DECODE != 0;
    let shape = (info.width, info.height, info.fourcc);
    if external_decoder && let Ok(mut allocations) = frames.allocations.lock() {
        let wanted = request.num_frame_min;
        if let Some(found) = allocations.iter_mut().find(|allocation| {
            allocation.decoder == Some(shape) && allocation.pointers.len() >= usize::from(wanted)
        }) {
            found.users += 1;
            found.ours |= frames.ours.load(std::sync::atomic::Ordering::Relaxed);
            response.mids = found.pointers.as_mut_ptr();
            response.num_frame_actual = found.pointers.len() as u16;
            response.mem_type = request.kind;
            return mfx::ERR_NONE;
        }
    }
    let format = if info.fourcc == mfx::FOURCC_P010 {
        DXGI_FORMAT_P010
    } else {
        DXGI_FORMAT_NV12
    };
    let count = request
        .num_frame_suggested
        .max(request.num_frame_min)
        .max(1);
    // デコーダーの出力は配列のスライス（デコーダーが求める）。VPP の出力は一枚ずつ。
    let decoder = request.kind & mfx::MEMTYPE_DXVA2_DECODER_TARGET != 0;
    let texture = |array: u16, bind: u32| -> Option<ID3D11Texture2D> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: u32::from(info.width),
            Height: u32::from(info.height),
            MipLevels: 1,
            ArraySize: u32::from(array),
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut made = None;
        // SAFETY: 記述は生きたローカル。
        unsafe {
            frames
                .device
                .CreateTexture2D(&description, None, Some(&mut made))
        }
        .ok()?;
        made
    };
    let mut mids = Vec::with_capacity(usize::from(count));
    if decoder {
        let Some(array) = texture(count, D3D11_BIND_DECODER.0 as u32) else {
            return mfx::ERR_MEMORY_ALLOC;
        };
        for index in 0..u32::from(count) {
            mids.push(Mid {
                texture: array.clone(),
                index,
                opened: None,
                lease: Arc::new(()),
            });
        }
    } else {
        // VPP の出力は D3D12 と共有して作る（写さずに渡す）。共有できなければ共有しないで作る。
        let bind = D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0);
        for _ in 0..count {
            let made = shared_texture(
                &frames.raw12,
                &frames.device,
                u32::from(info.width),
                u32::from(info.height),
                format,
                bind,
            );
            let (opened, one) = match made {
                Ok((one, opened)) => (Some(opened), one),
                Err(_) => match texture(1, bind.0 as u32) {
                    Some(one) => (None, one),
                    None => return mfx::ERR_MEMORY_ALLOC,
                },
            };
            mids.push(Mid {
                texture: one,
                index: 0,
                opened,
                lease: Arc::new(()),
            });
        }
    }
    let mut pointers: Vec<*mut c_void> = mids
        .iter_mut()
        .map(|mid| std::ptr::from_mut(mid).cast())
        .collect();
    response.mids = pointers.as_mut_ptr();
    response.num_frame_actual = count;
    response.mem_type = request.kind;
    if let Ok(mut allocations) = frames.allocations.lock() {
        allocations.push(Allocation {
            _mids: mids,
            pointers,
            decoder: external_decoder.then_some(shape),
            users: 1,
            ours: frames.ours.load(std::sync::atomic::Ordering::Relaxed),
        });
    }
    mfx::ERR_NONE
}

/// ビデオメモリは CPU から読み書きしない。
unsafe extern "C" fn lock(
    _pthis: *mut c_void,
    _mid: *mut c_void,
    _data: *mut mfx::FrameData,
) -> mfx::Status {
    mfx::ERR_UNSUPPORTED
}

unsafe extern "C" fn get_hdl(
    _pthis: *mut c_void,
    mid: *mut c_void,
    handle: *mut *mut c_void,
) -> mfx::Status {
    if mid.is_null() || handle.is_null() {
        return mfx::ERR_UNSUPPORTED;
    }
    // SAFETY: `mid` はこのアロケーターが作った `Mid`、`handle` は D3D11 では `mfxHDLPair` を指す。
    unsafe {
        let mid = &*mid.cast::<Mid>();
        let pair = &mut *handle.cast::<mfx::HdlPair>();
        pair.first = mid.texture.as_raw();
        pair.second = mid.index as usize as *mut c_void;
    }
    mfx::ERR_NONE
}

unsafe extern "C" fn free(
    pthis: *mut c_void,
    response: *mut mfx::FrameAllocResponse,
) -> mfx::Status {
    // SAFETY: `pthis` は `Frames`、`response` は `alloc` が埋めた物。
    let (frames, response) = unsafe { (&*pthis.cast::<Frames>(), &*response) };
    if let Ok(mut allocations) = frames.allocations.lock()
        && let Some(at) = allocations
            .iter()
            .position(|allocation| allocation.pointers.as_ptr().cast_mut() == response.mids)
    {
        allocations[at].users -= 1;
        if allocations[at].users == 0 {
            allocations.remove(at);
        }
    }
    mfx::ERR_NONE
}
