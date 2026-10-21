// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Intel の Quick Sync：oneVPL か Media SDK のランタイムを直接読み込み、1.x の API で呼ぶ。
//! Windows では検索パスに無いことがあるので `DriverStore` の下も探す。
//! フィールドは同じ session の VPP でデインターレースし、一枚から二枚出す。

#[cfg(all(windows, feature = "d3d12"))]
mod d3d11;
pub(crate) mod mfx;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, OnceLock};

use mediadecoder::es::{Codec, Format, Framing, Nals, Picture, Structure};
use mediadecoder::video::Frame;

use super::order::{Key, Keys};
use super::{About, Decode, Decoded, Error, HwDeinterlace, Offer, Way};

const ABOUTS: usize = 64;

const SEQUENCE: i64 = 1 << 32;

/// VPP の出力を、同期を待たずに何枚先まで進めるか（一フレームの二フィールド分）。
const VPP_AHEAD: usize = 2;

const TIMESTAMP_UNKNOWN: u64 = u64::MAX;

const BUSY_TRIES: u32 = 1000;

fn failed(what: &str, status: mfx::Status) -> Error {
    Error::Unavailable(format!("{what} が {status} を返しました"))
}

struct Library {
    entries: mfx::Entries,
    name: String,
    legacy: bool,
    _object: crate::dl::Object,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Library({})", self.name)
    }
}

fn library() -> Result<Arc<Library>, Error> {
    static OPENED: OnceLock<Result<Arc<Library>, String>> = OnceLock::new();
    OPENED
        .get_or_init(|| open_library().map(Arc::new))
        .clone()
        .map_err(Error::Unavailable)
}

fn open_library() -> Result<Library, String> {
    // 32 ビットのプロセスは 32 ビットのランタイムを読む（名前は libvpl のディスパッチャーと同じ）。
    let wide = cfg!(target_pointer_width = "64");
    let (modern, legacy): (&[&str], &[&str]) = match (cfg!(windows), wide) {
        (true, true) => (&["libmfx64-gen.dll"], &["libmfxhw64.dll"]),
        (true, false) => (&["libmfx32-gen.dll"], &["libmfxhw32.dll"]),
        (false, true) => (
            &["libmfx-gen.so.1.2", "libmfx-gen.so.1"],
            &["libmfxhw64.so.1", "libmfx.so.1"],
        ),
        (false, false) => (
            &["libmfx-gen.so.1.2", "libmfx-gen.so.1"],
            &["libmfxhw32.so.1", "libmfx.so.1"],
        ),
    };
    let mut tried = Vec::new();
    for (names, old) in [(modern, false), (legacy, true)] {
        let candidates: Vec<String> = names
            .iter()
            .map(|name| (*name).to_owned())
            .chain(names.iter().flat_map(|name| driver_store(name)))
            .collect();
        for name in candidates {
            let Ok(object) = crate::dl::Object::open(&name) else {
                tried.push(name);
                continue;
            };
            // SAFETY: シンボルの型はヘッダーの宣言のとおり（`mfx::Entries`）。
            match unsafe { mfx::Entries::of(&object) } {
                Ok(entries) if entries.init_ex.is_some() || entries.initialize.is_some() => {
                    return Ok(Library {
                        entries,
                        name,
                        legacy: old,
                        _object: object,
                    });
                }
                Ok(_) => tried.push(format!("{name}（初期化のエントリーポイントが無い）")),
                Err(missing) => tried.push(format!("{name}（{missing}）")),
            }
        }
    }
    let install = if cfg!(windows) {
        "Intel の GPU のドライバーに入っています"
    } else {
        "第 6〜10 世代は libmfx1、第 11 世代以降は libmfx-gen1.2 のパッケージ"
    };
    Err(format!(
        "Quick Sync のランタイムがありません（{install}。試した物：{}）",
        tried.join("、")
    ))
}

#[cfg(windows)]
fn driver_store(name: &str) -> Vec<String> {
    let Some(root) = std::env::var_os("SystemRoot") else {
        return Vec::new();
    };
    let repository = std::path::Path::new(&root).join(r"System32\DriverStore\FileRepository");
    let Ok(entries) = std::fs::read_dir(repository) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_file())
        .filter_map(|path| path.to_str().map(str::to_owned))
        .collect()
}

