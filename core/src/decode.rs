// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Choosing a decoder. The platform one is preferred; the software one is always there.

#[cfg(all(feature = "amf", windows))]
pub mod amf;
#[cfg(all(feature = "android", target_os = "android"))]
pub mod android;
#[cfg(all(feature = "d3d12", windows))]
pub mod d3d12;
#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows)
))]
pub mod frames;
#[cfg(all(feature = "mediafoundation", windows))]
pub mod mediafoundation;
#[cfg(all(
    unix,
    any(feature = "vaapi", all(feature = "v4l2", target_os = "linux"))
))]
mod mmap;
#[cfg(all(feature = "nvdec", any(target_os = "linux", windows)))]
pub mod nvdec;
/// 表示順のキー。デコーダーが並べ替えない、または並べ替えたピクチャーの時刻を当てにできないバックエンドが使う。
#[cfg(any(
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "mediafoundation", windows),
    all(feature = "amf", windows),
    all(feature = "qsv", any(windows, target_os = "linux"))
))]
mod order;
#[cfg(all(feature = "qsv", any(windows, target_os = "linux")))]
pub mod qsv;
#[cfg(any(
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows)
))]
pub mod rps;
/// VA-API・NVDEC・D3D12 で共通のサーフェス管理テーブル（V4L2 はタイムスタンプで対応付けるので別）。
#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows)
))]
pub mod slots;
pub mod sound;
#[cfg(unix)]
pub mod takes;
#[cfg(all(feature = "v4l2", target_os = "linux"))]
pub mod v4l2;
#[cfg(feature = "vaapi")]
pub mod vaapi;
/// `kind = "framework"` の `#[link]` は Apple 以外でコンパイルエラーになる。
#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
pub mod videotoolbox;
pub mod weave;

#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows)
))]
macro_rules! each_front {
    ($front:expr, |$it:ident| $body:expr) => {
        match $front {
            Front::Mpeg2($it) => $body,
            Front::H264($it) => $body,
            Front::Hevc($it) => $body,
        }
    };
}
#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows)
))]
pub(crate) use each_front;

