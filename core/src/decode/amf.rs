// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! AMD の AMF で UVD / VCN を呼ぶ（Windows）。バインディングは [`api`]、デインターレースは [`vpp`]。
//! 入力のタイムスタンプは表示順に増える値にする（でないとフレームと付随情報がずれる）。

mod api;
mod vpp;

use std::collections::{BTreeMap, VecDeque};

use mediadecoder::es::{Codec, Format, Framing, Nals, Picture, Structure};
use mediadecoder::video::Frame;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
};
use windows::core::Interface;

use super::order::{Key, Keys};
use super::{About, Decode, Decoded, Error, HwDeinterlace, Offer, Readback, Way};

const ABOUTS: usize = 64;

const SEQUENCE: i64 = 1 << 32;

/// 一枚も出さずに何回渡したら諦めるか。フィールドピクチャーの MPEG-2 で AMF はエラーを返さずに何も出さないことがある。
const QUIET: u32 = 30;

const AMD: u32 = 0x1002;

const FULL_PATIENCE: std::time::Duration = std::time::Duration::from_millis(500);

fn say() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("ZW_TRACE").is_ok_and(|said| {
            let said = said.to_ascii_lowercase();
            said == "1" || said == "all" || said.split(',').any(|name| name.trim() == "amf")
        })
    })
}

fn failed(what: &str, status: api::Status) -> Error {
    Error::Unavailable(format!(
        "AMF {what}: {} ({status})",
        api::status_name(status)
    ))
}

/// 読み込んだ `amfrt64.dll` とファクトリー。ファクトリーは解放する関数が無いので、プロセスで一度だけ開く。
struct Runtime {
    _library: crate::dl::Object,
    factory: api::Factory,
    version: u64,
}

fn runtime() -> Result<&'static Runtime, Error> {
    static RUNTIME: std::sync::OnceLock<Result<Runtime, String>> = std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let library = crate::dl::Object::open("amfrt64.dll")
                .map_err(|missing| format!("{missing}（AMD のドライバーが入っていない）"))?;
            // SAFETY: シグネチャーは `core/Factory.h` の `AMFInit_Fn` と `AMFQueryVersion_Fn`。
            let (init, query) = unsafe {
                (
                    library
                        .symbol::<api::Init>(b"AMFInit\0")
                        .map_err(|missing| missing.to_string())?,
                    library
                        .symbol::<api::QueryVersion>(b"AMFQueryVersion\0")
                        .ok(),
                )
            };
            // 要求するバージョンはランタイムとヘッダーの低い方（新しいバージョンを拒むドライバーがある）。
            let mut runtime_version = 0u64;
            if let Some(query) = query {
                // SAFETY: 出力は生きたローカル変数。
                let _ = unsafe { query(&mut runtime_version) };
            }
            let version = if runtime_version == 0 {
                api::HEADER_VERSION
            } else {
                runtime_version.min(api::HEADER_VERSION)
            };
            let mut factory = std::ptr::null_mut();
            // SAFETY: 出力は生きたローカル変数。返るファクトリーはライブラリと一緒にプロセスの終わりまで持つ。
            let (status, factory) = unsafe {
                let status = init(version, &mut factory);
                (status, api::Factory::from_raw(factory))
            };
            let factory = match factory {
                Some(factory) if status == api::AMF_OK => factory,
                _ => return Err(format!("AMFInit: AMF_RESULT {status}")),
            };
            Ok(Runtime {
                _library: library,
                factory,
                version: runtime_version,
            })
        })
        .as_ref()
        .map_err(|why| Error::Unavailable(why.clone()))
}

fn amd_adapters() -> Vec<(String, IDXGIAdapter1)> {
    // SAFETY: ファクトリーは関数の中で作って使い、アダプターは参照を数える。
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut found: Vec<(String, IDXGIAdapter1)> = Vec::new();
    for index in 0..16u32 {
        // SAFETY: 列挙し終えると失敗が返り、そこで止める。
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: 読むだけ。
        let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if DXGI_ADAPTER_FLAG(description.Flags as i32).0 & DXGI_ADAPTER_FLAG_SOFTWARE.0 != 0
            || description.VendorId != AMD
        {
            continue;
        }
        let end = description
            .Description
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(description.Description.len());
        let name = String::from_utf16_lossy(&description.Description[..end]);
        // 同じ GPU が出力の数だけ列挙される（`d3d12::offers`）。
        if found.iter().any(|(seen, _)| *seen == name) {
            continue;
        }
        found.push((name, adapter));
    }
    found
}