#[cfg(not(windows))]
fn driver_store(_name: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
struct Display {
    raw: *mut std::ffi::c_void,
    terminate: unsafe extern "C" fn(*mut std::ffi::c_void) -> i32,
    _node: std::fs::File,
    _va: crate::dl::Object,
    _drm: crate::dl::Object,
}

#[cfg(target_os = "linux")]
impl Display {
    fn open(node: &str) -> Result<Self, Error> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .map_err(|error| Error::Unavailable(format!("{node} を開けません: {error}")))?;
        let unavailable = |missing: crate::dl::Missing| Error::Unavailable(missing.0);
        let va = crate::dl::Object::open_any(&["libva.so.2", "libva.so"]).map_err(unavailable)?;
        let drm = crate::dl::Object::open_any(&["libva-drm.so.2", "libva-drm.so"])
            .map_err(unavailable)?;
        type GetDisplay = unsafe extern "C" fn(i32) -> *mut std::ffi::c_void;
        type Initialize = unsafe extern "C" fn(*mut std::ffi::c_void, *mut i32, *mut i32) -> i32;
        type Terminate = unsafe extern "C" fn(*mut std::ffi::c_void) -> i32;
        // SAFETY: シンボルの型は `va.h` と `va_drm.h` の宣言のとおり。
        let (get_display, initialize, terminate): (GetDisplay, Initialize, Terminate) = unsafe {
            (
                drm.symbol(b"vaGetDisplayDRM\0").map_err(unavailable)?,
                va.symbol(b"vaInitialize\0").map_err(unavailable)?,
                va.symbol(b"vaTerminate\0").map_err(unavailable)?,
            )
        };
        // SAFETY: 生きたレンダーノードのファイル記述子で、display より長生きする。
        let raw = unsafe { get_display(file.as_raw_fd()) };
        if raw.is_null() {
            return Err(Error::Unavailable(format!(
                "{node} で VA-API の display が作れません"
            )));
        }
        let (mut major, mut minor) = (0, 0);
        // SAFETY: 作ったばかりの display と生きたローカル変数。
        let status = unsafe { initialize(raw, &mut major, &mut minor) };
        if status != 0 {
            // SAFETY: `vaGetDisplayDRM` が作った display で、失敗しても一度だけ解放する。
            unsafe { terminate(raw) };
            return Err(Error::Unavailable(format!(
                "vaInitialize が {status} を返しました"
            )));
        }
        Ok(Self {
            raw,
            terminate,
            _node: file,
            _va: va,
            _drm: drm,
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: `open` で初期化した display を一度だけ終える（session は先に閉じてある）。
        unsafe { (self.terminate)(self.raw) };
    }
}

struct Session {
    raw: mfx::Session,
    library: Arc<Library>,
    hevc_loaded: std::cell::Cell<bool>,
    /// GPU を指定する display。session より後に drop する（宣言の順序）。
    #[cfg(target_os = "linux")]
    _display: Option<Display>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Session({})", self.library.name)
    }
}

// SAFETY: session は一度に一つのスレッドからしか触らず、ランタイムはスレッドに依存しない。
unsafe impl Send for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: `open` が作った session を一度だけ閉じる。
        unsafe { (self.library.entries.close)(self.raw) };
    }
}

impl Session {
    fn open(device: &str) -> Result<Self, Error> {
        let library = library()?;
        let entries = library.entries;
        #[cfg(windows)]
        let (implementation, index) = windows_implementation(device);
        #[cfg(target_os = "linux")]
        let (implementation, index) = (mfx::IMPL_HARDWARE_ANY | mfx::IMPL_VIA_VAAPI, 0u32);
        let mut raw: mfx::Session = std::ptr::null_mut();
        let status = match (entries.init_ex, entries.initialize) {
            // SAFETY: 引数は値渡しの構造体と生きたローカル変数。
            (Some(init_ex), _) => unsafe { init_ex(mfx::InitParam::new(implementation), &mut raw) },
            (None, Some(initialize)) => {
                let mode = if cfg!(windows) {
                    mfx::ACCEL_MODE_VIA_D3D11
                } else {
                    mfx::ACCEL_MODE_VIA_VAAPI
                };
                // SAFETY: 同上。
                unsafe { initialize(mfx::InitializationParam::new(mode, index), &mut raw) }
            }
            (None, None) => {
                return Err(Error::Unavailable(String::from(
                    "初期化のエントリーポイントが無い",
                )));
            }
        };
        if status < mfx::ERR_NONE || raw.is_null() {
            return Err(failed(&format!("{} の初期化", library.name), status));
        }
        let session = Self {
            raw,
            library,
            hevc_loaded: std::cell::Cell::new(false),
            #[cfg(target_os = "linux")]
            _display: None,
        };
        #[cfg(target_os = "linux")]
        let session = session.with_display(device)?;
        #[cfg(not(target_os = "linux"))]
        let _ = device;
        let _ = index;
        Ok(session)
    }

    #[cfg(target_os = "linux")]
    fn with_display(mut self, device: &str) -> Result<Self, Error> {
        let node = if device.is_empty() {
            intel_nodes().into_iter().next().ok_or_else(|| {
                Error::Unavailable(String::from("Intel のレンダーノードがありません"))
            })?
        } else {
            node_named(device)
                .ok_or_else(|| Error::Unavailable(format!("{device} が見つかりません")))?
        };
        let display = Display::open(&node)?;
        // SAFETY: 生きた session と display。display は session より長生きする。
        let status = unsafe {
            (self.library.entries.set_handle)(self.raw, mfx::HANDLE_VA_DISPLAY, display.raw)
        };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoCORE_SetHandle", status));
        }
        self._display = Some(display);
        Ok(self)
    }

    fn entries(&self) -> mfx::Entries {
        self.library.entries
    }

    fn load_hevc(&self, codec: Codec) {
        if codec != Codec::Hevc || !self.library.legacy || self.hevc_loaded.get() {
            return;
        }
        if let Some(load) = self.entries().user_load {
            // SAFETY: 生きた session と、関数より長生きする定数。
            let status = unsafe { load(self.raw, &mfx::PLUGIN_HEVC_DECODE_HW, 1) };
            self.hevc_loaded.set(status >= mfx::ERR_NONE);
        }
    }

    fn decodes(&self, codec: Codec) -> bool {
        self.load_hevc(codec);
        let mut asked = mfx::VideoParam::default();
        asked.io_pattern = mfx::IOPATTERN_OUT_SYSTEM_MEMORY;
        {
            let info = asked.mfx_mut();
            info.codec_id = codec_id(codec);
            info.frame_info.fourcc = mfx::FOURCC_NV12;
            info.frame_info.chroma_format = mfx::CHROMAFORMAT_YUV420;
            info.frame_info.width = 1920;
            info.frame_info.height = 1088;
            info.frame_info.crop_w = 1920;
            info.frame_info.crop_h = 1080;
            info.frame_info.pic_struct = mfx::PICSTRUCT_PROGRESSIVE;
        }
        let mut answer = asked;
        // SAFETY: 生きた session と二つの生きたローカル変数。
        let status = unsafe { (self.entries().decode_query)(self.raw, &mut asked, &mut answer) };
        status >= mfx::ERR_NONE
    }
}