use mediadecoder::es::{Codec, Format, Framing, Picture, Structure};
#[cfg(feature = "h264-decoder")]
use mediadecoder::video::h264::H264;
#[cfg(feature = "hevc-decoder")]
use mediadecoder::video::hevc::Hevc;
use mediadecoder::video::{self, Frame, mpeg2::Mpeg2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct About {
    pub tag: u64,
    pub format: Format,
    pub structure: Structure,
    pub top_field_first: bool,
    pub repeat_first_field: bool,
    pub progressive: bool,
}

impl About {
    pub fn of_frame(frame: &Frame) -> Self {
        Self {
            tag: frame.tag,
            format: frame.format,
            structure: frame.structure,
            top_field_first: frame.top_field_first,
            repeat_first_field: frame.repeat_first_field,
            progressive: frame.progressive,
        }
    }
}

#[derive(Debug)]
pub enum Decoded {
    Planes(Frame),
    Surface(Surface),
}

impl Decoded {
    pub fn about(&self) -> About {
        match self {
            Self::Planes(frame) => About::of_frame(frame),
            Self::Surface(surface) => surface.about,
        }
    }
}

#[derive(Debug)]
pub struct Surface {
    pub about: About,
    pub held: Held,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum Held {
    #[cfg(unix)]
    DmaBuf(DmaBuf),
    #[cfg(all(feature = "android", target_os = "android"))]
    HardwareBuffer(android::Frame),
    /// Never leaves the codec, so `read_back` never answers for it.
    #[cfg(all(feature = "android", target_os = "android"))]
    Screen(android::Screen),
    #[cfg(all(feature = "d3d12", windows))]
    D3d12(d3d12::Frame),
    /// `CVPixelBuffer`（二プレーン、`IOSurface` 付きで `CVMetalTextureCache` がコピー無しで読める）。
    #[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
    VideoToolbox(videotoolbox::Frame),
}

#[cfg(unix)]
#[derive(Debug)]
pub struct DmaBuf {
    pub width: u32,
    pub height: u32,
    /// A tiled frame read as linear is not a picture.
    pub modifier: u64,
    pub planes: Vec<DmaBufPlane>,
    /// 保持されている間、デコーダー（V4L2）はこのバッファを再利用しない（[`crate::decode::slots::Lease`]）。
    pub lease: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

#[cfg(unix)]
impl DmaBuf {
    pub const NV12: u32 = u32::from_le_bytes(*b"NV12");
    pub const P010: u32 = u32::from_le_bytes(*b"P010");
    pub const P030: u32 = u32::from_le_bytes(*b"P030");
    pub const NV15: u32 = u32::from_le_bytes(*b"NV15");
    const R8: u32 = u32::from_le_bytes(*b"R8  ");
    const GR88: u32 = u32::from_le_bytes(*b"GR88");
    const R16: u32 = u32::from_le_bytes(*b"R16 ");
    const GR1616: u32 = u32::from_le_bytes(*b"GR32");

    pub fn fourcc(&self) -> Option<u32> {
        let [luma, chroma] = self.planes.as_slice() else {
            return None;
        };
        match (luma.format, chroma.format) {
            (Self::R8, Self::GR88) => Some(Self::NV12),
            (Self::R16, Self::GR1616) => Some(Self::P010),
            (whole, again) if whole == again && matches!(whole, Self::P030 | Self::NV15) => {
                Some(whole)
            }
            _ => None,
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
    pub format: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unavailable(String),
    Unsupported(&'static str),
    Damaged(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(out, "{why}"),
            Self::Unsupported(what) => write!(out, "unsupported: {what}"),
            Self::Damaged(what) => write!(out, "damaged: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<video::Error> for Error {
    fn from(error: video::Error) -> Self {
        match error {
            video::Error::Unsupported(what) => Self::Unsupported(what),
            video::Error::Damaged(what) => Self::Damaged(what),
        }
    }
}

pub trait Decode: Send {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error>;

    fn take(&mut self) -> Option<Decoded>;

    fn flush(&mut self);

    fn name(&self) -> String {
        String::from("ハードウェアデコーダー（プラットフォーム）")
    }

    /// CPU でデコードするもの（内蔵のデコーダーと [`Prefer::System`]）は偽。
    fn hardware(&self) -> bool {
        true
    }

    fn reset(&mut self);

    fn read_back(&mut self) -> Option<Readback> {
        None
    }

    /// Whether fields are already woven; weaving again would be wrong.
    fn deinterlaces(&self) -> bool {
        false
    }

    /// 一枚が一フィールドのピクチャーに、自分でどちらのフィールドかを付けてそのまま渡す（読み戻して組まない）。
    fn marks_fields(&self) -> bool {
        false
    }

    fn pictures_per_frame(&self) -> u32 {
        1
    }
}

#[derive(Debug)]
pub struct Readback {
    pub about: About,
    pub planes: Vec<(Vec<u8>, usize, usize)>,
}

impl Readback {
    pub fn luma(&self) -> Option<(&[u8], usize)> {
        self.planes
            .first()
            .map(|(bytes, pitch, _)| (bytes.as_slice(), *pitch))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Luma<'a> {
    pub bytes: &'a [u8],
    pub pitch: usize,
    pub shift: u32,
}

impl<'a> Luma<'a> {
    pub fn of_frame(frame: &'a mediadecoder::video::Frame) -> Self {
        Self {
            bytes: &frame.planes[0].data,
            pitch: frame.planes[0].stride,
            shift: 0,
        }
    }
}

pub fn psnr(mine: Luma<'_>, theirs: Luma<'_>, format: Format) -> f64 {
    let wide = format.depth > 8;
    let peak = f64::from((1u32 << format.depth) - 1);
    let sample = |luma: &Luma<'_>, x: usize, y: usize| -> i64 {
        let at = y * luma.pitch + x * if wide { 2 } else { 1 };
        if wide {
            let low = u32::from(luma.bytes.get(at).copied().unwrap_or(0));
            let high = u32::from(luma.bytes.get(at + 1).copied().unwrap_or(0));
            i64::from((low | (high << 8)) >> luma.shift)
        } else {
            i64::from(luma.bytes.get(at).copied().unwrap_or(0))
        }
    };
    let mut total = 0f64;
    for y in 0..format.height as usize {
        for x in 0..format.width as usize {
            let difference = sample(&mine, x, y) - sample(&theirs, x, y);
            total += (difference * difference) as f64;
        }
    }
    let mean = total / (f64::from(format.width) * f64::from(format.height));
    if mean == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (peak * peak / mean).log10()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeinterlaceBy {
    #[default]
    Auto,
    Software,
    Hardware,
}
/// Every platform decoder's ring is sized against this; holding more overwrites a shown surface.
pub const MOST_HELD: usize = HANDOVER + AHEAD + crate::output::video::HELD_BY_FILTER + IN_FLIGHT;

/// GPU でのレンダリングが終わるまでリースは解放されない。一回のレンダリングで三枚を読み、提示は一つまでキューに積むので二回分。
pub const IN_FLIGHT: usize = 6;

pub const HELD_BY_PLAYER: usize = MOST_HELD - crate::output::video::HELD_BY_FILTER;

/// Small on purpose (a 4K picture is 25 MB); buffering belongs in [`ACCESS`].
pub const AHEAD: usize = 4;

pub const HANDOVER: usize = 4;

/// 満杯だとデコードスレッドが止まるので少し手前。
pub const PREROLL_PICTURES: u64 = (HANDOVER + AHEAD - 2) as u64;

/// Size and depth only; the colour can change alone, so the decoder must still take `wanted`.
#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows),
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "android", target_os = "android")
))]
pub(crate) fn session_fits(held: Format, wanted: Format) -> bool {
    held.width == wanted.width && held.height == wanted.height && held.depth == wanted.depth
}

#[cfg(any(
    feature = "vaapi",
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "d3d12", windows),
    all(feature = "mediafoundation", windows),
    all(feature = "amf", windows),
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "android", target_os = "android")
))]
pub(crate) fn openable(format: Format) -> Result<(), Error> {
    if format.chroma != 1 {
        return Err(Error::Unsupported("only 4:2:0 is broadcast here"));
    }
    if format.width == 0 || format.height == 0 {
        return Err(Error::Damaged("a picture of no size"));
    }
    // 大きなサイズを申告するストリームで巨大なサーフェスを作らせない。
    if !mediadecoder::video::fits(format.width.into(), format.height.into()) {
        return Err(Error::Unsupported("a picture larger than any level allows"));
    }
    Ok(())
}

/// Windows のハードウェアデコードの API。既定は D3D12、D3D11VA は AMD 向け。`ZW_DECODE_API` が優先される。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Api {
    #[default]
    Auto,
    D3d12,
    D3d11,
}

static API: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_api(api: Api) {
    let value = match api {
        Api::Auto => 0,
        Api::D3d12 => 1,
        Api::D3d11 => 2,
    };
    API.store(value, std::sync::atomic::Ordering::Relaxed);
}

