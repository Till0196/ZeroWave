// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Intel oneVPL の AI フレーム補間（x2 だけ）。サーフェスは GPU のメモリに置く（メインメモリだと壊れる）。
//! NV12 しか受けないので、共有の NV12 へレンダリングして渡し RGB に戻す。k フレーム目を渡すと k−1 と k の
//! 真ん中が `MFX_ERR_MORE_SURFACE` で出て、もう一度呼ぶと k フレーム目が出る。

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::core::Interface;

use super::bridge::{Lane, Worker, adapter_of};
use super::d3d12::{self, Fence};
use super::{Convert, Gpu, Run};
use crate::decode::qsv::mfx;

/// 90 kHz、入力を 30 フレーム/秒と指定するので一フレーム 3000。
const TICK: u64 = 3000;
const WAIT_MS: u32 = 1000;
const INTEL: u32 = 0x8086;

type Session = *mut c_void;

#[derive(Clone, Copy)]
struct Entries {
    load: unsafe extern "C" fn() -> *mut c_void,
    unload: unsafe extern "C" fn(*mut c_void),
    create_config: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    set_filter: unsafe extern "C" fn(*mut c_void, *const u8, mfx::Variant) -> mfx::Status,
    create_session: unsafe extern "C" fn(*mut c_void, u32, *mut Session) -> mfx::Status,
    close: unsafe extern "C" fn(Session) -> mfx::Status,
    set_handle: unsafe extern "C" fn(Session, i32, *mut c_void) -> mfx::Status,
    sync_operation: unsafe extern "C" fn(Session, mfx::SyncPoint, u32) -> mfx::Status,
    vpp_query:
        unsafe extern "C" fn(Session, *mut mfx::VideoParam, *mut mfx::VideoParam) -> mfx::Status,
    vpp_init: unsafe extern "C" fn(Session, *mut mfx::VideoParam) -> mfx::Status,
    vpp_close: unsafe extern "C" fn(Session) -> mfx::Status,
    vpp_run: unsafe extern "C" fn(
        Session,
        *mut mfx::FrameSurface1,
        *mut mfx::FrameSurface1,
        *mut c_void,
        *mut mfx::SyncPoint,
    ) -> mfx::Status,
    surface_in: unsafe extern "C" fn(Session, *mut *mut mfx::FrameSurface1) -> mfx::Status,
    surface_out: unsafe extern "C" fn(Session, *mut *mut mfx::FrameSurface1) -> mfx::Status,
}