#[cfg(windows)]
fn windows_implementation(device: &str) -> (i32, u32) {
    let any = (mfx::IMPL_HARDWARE_ANY | mfx::IMPL_VIA_D3D11, 0);
    if device.is_empty() {
        return any;
    }
    #[cfg(feature = "d3d12")]
    {
        let index = crate::decode::d3d12::dx::adapters()
            .iter()
            .position(|adapter| adapter.name == device);
        let hardware = [
            mfx::IMPL_HARDWARE,
            mfx::IMPL_HARDWARE2,
            mfx::IMPL_HARDWARE3,
            mfx::IMPL_HARDWARE4,
        ];
        if let Some(index) = index.filter(|index| *index < hardware.len()) {
            return (hardware[index] | mfx::IMPL_VIA_D3D11, index as u32);
        }
    }
    any
}

#[cfg(target_os = "linux")]
fn intel_nodes() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut nodes: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.starts_with("renderD").then_some(())?;
            let vendor = std::fs::read_to_string(entry.path().join("device/vendor")).ok()?;
            (vendor.trim() == "0x8086").then(|| format!("/dev/dri/{name}"))
        })
        .collect();
    nodes.sort();
    nodes
}

fn codec_id(codec: Codec) -> u32 {
    match codec {
        Codec::Mpeg2 => mfx::CODEC_MPEG2,
        Codec::H264 => mfx::CODEC_AVC,
        Codec::Hevc => mfx::CODEC_HEVC,
    }
}

pub fn offers(codec: Codec) -> Vec<Offer> {
    if library().is_err() {
        return Vec::new();
    }
    devices()
        .into_iter()
        .filter(|device| Session::open(device).is_ok_and(|session| session.decodes(codec)))
        .map(|device| Offer {
            device,
            ways: vec![Way::Qsv],
            gpu: true,
            draws: false,
        })
        .collect()
}

#[cfg(all(windows, feature = "d3d12"))]
fn devices() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for adapter in crate::decode::d3d12::dx::adapters() {
        if adapter.vendor == 0x8086 && !names.contains(&adapter.name) {
            names.push(adapter.name);
        }
    }
    names
}

#[cfg(all(windows, not(feature = "d3d12")))]
fn devices() -> Vec<String> {
    vec![String::new()]
}

#[cfg(target_os = "linux")]
fn devices() -> Vec<String> {
    intel_nodes().iter().map(|node| name_of(node)).collect()
}

#[cfg(target_os = "linux")]
fn name_of(node: &str) -> String {
    #[cfg(feature = "vaapi")]
    if let Some((_, name)) = crate::decode::vaapi::render_nodes()
        .into_iter()
        .find(|(seen, _)| seen == node)
    {
        return name;
    }
    node.to_owned()
}

#[cfg(target_os = "linux")]
fn node_named(device: &str) -> Option<String> {
    intel_nodes()
        .into_iter()
        .find(|node| node == device || name_of(node) == device)
}

const PENDING_MOST: usize = 32 << 20;

/// ランタイムが返すまで保持しているので、サーフェスの配列は作ったときの数から増やさない。
#[derive(Debug)]
struct Surface {
    surface: mfx::FrameSurface1,
    _memory: Vec<u8>,
}

impl Surface {
    /// ビデオメモリのサーフェス（中身はアロケーターが持つ）。
    #[cfg(all(windows, feature = "d3d12"))]
    fn video(info: mfx::FrameInfo, mem_id: *mut std::ffi::c_void) -> Self {
        let mut surface = mfx::FrameSurface1::default();
        surface.info = info;
        surface.data.mem_id = mem_id;
        Self {
            surface,
            _memory: Vec::new(),
        }
    }

    fn new(info: mfx::FrameInfo) -> Self {
        let wide = info.fourcc == mfx::FOURCC_P010;
        let width = usize::from(info.width);
        let height = usize::from(info.height);
        let pitch = (width * if wide { 2 } else { 1 }).next_multiple_of(64);
        let mut memory = vec![0u8; pitch * height + pitch * height.div_ceil(2) + 64];
        let mut surface = mfx::FrameSurface1::default();
        surface.info = info;
        surface.data.pitch_high = (pitch >> 16) as u16;
        surface.data.pitch_low = (pitch & 0xffff) as u16;
        surface.data.y = memory.as_mut_ptr();
        // 輝度の行の後ろに色差の行（確保した領域に収まる）。
        surface.data.uv = memory.as_mut_ptr().wrapping_add(pitch * height);
        Self {
            surface,
            _memory: memory,
        }
    }

    fn free(&self) -> bool {
        self.surface.data.locked == 0
    }
}