pub fn api() -> Api {
    match API.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Api::D3d12,
        2 => Api::D3d11,
        _ => Api::Auto,
    }
}

/// D3D12 のデコーダーが、ビデオプロセスでデインターレースするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoProcess {
    #[default]
    Adaptive,
    Bob,
    Off,
}

static VIDEO_PROCESS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_video_process(process: VideoProcess) {
    let value = match process {
        VideoProcess::Adaptive => 0,
        VideoProcess::Bob => 1,
        VideoProcess::Off => 2,
    };
    VIDEO_PROCESS.store(value, std::sync::atomic::Ordering::Relaxed);
}

pub fn video_process() -> VideoProcess {
    match VIDEO_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
        1 => VideoProcess::Bob,
        2 => VideoProcess::Off,
        _ => VideoProcess::Adaptive,
    }
}

/// 映像を出す画面の大きさ（物理画素、幅を上位 32 ビットに）。0 なら分からない。
static DISPLAY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// ウィンドウのある画面の大きさを知らせる。デコーダーは、これの二倍以上あるフレームを出す前に縮める。
pub fn set_display_size(width: u32, height: u32) {
    let packed = (u64::from(width) << 32) | u64::from(height);
    DISPLAY.store(packed, std::sync::atomic::Ordering::Relaxed);
}

pub fn display_size() -> Option<[u32; 2]> {
    let packed = DISPLAY.load(std::sync::atomic::Ordering::Relaxed);
    let size = [(packed >> 32) as u32, packed as u32];
    (size[0] > 0 && size[1] > 0).then_some(size)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    #[default]
    Platform,
    Software,
    /// 選んだときだけ使い、既定の候補には入れない。使えなければ内蔵のデコーダーにフォールバックする。
    System,
}

struct Software(Box<dyn video::Decoder + Send>, Codec);

impl std::fmt::Debug for Software {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Software")
    }
}

impl Decode for Software {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        Ok(self.0.decode(tag, picture, format, data)?)
    }

    fn take(&mut self) -> Option<Decoded> {
        self.0.take().map(Decoded::Planes)
    }

    fn flush(&mut self) {
        self.0.flush();
    }

    fn name(&self) -> String {
        String::from(zerowave_name(self.1))
    }

    fn hardware(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        self.0.reset();
    }
}

pub fn software(codec: Codec, framing: Framing) -> Box<dyn Decode> {
    #[cfg(not(feature = "hevc-decoder"))]
    let _ = framing;
    let decoder: Box<dyn video::Decoder + Send> = match codec {
        Codec::Mpeg2 => Box::new(Mpeg2::new()),
        #[cfg(feature = "h264-decoder")]
        Codec::H264 => Box::new(H264::new()),
        #[cfg(feature = "hevc-decoder")]
        Codec::Hevc => Box::new(Hevc::with_framing(framing)),
        #[allow(unreachable_patterns)]
        _ => {
            return Box::new(Missing::new(format!(
                "このビルドには内蔵の {} デコーダーが入っていません",
                coding_name(codec)
            )));
        }
    };
    Software(decoder, codec).into()
}

pub fn zerowave_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Mpeg2 => "ZeroWave MPEG2 Video Decoder",
        Codec::H264 => "ZeroWave H264 Video Decoder",
        Codec::Hevc => "ZeroWave H265 Video Decoder",
    }
}

pub fn has_software(codec: Codec) -> bool {
    match codec {
        Codec::Mpeg2 => true,
        Codec::H264 => cfg!(feature = "h264-decoder"),
        Codec::Hevc => cfg!(feature = "hevc-decoder"),
    }
}

fn coding_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Mpeg2 => "MPEG-2",
        Codec::H264 => "H.264",
        Codec::Hevc => "H.265",
    }
}

fn last_resort(codec: Codec, framing: Framing) -> Box<dyn Decode> {
    let off = plan().zerowave_off.contains(&codec);
    // 内蔵のデコーダーが無い（このビルドに入っていないか設定で外した）なら、OS のソフトウェアデコーダー
    // （Media Foundation など）を試す。ハードウェアのデコーダーが無い PC（仮想マシンなど）でも映像を出すため。
    if off || !built_in(codec) {
        match system(codec, framing) {
            Ok(decoder) => {
                tracing::info!(
                    "デコーダー {codec:?}: {}（内蔵のデコーダーが無いため）",
                    decoder.name()
                );
                return decoder;
            }
            Err(error) => said_once(codec, &format!("OS のソフトウェアデコーダー: {error}")),
        }
    }
    if off {
        return Box::new(Missing::new(format!(
            "{} の ZeroWave のデコーダーは設定で外しています",
            coding_name(codec)
        )));
    }
    software(codec, framing)
}

/// このビルドに内蔵のデコーダーがあるか（[`software`] が [`Missing`] を返さないか）。
fn built_in(codec: Codec) -> bool {
    match codec {
        Codec::Mpeg2 => true,
        Codec::H264 => cfg!(feature = "h264-decoder"),
        Codec::Hevc => cfg!(feature = "hevc-decoder"),
    }
}

/// 使えるデコーダーが無いときの代わり。最初の一回だけ警告し、あとは何も出力しない。
#[derive(Debug)]
struct Missing {
    why: String,
    said: bool,
}

impl Missing {
    fn new(why: String) -> Self {
        Self { why, said: false }
    }
}

impl Decode for Missing {
    fn decode(&mut self, _: u64, _: &Picture, _: Format, _: &[u8]) -> Result<(), Error> {
        if std::mem::replace(&mut self.said, true) {
            return Ok(());
        }
        Err(Error::Unavailable(self.why.clone()))
    }

