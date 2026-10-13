// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! VideoToolbox：macOS のデコーダー。パラメータセットもストリームに入れて渡す。
//! H.264・HEVC は 4 バイトの長さプレフィックス付き、MPEG-2 はスタートコードのまま渡す。
//! フレームはデコード順で出るので表示順は `order` が決める。ハードウェアは要求しない（MPEG-2 も通す）。

mod gate;
use super::order;
pub mod vt;

use std::ffi::c_void;

use mediadecoder::es::{Codec, Format, Framing, Nals, Picture, Structure};

use super::{About, Decode, Decoded, Error, Held as Where, Readback, Surface};
use crate::trace::Trace;
use vt::{CVPixelBufferRef, OSStatus, Owned, PixelBuffer};

static SAY: Trace = Trace::of("videotoolbox");

/// デコードしたフレーム。コールバックで `CFRetain` した CoreVideo のピクセルバッファ。
pub struct Frame {
    buffer: PixelBuffer,
    pub width: u32,
    pub height: u32,
    /// `'x420'`（P010 と同じ上位ビット寄せ）か。
    pub ten_bit: bool,
}

impl Frame {
    /// 借用で、受け取った側が解放してはならない。
    pub fn pixel_buffer(&self) -> CVPixelBufferRef {
        self.buffer.as_ptr()
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Frame({}x{}, {})",
            self.width,
            self.height,
            if self.ten_bit { "x420" } else { "420v" }
        )
    }
}

/// 出力コールバックが返した一枚：フレームの番号、結果、デコードしたイメージ。
type Answer = (u64, OSStatus, Option<Owned<c_void>>);

/// 出力コールバックが書き込む先。コールバックは別スレッドから来る。
#[derive(Debug, Default)]
struct Sink {
    got: std::sync::Mutex<Vec<Answer>>,
    ready: std::sync::Condvar,
}