/// 一度だけ開く（開けなかった理由も覚える）。
fn library() -> Result<&'static (crate::dl::Object, Entries), String> {
    static OPENED: std::sync::OnceLock<Result<(crate::dl::Object, Entries), String>> =
        std::sync::OnceLock::new();
    OPENED
        .get_or_init(|| {
            let object = crate::dl::Object::open("libvpl.dll").map_err(|e| e.0)?;
            // SAFETY: シンボルの型はヘッダー（`mfxdispatcher.h` など）の宣言のとおり。
            let entries = unsafe {
                Entries {
                    load: object.symbol(b"MFXLoad\0").map_err(|e| e.0)?,
                    unload: object.symbol(b"MFXUnload\0").map_err(|e| e.0)?,
                    create_config: object.symbol(b"MFXCreateConfig\0").map_err(|e| e.0)?,
                    set_filter: object
                        .symbol(b"MFXSetConfigFilterProperty\0")
                        .map_err(|e| e.0)?,
                    create_session: object.symbol(b"MFXCreateSession\0").map_err(|e| e.0)?,
                    close: object.symbol(b"MFXClose\0").map_err(|e| e.0)?,
                    set_handle: object
                        .symbol(b"MFXVideoCORE_SetHandle\0")
                        .map_err(|e| e.0)?,
                    sync_operation: object
                        .symbol(b"MFXVideoCORE_SyncOperation\0")
                        .map_err(|e| e.0)?,
                    vpp_query: object.symbol(b"MFXVideoVPP_Query\0").map_err(|e| e.0)?,
                    vpp_init: object.symbol(b"MFXVideoVPP_Init\0").map_err(|e| e.0)?,
                    vpp_close: object.symbol(b"MFXVideoVPP_Close\0").map_err(|e| e.0)?,
                    vpp_run: object
                        .symbol(b"MFXVideoVPP_RunFrameVPPAsync\0")
                        .map_err(|e| e.0)?,
                    surface_in: object
                        .symbol(b"MFXMemory_GetSurfaceForVPP\0")
                        .map_err(|e| e.0)?,
                    surface_out: object
                        .symbol(b"MFXMemory_GetSurfaceForVPPOut\0")
                        .map_err(|e| e.0)?,
                }
            };
            Ok((object, entries))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn failed(what: &str, status: mfx::Status) -> String {
    format!("{what} が {status} を返した")
}

struct Opened {
    entries: Entries,
    loader: *mut c_void,
    session: Session,
}

impl Opened {
    fn open() -> Result<Self, String> {
        let (_, entries) = library()?;
        let entries = *entries;
        // SAFETY: エントリーポイントの型は宣言のとおり。名前は NUL 終端。
        unsafe {
            let loader = (entries.load)();
            if loader.is_null() {
                return Err(String::from("MFXLoad が null を返した"));
            }
            let opened = Self {
                entries,
                loader,
                session: std::ptr::null_mut(),
            };
            for (name, value) in [
                (c"mfxImplDescription.Impl", 2u32),
                (c"mfxImplDescription.AccelerationMode", 0x0300),
                (c"mfxImplDescription.VendorID", 0x8086),
            ] {
                let config = (entries.create_config)(loader);
                let status =
                    (entries.set_filter)(config, name.as_ptr().cast(), mfx::Variant::u32(value));
                if status < mfx::ERR_NONE {
                    return Err(failed(
                        &format!("MFXSetConfigFilterProperty（{}）", name.to_string_lossy()),
                        status,
                    ));
                }
            }
            let mut opened = opened;
            let status = (entries.create_session)(loader, 0, &mut opened.session);
            if status < mfx::ERR_NONE || opened.session.is_null() {
                opened.session = std::ptr::null_mut();
                return Err(failed("MFXCreateSession", status));
            }
            Ok(opened)
        }
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: 開いたものを、session・ローダーの順に一度ずつ閉じる。
        unsafe {
            if !self.session.is_null() {
                (self.entries.vpp_close)(self.session);
                (self.entries.close)(self.session);
            }
            (self.entries.unload)(self.loader);
        }
    }
}

/// NV12、入力 30 フレーム/秒・出力 60 フレーム/秒、サーフェスは GPU のメモリ。
fn param(size: [u32; 2], ext: *mut *mut mfx::ExtBuffer) -> mfx::VideoParam {
    let mut param = mfx::VideoParam::default();
    param.io_pattern = mfx::IOPATTERN_IN_VIDEO_MEMORY | mfx::IOPATTERN_OUT_VIDEO_MEMORY;
    param.async_depth = 1;
    param.ext_param = ext;
    param.num_ext_param = 1;
    let vpp = param.vpp_mut();
    for (info, rate) in [(&mut vpp.input, 30), (&mut vpp.output, 60)] {
        info.fourcc = mfx::FOURCC_NV12;
        info.chroma_format = mfx::CHROMAFORMAT_YUV420;
        info.pic_struct = mfx::PICSTRUCT_PROGRESSIVE;
        info.width = size[0].next_multiple_of(16) as u16;
        info.height = size[1].next_multiple_of(16) as u16;
        info.crop_w = size[0] as u16;
        info.crop_h = size[1] as u16;
        info.frame_rate_ext_n = rate;
        info.frame_rate_ext_d = 1;
        info.aspect_ratio_w = 1;
        info.aspect_ratio_h = 1;
    }
    param
}

/// 一度だけ `MFXVideoVPP_Query` で問い合わせる。
pub fn supported() -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| {
        let Ok(opened) = Opened::open() else {
            return false;
        };
        let mut ai = Box::new(mfx::ExtVppAiFrameInterpolation::new(
            mfx::AI_FRAME_INTERPOLATION_MODE_DEFAULT,
        ));
        let mut ext = [std::ptr::from_mut(&mut ai.header)];
        let mut asked = param([1920, 1080], ext.as_mut_ptr());
        let mut answer = asked;
        // SAFETY: 開いた session と有効なローカル変数。拡張バッファは呼び出しの間有効。
        let status = unsafe { (opened.entries.vpp_query)(opened.session, &mut asked, &mut answer) };
        status >= mfx::ERR_NONE
    })
}