    fn take(&mut self) -> Option<Decoded> {
        None
    }

    fn flush(&mut self) {}

    fn name(&self) -> String {
        format!("なし（{}）", self.why)
    }

    fn hardware(&self) -> bool {
        false
    }

    fn reset(&mut self) {}
}

impl From<Software> for Box<dyn Decode> {
    fn from(software: Software) -> Self {
        Box::new(software)
    }
}

pub fn open(codec: Codec, framing: Framing, prefer: Prefer) -> Box<dyn Decode> {
    open_with(codec, framing, prefer, DeinterlaceBy::default())
}

pub fn open_with(
    codec: Codec,
    framing: Framing,
    prefer: Prefer,
    by: DeinterlaceBy,
) -> Box<dyn Decode> {
    open_from(codec, framing, prefer, by, 0).0
}

pub fn open_from(
    codec: Codec,
    framing: Framing,
    prefer: Prefer,
    by: DeinterlaceBy,
    from: usize,
) -> (Box<dyn Decode>, Option<usize>) {
    let picks = plan().of(codec).to_vec();
    if prefer == Prefer::Platform && !picks.is_empty() {
        for (at, pick) in picks.iter().enumerate().skip(from) {
            match open_pick(codec, framing, pick, by) {
                Ok(decoder) => {
                    tracing::info!("デコーダー {codec:?}: {}", decoder.name());
                    return (decoder, Some(at + 1));
                }
                Err(Error::Unavailable(why)) => {
                    said_once(codec, &format!("{} は使えない: {why}", pick.spelling()));
                }
                Err(error) => tracing::warn!("{} を開けない: {error:?}", pick.spelling()),
            }
        }
        // 内蔵のデコーダーにフォールバックする前に既定のハードウェアデコーダーを試す（候補のデコーダーがピクチャーの形式で拒否することがある）。
        if from <= picks.len() {
            match platform_with(codec, framing, by) {
                Ok(decoder) => {
                    tracing::info!(
                        "デコーダー {codec:?}: {}（候補のデコーダーが使えないため既定）",
                        decoder.name()
                    );
                    return (decoder, Some(picks.len() + 1));
                }
                Err(Error::Unavailable(why)) => said_once(codec, &why),
                Err(error) => tracing::warn!("ハードウェアデコーダーを開けない: {error:?}"),
            }
        }
        return (last_resort(codec, framing), None);
    }
    if from > 0 {
        return (last_resort(codec, framing), None);
    }
    let opened = match prefer {
        Prefer::Platform => platform_with(codec, framing, by),
        Prefer::System => system(codec, framing),
        Prefer::Software => return (software(codec, framing), None),
    };
    match opened {
        Ok(decoder) => {
            tracing::info!("デコーダー {codec:?}: {}", decoder.name());
            return (decoder, Some(1));
        }
        Err(Error::Unavailable(why)) => said_once(codec, &why),
        Err(error) => tracing::warn!("ハードウェアデコーダーを開けない: {error:?}"),
    }
    (last_resort(codec, framing), None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Way {
    Auto,
    D3d12,
    D3d11,
    Nvdec,
    Vaapi,
    V4l2,
    VideoToolbox,
    MediaCodec,
    Amf,
    Qsv,
    System,
}

impl Way {
    const ALL: [Self; 11] = [
        Self::Auto,
        Self::D3d12,
        Self::D3d11,
        Self::Nvdec,
        Self::Vaapi,
        Self::V4l2,
        Self::VideoToolbox,
        Self::MediaCodec,
        Self::Amf,
        Self::Qsv,
        Self::System,
    ];

    pub fn spelling(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::D3d12 => "d3d12",
            Self::D3d11 => "d3d11",
            Self::Nvdec => "nvdec",
            Self::Vaapi => "vaapi",
            Self::V4l2 => "v4l2",
            Self::VideoToolbox => "videotoolbox",
            Self::MediaCodec => "mediacodec",
            Self::Amf => "amf",
            Self::Qsv => "qsv",
            Self::System => "system",
        }
    }

    pub fn from_spelling(spelling: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|way| way.spelling() == spelling)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "自動",
            Self::D3d12 => "D3D12",
            Self::D3d11 => "D3D11VA",
            Self::Nvdec => "NVDEC",
            Self::Vaapi => "VA-API",
            Self::V4l2 => "V4L2",
            Self::VideoToolbox => "VideoToolbox",
            Self::MediaCodec => "MediaCodec",
            Self::Amf => "AMF",
            Self::Qsv => "QSV",
            Self::System => {
                if cfg!(windows) {
                    "Media Foundation"
                } else if cfg!(target_vendor = "apple") {
                    "VideoToolbox"
                } else {
                    "MediaCodec"
                }
            }
        }
    }
}

impl Way {
    pub fn deinterlacers(self) -> &'static [HwDeinterlace] {
        match self {
            Self::Nvdec => &[HwDeinterlace::Adaptive, HwDeinterlace::Bob],
            Self::Vaapi => &[
                HwDeinterlace::Adaptive,
                HwDeinterlace::MotionCompensated,
                HwDeinterlace::Bob,
            ],
            Self::Qsv => &[HwDeinterlace::Adaptive, HwDeinterlace::Bob],
            Self::Amf => &[
                HwDeinterlace::Adaptive,
                HwDeinterlace::MotionCompensated,
                HwDeinterlace::Bob,
            ],
            // どの SoC のデインターレーサーがあるかは開くまで分からないので、一つだけ示す。
            Self::V4l2 => &[HwDeinterlace::Adaptive],
            _ => &[],
        }
    }
}