#[derive(Debug)]
struct Stream {
    surfaces: Vec<Surface>,
    vpp: Option<Vpp>,
}

#[derive(Debug)]
struct Vpp {
    outputs: Vec<Surface>,
    _deinterlacing: Box<mfx::ExtVppDeinterlacing>,
    _ext: Box<[*mut mfx::ExtBuffer; 1]>,
}

pub struct Qsv {
    session: Session,
    codec: Codec,
    framing: Framing,
    device: String,
    deinterlace: Option<HwDeinterlace>,
    pending: Vec<u8>,
    /// フィールド対の一枚目。一枚ずつ渡すと半分のフレームとして出るので二枚目まで待つ。
    half: Option<(i64, mediadecoder::es::Structure)>,
    stream: Option<Stream>,
    keys: Keys,
    base: i64,
    format: Format,
    abouts: BTreeMap<i64, About>,
    ready: VecDeque<Decoded>,
    said: bool,
    /// 二枚目のフィールドはタイムスタンプを「不明」で返すので、最後に渡されたタイムスタンプで付け直す。
    vpp_time: u64,
    /// 同期を待たずに先へ進めた VPP の出力（古い順）。デコードと VPP を重ねて流すため、[`VPP_AHEAD`] 枚まで持つ。
    /// 三つ目は投げたときの [`Qsv::vpp_time`]（二枚目のフィールドに付け直す時刻）。
    vpp_pending: VecDeque<(mfx::SyncPoint, *mut mfx::FrameSurface1, u64)>,
    /// サーフェスを GPU のメモリに置いて D3D12 のレンダラーへ渡す（`None` なら主記憶）。session より後に drop する。
    #[cfg(all(windows, feature = "d3d12"))]
    gpu: Option<d3d11::Gpu>,
}

impl std::fmt::Debug for Qsv {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Qsv({:?} {:?})", self.codec, self.session)
    }
}