/// `refcon` はセッションより長生きする [`Sink`] のアドレス。`image` は借用なので `CFRetain` する。
unsafe extern "C" fn on_decoded(
    refcon: *mut c_void,
    source: *mut c_void,
    status: OSStatus,
    _info: u32,
    image: vt::CVImageBufferRef,
    _presentation: vt::CMTime,
    _duration: vt::CMTime,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` はセッションより長生きする `Sink` のアドレスで、中身はロックの中。
    let sink = unsafe { &*refcon.cast::<Sink>() };
    let tag = source as usize as u64;
    let held = if image.is_null() {
        None
    } else {
        // SAFETY: 借用を `CFRetain` で所有へ変える。対の `CFRelease` は `Owned` の `Drop`。
        unsafe { Owned::new(vt::CFRetain(image.cast_const()).cast_mut()) }
    };
    let mut got = match sink.got.lock() {
        Ok(got) => got,
        Err(poisoned) => poisoned.into_inner(),
    };
    got.push((tag, status, held));
    drop(got);
    sink.ready.notify_all();
}

struct Session {
    raw: Owned<c_void>,
    description: Owned<c_void>,
    format: Format,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Session({:?})", self.format)
    }
}

impl Drop for Session {
    /// 無効化して残りのコールバックを呼び切らせる。手放すのはこのあとの `raw` の `Drop`。
    fn drop(&mut self) {
        // SAFETY: 生きたセッション。
        unsafe { vt::VTDecompressionSessionInvalidate(self.raw.as_ptr()) };
    }
}

/// ストリームで見つけたパラメータセット。識別子の違う PPS が送られることがあるので複数持つ。
#[derive(Debug, Default, PartialEq, Eq, Clone)]
struct Parameters {
    vps: Vec<Vec<u8>>,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
}

const MOST_SETS: usize = 16;

const MOST_COMPLAINTS: usize = 12;

/// 同時にデコードへ送ってよい枚数。ハードウェアデコーダーは並列に処理させて初めて速くなる。
const DEEP: usize = 4;

const WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// 作り直さないと回復しないエラーか。macOS はバックグラウンドに回ったときやスリープの後にセッションを無効にする。
fn lost(status: OSStatus) -> bool {
    matches!(
        status,
        -12903 | vt::kVTVideoDecoderMalfunctionErr | -17690 | -17691
    )
}

impl Parameters {
    fn ready(&self, codec: Codec) -> bool {
        match codec {
            Codec::Mpeg2 => true,
            Codec::H264 => !self.sps.is_empty() && !self.pps.is_empty(),
            Codec::Hevc => !self.vps.is_empty() && !self.sps.is_empty() && !self.pps.is_empty(),
        }
    }

    fn all(&self, codec: Codec) -> Vec<&[u8]> {
        let mut out: Vec<&[u8]> = Vec::new();
        if codec == Codec::Hevc {
            out.extend(self.vps.iter().map(Vec::as_slice));
        }
        out.extend(self.sps.iter().map(Vec::as_slice));
        out.extend(self.pps.iter().map(Vec::as_slice));
        out
    }
}

fn remember(into: &mut Vec<Vec<u8>>, unit: &[u8]) {
    if into.iter().any(|set| set == unit) || into.len() >= MOST_SETS {
        return;
    }
    into.push(unit.to_vec());
}

pub struct VideoToolbox {
    // セッションは `sink` より先に落とす（`Drop` がコールバックを呼び切り、それが `sink` に書く）。
    session: Option<Session>,
    sink: Box<Sink>,
    codec: Codec,
    framing: Framing,
    keys: order::Keys,
    queue: order::Queue<(About, Frame)>,
    parameters: Parameters,
    format: Format,
    hardware: Option<bool>,
    software: bool,
    /// フィールド対の一枚目。二枚目と繋いで一つのサンプルにする。
    half: Option<(u64, About, Vec<u8>)>,
    /// 一枚目のフィールドのパリティ。同じパリティが続いたら対のフィールドは来なかった。
    parity: Option<Structure>,
    pending_key: Option<order::Key>,
    /// 送ってあって、まだ返っていないフレーム。どの要求のものかは番号でしか分からない。
    flight: std::collections::BTreeMap<u64, (About, order::Key)>,
    refused: usize,
    why: Option<String>,
    stale: bool,
    /// HEVC で渡すアクセスユニットを選ぶ（[`gate::Gate`]）。
    gate: Option<gate::Gate>,
}

impl std::fmt::Debug for VideoToolbox {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "VideoToolbox({:?} {:?})", self.codec, self.session)
    }
}

impl VideoToolbox {
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Ok(Self {
            session: None,
            sink: Box::default(),
            codec,
            framing,
            keys: match codec {
                Codec::Mpeg2 => order::Keys::Mpeg2,
                Codec::H264 => order::Keys::H264(Box::default()),
                Codec::Hevc => order::Keys::Hevc(Box::default()),
            },
            queue: order::Queue::new(codec == Codec::Mpeg2),
            parameters: Parameters::default(),
            format: Format::default(),
            hardware: None,
            software: false,
            half: None,
            parity: None,
            pending_key: None,
            flight: std::collections::BTreeMap::new(),
            refused: 0,
            why: None,
            stale: false,
            gate: (codec == Codec::Hevc).then(gate::Gate::default),
        })
    }

    pub fn open_software(codec: Codec, framing: Framing) -> Result<Self, Error> {
        let mut decoder = Self::open(codec, framing)?;
        decoder.software = true;
        Ok(decoder)
    }

    pub fn hardware(&self) -> Option<bool> {
        self.hardware
    }

    fn collect(&mut self, data: &[u8]) {
        if self.codec == Codec::Mpeg2 {
            return;
        }
        for nal in Nals::new(data, self.framing) {
            let Some(unit) = data.get(nal.body.start.saturating_sub(1)..nal.body.end) else {
                continue;
            };
            let slot = match self.codec {
                Codec::H264 => match nal.header & 0x1f {
                    7 => &mut self.parameters.sps,
                    8 => &mut self.parameters.pps,
                    _ => continue,
                },
                Codec::Hevc => match (nal.header >> 1) & 0x3f {
                    32 => &mut self.parameters.vps,
                    33 => &mut self.parameters.sps,
                    34 => &mut self.parameters.pps,
                    _ => continue,
                },
                Codec::Mpeg2 => continue,
            };
            remember(slot, unit);
        }
    }

    /// アクセスユニットを VideoToolbox が取る形に直す。パラメータセットもストリームに入れる
    /// （後から来た PPS でフォーマット記述を作り直すと参照が消えるため）。
    fn coded(&self, data: &[u8]) -> Vec<u8> {
        if self.codec == Codec::Mpeg2 {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(data.len());
        for nal in Nals::new(data, self.framing) {
            let wanted = match self.codec {
                Codec::H264 => matches!(nal.header & 0x1f, 1..=5 | 7 | 8),
                // VCL 0..=31 と VPS・SPS・PPS（32..=34）。SEI と AUD などは渡さない。
                Codec::Hevc => matches!((nal.header >> 1) & 0x3f, 0..=34),
                Codec::Mpeg2 => false,
            };
            if !wanted {
                continue;
            }
            let Some(unit) = data.get(nal.body.start.saturating_sub(1)..nal.body.end) else {
                continue;
            };
            out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
            out.extend_from_slice(unit);
        }
        out
    }

    fn description(&self, format: Format) -> Result<Owned<c_void>, Error> {
        let made = match self.codec {
            Codec::Mpeg2 => {
                // SAFETY: Create で、出力の一つを除いて全て値渡し。
                unsafe {
                    vt::created(|out| {
                        vt::CMVideoFormatDescriptionCreate(
                            vt::kCFAllocatorDefault,
                            vt::kCMVideoCodecType_MPEG2Video,
                            format.width as i32,
                            format.height as i32,
                            std::ptr::null(),
                            out,
                        )
                    })
                }
            }
            codec => {
                let sets = self.parameters.all(codec);
                let pointers: Vec<*const u8> = sets.iter().map(|set| set.as_ptr()).collect();
                let sizes: Vec<usize> = sets.iter().map(|set| set.len()).collect();
                // SAFETY: Create で、二つの配列は同じ長さ。CoreMedia は中身をコピーする。
                unsafe {
                    vt::created(|out| {
                        if codec == Codec::Hevc {
                            vt::CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                                vt::kCFAllocatorDefault,
                                sets.len(),
                                pointers.as_ptr(),
                                sizes.as_ptr(),
                                4,
                                std::ptr::null(),
                                out,
                            )
                        } else {
                            vt::CMVideoFormatDescriptionCreateFromH264ParameterSets(
                                vt::kCFAllocatorDefault,
                                sets.len(),
                                pointers.as_ptr(),
                                sizes.as_ptr(),
                                4,
                                out,
                            )
                        }
                    })
                }
            }
        };
        made.map_err(|status| {
            Error::Unavailable(vt::status("CMVideoFormatDescriptionCreate", status))
        })?
        .ok_or_else(|| Error::Unavailable(String::from("フォーマット記述が空です")))
    }

    fn attributes(&self, ten_bit: bool) -> Result<Owned<c_void>, Error> {
        let kind = if ten_bit {
            vt::kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange
        } else {
            vt::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        };
        let format = vt::number(kind as i32)
            .ok_or_else(|| Error::Unavailable(String::from("CFNumber を作れません")))?;
        // 空の辞書が IOSurface で確保せよという意味（Metal が texture を作れる）。
        // SAFETY: 対が無いのでキーも値も渡さない。
        let surface = unsafe { vt::dictionary(&[]) }
            .ok_or_else(|| Error::Unavailable(String::from("CFDictionary を作れません")))?;
        // SAFETY: キーは外部シンボル、値は生きた所有物と不変の `kCFBooleanTrue`。
        unsafe {
            vt::dictionary(&[
                (
                    vt::kCVPixelBufferPixelFormatTypeKey,
                    format.as_ptr().cast_const(),
                ),
                (
                    vt::kCVPixelBufferIOSurfacePropertiesKey,
                    surface.as_ptr().cast_const(),
                ),
                (vt::kCVPixelBufferMetalCompatibilityKey, vt::kCFBooleanTrue),
            ])
        }
        .ok_or_else(|| Error::Unavailable(String::from("CFDictionary を作れません")))
    }

    fn configure(&mut self, format: Format) -> Result<(), Error> {
        if self.stale {
            self.stale = false;
            tracing::info!(
                "VideoToolbox {:?}: セッションが無効になったので作り直す",
                self.codec
            );
            self.flight.clear();
            let _ = self.harvest(false);
            self.half = None;
            self.parity = None;
            self.pending_key = None;
            self.session = None;
        }
        if let Some(session) = &self.session
            && crate::decode::session_fits(session.format, format)
        {
            return Ok(());
        }
        crate::decode::openable(format)?;
        if !self.parameters.ready(self.codec) {
            return Err(Error::Damaged("a picture with no parameter sets"));
        }
        // 手放す前に未完了のフレームを出し切らせる。`flight` が残ると返らないフレームを待ってしまう。
        self.settle();
        self.session = None;
        let description = self.description(format)?;
        let attributes = self.attributes(format.depth > 8)?;
        let specification = if self.software {
            // SAFETY: キーは外部シンボル、値は不変の `kCFBooleanFalse`。
            Some(
                unsafe {
                    vt::dictionary(&[(
                        vt::kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder,
                        vt::kCFBooleanFalse,
                    )])
                }
                .ok_or_else(|| Error::Unavailable(String::from("CFDictionary を作れません")))?,
            )
        } else {
            None
        };
        let record = vt::VTDecompressionOutputCallbackRecord {
            callback: Some(on_decoded),
            refcon: std::ptr::from_mut(&mut *self.sink).cast(),
        };
        // SAFETY: Create で、引数は呼び出しの間生きている。`Sink` は Box の中で動かず、セッションより長生きする。
        let made = unsafe {
            vt::created(|out| {
                vt::VTDecompressionSessionCreate(
                    vt::kCFAllocatorDefault,
                    description.as_ptr(),
                    specification
                        .as_ref()
                        .map_or(std::ptr::null(), |it| it.as_ptr().cast_const()),
                    attributes.as_ptr().cast_const(),
                    &raw const record,
                    out,
                )
            })
        };
        let session = made
            .map_err(|status| {
                Error::Unavailable(vt::status("VTDecompressionSessionCreate", status))
            })?
            .ok_or_else(|| Error::Unavailable(String::from("セッションが空です")))?;
        let raw = session.as_ptr();
        self.session = Some(Session {
            raw: session,
            description,
            format,
        });
        self.hardware = self.using_hardware(raw);
        self.hold_pool(raw, format);
        if SAY.on() {
            tracing::info!(
                "VideoToolbox {:?}: {} {}x{} {}",
                self.codec,
                if format.depth > 8 { "x420" } else { "420v" },
                format.width,
                format.height,
                match self.hardware {
                    Some(true) => "ハードウェア",
                    Some(false) => "ソフトウェア（VideoToolbox 内）",
                    None => "ハードウェアかどうか不明",
                }
            );
        }
        Ok(())
    }

    /// 出力プールの枚数を、レンダリング側が保持しうる枚数より小さくさせない（作り直したサーフェスは初回の読み込みが重い）。
    fn hold_pool(&self, session: vt::VTDecompressionSessionRef, format: Format) {
        const REFERENCES: usize = 8;
        const MEMORY: u64 = 512 << 20;
        let bytes = u64::from(format.width)
            * u64::from(format.height)
            * if format.depth > 8 { 2 } else { 1 }
            * 3
            / 2;
        let count = (crate::decode::MOST_HELD + REFERENCES).min((MEMORY / bytes.max(1)) as usize);
        let Some(value) = vt::number(count as i32) else {
            return;
        };
        // SAFETY: セッションは生きていて、キーは VideoToolbox の外部シンボル、値は生きた CFNumber。
        let status = unsafe {
            vt::VTSessionSetProperty(
                session,
                vt::kVTDecompressionPropertyKey_OutputPoolRequestedMinimumBufferCount,
                value.as_ptr().cast_const(),
            )
        };
        if SAY.on() {
            tracing::info!("VideoToolbox: 出力プールの最小枚数 {count}（{status}）");
        }
    }

    fn using_hardware(&self, session: vt::VTDecompressionSessionRef) -> Option<bool> {
        // SAFETY: Copy で、セッションは生きていて、キーは外部シンボル。
        let value = unsafe {
            vt::created(|out: *mut *mut c_void| {
                vt::VTSessionCopyProperty(
                    session,
                    vt::kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                    vt::kCFAllocatorDefault,
                    out.cast(),
                )
            })
        }
        .ok()??;
        // SAFETY: キーの値は CFBoolean。読んでから `Owned` が手放す。
        Some(unsafe { vt::CFBooleanGetValue(value.as_ptr()) } != 0)
    }

    fn submit(
        &mut self,
        tag: u64,
        about: About,
        key: order::Key,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let (session, description) = match self.session.as_ref() {
            Some(session) => (session.raw.as_ptr(), session.description.as_ptr()),
            None => return Err(Error::Unavailable(String::from("セッションがありません"))),
        };
        if bytes.is_empty() {
            return Ok(());
        }
        // 待たないので、この関数を抜けたあとも VT が読む。CoreMedia に確保させてコピーする。
        // SAFETY: Create で、`memory` が空で `block_allocator` が既定なら、CoreMedia が確保し解放する。
        let made = unsafe {
            vt::created(|out| {
                vt::CMBlockBufferCreateWithMemoryBlock(
                    vt::kCFAllocatorDefault,
                    std::ptr::null_mut(),
                    bytes.len(),
                    vt::kCFAllocatorDefault,
                    std::ptr::null(),
                    0,
                    bytes.len(),
                    vt::ASSURE_MEMORY_NOW,
                    out,
                )
            })
        };
        let block = match made {
            Err(status) => {
                return Err(self.refuse(
                    tag,
                    vt::status("CMBlockBufferCreateWithMemoryBlock", status),
                ));
            }
            Ok(None) => return Err(self.refuse(tag, String::from("ブロックバッファが null"))),
            Ok(Some(block)) => block,
        };
        // SAFETY: いま確保させたブロックへ、同じ長さだけコピーする。
        let status = unsafe {
            vt::CMBlockBufferReplaceDataBytes(bytes.as_ptr().cast(), block.as_ptr(), 0, bytes.len())
        };
        if status != 0 {
            return Err(self.refuse(tag, vt::status("CMBlockBufferReplaceDataBytes", status)));
        }
        // SAFETY: Create で、ブロックと形式記述は生きている。並べ替えをさせないので時刻は渡さない。
        let made = unsafe {
            vt::created(|out| {
                vt::CMSampleBufferCreate(
                    vt::kCFAllocatorDefault,
                    block.as_ptr(),
                    1,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    description,
                    1,
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    out,
                )
            })
        };
        let sample = match made {
            Err(status) => {
                return Err(self.refuse(tag, vt::status("CMSampleBufferCreate", status)));
            }
            Ok(None) => return Err(self.refuse(tag, String::from("サンプルバッファが null"))),
            Ok(Some(sample)) => sample,
        };
        // 一枚ごとに待たない。待つとハードウェアデコーダーが一枚ずつ止まる。
        self.flight.insert(tag, (about, key));
        // SAFETY: セッションとサンプルは生きている。`sourceFrameRefCon` はアドレスとして使われない。
        let status = unsafe {
            vt::VTDecompressionSessionDecodeFrame(
                session,
                sample.as_ptr(),
                vt::DECODE_ASYNCHRONOUSLY,
                tag as usize as *mut c_void,
                std::ptr::null_mut(),
            )
        };
        drop(sample);
        drop(block);
        if status != 0 {
            self.flight.remove(&tag);
            self.stale |= lost(status);
            return Err(self.refuse(tag, vt::status("VTDecompressionSessionDecodeFrame", status)));
        }
        self.harvest(false)
    }

    /// 返ってきたフレームを受け取る。送りすぎたら一枚返るまで待つ。`all` はストリームの終わり。
    fn harvest(&mut self, all: bool) -> Result<(), Error> {
        let mut got = {
            let mut held = match self.sink.got.lock() {
                Ok(held) => held,
                Err(poisoned) => poisoned.into_inner(),
            };
            while held.is_empty() && (all || self.flight.len() > DEEP) {
                let (next, timeout) = match self.sink.ready.wait_timeout(held, WAIT) {
                    Ok(pair) => pair,
                    Err(poisoned) => poisoned.into_inner(),
                };
                held = next;
                if timeout.timed_out() {
                    break;
                }
            }
            std::mem::take(&mut *held)
        };
        let mut trouble = None;
        for (tag, status, buffer) in got.drain(..) {
            let known = self.flight.remove(&tag);
            let Some(buffer) = buffer.filter(|_| status == 0) else {
                self.stale |= lost(status);
                trouble.get_or_insert((tag, vt::status("出力コールバック", status)));
                continue;
            };
            // 要求していないフレームは捨てる。
            let Some((about, key)) = known else {
                continue;
            };
            match self.frame(buffer) {
                Ok(frame) => self.queue.push(key, (about, frame)),
                Err(error) => {
                    trouble.get_or_insert((tag, format!("{error:?}")));
                }
            }
        }
        match trouble {
            Some((tag, what)) => Err(self.refuse(tag, what)),
            None => Ok(()),
        }
    }

    fn settle(&mut self) {
        if let Some(session) = self.session.as_ref() {
            // SAFETY: 生きているセッション。抜けた時点でコールバックは呼び終わっている。
            unsafe { vt::VTDecompressionSessionWaitForAsynchronousFrames(session.raw.as_ptr()) };
        }
        let _ = self.harvest(false);
        self.flight.clear();
    }

    fn refuse(&mut self, tag: u64, what: String) -> Error {
        self.refused += 1;
        if self.refused <= MOST_COMPLAINTS {
            tracing::info!("VideoToolbox {:?}: tag {tag} {what}", self.codec);
        }
        if self.why.is_none() {
            self.why = Some(what);
        }
        Error::Damaged("VideoToolbox がこのアクセスユニットを拒否しました")
    }

    pub fn refused(&self) -> usize {
        self.refused
    }

    pub fn why(&self) -> Option<&str> {
        self.why.as_deref()
    }

    fn frame(&self, buffer: Owned<c_void>) -> Result<Frame, Error> {
        // SAFETY: デコードの出力コールバックが渡すのは CVPixelBuffer。
        let owned = unsafe { PixelBuffer::new(buffer) };
        let (kind, width, height) = (owned.format(), owned.width(), owned.height());
        let ten_bit = kind == vt::kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange;
        if !ten_bit && kind != vt::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
            return Err(Error::Unsupported("a pixel format nothing here reads"));
        }
        if (width as u32) < self.format.width || (height as u32) < self.format.height {
            return Err(Error::Damaged("a picture smaller than the stream declared"));
        }
        Ok(Frame {
            buffer: owned,
            width: self.format.width,
            height: self.format.height,
            ten_bit,
        })
    }

    fn read(frame: &Frame) -> Option<Vec<(Vec<u8>, usize, usize)>> {
        let locked = frame.buffer.lock_read()?;
        let planes = (0..locked.planes())
            .filter_map(|plane| locked.plane(plane))
            .map(|(bytes, pitch, rows)| (bytes.to_vec(), pitch, rows))
            .collect();
        Some(planes)
    }

    fn about(&self, tag: u64, picture: &Picture, format: Format) -> About {
        About {
            tag,
            format,
            structure: picture.structure,
            top_field_first: picture.top_field_first,
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
        }
    }
}

impl Decode for VideoToolbox {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        self.format = format;
        self.collect(data);
        if let Some(gate) = self.gate.as_mut()
            && !gate.admits(self.framing, data)
        {
            tracing::debug!("VideoToolbox: 参照がそろわないピクチャーは渡さない（tag {tag}）");
            return Ok(());
        }
        self.configure(format)?;
        let key = self.keys.key(picture, self.framing, data);
        let about = self.about(tag, picture, format);
        let coded = self.coded(data);
        if !picture.structure.field() && self.half.take().is_some() {
            // 対のフィールドが来なかったフィールドは捨てる（フレームとフィールドが混ざることがある）。
            self.parity = None;
            self.pending_key = None;
        }
        if picture.structure.field() {
            match self.half.take() {
                Some((first, first_about, mut bytes)) if self.parity != Some(picture.structure) => {
                    bytes.extend_from_slice(&coded);
                    self.parity = None;
                    let key = self.pending_key.take().unwrap_or(key);
                    return self.submit(first, first_about, key, &bytes);
                }
                _ => {
                    self.half = Some((tag, about, coded));
                    self.parity = Some(picture.structure);
                    self.pending_key = Some(key);
                    return Ok(());
                }
            }
        }
        self.submit(tag, about, key, &coded)
    }

    fn name(&self) -> String {
        String::from(if self.software {
            "VideoToolbox（ソフトウェア）"
        } else {
            "VideoToolbox"
        })
    }

    fn hardware(&self) -> bool {
        !self.software
    }

    fn take(&mut self) -> Option<Decoded> {
        let (about, frame) = self.queue.take()?;
        Some(Decoded::Surface(Surface {
            about,
            held: Where::VideoToolbox(frame),
        }))
    }

    fn read_back(&mut self) -> Option<Readback> {
        let (about, frame) = self.queue.take()?;
        // 読めなかったフレームを黙って捨てると、動いているデコーダーが何も出さないように見える。
        let Some(planes) = Self::read(&frame) else {
            self.refused += 1;
            if self.why.is_none() {
                let what = String::from("CVPixelBufferLockBaseAddress に失敗");
                tracing::info!("VideoToolbox {:?}: {what}", self.codec);
                self.why = Some(what);
            }
            return None;
        };
        Some(Readback { about, planes })
    }

    fn flush(&mut self) {
        self.settle();
        self.queue.drain();
    }

    fn reset(&mut self) {
        self.settle();
        self.queue.clear();
        self.keys.reset();
        if let Some(gate) = self.gate.as_mut() {
            gate.reset();
        }
        self.half = None;
        self.parity = None;
        self.pending_key = None;
    }
}

#[cfg(test)]
mod lost_tests {
    use super::{lost, vt};

    #[test]
    fn only_a_dead_session_is_rebuilt() {
        assert!(lost(-12903), "kVTInvalidSessionErr");
        assert!(lost(vt::kVTVideoDecoderMalfunctionErr));
        assert!(lost(-17690), "kVTVideoDecoderRemovedErr");
        assert!(lost(-17691), "kVTSessionMalfunctionErr");
        assert!(!lost(0));
        assert!(!lost(vt::kVTVideoDecoderBadDataErr));
        assert!(!lost(-17694), "参照が欠けただけ");
    }
}