/// `FrameInterface->Release`。
///
/// # Safety
///
/// `surface` はランタイムが返した、まだ解放していないサーフェス（null なら何もしない）。
unsafe fn release(surface: *mut mfx::FrameSurface1) {
    if surface.is_null() {
        return;
    }
    // SAFETY: 呼び出し側が保証する。テーブルのオフセットはヘッダーの定義どおり。
    unsafe {
        let table = (*surface).frame_interface().cast::<u8>();
        let f: unsafe extern "C" fn(*mut mfx::FrameSurface1) -> mfx::Status =
            std::mem::transmute_copy(&*table.add(mfx::INTERFACE_RELEASE).cast::<*const c_void>());
        f(surface);
    }
}

/// 参照カウントは増やさない：サーフェスを解放するまで使うだけ。
///
/// # Safety
///
/// `release` と同じ。
unsafe fn native(surface: *mut mfx::FrameSurface1) -> Result<ID3D11Texture2D, String> {
    // SAFETY: 呼び出し側が保証する。
    unsafe {
        let table = (*surface).frame_interface().cast::<u8>();
        let f: unsafe extern "C" fn(
            *mut mfx::FrameSurface1,
            *mut *mut c_void,
            *mut i32,
        ) -> mfx::Status = std::mem::transmute_copy(
            &*table
                .add(mfx::INTERFACE_GET_NATIVE_HANDLE)
                .cast::<*const c_void>(),
        );
        let mut resource = std::ptr::null_mut();
        let mut kind = 0;
        let status = f(surface, &mut resource, &mut kind);
        if status < mfx::ERR_NONE || resource.is_null() {
            return Err(failed("GetNativeHandle", status));
        }
        if kind != mfx::RESOURCE_DX11_TEXTURE {
            return Err(format!("サーフェスが D3D11 のテクスチャでない（{kind}）"));
        }
        let texture = ID3D11Texture2D::from_raw_borrowed(&resource)
            .ok_or("GetNativeHandle が空")?
            .clone();
        let mut description = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut description);
        // テクスチャ配列のインデックスは取得できないので、配列でない物だけを受ける。
        if description.ArraySize != 1 {
            return Err(format!(
                "サーフェスがテクスチャ配列（要素数 {}）で、どれか分からない",
                description.ArraySize
            ));
        }
        Ok(texture)
    }
}

struct Shared {
    lane: Lane,
    d3d11: ID3D11Texture2D,
}

pub struct Frc {
    /// レンダリング用 GPU と別なら [`Worker`] が転送する。
    worker: Worker,
    opened: Opened,
    _ai: Box<mfx::ExtVppAiFrameInterpolation>,
    _ext: Box<[*mut mfx::ExtBuffer; 1]>,
    _device: ID3D11Device,
    immediate: ID3D11DeviceContext4,
    size: [u32; 2],
    /// 16 の倍数に揃えない：D3D12 のテクスチャのビューはリソースの大きさで引き伸ばされ、映像が縦に縮む。
    /// ランタイムのサーフェスとは左上の領域だけをコピーし合う。
    padded: [u32; 2],
    input: Shared,
    out: Shared,
    to_vpl: (Fence, ID3D11Fence),
    from_vpl: (Fence, ID3D11Fence),
    frames: u64,
    has_mid: bool,
    /// wgpu の外なのでパスごとの計時に入らない。
    outside: f64,
}

impl Frc {
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn outside_ms(&self) -> f64 {
        self.outside
    }