/// デコーダー自身が行うデインターレース。使うと ZeroWave のデインターレースと逆テレシネは掛からない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HwDeinterlace {
    Bob,
    Adaptive,
    MotionCompensated,
}

impl HwDeinterlace {
    const ALL: [Self; 3] = [Self::Bob, Self::Adaptive, Self::MotionCompensated];

    pub fn spelling(self) -> &'static str {
        match self {
            Self::Bob => "bob",
            Self::Adaptive => "adaptive",
            Self::MotionCompensated => "motion",
        }
    }

    pub fn from_spelling(spelling: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.spelling() == spelling)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Bob => "Bob",
            Self::Adaptive => "適応型",
            Self::MotionCompensated => "動き補償",
        }
    }
}

/// デバイスは名前で指定する（LUID は起動ごとに変わる）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pick {
    pub way: Way,
    pub device: String,
    pub deinterlace: Option<HwDeinterlace>,
}

impl Pick {
    /// `d3d12:デバイス名`、`nvdec:名前:adaptive` の形。末尾は既知の指定のときだけ取り除く。
    pub fn parse(spelled: &str) -> Option<Self> {
        let (way, rest) = spelled.split_once(':').unwrap_or((spelled, ""));
        let (device, deinterlace) = rest
            .rsplit_once(':')
            .and_then(|(device, mode)| Some((device, HwDeinterlace::from_spelling(mode.trim())?)))
            .map_or((rest, None), |(device, mode)| (device, Some(mode)));
        Some(Self {
            way: Way::from_spelling(way.trim())?,
            device: device.trim().to_owned(),
            deinterlace,
        })
    }