impl Qsv {
    pub fn open(
        codec: Codec,
        framing: Framing,
        device: &str,
        deinterlace: Option<HwDeinterlace>,
    ) -> Result<Self, Error> {
        if deinterlace == Some(HwDeinterlace::MotionCompensated) {
            return Err(Error::Unavailable(String::from(
                "QSV の動き補償のデインターレースはまだ使いません",
            )));
        }
        // GPU を指定していなければ、レンダリングする GPU で開く（GPU のメモリで渡せる）。
        #[cfg(all(windows, feature = "d3d12"))]
        let drawing = crate::decode::d3d12::dx::adapters()
            .into_iter()
            .map(|adapter| adapter.name)
            .find(|name| crate::decode::d3d12::dx::draws_on(name));
        #[cfg(all(windows, feature = "d3d12"))]
        let (session, on_drawing) = match drawing.filter(|_| device.is_empty()) {
            Some(name) => match Session::open(&name) {
                Ok(session) => (session, true),
                Err(_) => (Session::open(device)?, false),
            },
            None => (
                Session::open(device)?,
                !device.is_empty() && crate::decode::d3d12::dx::draws_on(device),
            ),
        };
        #[cfg(not(all(windows, feature = "d3d12")))]
        let session = Session::open(device)?;
        if !session.decodes(codec) {
            return Err(Error::Unavailable(format!(
                "{} は {codec:?} をデコードできません",
                session.library.name
            )));
        }
        // 別の GPU でデコードするときは主記憶で渡す。
        #[cfg(all(windows, feature = "d3d12"))]
        let gpu = match on_drawing
            .then_some(())
            .ok_or_else(|| {
                Error::Unavailable(String::from("レンダリングと別の GPU でデコードする"))
            })
            .and_then(|()| d3d11::Gpu::open())
            .and_then(|mut gpu| gpu.attach(&session.entries(), session.raw).map(|()| gpu))
        {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                tracing::info!("QSV: GPU のメモリで受け渡せないので主記憶を使う: {error}");
                None
            }
        };
        Ok(Self {
            session,
            codec,
            framing,
            device: device.to_owned(),
            deinterlace,
            pending: Vec::new(),
            half: None,
            stream: None,
            keys: match codec {
                Codec::Mpeg2 => Keys::Mpeg2,
                Codec::H264 => Keys::H264(Box::default()),
                Codec::Hevc => Keys::Hevc(Box::default()),
            },
            base: 0,
            format: Format::default(),
            abouts: BTreeMap::new(),
            ready: VecDeque::new(),
            said: false,
            vpp_time: TIMESTAMP_UNKNOWN,
            vpp_pending: VecDeque::new(),
            #[cfg(all(windows, feature = "d3d12"))]
            gpu,
        })
    }

    fn time(&mut self, tag: u64, picture: &Picture, data: &[u8]) -> i64 {
        if matches!(self.keys, Keys::Mpeg2) {
            return tag as i64;
        }
        let Key {
            order,
            drain_before,
            ..
        } = self.keys.key(picture, Framing::StartCodes, data);
        if drain_before {
            self.base += SEQUENCE;
        }
        self.base + i64::from(order)
    }

    fn configure(&mut self, bitstream: &mut mfx::Bitstream) -> Result<bool, Error> {
        let entries = self.session.entries();
        self.session.load_hevc(self.codec);
        let mut param = mfx::VideoParam::default();
        param.mfx_mut().codec_id = codec_id(self.codec);
        // SAFETY: 生きた session と生きたローカル変数。
        let status = unsafe { (entries.decode_header)(self.session.raw, bitstream, &mut param) };
        if status == mfx::ERR_MORE_DATA {
            return Ok(false);
        }
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoDECODE_DecodeHeader", status));
        }
        let wide = self.format.depth > 8;
        param.io_pattern = if self.video_memory() {
            mfx::IOPATTERN_OUT_VIDEO_MEMORY
        } else {
            mfx::IOPATTERN_OUT_SYSTEM_MEMORY
        };
        param.async_depth = 1;
        {
            let info = &mut param.mfx_mut().frame_info;
            info.fourcc = if wide {
                mfx::FOURCC_P010
            } else {
                mfx::FOURCC_NV12
            };
            info.chroma_format = mfx::CHROMAFORMAT_YUV420;
            info.shift = u16::from(wide);
        }
        let mut request = mfx::FrameAllocRequest::default();
        // SAFETY: 生きた session と生きたローカル変数。
        let status =
            unsafe { (entries.decode_query_io_surf)(self.session.raw, &mut param, &mut request) };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoDECODE_QueryIOSurf", status));
        }
        let info = param.mfx().frame_info;
        if !mediadecoder::video::fits(info.width.into(), info.height.into()) {
            return Err(Error::Unsupported("a picture larger than any level allows"));
        }
        let mut count = usize::from(request.num_frame_suggested.max(request.num_frame_min)).max(4);
        let vpp = match self.deinterlace {
            Some(mode) => {
                let (vpp, inputs) = self.configure_vpp(info, mode)?;
                count += inputs;
                Some(vpp)
            }
            None => None,
        };
        let surfaces = self.surfaces(&mut request, info, count, vpp.is_some())?;
        // SAFETY: 生きた session と生きたローカル変数。
        let status = unsafe { (entries.decode_init)(self.session.raw, &mut param) };
        if status < mfx::ERR_NONE {
            // 作った VPP は閉じてからエラーを返す（開いたままだと次の `VPP_Init` が通らない）。
            if vpp.is_some() {
                // SAFETY: 生きた session で、VPP はすぐ上で初期化した。
                unsafe { (entries.vpp_close)(self.session.raw) };
            }
            return Err(failed("MFXVideoDECODE_Init", status));
        }
        self.stream = Some(Stream { surfaces, vpp });
        Ok(true)
    }

    fn configure_vpp(
        &mut self,
        decoded: mfx::FrameInfo,
        mode: HwDeinterlace,
    ) -> Result<(Vpp, usize), Error> {
        let entries = self.session.entries();
        let mut deinterlacing = Box::new(mfx::ExtVppDeinterlacing::new(match mode {
            HwDeinterlace::Bob => mfx::DEINTERLACING_BOB,
            _ => mfx::DEINTERLACING_ADVANCED,
        }));
        let mut ext: Box<[*mut mfx::ExtBuffer; 1]> =
            Box::new([std::ptr::from_mut(&mut deinterlacing.header)]);
        let mut param = mfx::VideoParam::default();
        param.io_pattern = if self.video_memory() {
            mfx::IOPATTERN_IN_VIDEO_MEMORY | mfx::IOPATTERN_OUT_VIDEO_MEMORY
        } else {
            mfx::IOPATTERN_IN_SYSTEM_MEMORY | mfx::IOPATTERN_OUT_SYSTEM_MEMORY
        };
        // 同期を待たずに次を投げるので、その分だけ先まで受け付けさせる。
        param.async_depth = VPP_AHEAD as u16 + 1;
        param.ext_param = ext.as_mut_ptr();
        param.num_ext_param = 1;
        {
            let vpp = param.vpp_mut();
            vpp.input = decoded;
            // フィールドの順序は一枚ごとにサーフェスが示す（フィールドとフレームが混ざるストリームがある）。
            vpp.input.pic_struct = mfx::PICSTRUCT_UNKNOWN;
            vpp.output = decoded;
            vpp.output.pic_struct = mfx::PICSTRUCT_PROGRESSIVE;
            let (num, den) = (decoded.frame_rate_ext_n, decoded.frame_rate_ext_d);
            let (num, den) = if num == 0 || den == 0 {
                (self.format.rate.0.max(30000), self.format.rate.1.max(1001))
            } else {
                (num, den)
            };
            vpp.input.frame_rate_ext_n = num;
            vpp.input.frame_rate_ext_d = den;
            vpp.output.frame_rate_ext_n = num * 2;
            vpp.output.frame_rate_ext_d = den;
        }
        let mut requests = [mfx::FrameAllocRequest::default(); 2];
        // SAFETY: 生きた session と、二つ並んだ生きたローカル変数。
        let status = unsafe {
            (entries.vpp_query_io_surf)(self.session.raw, &mut param, requests.as_mut_ptr())
        };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoVPP_QueryIOSurf", status));
        }
        // SAFETY: 生きた session とローカル変数。拡張バッファは `Vpp` が持ち、VPP より長生きする。
        let status = unsafe { (entries.vpp_init)(self.session.raw, &mut param) };
        if status < mfx::ERR_NONE {
            return Err(failed("MFXVideoVPP_Init", status));
        }
        let output = param.vpp_mut().output;
        // GPU のメモリでは出力をそのままレンダラーへ渡し、持たれている間は出力先にしないので、その分を足す。
        let held = if self.video_memory() {
            super::MOST_HELD
        } else {
            0
        };
        let outputs = usize::from(
            requests[1]
                .num_frame_suggested
                .max(requests[1].num_frame_min),
        )
        .max(2)
            + held;
        let outputs = match self.surfaces(&mut requests[1], output, outputs, false) {
            Ok(outputs) => outputs,
            Err(error) => {
                // SAFETY: 生きた session で、VPP はすぐ上で初期化した。
                unsafe { (entries.vpp_close)(self.session.raw) };
                return Err(error);
            }
        };
        Ok((
            Vpp {
                outputs,
                _deinterlacing: deinterlacing,
                _ext: ext,
            },
            usize::from(
                requests[0]
                    .num_frame_suggested
                    .max(requests[0].num_frame_min),
            ),
        ))
    }

    fn video_memory(&self) -> bool {
        #[cfg(all(windows, feature = "d3d12"))]
        return self.gpu.is_some();
        #[cfg(not(all(windows, feature = "d3d12")))]
        false
    }

    /// `count` 枚のサーフェス。GPU のメモリならアロケーターで作る（`vpp_input` なら VPP の入力にも使う）。
    #[cfg_attr(not(all(windows, feature = "d3d12")), allow(unused_variables))]
    fn surfaces(
        &mut self,
        request: &mut mfx::FrameAllocRequest,
        info: mfx::FrameInfo,
        count: usize,
        vpp_input: bool,
    ) -> Result<Vec<Surface>, Error> {
        #[cfg(all(windows, feature = "d3d12"))]
        if let Some(gpu) = self.gpu.as_mut() {
            request.info = info;
            request.num_frame_suggested = count as u16;
            request.num_frame_min = count as u16;
            request.kind |= mfx::MEMTYPE_EXTERNAL_FRAME;
            if vpp_input {
                request.kind |= mfx::MEMTYPE_FROM_VPPIN;
            }
            return Ok(gpu
                .surfaces(request)?
                .into_iter()
                .map(|mid| Surface::video(info, mid))
                .collect());
        }
        Ok((0..count).map(|_| Surface::new(info)).collect())
    }

    fn run(&mut self, time: Option<i64>, end: bool) -> Result<(), Error> {
        let entries = self.session.entries();
        let mut bitstream = mfx::Bitstream::default();
        bitstream.data = self.pending.as_mut_ptr();
        bitstream.data_offset = 0;
        bitstream.data_length = self.pending.len() as u32;
        bitstream.max_length = self.pending.len() as u32;
        bitstream.time_stamp = time.unwrap_or(-1) as u64;
        bitstream.data_flag = mfx::BITSTREAM_COMPLETE_FRAME;
        if self.stream.is_none() && !end && !self.configure(&mut bitstream)? {
            return Ok(());
        }
        let mut busy = 0;
        loop {
            let Some(stream) = self.stream.as_mut() else {
                return Ok(());
            };
            let Some(work) = stream
                .surfaces
                .iter_mut()
                .find(|surface| surface.free())
                .map(|surface| std::ptr::from_mut(&mut surface.surface))
            else {
                return Err(Error::Unavailable(String::from(
                    "空いたサーフェスがありません",
                )));
            };
            let mut out: *mut mfx::FrameSurface1 = std::ptr::null_mut();
            let mut sync: mfx::SyncPoint = std::ptr::null_mut();
            let input = if end {
                std::ptr::null_mut()
            } else {
                std::ptr::from_mut(&mut bitstream)
            };
            // SAFETY: ビットストリームのバッファとサーフェスは呼び出しの間・返すまで動かない。
            let status = unsafe {
                (entries.decode_frame_async)(self.session.raw, input, work, &mut out, &mut sync)
            };
            match status {
                mfx::ERR_MORE_SURFACE => continue,
                mfx::ERR_MORE_DATA => break,
                mfx::WRN_DEVICE_BUSY if busy < BUSY_TRIES => {
                    busy += 1;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                mfx::ERR_INCOMPATIBLE_VIDEO_PARAM => {
                    self.close_stream();
                    if end || !self.configure(&mut bitstream)? {
                        break;
                    }
                    continue;
                }
                status if status < mfx::ERR_NONE => {
                    return Err(failed("MFXVideoDECODE_DecodeFrameAsync", status));
                }
                _ => {}
            }
            if sync.is_null() || out.is_null() {
                if status == mfx::WRN_VIDEO_PARAM_CHANGED {
                    continue;
                }
                if end {
                    continue;
                }
                break;
            }
            // SAFETY: ランタイムが返した同期ポイント。
            let status = unsafe { (entries.sync_operation)(self.session.raw, sync, 60_000) };
            if status < mfx::ERR_NONE {
                return Err(failed("MFXVideoCORE_SyncOperation", status));
            }
            self.receive(out)?;
        }
        let used = (bitstream.data_offset as usize).min(self.pending.len());
        self.pending.drain(..used);
        if end {
            self.drain_vpp()?;
        }
        Ok(())
    }

    fn receive(&mut self, out: *mut mfx::FrameSurface1) -> Result<(), Error> {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.vpp.is_some())
        {
            return self.deinterlace_frame(out);
        }
        // SAFETY: 同期を済ませた、サーフェスの配列の中のサーフェス。
        let surface = unsafe { &*out };
        self.read(surface, false);
        Ok(())
    }

    fn deinterlace_frame(&mut self, input: *mut mfx::FrameSurface1) -> Result<(), Error> {
        let entries = self.session.entries();
        if !input.is_null() {
            // SAFETY: 同期を済ませた、サーフェスの配列の中のサーフェス。
            let time = unsafe { (*input).data.time_stamp };
            if time != TIMESTAMP_UNKNOWN {
                self.vpp_time = time;
            }
        }
        loop {
            // まだ受け取っていない出力は、ランタイムがロックを外していても出力先にしない。
            let waiting: Vec<*mut mfx::FrameSurface1> = self
                .vpp_pending
                .iter()
                .map(|(_, target, _)| *target)
                .collect();
            let target = self
                .stream
                .as_mut()
                .and_then(|stream| stream.vpp.as_mut())
                .and_then(|vpp| {
                    vpp.outputs.iter_mut().find(|surface| {
                        if waiting.contains(&std::ptr::from_ref(&surface.surface).cast_mut()) {
                            return false;
                        }
                        #[cfg(all(windows, feature = "d3d12"))]
                        if self
                            .gpu
                            .as_ref()
                            .is_some_and(|gpu| gpu.busy(surface.surface.data.mem_id))
                        {
                            return false;
                        }
                        surface.free()
                    })
                })
                .map(|surface| std::ptr::from_mut(&mut surface.surface));
            let Some(target) = target else {
                return Err(Error::Unavailable(String::from(
                    "VPP の空いたサーフェスがありません",
                )));
            };
            let mut sync: mfx::SyncPoint = std::ptr::null_mut();
            // SAFETY: 生きた session、サーフェスの配列の中の二つのサーフェス、生きたローカル変数。
            let status = unsafe {
                (entries.vpp_run_frame_async)(
                    self.session.raw,
                    input,
                    target,
                    std::ptr::null_mut(),
                    &mut sync,
                )
            };
            let again = status == mfx::ERR_MORE_SURFACE;
            if status == mfx::ERR_MORE_DATA {
                return Ok(());
            }
            if status < mfx::ERR_NONE && !again {
                return Err(failed("MFXVideoVPP_RunFrameVPPAsync", status));
            }
            if !sync.is_null() {
                self.vpp_pending.push_back((sync, target, self.vpp_time));
                self.settle_vpp(VPP_AHEAD)?;
            }
            if !again {
                return Ok(());
            }
        }
    }

    fn drain_vpp(&mut self) -> Result<(), Error> {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.vpp.is_some())
        {
            self.deinterlace_frame(std::ptr::null_mut())?;
            self.settle_vpp(0)?;
        }
        Ok(())
    }

    /// 先へ進めた VPP の出力を、`keep` 枚を残して古い順に同期を待って受け取る。
    fn settle_vpp(&mut self, keep: usize) -> Result<(), Error> {
        let entries = self.session.entries();
        let latest = self.vpp_time;
        while self.vpp_pending.len() > keep {
            let Some((sync, target, time)) = self.vpp_pending.pop_front() else {
                break;
            };
            // SAFETY: ランタイムが返した同期ポイント。
            let status = unsafe { (entries.sync_operation)(self.session.raw, sync, 60_000) };
            if status < mfx::ERR_NONE {
                return Err(failed("MFXVideoCORE_SyncOperation", status));
            }
            // SAFETY: 同期を済ませた、VPP の出力の配列の中のサーフェス（ストリームを閉じるまで動かない）。
            let surface = unsafe { &*target };
            self.vpp_time = time;
            self.read(surface, true);
        }
        self.vpp_time = latest;
        Ok(())
    }

    fn read(&mut self, surface: &mfx::FrameSurface1, woven: bool) {
        let data = surface.data;
        let stamped = if woven && data.time_stamp == TIMESTAMP_UNKNOWN {
            self.vpp_time
        } else {
            data.time_stamp
        };
        if woven && stamped != TIMESTAMP_UNKNOWN {
            self.vpp_time = stamped;
        }
        let time = stamped as i64;
        let pitch = (usize::from(data.pitch_high) << 16) | usize::from(data.pitch_low);
        let info = surface.info;
        let wide = info.fourcc == mfx::FOURCC_P010;
        // 二枚目のフィールドは同じタイムスタンプで来る。付随情報は一枚目で消さず、上限で捨てる。
        let found = self.abouts.get(&time).copied();
        let mut about = found.unwrap_or(About {
            tag: time as u64,
            format: self.format,
            structure: Structure::Frame,
            top_field_first: info.pic_struct & mfx::PICSTRUCT_FIELD_BFF == 0,
            repeat_first_field: false,
            progressive: info.pic_struct & mfx::PICSTRUCT_PROGRESSIVE != 0,
        });
        if woven {
            about = About {
                structure: Structure::Frame,
                progressive: true,
                repeat_first_field: false,
                ..about
            };
        } else if about.structure.field() {
            about = About {
                structure: Structure::Frame,
                top_field_first: about.structure == Structure::Top,
                progressive: false,
                ..about
            };
        }
        if !woven {
            self.abouts.remove(&time);
        }
        #[cfg(all(windows, feature = "d3d12"))]
        if let Some(handed) = self
            .gpu
            .as_mut()
            .map(|gpu| gpu.hand(data.mem_id, wide, about.format.width, about.format.height))
        {
            match handed {
                Ok(Some(frame)) => self.ready.push_back(Decoded::Surface(super::Surface {
                    about,
                    held: super::Held::D3d12(frame),
                })),
                Ok(None) => self.say("サーフェスにデータがない"),
                Err(error) => self.say(&format!("フレームを渡せない: {error:?}")),
            }
            return;
        }
        let width = (about.format.width as usize).min(usize::from(info.width));
        let height = (about.format.height as usize).min(usize::from(info.height));
        let rows = usize::from(info.height);
        if data.y.is_null() || data.uv.is_null() || pitch == 0 {
            self.say("サーフェスにデータがない");
            return;
        }
        // SAFETY: 自分で確保したサーフェスで、輝度 `pitch * rows`、色差 `pitch * rows / 2` ある。
        let (luma, chroma) = unsafe {
            (
                std::slice::from_raw_parts(data.y.cast_const(), pitch * rows),
                std::slice::from_raw_parts(data.uv.cast_const(), pitch * rows.div_ceil(2)),
            )
        };
        let Some(planes) = super::semi_planar((luma, pitch), (chroma, pitch), width, height, wide)
        else {
            self.say("サーフェスがフレームより小さい");
            return;
        };
        self.ready.push_back(Decoded::Planes(Frame {
            tag: about.tag,
            format: about.format,
            planes,
            structure: about.structure,
            top_field_first: about.top_field_first,
            repeat_first_field: about.repeat_first_field,
            progressive: about.progressive,
        }));
    }

    fn say(&mut self, what: &str) {
        if !self.said {
            self.said = true;
            tracing::info!("QSV {:?}: {what}", self.codec);
        }
    }

    fn close_stream(&mut self) {
        let entries = self.session.entries();
        // 先へ進めた VPP の出力は捨てる（サーフェスはストリームと一緒に消える）。
        self.vpp_pending.clear();
        if let Some(stream) = self.stream.take() {
            // SAFETY: 生きた session。閉じてからサーフェスを解放する。
            unsafe {
                if stream.vpp.is_some() {
                    (entries.vpp_close)(self.session.raw);
                }
                (entries.decode_close)(self.session.raw);
            }
            drop(stream);
            #[cfg(all(windows, feature = "d3d12"))]
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.forget();
            }
        }
    }
}