fn device_on(adapter: &IDXGIAdapter1) -> Result<ID3D11Device, Error> {
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    // SAFETY: 出力は生きたローカル変数で、書かれるのは一度だけ。
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .map_err(|error| Error::Unavailable(format!("D3D11CreateDevice: {error}")))?;
    let device =
        device.ok_or_else(|| Error::Unavailable(String::from("D3D11 のデバイスがありません")))?;
    // AMF も同じデバイスを使う。イミディエイトコンテキストはスレッドセーフではない。
    if let Ok(threads) = device.cast::<ID3D11Multithread>() {
        // SAFETY: デバイスは生きている。
        unsafe {
            let _ = threads.SetMultithreadProtected(true);
        }
    }
    Ok(device)
}

struct Context {
    // コンテキストはデバイスより先に drop する（宣言の順）。
    raw: api::Context,
    _device: Option<ID3D11Device>,
}

impl Context {
    fn open(device: Option<ID3D11Device>) -> Result<Self, Error> {
        let runtime = runtime()?;
        let raw = runtime
            .factory
            .create_context()
            .map_err(|status| failed("CreateContext", status))?;
        let context = Self {
            raw,
            _device: device,
        };
        let status = context.raw.init_dx11(context._device.as_ref());
        if status != api::AMF_OK {
            return Err(failed("InitDX11", status));
        }
        Ok(context)
    }

    fn component(&self, name: &str) -> Result<api::Component, Error> {
        runtime()?
            .factory
            .create_component(&self.raw, name)
            .map_err(|status| failed(&format!("CreateComponent({name})"), status))
    }

    fn buffer(&self, data: &[u8]) -> Result<api::Buffer, Error> {
        self.raw.buffer(data).map_err(|status| match status {
            Some(status) => failed("AllocBuffer", status),
            None => Error::Unavailable(String::from("AMF のバッファにアドレスがありません")),
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // 終了してから参照を解放する（`raw` を drop する時）。
        self.raw.terminate();
    }
}

fn configure(component: &api::Component, cpu: bool) {
    component.set(
        api::TIMESTAMP_MODE,
        api::Variant::int(api::AMF_TS_PRESENTATION),
    );
    component.set(api::SURFACE_CPU, api::Variant::boolean(cpu));
    // 既定の数では参照フレームと受け取り待ちのフレームで空きがなくなり、入力も出力も止まる。
    component.set(api::SURFACE_POOL_SIZE, api::Variant::int(SURFACE_POOL));
}

const SURFACE_POOL: i64 = 24;

fn decoder_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Mpeg2 => api::DECODER_MPEG2,
        Codec::H264 => api::DECODER_H264,
        Codec::Hevc => api::DECODER_HEVC,
    }
}

pub fn offers(codec: Codec) -> Vec<Offer> {
    if runtime().is_err() {
        return Vec::new();
    }
    let mut offers = Vec::new();
    for (name, adapter) in amd_adapters() {
        let Ok(device) = device_on(&adapter) else {
            continue;
        };
        if say() {
            tracing::info!(
                "AMF {codec:?}: {name} の D3D11 ビデオプロセッサーのデインターレース {:?}",
                vpp::modes(&device)
            );
        }
        let Ok(context) = Context::open(Some(device)) else {
            continue;
        };
        let Ok(component) = context.component(decoder_name(codec)) else {
            continue;
        };
        let accel = component.acceleration();
        if say() {
            tracing::info!("AMF {codec:?}: {name} のアクセラレーション {accel:?}");
        }
        if accel != Ok(api::AMF_ACCEL_HARDWARE) {
            continue;
        }
        let usable = component.init(api::AMF_SURFACE_NV12, 1920, 1088) == api::AMF_OK;
        component.terminate();
        drop(component);
        if !usable {
            continue;
        }
        #[cfg(feature = "d3d12")]
        let draws = super::d3d12::dx::draws_on(&name);
        #[cfg(not(feature = "d3d12"))]
        let draws = false;
        offers.push(Offer {
            device: name,
            ways: vec![Way::Amf],
            gpu: true,
            draws,
        });
    }
    offers
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    width: u32,
    height: u32,
    wide: bool,
}