    pub fn spelling(&self) -> String {
        let mut spelled = self.way.spelling().to_owned();
        if !self.device.is_empty() || self.deinterlace.is_some() {
            spelled.push(':');
            spelled.push_str(&self.device);
        }
        if let Some(mode) = self.deinterlace {
            spelled.push(':');
            spelled.push_str(mode.spelling());
        }
        spelled
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub mpeg2: Vec<Pick>,
    pub h264: Vec<Pick>,
    pub hevc: Vec<Pick>,
    pub zerowave_off: Vec<Codec>,
}

impl Plan {
    pub fn of(&self, codec: Codec) -> &[Pick] {
        match codec {
            Codec::Mpeg2 => &self.mpeg2,
            Codec::H264 => &self.h264,
            Codec::Hevc => &self.hevc,
        }
    }
}

static MEMORY_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_memory_only(on: bool) {
    MEMORY_ONLY.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(
    not(any(feature = "vaapi", all(feature = "nvdec", target_os = "linux"))),
    allow(dead_code)
)]
pub(crate) fn memory_only() -> bool {
    MEMORY_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

static PLAN: std::sync::Mutex<Plan> = std::sync::Mutex::new(Plan {
    mpeg2: Vec::new(),
    h264: Vec::new(),
    hevc: Vec::new(),
    zerowave_off: Vec::new(),
});

pub fn set_plan(plan: Plan) {
    if let Ok(mut slot) = PLAN.lock() {
        *slot = plan;
    }
}

pub fn plan() -> Plan {
    PLAN.lock().map(|slot| slot.clone()).unwrap_or_default()
}

#[cfg(windows)]
fn open_pick(
    codec: Codec,
    framing: Framing,
    pick: &Pick,
    _by: DeinterlaceBy,
) -> Result<Box<dyn Decode>, Error> {
    match pick.way {
        #[cfg(feature = "mediafoundation")]
        Way::System => Ok(Box::new(mediafoundation::MediaFoundation::open_named(
            codec,
            framing,
            (!pick.device.is_empty()).then_some(pick.device.as_str()),
        )?)),
        #[cfg(feature = "d3d12")]
        Way::Auto | Way::D3d12 | Way::D3d11 => Ok(Box::new(d3d12::D3d12::open_pick(
            codec,
            framing,
            &pick.device,
            pick.way,
        )?)),
        #[cfg(feature = "nvdec")]
        Way::Nvdec => {
            #[cfg(feature = "d3d12")]
            if !pick.device.is_empty() && !d3d12::dx::draws_on(&pick.device) {
                return Err(Error::Unavailable(format!(
                    "NVDEC はレンダリングに使う GPU でだけ使えます（{} はレンダリングに使っていません）",
                    pick.device
                )));
            }
            Ok(Box::new(nvdec::Nvdec::open_with(
                codec,
                framing,
                pick.deinterlace,
            )?))
        }
        #[cfg(feature = "amf")]
        Way::Amf => Ok(Box::new(amf::Amf::open(
            codec,
            framing,
            &pick.device,
            pick.deinterlace,
        )?)),
        #[cfg(feature = "qsv")]
        Way::Qsv => Ok(Box::new(qsv::Qsv::open(
            codec,
            framing,
            &pick.device,
            pick.deinterlace,
        )?)),
        way => Err(Error::Unavailable(format!(
            "{} はこのビルドにありません",
            way.name()
        ))),
    }
}

#[cfg(not(windows))]
fn open_pick(
    codec: Codec,
    framing: Framing,
    pick: &Pick,
    by: DeinterlaceBy,
) -> Result<Box<dyn Decode>, Error> {
    match pick.way {
        Way::System => system(codec, framing),
        #[cfg(all(feature = "nvdec", target_os = "linux"))]
        Way::Nvdec => Ok(Box::new(nvdec::Nvdec::open_with(
            codec,
            framing,
            pick.deinterlace,
        )?)),
        #[cfg(all(feature = "vaapi", target_os = "linux"))]
        Way::Vaapi => Ok(Box::new(vaapi::Vaapi::open_pick(
            codec,
            framing,
            &pick.device,
            pick.deinterlace,
        )?)),
        #[cfg(all(feature = "qsv", target_os = "linux"))]
        Way::Qsv => Ok(Box::new(qsv::Qsv::open(
            codec,
            framing,
            &pick.device,
            pick.deinterlace,
        )?)),
        #[cfg(all(feature = "v4l2", target_os = "linux"))]
        Way::V4l2 => {
            let by = if pick.deinterlace.is_some() {
                DeinterlaceBy::Hardware
            } else {
                by
            };
            Ok(Box::new(v4l2::V4l2::open_with(codec, framing, by)?))
        }
        _ => platform_with(codec, framing, by),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub device: String,
    pub ways: Vec<Way>,
    pub gpu: bool,
    pub draws: bool,
}

pub fn offers(codec: Codec) -> Vec<Offer> {
    let mut offers = gpu_offers(codec);
    offers.extend(system_offers(codec));
    offers
}

#[cfg(all(windows, feature = "d3d12"))]
fn gpu_offers(codec: Codec) -> Vec<Offer> {
    // amf も qsv も無いビルドでは足す物が無い。
    #[allow(unused_mut)]
    let mut offers = d3d12::offers(codec);
    #[cfg(feature = "amf")]
    merge(&mut offers, amf::offers(codec));
    #[cfg(feature = "qsv")]
    merge(&mut offers, qsv::offers(codec));
    offers
}

#[cfg_attr(
    not(any(
        all(windows, feature = "d3d12", any(feature = "amf", feature = "qsv")),
        all(target_os = "linux", any(feature = "vaapi", feature = "qsv"))
    )),
    allow(dead_code)
)]
fn merge(into: &mut Vec<Offer>, more: Vec<Offer>) {
    for offer in more {
        match into
            .iter_mut()
            .find(|have| have.gpu == offer.gpu && have.device == offer.device)
        {
            Some(have) => {
                for way in offer.ways {
                    if !have.ways.contains(&way) {
                        have.ways.push(way);
                    }
                }
                have.draws |= offer.draws;
            }
            None => into.push(offer),
        }
    }
}

#[cfg(not(all(windows, feature = "d3d12")))]
fn gpu_offers(codec: Codec) -> Vec<Offer> {
    #[cfg(target_os = "linux")]
    {
        #[cfg_attr(
            not(any(feature = "nvdec", feature = "vaapi", feature = "qsv")),
            allow(unused_mut)
        )]
        let mut offers: Vec<Offer> = Vec::new();
        #[cfg(feature = "nvdec")]
        offers.extend(nvdec_offer(codec));
        #[cfg(feature = "vaapi")]
        merge(&mut offers, vaapi::offers(codec));
        #[cfg(feature = "qsv")]
        merge(&mut offers, qsv::offers(codec));
        #[cfg(feature = "v4l2")]
        if offers.is_empty()
            && let Ok(found) = v4l2::probe::find(codec)
        {
            offers.push(Offer {
                device: found.device.driver.clone(),
                ways: vec![Way::V4l2],
                gpu: true,
                draws: true,
            });
        }
        if !offers.is_empty() {
            return offers;
        }
    }
    #[cfg(windows)]
    {
        let mut offers: Vec<Offer> = Vec::new();
        #[cfg(feature = "amf")]
        merge(&mut offers, amf::offers(codec));
        #[cfg(feature = "qsv")]
        merge(&mut offers, qsv::offers(codec));
        if !offers.is_empty() {
            return offers;
        }
    }
    let _ = codec;
    let built = cfg!(any(
        feature = "vaapi",
        all(feature = "nvdec", any(target_os = "linux", windows)),
        all(feature = "videotoolbox", target_vendor = "apple"),
        all(feature = "v4l2", target_os = "linux"),
        all(feature = "android", target_os = "android")
    ));
    if !built {
        return Vec::new();
    }
    vec![Offer {
        device: String::new(),
        ways: vec![Way::Auto],
        gpu: true,
        draws: true,
    }]
}

#[cfg(all(feature = "nvdec", target_os = "linux"))]
fn nvdec_offer(codec: Codec) -> Option<Offer> {
    let context = nvdec::cu::Context::open().ok()?;
    let which = match codec {
        Codec::Mpeg2 => nvdec::cu::CUDA_VIDEO_CODEC_MPEG2,
        Codec::H264 => nvdec::cu::CUDA_VIDEO_CODEC_H264,
        Codec::Hevc => nvdec::cu::CUDA_VIDEO_CODEC_HEVC,
    };
    (context.caps(which, 8).ok()?.supported != 0).then(|| Offer {
        device: context.name.clone(),
        ways: vec![Way::Auto, Way::Nvdec],
        gpu: true,
        draws: true,
    })
}

#[cfg(all(windows, feature = "mediafoundation"))]
fn system_offers(codec: Codec) -> Vec<Offer> {
    mediafoundation::usable(codec)
        .into_iter()
        .map(|name| Offer {
            device: name,
            ways: vec![Way::System],
            gpu: false,
            draws: false,
        })
        .collect()
}

#[cfg(all(
    not(windows),
    any(
        all(feature = "videotoolbox", target_vendor = "apple"),
        all(feature = "android", target_os = "android")
    )
))]
fn system_offers(_codec: Codec) -> Vec<Offer> {
    vec![Offer {
        device: String::new(),
        ways: vec![Way::System],
        gpu: false,
        draws: false,
    }]
}