    pub fn open(device: &wgpu::Device, size: [u32; 2]) -> Result<Self, String> {
        let worker = Worker::open(device, INTEL)?;
        tracing::info!("oneVPL: {} で補間{}", worker.name, worker.manner());
        let raw = worker.device.clone();
        let adapter = adapter_of(&raw)?;
        let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut made = None;
        // SAFETY: 出力先は有効なローカル変数。映像処理に使うので VIDEO_SUPPORT のフラグを付ける。
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
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
            // SAFETY: デバイスは有効。ランタイムは自分のスレッドからもコンテキストを使う。
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

        let opened = Opened::open()?;
        // SAFETY: 開いた session と有効なデバイス。デバイスは `Frc` が session より長く持つ（フィールドの順）。
        let status = unsafe {
            (opened.entries.set_handle)(opened.session, mfx::HANDLE_D3D11_DEVICE, d3d11.as_raw())
        };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoCORE_SetHandle（D3D11）", status));
        }
        let mut ai = Box::new(mfx::ExtVppAiFrameInterpolation::new(
            mfx::AI_FRAME_INTERPOLATION_MODE_DEFAULT,
        ));
        let mut ext: Box<[*mut mfx::ExtBuffer; 1]> = Box::new([std::ptr::from_mut(&mut ai.header)]);
        let mut asked = param(size, ext.as_mut_ptr());
        let mut answer = asked;
        // SAFETY: 開いた session と有効なローカル変数。拡張バッファは Box で、`Frc` が持つ。
        let status = unsafe { (opened.entries.vpp_query)(opened.session, &mut asked, &mut answer) };
        if status < mfx::ERR_NONE {
            return Err(failed(
                "MFXVideoVPP_Query（この GPU やドライバーに AI のフレーム補間が無い）",
                status,
            ));
        }
        // SAFETY: 同上。
        let status = unsafe { (opened.entries.vpp_init)(opened.session, &mut asked) };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoVPP_Init", status));
        }

        let padded = [size[0].next_multiple_of(2), size[1].next_multiple_of(2)];
        let input = Self::shared(device, &worker, &device1, padded, size, true, "zw vpl in")?;
        let out = Self::shared(device, &worker, &device1, padded, size, false, "zw vpl out")?;
        let fences = |raw: &ID3D12Device| -> Result<(Fence, ID3D11Fence), String> {
            let fence = Fence::new(raw, true)?;
            let opened = d3d12::d3d11_fence(raw, &fence.fence, &device5)?;
            Ok((fence, opened))
        };
        let to_vpl = fences(&raw)?;
        let from_vpl = fences(&raw)?;
        Ok(Self {
            worker,
            opened,
            _ai: ai,
            _ext: ext,
            _device: d3d11,
            immediate,
            size,
            padded,
            input,
            out,
            to_vpl,
            from_vpl,
            frames: 0,
            has_mid: false,
            outside: 0.0,
        })
    }

    /// `write` なら wgpu が書き込む側（入力）、でなければ読む側（出力）。
    fn shared(
        device: &wgpu::Device,
        worker: &Worker,
        device1: &ID3D11Device1,
        padded: [u32; 2],
        size: [u32; 2],
        write: bool,
        label: &str,
    ) -> Result<Shared, String> {
        let usage = if write {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        } else {
            wgpu::TextureUsages::TEXTURE_BINDING
        };
        let lane = worker.lane(
            device,
            padded,
            DXGI_FORMAT_NV12,
            &[
                (wgpu::TextureFormat::R8Unorm, size),
                (
                    wgpu::TextureFormat::Rg8Unorm,
                    [size[0].div_ceil(2), size[1].div_ceil(2)],
                ),
            ],
            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            usage,
            label,
        )?;
        let d3d11 = d3d12::d3d11_texture(&worker.device, &lane.local, device1)?;
        Ok(Shared { lane, d3d11 })
    }

    pub fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        if run.new_input {
            let planes = &self.input.lane.planes;
            convert.nv12_in(gpu, run.cur, &planes[0], &planes[1]);
            self.worker.send(gpu, &[&self.input.lane])?;
            gpu.flush();
            let queue = self.worker.queue(gpu)?;
            let ready = self.to_vpl.0.next();
            // wgpu のサブミットの後にシグナルし、D3D11 はそれを待つ。
            d3d12::signal(&queue, &self.to_vpl.0.fence, ready)?;
            d3d12::d3d11_wait(&self.immediate, &self.to_vpl.1, ready)?;
            let began = std::time::Instant::now();
            let mid = self.interpolate();
            let ms = began.elapsed().as_secs_f64() * 1000.0;
            self.outside = if self.outside > 0.0 {
                self.outside * 0.8 + ms * 0.2
            } else {
                ms
            };
            let done = self.from_vpl.0.next();
            // コピーし終えたらシグナルする。失敗してもシグナルする（しないと wgpu のキューが止まる）。
            d3d12::d3d11_signal(&self.immediate, &self.from_vpl.1, done)?;
            d3d12::wait(&queue, &self.from_vpl.0.fence, done)?;
            self.has_mid = mid?;
            if self.has_mid {
                self.worker.receive(gpu, &[&self.out.lane])?;
            }
        }
        if !self.has_mid || run.phase > 0.75 {
            convert.put(gpu, run.cur, run.target);
        } else if run.phase < 0.25 {
            convert.put(gpu, run.prev, run.target);
        } else {
            let planes = &self.out.lane.planes;
            convert.nv12_out(gpu, &planes[0], &planes[1], run.target);
        }
        Ok(())
    }

    /// 中間フレームが出たか（最初の一フレームでは出ない）。
    fn interpolate(&mut self) -> Result<bool, String> {
        let entries = self.opened.entries;
        let session = self.opened.session;
        let mut input: *mut mfx::FrameSurface1 = std::ptr::null_mut();
        // SAFETY: 開いた session とローカル変数。取得したサーフェスは下で必ず解放する。
        let status = unsafe { (entries.surface_in)(session, &mut input) };
        if status < mfx::ERR_NONE || input.is_null() {
            return Err(failed("MFXMemory_GetSurfaceForVPP", status));
        }
        let result = (|| {
            let full = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: self.padded[0],
                bottom: self.padded[1],
                back: 1,
            };
            // SAFETY: 取得したサーフェスと同じデバイスの共有テクスチャ。イミディエイトコンテキストの順序で、コピーは映像処理より先。
            unsafe {
                let texture = native(input)?;
                self.immediate.CopySubresourceRegion(
                    &texture,
                    0,
                    0,
                    0,
                    0,
                    &self.input.d3d11,
                    0,
                    Some(&full),
                );
                (*input).data.time_stamp = self.frames * TICK;
                (*input).data.frame_order = self.frames as u32;
            }
            self.frames += 1;
            let mut mid = false;
            for _ in 0..2 {
                let mut target: *mut mfx::FrameSurface1 = std::ptr::null_mut();
                // SAFETY: 同上。
                let status = unsafe { (entries.surface_out)(session, &mut target) };
                if status < mfx::ERR_NONE || target.is_null() {
                    return Err(failed("MFXMemory_GetSurfaceForVPPOut", status));
                }
                let mut sync: mfx::SyncPoint = std::ptr::null_mut();
                // SAFETY: 有効な session と取得した二つのサーフェス。
                let status = unsafe {
                    (entries.vpp_run)(session, input, target, std::ptr::null_mut(), &mut sync)
                };
                let again = status == mfx::ERR_MORE_SURFACE;
                let outcome = (|| {
                    if status < mfx::ERR_NONE && !again {
                        return Err(failed("MFXVideoVPP_RunFrameVPPAsync", status));
                    }
                    if again && !sync.is_null() {
                        // SAFETY: ランタイムが返した同期ポイントと、取得したサーフェス。
                        unsafe {
                            let status = (entries.sync_operation)(session, sync, WAIT_MS);
                            if status < mfx::ERR_NONE {
                                return Err(failed("MFXVideoCORE_SyncOperation", status));
                            }
                            let texture = native(target)?;
                            self.immediate.CopySubresourceRegion(
                                &self.out.d3d11,
                                0,
                                0,
                                0,
                                0,
                                &texture,
                                0,
                                Some(&full),
                            );
                        }
                        mid = true;
                    } else if !sync.is_null() {
                        // SAFETY: 同上。
                        unsafe { (entries.sync_operation)(session, sync, WAIT_MS) };
                    }
                    Ok(())
                })();
                // SAFETY: 取得した出力サーフェスを解放する。
                unsafe { release(target) };
                outcome?;
                if !again {
                    break;
                }
            }
            Ok(mid)
        })();
        // SAFETY: 取得した入力サーフェスを解放する（必要ならランタイムが自分で参照を持つ）。
        unsafe { release(input) };
        result
    }
}

// SAFETY: ローダーと session はこの `Frc` だけが持ち、`&mut self` の一つのスレッドで使う。
unsafe impl Send for Frc {}

impl Drop for Frc {
    fn drop(&mut self) {
        // 最後に依頼したコピーが終わるまで待ってから閉じる。
        if let Err(why) = self.from_vpl.0.wait(self.from_vpl.0.value) {
            tracing::warn!("oneVPL: 終了前の待機でエラー（{why}）");
        }
    }
}