// SAFETY: ポインターは全てこの値かその session の物で、一度に一つのスレッドからしか触らない。
unsafe impl Send for Qsv {}

impl Drop for Qsv {
    fn drop(&mut self) {
        self.close_stream();
    }
}

impl Decode for Qsv {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("only 4:2:0 is broadcast here"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Damaged("a picture of no size"));
        }
        if !mediadecoder::video::fits(format.width.into(), format.height.into()) {
            return Err(Error::Unsupported("a picture larger than any level allows"));
        }
        let fits = self.format.width == format.width
            && self.format.height == format.height
            && self.format.depth == format.depth;
        if self.stream.is_some() && !fits {
            self.close_stream();
        }
        self.format = format;
        let converted;
        let data = if self.codec != Codec::Mpeg2 && self.framing != Framing::StartCodes {
            converted = start_codes(data, self.framing);
            converted.as_slice()
        } else {
            data
        };
        let time = self.time(tag, picture, data);
        let about = About {
            tag,
            format,
            structure: picture.structure,
            top_field_first: picture.top_field_first,
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
        };
        self.abouts.entry(time).or_insert(about);
        while self.abouts.len() > ABOUTS {
            self.abouts.pop_first();
        }
        // 溜まり続けないよう、上限を超えたら捨てて次のフレームから始める。
        if self.pending.len() + data.len() > PENDING_MOST {
            tracing::warn!(
                "QSV: 渡せていないビットストリームが {} バイトに達したので捨てる",
                self.pending.len()
            );
            self.pending.clear();
        }
        self.pending.extend_from_slice(data);
        if picture.structure.field() {
            match self.half.take() {
                Some((first, parity)) if parity != picture.structure => {
                    return self.run(Some(first), false);
                }
                _ => {
                    self.half = Some((time, picture.structure));
                    return Ok(());
                }
            }
        }
        self.half = None;
        self.run(Some(time), false)
    }

    fn take(&mut self) -> Option<Decoded> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        if let Err(error) = self.run(None, true) {
            tracing::warn!("QSV {:?}: {error}", self.codec);
        }
    }

    fn name(&self) -> String {
        let device = if self.device.is_empty() {
            String::from("既定の GPU")
        } else {
            self.device.clone()
        };
        let memory = if self.video_memory() {
            "・GPU メモリ"
        } else {
            ""
        };
        match self.deinterlace {
            Some(mode) => format!(
                "QSV ({device}・{}{memory}・デインターレース {})",
                self.session.library.name,
                mode.name()
            ),
            None => format!("QSV ({device}・{}{memory})", self.session.library.name),
        }
    }

    fn deinterlaces(&self) -> bool {
        self.deinterlace.is_some()
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.deinterlace.is_some() { 2 } else { 1 }
    }

    fn reset(&mut self) {
        self.close_stream();
        self.pending.clear();
        self.half = None;
        self.ready.clear();
        self.abouts.clear();
        self.keys.reset();
        self.base += SEQUENCE;
        self.vpp_time = TIMESTAMP_UNKNOWN;
    }
}

fn start_codes(data: &[u8], framing: Framing) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for nal in Nals::new(data, framing) {
        let Some(unit) = data.get(nal.body.start.saturating_sub(1)..nal.body.end) else {
            continue;
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
    }
    out
}