#[cfg(not(any(
    all(windows, feature = "mediafoundation"),
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "android", target_os = "android")
)))]
fn system_offers(_codec: Codec) -> Vec<Offer> {
    Vec::new()
}

#[cfg(any(
    all(
        windows,
        any(feature = "d3d12", feature = "mediafoundation", feature = "amf")
    ),
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "qsv", any(windows, target_os = "linux"))
))]
pub(crate) fn semi_planar(
    luma: (&[u8], usize),
    chroma: (&[u8], usize),
    width: usize,
    height: usize,
    wide: bool,
) -> Option<[video::Plane; 3]> {
    let (luma, luma_pitch) = luma;
    let (chroma, chroma_pitch) = chroma;
    let half_w = width.div_ceil(2);
    let half_h = height.div_ceil(2);
    let size = if wide { 2 } else { 1 };
    let sample = |bytes: &[u8], at: usize| -> Option<[u8; 2]> {
        let value = u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) >> 6;
        Some(value.to_le_bytes())
    };
    let mut y = Vec::with_capacity(width * height * size);
    for row in 0..height {
        let at = row * luma_pitch;
        if wide {
            for x in 0..width {
                y.extend_from_slice(&sample(luma, at + x * 2)?);
            }
        } else {
            y.extend_from_slice(luma.get(at..at + width)?);
        }
    }
    let mut blue = Vec::with_capacity(half_w * half_h * size);
    let mut red = Vec::with_capacity(half_w * half_h * size);
    for row in 0..half_h {
        let at = row * chroma_pitch;
        if wide {
            for x in 0..half_w {
                blue.extend_from_slice(&sample(chroma, at + x * 4)?);
                red.extend_from_slice(&sample(chroma, at + x * 4 + 2)?);
            }
        } else {
            let pairs = chroma.get(at..at + half_w * 2)?;
            for [b, r] in pairs.as_chunks::<2>().0 {
                blue.push(*b);
                red.push(*r);
            }
        }
    }
    let plane = |data, width, height| video::Plane {
        data,
        stride: width * size,
        width,
        height,
    };
    Some([
        plane(y, width, height),
        plane(blue, half_w, half_h),
        plane(red, half_w, half_h),
    ])
}

#[cfg(any(
    all(windows, feature = "d3d12"),
    all(feature = "nvdec", any(target_os = "linux", windows))
))]
pub(crate) fn frame_of(read: &Readback) -> Option<Frame> {
    let about = read.about;
    let (luma, luma_pitch, _) = read.planes.first()?;
    let (chroma, chroma_pitch, _) = read.planes.get(1)?;
    let planes = semi_planar(
        (luma, *luma_pitch),
        (chroma, *chroma_pitch),
        about.format.width as usize,
        about.format.height as usize,
        about.format.depth > 8,
    )?;
    Some(Frame {
        tag: about.tag,
        format: about.format,
        planes,
        structure: about.structure,
        top_field_first: about.top_field_first,
        repeat_first_field: about.repeat_first_field,
        progressive: about.progressive,
    })
}

#[cfg(all(feature = "mediafoundation", windows))]
pub fn system(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(mediafoundation::MediaFoundation::open(
        codec, framing,
    )?))
}

#[cfg(all(feature = "videotoolbox", target_vendor = "apple"))]
pub fn system(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(videotoolbox::VideoToolbox::open_software(
        codec, framing,
    )?))
}

#[cfg(all(feature = "android", target_os = "android"))]
pub fn system(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(android::Android::open_software(codec, framing)?))
}

#[cfg(not(any(
    all(feature = "mediafoundation", windows),
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "android", target_os = "android")
)))]
pub fn system(codec: Codec, _framing: Framing) -> Result<Box<dyn Decode>, Error> {
    let _ = codec;
    Err(Error::Unavailable(String::from(
        "このビルドは OS のソフトウェアデコーダーに対応していない",
    )))
}

fn said_once(codec: Codec, why: &str) {
    static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let line = format!("デコーダー {codec:?}: ソフトウェア（{why}）");
    let Ok(mut said) = SAID.lock() else {
        return;
    };
    if said.contains(&line) {
        return;
    }
    tracing::info!("{line}");
    said.push(line);
}

#[cfg(all(feature = "v4l2", target_os = "linux", not(feature = "vaapi")))]
fn platform_with(
    codec: Codec,
    framing: Framing,
    by: DeinterlaceBy,
) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(v4l2::V4l2::open_with(codec, framing, by)?))
}

#[cfg(all(
    feature = "v4l2",
    feature = "vaapi",
    target_os = "linux",
    not(all(feature = "android", target_os = "android"))
))]
fn platform_with(
    codec: Codec,
    framing: Framing,
    by: DeinterlaceBy,
) -> Result<Box<dyn Decode>, Error> {
    match vaapi::Vaapi::open(codec, framing) {
        Ok(decoder) => {
            if by == DeinterlaceBy::Hardware {
                tracing::info!("VA-API ではハードウェアのデインターレースに対応していない");
            }
            Ok(Box::new(decoder))
        }
        // VA-API の理由も残す（V4L2 の理由だけでは VA-API の環境で何が足りないか分からない）。
        Err(Error::Unavailable(vaapi)) => match v4l2::V4l2::open_with(codec, framing, by) {
            Ok(decoder) => Ok(Box::new(decoder)),
            Err(Error::Unavailable(v4l2)) => {
                Err(Error::Unavailable(format!("VA-API: {vaapi}。V4L2: {v4l2}")))
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}

#[cfg(not(all(feature = "v4l2", target_os = "linux")))]
fn platform_with(
    codec: Codec,
    framing: Framing,
    by: DeinterlaceBy,
) -> Result<Box<dyn Decode>, Error> {
    if by == DeinterlaceBy::Hardware {
        tracing::info!("このビルドはハードウェアのデインターレースに対応していない");
    }
    platform(codec, framing)
}

#[cfg(all(feature = "android", target_os = "android"))]
fn platform(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(android::Android::open(codec, framing)?))
}

#[cfg(all(
    feature = "vaapi",
    not(all(feature = "v4l2", target_os = "linux")),
    not(all(feature = "android", target_os = "android"))
))]
fn platform(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    after_vaapi(vaapi::Vaapi::open(codec, framing), codec, framing)
}