pub struct Amf {
    // コンポーネントはコンテキストより先に drop する（宣言の順）。
    component: api::Component,
    main10: bool,
    context: Context,
    codec: Codec,
    framing: Framing,
    device: String,
    shape: Option<Shape>,
    /// `Init` したか。大きさが変わってもコンポーネントは初期化されたままなので `shape` とは別に持つ。
    initialised: bool,
    format: Format,
    keys: Keys,
    base: i64,
    abouts: BTreeMap<i64, About>,
    ready: VecDeque<Frame>,
    deinterlace: Option<HwDeinterlace>,
    device3d: Option<ID3D11Device>,
    vpp: Option<vpp::Deinterlacer>,
    /// 失敗した後は AMF に触らない（触ると AMF の中でクラッシュする）。
    broken: Option<String>,
    /// フィールドの対の一枚目の付随情報。二枚目のタイムスタンプにも結び付ける。
    first_field: Option<About>,
    submitted: u32,
    made: bool,
    unreadable: usize,
    why: Option<String>,
}

impl std::fmt::Debug for Amf {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Amf({:?} {})", self.codec, self.device)
    }
}

// SAFETY: 一度に一つの呼び出しでしか触らない。D3D11 のデバイスはマルチスレッド保護を有効にした。
unsafe impl Send for Amf {}

impl Amf {
    pub fn open(
        codec: Codec,
        framing: Framing,
        device: &str,
        deinterlace: Option<HwDeinterlace>,
    ) -> Result<Self, Error> {
        let runtime = runtime()?;
        // デインターレースには D3D11 のデバイスが要るので、名前が空でも最初の AMD の GPU に作る。
        let device = if device.is_empty() && deinterlace.is_some() {
            amd_adapters()
                .into_iter()
                .next()
                .map(|(name, _)| name)
                .unwrap_or_default()
        } else {
            device.to_owned()
        };
        let device = device.as_str();
        let (name, d3d) = if device.is_empty() {
            (String::from("既定の GPU"), None)
        } else {
            let (name, adapter) = amd_adapters()
                .into_iter()
                .find(|(name, _)| name == device)
                .ok_or_else(|| {
                    Error::Unavailable(format!("AMD の GPU {device} が見つかりません"))
                })?;
            (name, Some(device_on(&adapter)?))
        };
        if let (Some(mode), Some(d3d)) = (deinterlace, &d3d)
            && !vpp::modes(d3d).contains(&mode)
        {
            return Err(Error::Unavailable(format!(
                "{name} の D3D11 ビデオプロセッサーに {} のデインターレースはありません",
                mode.name()
            )));
        }
        let device3d = d3d.clone();
        let context = Context::open(d3d)?;
        let component = context.component(decoder_name(codec))?;
        let accel = component.acceleration();
        if accel != Ok(api::AMF_ACCEL_HARDWARE) {
            return Err(Error::Unavailable(match accel {
                Err(status) => format!(
                    "{name} の AMF は {codec:?} のケイパビリティを返しません（GetCaps: {} ({status})）",
                    api::status_name(status)
                ),
                Ok(kind) => {
                    format!(
                        "{name} の AMF は {codec:?} をハードウェアでデコードできません（アクセラレーションの種類 {kind}）"
                    )
                }
            }));
        }
        configure(&component, deinterlace.is_none());
        if say() {
            tracing::info!(
                "AMF {codec:?}: {name} を開いた（ランタイム {:#018x}）",
                runtime.version
            );
        }
        Ok(Self {
            component,
            main10: false,
            context,
            codec,
            framing,
            device: name,
            shape: None,
            initialised: false,
            format: Format::default(),
            keys: match codec {
                Codec::Mpeg2 => Keys::Mpeg2,
                Codec::H264 => Keys::H264(Box::default()),
                Codec::Hevc => Keys::Hevc(Box::default()),
            },
            base: 0,
            abouts: BTreeMap::new(),
            ready: VecDeque::new(),
            deinterlace,
            device3d,
            vpp: None,
            broken: None,
            first_field: None,
            submitted: 0,
            made: false,
            unreadable: 0,
            why: None,
        })
    }