#[cfg(all(
    feature = "vaapi",
    not(all(feature = "v4l2", target_os = "linux")),
    not(all(feature = "android", target_os = "android"))
))]
fn after_vaapi(
    first: Result<vaapi::Vaapi, Error>,
    _codec: Codec,
    _framing: Framing,
) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(first?))
}

#[cfg(all(
    feature = "d3d12",
    windows,
    not(feature = "vaapi"),
    not(all(feature = "v4l2", target_os = "linux")),
    not(all(feature = "android", target_os = "android"))
))]
fn platform(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    after_d3d12(d3d12::D3d12::open(codec, framing), codec, framing)
}

/// D3D12 が先（リソースをそのままレンダリングできる）。D3D12 が対応しないコーデックは NVDEC が受け持つ。
#[cfg(all(feature = "d3d12", feature = "nvdec", windows))]
fn after_d3d12(
    first: Result<d3d12::D3d12, Error>,
    codec: Codec,
    framing: Framing,
) -> Result<Box<dyn Decode>, Error> {
    match first {
        Ok(decoder) => Ok(Box::new(decoder)),
        Err(Error::Unavailable(d3d12)) => match nvdec::Nvdec::open(codec, framing) {
            Ok(decoder) => Ok(Box::new(decoder)),
            Err(Error::Unavailable(nvdec)) => Err(Error::Unavailable(format!(
                "D3D12: {d3d12}。NVDEC: {nvdec}"
            ))),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}

#[cfg(all(feature = "d3d12", not(feature = "nvdec"), windows))]
fn after_d3d12(
    first: Result<d3d12::D3d12, Error>,
    _codec: Codec,
    _framing: Framing,
) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(first?))
}

#[cfg(all(
    feature = "videotoolbox",
    target_vendor = "apple",
    not(feature = "vaapi"),
    not(all(feature = "v4l2", target_os = "linux")),
    not(all(feature = "d3d12", windows)),
    not(all(feature = "android", target_os = "android"))
))]
fn platform(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(videotoolbox::VideoToolbox::open(codec, framing)?))
}

#[cfg(all(
    feature = "nvdec",
    any(target_os = "linux", windows),
    not(feature = "vaapi"),
    not(all(feature = "v4l2", target_os = "linux")),
    not(all(feature = "d3d12", windows)),
    not(all(feature = "android", target_os = "android"))
))]
fn platform(codec: Codec, framing: Framing) -> Result<Box<dyn Decode>, Error> {
    Ok(Box::new(nvdec::Nvdec::open(codec, framing)?))
}

#[cfg(not(any(
    feature = "vaapi",
    all(feature = "nvdec", any(target_os = "linux", windows)),
    all(feature = "videotoolbox", target_vendor = "apple"),
    all(feature = "v4l2", target_os = "linux"),
    all(feature = "d3d12", windows),
    all(feature = "android", target_os = "android")
)))]
fn platform(codec: Codec, _framing: Framing) -> Result<Box<dyn Decode>, Error> {
    let _ = codec;
    Err(Error::Unavailable(String::from(
        "このビルドはハードウェアデコーダーに対応していない",
    )))
}

#[cfg(all(
    test,
    not(any(
        all(feature = "mediafoundation", windows),
        all(feature = "videotoolbox", target_vendor = "apple"),
        all(feature = "android", target_os = "android")
    ))
))]
mod tests {
    use super::*;

    #[test]
    fn system_without_a_path_falls_to_the_software_one() {
        assert!(matches!(
            system(Codec::Mpeg2, Framing::StartCodes),
            Err(Error::Unavailable(_))
        ));
        let decoder = open(Codec::Mpeg2, Framing::StartCodes, Prefer::System);
        assert_eq!(decoder.name(), "ZeroWave MPEG2 Video Decoder");
        assert!(!decoder.hardware());
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn a_pick_reads_back_as_written() {
        for spelled in [
            "auto",
            "system",
            "d3d12:AMD Radeon(TM) Graphics",
            "nvdec:NVIDIA GeForce RTX 2060:adaptive",
            "nvdec::bob",
            "system:名前:に:コロン",
        ] {
            let pick = Pick::parse(spelled).expect(spelled);
            assert_eq!(pick.spelling(), spelled);
        }
        let pick = Pick::parse("nvdec:NVIDIA GeForce RTX 2060:adaptive").unwrap();
        assert_eq!(pick.device, "NVIDIA GeForce RTX 2060");
        assert_eq!(pick.deinterlace, Some(HwDeinterlace::Adaptive));
        assert_eq!(Pick::parse("nvdec::bob").unwrap().device, "");
        assert!(Pick::parse("dxva2").is_none());
    }
}