    fn shape_for(&mut self, format: Format) -> Result<(), Error> {
        let wanted = Shape {
            width: format.width,
            height: format.height,
            wide: format.depth > 8,
        };
        if self.shape == Some(wanted) {
            return Ok(());
        }
        if self.codec == Codec::Hevc && wanted.wide != self.main10 {
            let name = if wanted.wide {
                api::DECODER_HEVC_MAIN10
            } else {
                api::DECODER_HEVC
            };
            let component = self.context.component(name)?;
            if component.acceleration() != Ok(api::AMF_ACCEL_HARDWARE) {
                return Err(Error::Unavailable(format!(
                    "{} の AMF は HEVC の{}ビットをハードウェアでデコードできません",
                    self.device,
                    if wanted.wide { "10 " } else { "8 " }
                )));
            }
            // 前のコンポーネントを終了して解放してから、新しいコンポーネントを用意する。
            if self.initialised {
                self.component.terminate();
                self.initialised = false;
                self.ready.clear();
                self.abouts.clear();
            }
            drop(std::mem::replace(&mut self.component, component));
            configure(&self.component, self.deinterlace.is_none());
            self.main10 = wanted.wide;
            self.shape = None;
        }
        if self.initialised {
            self.component.terminate();
            self.initialised = false;
            self.ready.clear();
            self.abouts.clear();
        }
        let status = self.component.init(
            if wanted.wide {
                api::AMF_SURFACE_P010
            } else {
                api::AMF_SURFACE_NV12
            },
            wanted.width as i32,
            wanted.height as i32,
        );
        if status != api::AMF_OK {
            self.shape = None;
            return Err(failed("Init", status));
        }
        self.initialised = true;
        self.shape = Some(wanted);
        Ok(())
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

    fn pull(&mut self) -> Result<(), Error> {
        loop {
            let (status, data) = self.component.query_output();
            let got = data.is_some();
            if say() && (status != api::AMF_REPEAT || got) {
                tracing::info!(
                    "AMF {:?}: QueryOutput → {} ({status})、フレーム{}",
                    self.codec,
                    api::status_name(status),
                    if got { "あり" } else { "なし" }
                );
            }
            if let Some(data) = data {
                let read = if self.deinterlace.is_some() {
                    self.read_deinterlaced(&data)
                } else {
                    self.read(&data).map(|frame| vec![frame])
                };
                drop(data);
                match read {
                    Ok(frames) => {
                        self.made = true;
                        self.ready.extend(frames);
                    }
                    Err(why) => {
                        self.unreadable += 1;
                        if self.why.is_none() {
                            tracing::warn!("AMF {:?}: {why}", self.codec);
                            self.why = Some(why);
                        }
                    }
                }
            }
            match status {
                api::AMF_OK if got => {}
                api::AMF_OK | api::AMF_REPEAT | api::AMF_EOF | api::AMF_NEED_MORE_INPUT => {
                    return Ok(());
                }
                api::AMF_RESOLUTION_CHANGED | api::AMF_RESOLUTION_UPDATED => {
                    self.shape = None;
                    return Ok(());
                }
                other => return Err(failed("QueryOutput", other)),
            }
        }
    }

    fn read_deinterlaced(&mut self, data: &api::Surface) -> Result<Vec<Frame>, String> {
        let mode = self
            .deinterlace
            .ok_or("デインターレースは要求されていません")?;
        let (memory, time, wide) = (
            data.memory_type(),
            data.pts(),
            data.format() == api::AMF_SURFACE_P010,
        );
        if memory != api::AMF_MEMORY_DX11 {
            return Err(format!(
                "AMF のフレームが DX11 ではありません（メモリの種類 {memory}）"
            ));
        }
        let about = self.about_of(time);
        let (width, height) = (about.format.width, about.format.height);
        let mut frames = Vec::new();
        if !self
            .vpp
            .as_ref()
            .is_some_and(|vpp| vpp.fits(width, height, wide))
        {
            if let Some(old) = self.vpp.as_mut() {
                frames.extend(old.drain()?);
            }
            let device = self
                .device3d
                .as_ref()
                .ok_or("D3D11 のデバイスがありません")?;
            self.vpp = Some(
                vpp::Deinterlacer::open(device, mode, width, height, wide)
                    .map_err(|error| error.to_string())?,
            );
        }
        let vpp = self.vpp.as_mut().ok_or("ビデオプロセッサーがありません")?;
        // SAFETY: メモリが DX11 なのは上で確かめた。
        frames.extend(unsafe { vpp.process(data, about) }?);
        Ok(frames)
    }

    fn read(&mut self, data: &api::Surface) -> Result<Frame, String> {
        let status = data.convert(api::AMF_MEMORY_HOST);
        if status != api::AMF_OK {
            return Err(format!("Convert(HOST): AMF_RESULT {status}"));
        }
        let time = data.pts();
        let wide = match data.format() {
            api::AMF_SURFACE_NV12 => false,
            api::AMF_SURFACE_P010 => true,
            other => return Err(format!("想定外の形式 {other}")),
        };
        if data.planes_count() < 2 {
            return Err(String::from("プレーンが二つではありません"));
        }
        let mut planes = [(std::ptr::null::<u8>(), 0usize, 0usize); 2];
        for (index, into) in planes.iter_mut().enumerate() {
            let plane = data
                .plane(index)
                .ok_or_else(|| format!("{index} 番のプレーンがありません"))?;
            let base = plane.native().cast::<u8>().cast_const();
            let (pitch, rows) = (plane.h_pitch(), plane.v_pitch());
            if base.is_null() || pitch <= 0 || rows <= 0 {
                return Err(format!(
                    "{index} 番のプレーンが読めません（ピッチ {pitch}、行 {rows}）"
                ));
            }
            *into = (base, pitch as usize, rows as usize);
        }
        let about = self.about_of(time);
        let width = about.format.width as usize;
        let height = about.format.height as usize;
        let [
            (luma, luma_pitch, luma_rows),
            (chroma, chroma_pitch, chroma_rows),
        ] = planes;
        if luma_rows < height || chroma_rows < height.div_ceil(2) {
            return Err(format!(
                "プレーンがフレームより小さい（輝度 {luma_rows} 行、彩度 {chroma_rows} 行、フレーム {height} 行）"
            ));
        }
        // SAFETY: メインメモリへ移したプレーンで、ピッチ×行の大きさがあり、`data` より長く持たない。
        let (luma, chroma) = unsafe {
            (
                std::slice::from_raw_parts(luma, luma_pitch * luma_rows),
                std::slice::from_raw_parts(chroma, chroma_pitch * chroma_rows),
            )
        };
        let planes = super::semi_planar(
            (luma, luma_pitch),
            (chroma, chroma_pitch),
            width,
            height,
            wide,
        )
        .ok_or_else(|| String::from("プレーンがフレームの大きさに足りません"))?;
        Ok(Frame {
            tag: about.tag,
            format: about.format,
            planes,
            structure: about.structure,
            top_field_first: about.top_field_first,
            repeat_first_field: about.repeat_first_field,
            progressive: about.progressive,
        })
    }

    fn about_of(&mut self, time: i64) -> About {
        if say() {
            tracing::info!(
                "AMF {:?}: 出力 タイムスタンプ {time} 付随情報 {:?}",
                self.codec,
                self.abouts
                    .get(&time)
                    .map(|about| (about.tag, about.structure, about.progressive))
            );
        }
        match self.abouts.remove(&time) {
            // フィールドピクチャーは二枚が一つのフレームになって出てくる。
            Some(about) if about.structure.field() => About {
                structure: Structure::Frame,
                top_field_first: about.structure == Structure::Top,
                progressive: false,
                ..about
            },
            Some(about) => about,
            None => About {
                tag: time as u64,
                format: self.format,
                structure: Structure::Frame,
                top_field_first: true,
                repeat_first_field: false,
                progressive: self.format.progressive,
            },
        }
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

impl Decode for Amf {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if let Some(why) = &self.broken {
            return Err(Error::Unavailable(why.clone()));
        }
        // フィールドピクチャーの MPEG-2 は AMF ではデコードできない（[`QUIET`]）。
        if self.codec == Codec::Mpeg2 && picture.structure.field() {
            let why = String::from(
                "AMF はフィールドピクチャーの MPEG-2 をデコードできません（受け付けてもフレームを出さない）",
            );
            self.broken = Some(why.clone());
            return Err(Error::Unavailable(why));
        }
        // 一枚が一フィールドの HEVC（`field_seq_flag`）も、フレームを出さずに詰まり、読み出しでクラッシュする。
        if self.codec == Codec::Hevc && format.field_pictures {
            let why = String::from(
                "AMF は一枚が一フィールドの HEVC をデコードできません（フレームを出さずに詰まる）",
            );
            self.broken = Some(why.clone());
            return Err(Error::Unavailable(why));
        }
        if !self.made && self.submitted >= QUIET {
            let why = format!(
                "AMF が {} 回渡してもフレームを一枚も出しません",
                self.submitted
            );
            self.broken = Some(why.clone());
            return Err(Error::Unavailable(why));
        }
        super::openable(format)?;
        self.format = format;
        self.shape_for(format)?;
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
        // フィールドの対のフレームには二枚目のタイムスタンプが付くので、それにも一枚目の付随情報を結び付ける。
        let about = match self.first_field.take() {
            Some(first) if picture.structure.field() && first.structure != picture.structure => {
                first
            }
            _ => {
                if picture.structure.field() {
                    self.first_field = Some(about);
                }
                about
            }
        };
        self.abouts.entry(time).or_insert(about);
        while self.abouts.len() > ABOUTS {
            self.abouts.pop_first();
        }
        let buffer = self.context.buffer(data)?;
        buffer.set_pts(time);
        let mut status = self.component.submit(&buffer);
        self.submitted += 1;
        if say() {
            tracing::info!(
                "AMF {:?}: SubmitInput {tag}（タイムスタンプ {time}、{} バイト、{:?}、progressive {}）→ {status}",
                self.codec,
                data.len(),
                picture.structure,
                picture.progressive
            );
        }
        // 出していないフレームで詰まっている。受け取りながら空きが出るまで少し待って渡し直す。
        let began = std::time::Instant::now();
        while (status == api::AMF_INPUT_FULL || status == api::AMF_DECODER_NO_FREE_SURFACES)
            && began.elapsed() < FULL_PATIENCE
        {
            if let Err(error) = self.pull() {
                drop(buffer);
                return Err(error);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            status = self.component.submit(&buffer);
        }
        // 渡したバッファの参照を解放する（コンポーネントは自分の参照を持つ）。
        drop(buffer);
        match status {
            api::AMF_OK | api::AMF_NEED_MORE_INPUT => {}
            api::AMF_RESOLUTION_CHANGED | api::AMF_RESOLUTION_UPDATED => self.shape = None,
            other => {
                let error = failed("SubmitInput", other);
                self.broken = Some(error.to_string());
                return Err(error);
            }
        }
        self.pull()
    }

    fn take(&mut self) -> Option<Decoded> {
        self.ready.pop_front().map(Decoded::Planes)
    }

    fn read_back(&mut self) -> Option<Readback> {
        let frame = self.ready.pop_front()?;
        let size = if frame.format.depth > 8 { 2 } else { 1 };
        let [luma, blue, red] = &frame.planes;
        let mut chroma = Vec::with_capacity(blue.data.len() * 2);
        for (b, r) in blue
            .data
            .chunks_exact(size)
            .zip(red.data.chunks_exact(size))
        {
            chroma.extend_from_slice(b);
            chroma.extend_from_slice(r);
        }
        Some(Readback {
            about: About::of_frame(&frame),
            planes: vec![
                (luma.data.clone(), luma.stride, luma.height),
                (chroma, blue.width * size * 2, blue.height),
            ],
        })
    }

    fn flush(&mut self) {
        if self.shape.is_none() {
            return;
        }
        self.component.drain();
        if let Err(error) = self.pull() {
            tracing::warn!("AMF {:?}: {error}", self.codec);
        }
        if let Some(vpp) = self.vpp.as_mut() {
            match vpp.drain() {
                Ok(frames) => self.ready.extend(frames),
                Err(why) => tracing::warn!("AMF {:?}: {why}", self.codec),
            }
        }
    }

    fn name(&self) -> String {
        match self.deinterlace {
            Some(mode) => format!("AMF ({}・デインターレース {})", self.device, mode.name()),
            None => format!("AMF ({})", self.device),
        }
    }

    fn deinterlaces(&self) -> bool {
        self.deinterlace.is_some()
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.deinterlace.is_some() { 2 } else { 1 }
    }

    fn reset(&mut self) {
        if self.shape.is_some() {
            self.component.flush();
        }
        self.ready.clear();
        self.abouts.clear();
        self.first_field = None;
        if let Some(vpp) = self.vpp.as_mut() {
            vpp.reset();
        }
        self.keys.reset();
        self.base += SEQUENCE;
    }
}

impl Drop for Amf {
    fn drop(&mut self) {
        if say() {
            tracing::info!("AMF {:?}: 終了", self.codec);
        }
        // vpp が持つ AMF のサーフェスはコンテキストより先に解放する。
        self.vpp = None;
        // 参照は `component` を drop する時に、コンテキストより先に解放される。
        self.component.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_become_start_codes() {
        let data = [0, 0, 0, 2, 0x40, 0x01, 0, 0, 0, 3, 0x26, 0x01, 0xaf];
        assert_eq!(
            start_codes(&data, Framing::Lengths),
            [0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x26, 0x01, 0xaf]
        );
    }
}
