// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MediaCodec デコーダーの Android 側。NDK の C API をラップし、その `unsafe` を引き受ける。

use std::sync::Arc;

use mediadecoder::es::{Codec, Format, Structure};
use mediadecoder::video::{Frame as Picture, Plane};
use ndk::hardware_buffer::{HardwareBuffer, HardwareBufferUsage};
use ndk::media::media_codec::OutputBuffer;
use ndk::media::media_format::MediaFormat;
use ndk::native_window::NativeWindow;

use crate::decode::Error;

pub const FLAG_END_OF_STREAM: u32 = 4;

/// `COLOR_FormatYUV420Flexible`。最後に指定する（先に指定すると独自のレイアウトで返す）。
pub const COLOR_FORMAT_YUV420_FLEXIBLE: i32 = 0x7f42_0888;

pub const COLOR_FORMAT_YUV420_PLANAR: i32 = 19;

pub const COLOR_FORMAT_YUV420_SEMI_PLANAR: i32 = 21;

/// Qualcomm のセミプレーナー（NV12 でアライメントが独自）。アライメントは `stride` と `slice-height` から読む。
const COLOR_FORMAT_QCOM_SEMI_PLANAR: i32 = 0x7fa3_0c04;

pub const COLOR_FORMAT_YUV_P010: i32 = 54;

const IMAGE_FORMAT_YUV_420_888: i32 = 0x23;

/// `AIMAGE_FORMAT_YCBCR_P010`。API 31 未満では開けない。
const IMAGE_FORMAT_P010: i32 = 0x36;

pub fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::Mpeg2 => "video/mpeg2",
        Codec::H264 => "video/avc",
        Codec::Hevc => "video/hevc",
    }
}

pub fn to_start_codes(data: &[u8], out: &mut Vec<u8>) {
    out.clear();
    let mut at = 0;
    while at + 4 <= data.len() {
        let length =
            u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        at += 4;
        let end = (at + length).min(data.len());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[at..end]);
        at = end;
    }
}

/// メディアフレームワークのオブジェクトをスレッド間で移すラッパー。一度に一つのスレッドからだけ触る。
pub struct Send1<T>(T);

impl<T> Send1<T> {
    /// # Safety
    ///
    /// The caller must not share the value across threads or call it from two at once.
    pub unsafe fn new(value: T) -> Self {
        Self(value)
    }
}

// SAFETY: `Send1::new` の前提を引き継ぐ。AMediaCodec などはスレッドに縛られない。
unsafe impl<T> Send for Send1<T> {}
// SAFETY: 同上。`&T` で触るのはフレームワーク自身が同期する物だけ。
unsafe impl<T> Sync for Send1<T> {}

impl<T> std::ops::Deref for Send1<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::fmt::Debug for Send1<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Send1")
    }
}

/// AImageReader から受けたフレーム。破棄するとバッファーが AImageReader に返る。
/// 失敗の `media_status_t` をそのまま返すため、生の呼び出しで触る。
pub struct Image {
    image: Send1<std::ptr::NonNull<ndk_sys::AImage>>,
    /// フレームはデコーダーより長く生きうるので、最後に破棄される方が解放する。
    _reader: Arc<Reader>,
}

impl Image {
    fn as_ptr(&self) -> *mut ndk_sys::AImage {
        self.image.as_ptr()
    }

    fn planes(&self) -> Result<i32, i32> {
        let mut count = 0i32;
        // SAFETY: この構造体がフレームを持つ間の呼び出しで、`count` は生きたローカル変数。
        let status = unsafe { ndk_sys::AImage_getNumberOfPlanes(self.as_ptr(), &mut count) };
        ok(status).map(|()| count)
    }

    fn plane(&self, plane: i32) -> Result<(&[u8], usize, usize), Error> {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut length = 0i32;
        // SAFETY: フレームは生きていて、出力の引数はローカル変数。スライスは `&self` に縛られる。
        let status =
            unsafe { ndk_sys::AImage_getPlaneData(self.as_ptr(), plane, &mut data, &mut length) };
        ok(status)
            .map_err(|status| Error::Unavailable(said("AImage_getPlaneData", plane, status)))?;
        // 成功で null が返るのは、gralloc が輝度だけ埋めて色差を 0 のまま残したとき。
        if data.is_null() || length <= 0 {
            let (row, step) = self.strides(plane);
            return Err(Error::Unavailable(format!(
                "AImage_getPlaneData プレーン{plane} は成功しましたが{}（長さ {length}、行送り {row:?}、サンプル送り {step:?}）",
                if data.is_null() {
                    "、プレーンの先頭が null です。gralloc のマッパーが輝度しか書いていません"
                } else {
                    "、長さが 0 です"
                }
            )));
        }
        let pitch = self.row_stride(plane).map_err(|status| {
            Error::Unavailable(said("AImage_getPlaneRowStride", plane, status))
        })?;
        let step = self.pixel_stride(plane).map_err(|status| {
            Error::Unavailable(said("AImage_getPlanePixelStride", plane, status))
        })?;
        // SAFETY: フレームワークが返したマッピングで、`self` の借用がフレームを生かす。
        let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) };
        Ok((bytes, pitch.max(1) as usize, step.max(1) as usize))
    }

    fn row_stride(&self, plane: i32) -> Result<i32, i32> {
        let mut row = 0i32;
        // SAFETY: フレームは生きていて、`row` はローカル変数。
        let status = unsafe { ndk_sys::AImage_getPlaneRowStride(self.as_ptr(), plane, &mut row) };
        ok(status).map(|()| row)
    }

    fn pixel_stride(&self, plane: i32) -> Result<i32, i32> {
        let mut step = 0i32;
        // SAFETY: フレームは生きていて、`step` はローカル変数。
        let status =
            unsafe { ndk_sys::AImage_getPlanePixelStride(self.as_ptr(), plane, &mut step) };
        ok(status).map(|()| step)
    }

    fn strides(&self, plane: i32) -> (Option<i32>, Option<i32>) {
        (self.row_stride(plane).ok(), self.pixel_stride(plane).ok())
    }

    fn buffer(&self) -> Option<HardwareBuffer> {
        let mut buffer: *mut ndk_sys::AHardwareBuffer = std::ptr::null_mut();
        // SAFETY: フレームは生きていて、`buffer` はローカル変数。
        let status = unsafe { ndk_sys::AImage_getHardwareBuffer(self.as_ptr(), &mut buffer) };
        ok(status).ok()?;
        let buffer = std::ptr::NonNull::new(buffer)?;
        // SAFETY: 所有権を持たない handle を作る。バッファーはフレームが持ち、フレームは `self` の借用で生きる。
        Some(unsafe { HardwareBuffer::from_ptr(buffer.cast()) })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: この構造体が取ったフレームを一度だけ解放する。
        unsafe { ndk_sys::AImage_delete(self.as_ptr()) };
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Image")
    }
}

fn ok(status: ndk_sys::media_status_t) -> Result<(), i32> {
    if status.0 == 0 { Ok(()) } else { Err(status.0) }
}

pub enum Got {
    Image(Image),
    None,
    Full,
    Failed(i32),
}

struct Reader(Send1<std::ptr::NonNull<ndk_sys::AImageReader>>);

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: `Images::open` で作り、`Images` とすべての `Image` が消えた後に一度だけ手放す。
        unsafe { ndk_sys::AImageReader_delete(self.0.as_ptr()) };
    }
}

pub struct Images {
    reader: Arc<Reader>,
    window: NativeWindow,
}

impl std::fmt::Debug for Images {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Images")
    }
}

impl Images {
    /// AImageReader を開く（指定しても色差が読めない端末はある）。
    pub fn open(format: Format, count: i32) -> Result<Self, Error> {
        let shape = if format.depth > 8 {
            IMAGE_FORMAT_P010
        } else {
            IMAGE_FORMAT_YUV_420_888
        };
        let (width, height) = Self::size(format);
        let mut reader: *mut ndk_sys::AImageReader = std::ptr::null_mut();
        // SAFETY: 引数は値かローカル変数で、作った AImageReader は下の構造体が持つ。
        let status = unsafe {
            ndk_sys::AImageReader_newWithUsage(
                width as i32,
                height as i32,
                shape,
                Self::usage().bits(),
                count,
                &mut reader,
            )
        };
        ok(status).map_err(|status| {
            Error::Unavailable(format!(
                "AImageReader_newWithUsage → media_status_t {status}（{}）",
                name_of(status).unwrap_or("名前の分からない値")
            ))
        })?;
        let reader = std::ptr::NonNull::new(reader)
            .ok_or_else(|| Error::Unavailable(String::from("AImageReader が null です")))?;
        let mut window: *mut ndk_sys::ANativeWindow = std::ptr::null_mut();
        // SAFETY: AImageReader は作ったばかりで、`window` はローカル変数。
        let status = unsafe { ndk_sys::AImageReader_getWindow(reader.as_ptr(), &mut window) };
        let window = ok(status)
            .ok()
            .and_then(|()| std::ptr::NonNull::new(window))
            .ok_or_else(|| {
                // SAFETY: AImageReader はここだけが持つので、ここで解放する。
                unsafe { ndk_sys::AImageReader_delete(reader.as_ptr()) };
                Error::Unavailable(String::from("AImageReader_getWindow が返りません"))
            })?;
        // SAFETY: ウィンドウは AImageReader が持ち、`clone_from_ptr` が自分の参照を取る。
        let window = unsafe { NativeWindow::clone_from_ptr(window) };
        Ok(Self {
            // SAFETY: `Send1` を見よ。
            reader: Arc::new(Reader(unsafe { Send1::new(reader) })),
            window,
        })
    }

    pub fn usage() -> HardwareBufferUsage {
        HardwareBufferUsage::CPU_READ_OFTEN | HardwareBufferUsage::GPU_SAMPLED_IMAGE
    }

    /// AImageReader のサイズ。ハードウェアはマクロブロック単位で書くので 16 に揃える。
    pub fn size(format: Format) -> (u32, u32) {
        (
            format.width.next_multiple_of(16),
            format.height.next_multiple_of(16),
        )
    }

    pub fn acquire(&self) -> Got {
        let mut image: *mut ndk_sys::AImage = std::ptr::null_mut();
        // SAFETY: AImageReader は生きていて、`image` はローカル変数。返る物はこちらが持つ。
        let status =
            unsafe { ndk_sys::AImageReader_acquireNextImage(self.reader.0.as_ptr(), &mut image) };
        match status.0 {
            0 => match std::ptr::NonNull::new(image) {
                Some(image) => Got::Image(Image {
                    // SAFETY: 渡された物を `Image` が一度だけ消す。
                    image: unsafe { Send1::new(image) },
                    _reader: Arc::clone(&self.reader),
                }),
                None => Got::None,
            },
            -30001 => Got::None,
            -30002 => Got::Full,
            other => Got::Failed(other),
        }
    }

    pub fn window(&self) -> &NativeWindow {
        &self.window
    }
}

pub struct Frame {
    image: Arc<Image>,
    pub width: u32,
    pub height: u32,
    pub ten_bit: bool,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Frame({}x{}{})",
            self.width,
            self.height,
            if self.ten_bit { " 10bit" } else { "" }
        )
    }
}

impl Frame {
    pub(super) fn new(image: Arc<Image>, format: Format) -> Self {
        Self {
            image,
            width: format.width,
            height: format.height,
            ten_bit: format.depth > 8,
        }
    }

    pub fn buffer(&self) -> Option<HardwareBuffer> {
        self.image.buffer()
    }

    /// この AImageReader が生きている間このバッファーを指す値（`AHardwareBuffer_getId` は API 31）。
    pub fn key(&self) -> usize {
        self.buffer().map_or(0, |buffer| buffer.as_ptr() as usize)
    }

    pub fn describe(&self) -> Option<(u32, u32, u32)> {
        let buffer = self.buffer()?;
        let found = buffer.describe();
        Some((found.width, found.height, i32::from(found.format) as u32))
    }

    /// ハードウェアが書いた位置から読むサンプル（輝度、色差二つ）。インターリーブされた色差はここで分ける。
    pub fn read(&self) -> Result<Vec<(Vec<u8>, usize, usize)>, Error> {
        let image = &self.image;
        let count = image
            .planes()
            .map_err(|status| Error::Unavailable(said("AImage_getNumberOfPlanes", 0, status)))?
            .min(3);
        let wide = if self.ten_bit { 2usize } else { 1 };
        let mut out = Vec::new();
        for plane in 0..count {
            let (data, pitch, step) = image.plane(plane)?;
            let (width, height) = if plane == 0 {
                (self.width as usize, self.height as usize)
            } else {
                (
                    (self.width as usize).div_ceil(2),
                    (self.height as usize).div_ceil(2),
                )
            };
            if step == wide {
                out.push((data.to_vec(), pitch, height));
                continue;
            }
            let mut tight = Vec::with_capacity(width * height * wide);
            for y in 0..height {
                for x in 0..width {
                    let at = y * pitch + x * step;
                    for byte in 0..wide {
                        tight.push(data.get(at + byte).copied().unwrap_or(0));
                    }
                }
            }
            out.push((tight, width * wide, height));
        }
        if out.is_empty() {
            return Err(Error::Unavailable(String::from(
                "プレーンが一つもありません",
            )));
        }
        Ok(out)
    }
}

/// MPEG-2 の `csd-0`：シーケンスヘッダーから続く拡張まで。それ以外のスタートコードで止まる。
/// MediaCodec が start で自分で入れるので、CODEC_CONFIG の入力は出さない。
pub fn mpeg2_csd(data: &[u8]) -> Option<std::ops::Range<usize>> {
    const SEQUENCE: u8 = 0xb3;
    const EXTENSION: u8 = 0xb5;
    let mut begin = None;
    let mut at = 0;
    while at + 4 <= data.len() {
        if data[at] != 0 || data[at + 1] != 0 || data[at + 2] != 1 {
            at += 1;
            continue;
        }
        let kind = data[at + 3];
        match begin {
            None if kind == SEQUENCE => begin = Some(at),
            None => {}
            Some(_) if kind == SEQUENCE || kind == EXTENSION => {}
            Some(from) => return Some(from..at),
        }
        at += 4;
    }
    begin.map(|from| from..data.len())
}

/// フレームワークの戻り値（ndk の `MediaError` 経由だと値が変わるので戻り値をそのまま）。
pub fn status(call: &str, status: i32) -> String {
    match name_of(status) {
        Some(name) => format!("{call} → media_status_t {status}（{name}）"),
        None => format!("{call} → media_status_t {status}（名前の分からない値）"),
    }
}

fn said(call: &str, plane: i32, status: i32) -> String {
    match name_of(status) {
        Some(name) => format!("{call} プレーン{plane} → media_status_t {status}（{name}）"),
        None => format!("{call} プレーン{plane} → media_status_t {status}（名前の分からない値）"),
    }
}

fn name_of(status: i32) -> Option<&'static str> {
    Some(match status {
        0 => "AMEDIA_OK",
        -10000 => "AMEDIA_ERROR_UNKNOWN",
        -10001 => "AMEDIA_ERROR_MALFORMED",
        -10002 => "AMEDIA_ERROR_UNSUPPORTED",
        -10003 => "AMEDIA_ERROR_INVALID_OBJECT",
        -10004 => "AMEDIA_ERROR_INVALID_PARAMETER",
        -10005 => "AMEDIA_ERROR_INVALID_OPERATION",
        -10006 => "AMEDIA_ERROR_END_OF_STREAM",
        -10007 => "AMEDIA_ERROR_IO",
        -10008 => "AMEDIA_ERROR_WOULD_BLOCK",
        -30001 => "AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE",
        -30002 => "AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED",
        -30003 => "AMEDIA_IMGREADER_CANNOT_LOCK_IMAGE",
        -30004 => "AMEDIA_IMGREADER_CANNOT_UNLOCK_IMAGE",
        -30005 => "AMEDIA_IMGREADER_IMAGE_NOT_LOCKED",
        1100 => "AMEDIACODEC_ERROR_INSUFFICIENT_RESOURCE",
        1101 => "AMEDIACODEC_ERROR_RECLAIMED",
        _ => return None,
    })
}

/// デコーダー自身のメモリーに書かれた出力バッファーのサンプル。宣言は信じず、試して読む。
pub fn planes(
    buffer: &OutputBuffer<'_>,
    shape: &MediaFormat,
    tag: u64,
    format: Format,
    weave: bool,
) -> Result<Picture, Error> {
    let bytes = buffer.buffer();
    let info = buffer.info();
    let at = info.offset().max(0) as usize;
    let bytes = bytes
        .get(at..)
        .ok_or_else(|| Error::Unavailable(format!("出力バッファが {at} より短いです")))?;
    let colour = shape
        .i32("color-format")
        .unwrap_or(COLOR_FORMAT_YUV420_FLEXIBLE);
    let pitch = shape
        .i32("stride")
        .filter(|value| *value > 0)
        .unwrap_or(format.width as i32) as usize;
    let rows = shape
        .i32("slice-height")
        .filter(|value| *value > 0)
        .unwrap_or(format.height as i32) as usize;
    let width = format.width as usize;
    let height = format.height as usize;
    // サンプルの幅がビット深度と合わないレイアウトは読めない。どちらも変えずにエラーにする。
    if (format.depth > 8) != (colour == COLOR_FORMAT_YUV_P010) {
        return Err(Error::Unavailable(format!(
            "{} ビットのストリームに、デコーダーはカラーフォーマット {colour:#x}（{colour}）で返しました。\
             stride {pitch} / slice-height {rows}",
            format.depth
        )));
    }
    let luma = match colour {
        COLOR_FORMAT_YUV_P010 => read_wide(bytes, 0, pitch, width, height, 2),
        _ => read_plane(bytes, 0, pitch, width, height, 1),
    }
    .ok_or_else(|| Error::Unavailable(String::from("輝度のプレーンが読めません")))?;
    let half_w = width.div_ceil(2);
    let half_h = height.div_ceil(2);
    let after = pitch * rows;
    // 色差をインターリーブのまま渡すと変換の往復が無くなる。呼ぶ側が受けられるときだけ。
    let woven = weave
        && matches!(
            colour,
            COLOR_FORMAT_YUV420_SEMI_PLANAR | COLOR_FORMAT_QCOM_SEMI_PLANAR | COLOR_FORMAT_YUV_P010
        );
    if woven {
        let pair = if colour == COLOR_FORMAT_YUV_P010 {
            4
        } else {
            2
        };
        let chroma = read_woven(bytes, after, pitch, half_w, half_h, pair)
            .ok_or_else(|| Error::Unavailable(String::from("色差のプレーンが読めません")))?;
        return Ok(Picture {
            tag,
            format,
            planes: [luma, chroma, Plane::none()],
            structure: Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: format.progressive,
        });
    }
    let (blue, red) = match colour {
        COLOR_FORMAT_YUV420_PLANAR => {
            let plane = pitch / 2;
            let size = plane * rows.div_ceil(2);
            (
                read_plane(bytes, after, plane, half_w, half_h, 1).ok_or_else(|| {
                    Error::Unavailable(String::from("色差のプレーンが読めません"))
                })?,
                read_plane(bytes, after + size, plane, half_w, half_h, 1).ok_or_else(|| {
                    Error::Unavailable(String::from("色差のプレーンが読めません"))
                })?,
            )
        }
        COLOR_FORMAT_YUV420_SEMI_PLANAR | COLOR_FORMAT_QCOM_SEMI_PLANAR => (
            read_plane(bytes, after, pitch, half_w, half_h, 2)
                .ok_or_else(|| Error::Unavailable(String::from("色差のプレーンが読めません")))?,
            read_plane(bytes, after + 1, pitch, half_w, half_h, 2)
                .ok_or_else(|| Error::Unavailable(String::from("色差のプレーンが読めません")))?,
        ),
        // P010 の 10 ビットは上位にあり、こちらの形式と同じなのでシフトしない。
        COLOR_FORMAT_YUV_P010 => (
            read_wide(bytes, after, pitch, half_w, half_h, 4)
                .ok_or_else(|| Error::Unavailable(String::from("色差のプレーンが読めません")))?,
            read_wide(bytes, after + 2, pitch, half_w, half_h, 4)
                .ok_or_else(|| Error::Unavailable(String::from("色差のプレーンが読めません")))?,
        ),
        // 読めないレイアウト。「もう一度」にすると何も出さないデコーダーに見える。
        other => {
            return Err(Error::Unavailable(format!(
                "読めないカラーフォーマット {other:#x}（{other}）。stride {pitch} / slice-height {rows}",
            )));
        }
    };
    Ok(Picture {
        tag,
        format,
        planes: [luma, blue, red],
        structure: Structure::Frame,
        top_field_first: true,
        repeat_first_field: false,
        progressive: format.progressive,
    })
}

pub fn layout(shape: &MediaFormat) -> String {
    let colour = shape.i32("color-format").unwrap_or(0);
    format!(
        "color-format {colour:#x}（{colour}）  stride {:?}  slice-height {:?}  {:?}x{:?}",
        shape.i32("stride"),
        shape.i32("slice-height"),
        shape.i32("width"),
        shape.i32("height"),
    )
}

fn read_plane(
    bytes: &[u8],
    at: usize,
    pitch: usize,
    width: usize,
    height: usize,
    step: usize,
) -> Option<Plane> {
    if pitch == 0 || width == 0 || height == 0 {
        return None;
    }
    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = at + y * pitch;
        if step == 1 {
            // 一行ずつコピーする（一サンプルずつでは遅い）。
            data.extend_from_slice(bytes.get(row..row + width)?);
            continue;
        }
        for x in 0..width {
            data.push(*bytes.get(row + x * step)?);
        }
    }
    Some(Plane {
        data,
        stride: width,
        width,
        height,
    })
}

/// デコーダーが書いたままの色差のプレーン。`pair` は一組のバイト数（NV12 は 2、P010 は 4）。
fn read_woven(
    bytes: &[u8],
    at: usize,
    pitch: usize,
    width: usize,
    height: usize,
    pair: usize,
) -> Option<Plane> {
    if pitch == 0 || width == 0 || height == 0 {
        return None;
    }
    let row = width * pair;
    let mut data = Vec::with_capacity(row * height);
    if pitch == row {
        data.extend_from_slice(bytes.get(at..at + row * height)?);
    } else {
        for y in 0..height {
            let from = at + y * pitch;
            data.extend_from_slice(bytes.get(from..from + row)?);
        }
    }
    Some(Plane {
        data,
        stride: row,
        width,
        height,
    })
}

/// 直接出力専用のデコーダー。`ndk` の `OutputBuffer` はインデックスを隠すが、出力に残すフレームはインデックスで扱うしかない。
#[derive(Debug)]
struct Raw(std::ptr::NonNull<ndk_sys::AMediaCodec>);

impl Raw {
    fn as_ptr(&self) -> *mut ndk_sys::AMediaCodec {
        self.0.as_ptr()
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: デコーダーを持ち、一度だけ破棄する。
        unsafe { ndk_sys::AMediaCodec_delete(self.0.as_ptr()) };
    }
}

pub struct Direct {
    codec: Arc<Send1<Raw>>,
}

impl Direct {
    pub fn open(name: Option<&str>, mime: &str) -> Result<Self, Error> {
        let text = std::ffi::CString::new(name.unwrap_or(mime)).map_err(|_| {
            Error::Unavailable(String::from("デコーダーの名前に NUL が入っています"))
        })?;
        // SAFETY: 呼び出しの間生きた NUL 終端の文字列。
        let codec = unsafe {
            match name {
                Some(_) => ndk_sys::AMediaCodec_createCodecByName(text.as_ptr()),
                None => ndk_sys::AMediaCodec_createDecoderByType(text.as_ptr()),
            }
        };
        let codec = std::ptr::NonNull::new(codec)
            .ok_or_else(|| Error::Unavailable(format!("{mime} のデコーダーを開けません")))?;
        Ok(Self {
            // SAFETY: `Send1` を見よ。
            codec: Arc::new(unsafe { Send1::new(Raw(codec)) }),
        })
    }

    fn as_ptr(&self) -> *mut ndk_sys::AMediaCodec {
        self.codec.as_ptr()
    }

    pub fn start(&self, format: &MediaFormat, window: &NativeWindow) -> Result<(), Error> {
        // SAFETY: 三つのポインターは呼び出しの間生きている。
        let status = unsafe {
            ndk_sys::AMediaCodec_configure(
                self.as_ptr(),
                format.as_ptr(),
                window.ptr().as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        ok(status)
            .map_err(|status| Error::Unavailable(self::status("AMediaCodec_configure", status)))?;
        // SAFETY: デコーダーは configure 済みで生きている。
        let status = unsafe { ndk_sys::AMediaCodec_start(self.as_ptr()) };
        ok(status).map_err(|status| Error::Unavailable(self::status("AMediaCodec_start", status)))
    }

    pub fn input(&self, patience: std::time::Duration) -> Option<usize> {
        // SAFETY: デコーダーは生きている。待ち時間はマイクロ秒。
        let index = unsafe {
            ndk_sys::AMediaCodec_dequeueInputBuffer(self.as_ptr(), patience.as_micros() as i64)
        };
        (index >= 0).then_some(index as usize)
    }

    pub fn feed(&self, index: usize, bytes: &[u8], tag: u64, flags: u32) -> Result<(), Error> {
        let mut room = 0usize;
        // SAFETY: デコーダーは生きていて、インデックスは今受け取った物、`room` はローカル変数。
        let into = unsafe { ndk_sys::AMediaCodec_getInputBuffer(self.as_ptr(), index, &mut room) };
        let refused = if into.is_null() {
            Some(Error::Unavailable(String::from(
                "AMediaCodec_getInputBuffer が入力バッファを返しません",
            )))
        } else if room < bytes.len() {
            Some(Error::Unsupported(
                "an access unit larger than the codec's buffer",
            ))
        } else {
            None
        };
        if let Some(error) = refused {
            // 取ったバッファーは空のまま返してからエラーにする（返さないと入力バッファが減る）。
            // SAFETY: この呼び出しまでインデックスはこのプロセスが持つ。
            let _ =
                unsafe { ndk_sys::AMediaCodec_queueInputBuffer(self.as_ptr(), index, 0, 0, 0, 0) };
            return Err(error);
        }
        // SAFETY: `into` は `room` バイト書けて `room >= bytes.len()`。重ならない。
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), into, bytes.len()) };
        // SAFETY: この呼び出しまでインデックスはこのプロセスが持つ。
        let status = unsafe {
            ndk_sys::AMediaCodec_queueInputBuffer(self.as_ptr(), index, 0, bytes.len(), tag, flags)
        };
        ok(status).map_err(|status| {
            Error::Unavailable(self::status("AMediaCodec_queueInputBuffer", status))
        })
    }

    /// 出力バッファーをインデックスで受ける。負のインデックスはどれも「まだ無い」。
    pub fn output(&self, patience: std::time::Duration) -> Option<(usize, u32, i64)> {
        let mut info = ndk_sys::AMediaCodecBufferInfo {
            offset: 0,
            size: 0,
            presentationTimeUs: 0,
            flags: 0,
        };
        // SAFETY: デコーダーは生きていて、`info` はローカル変数。
        let index = unsafe {
            ndk_sys::AMediaCodec_dequeueOutputBuffer(
                self.as_ptr(),
                &mut info,
                patience.as_micros() as i64,
            )
        };
        (index >= 0).then_some((index as usize, info.flags, info.presentationTimeUs))
    }

    pub fn declared(&self) -> Option<MediaFormat> {
        // SAFETY: デコーダーは生きている。フォーマットはこちらが所有する物として返る。
        let format = unsafe { ndk_sys::AMediaCodec_getOutputFormat(self.as_ptr()) };
        // SAFETY: null でないフォーマットは新しく所有する `AMediaFormat`。
        std::ptr::NonNull::new(format).map(|format| unsafe { MediaFormat::from_ptr(format) })
    }

    pub fn hold(&self, index: usize) -> Screen {
        Screen::new(Arc::clone(&self.codec), index)
    }

    pub fn flush(&self) {
        // SAFETY: デコーダーは生きている。
        let _ = unsafe { ndk_sys::AMediaCodec_flush(self.as_ptr()) };
    }

    pub fn stop(&self) {
        // SAFETY: デコーダーは生きている。
        let _ = unsafe { ndk_sys::AMediaCodec_stop(self.as_ptr()) };
    }
}

impl std::fmt::Debug for Direct {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Direct({:?})", self.codec.0.0)
    }
}

/// デコーダーから取り出さないフレーム（直接出力）。表示も破棄もしないフレームにはデコーダーが書けなくなるので、
/// [`Drop`] で表示せずに返す。
pub struct Screen {
    codec: Arc<Send1<Raw>>,
    index: usize,
    done: std::cell::Cell<bool>,
}

impl Screen {
    /// 先に指定できる時刻の幅（`releaseOutputBuffer` の「1 秒以内」）。
    pub const REACH: i64 = 1_000_000_000;

    fn new(codec: Arc<Send1<Raw>>, index: usize) -> Self {
        Self {
            codec,
            index,
            done: std::cell::Cell::new(false),
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn show(&self) -> Result<(), Error> {
        // SAFETY: デコーダーは生きていて（共有を持つ）、インデックスはまだ返していない。
        self.give(|codec, index| unsafe {
            ndk_sys::AMediaCodec_releaseOutputBuffer(codec, index, true)
        })
    }

    /// `CLOCK_MONOTONIC` のナノ秒の時刻の後の VSYNC で表示する。[`Screen::REACH`] を超えると無視。
    /// 時刻を過ぎるまでバッファーは戻らないので、遠い先を指定するとデコーダーの出力バッファが足りなくなる。
    pub fn show_at(&self, at: i64) -> Result<(), Error> {
        // SAFETY: 同上。
        self.give(|codec, index| unsafe {
            ndk_sys::AMediaCodec_releaseOutputBufferAtTime(codec, index, at)
        })
    }

    pub fn drop_it(&self) {
        // SAFETY: 同上。
        let _ = self.give(|codec, index| unsafe {
            ndk_sys::AMediaCodec_releaseOutputBuffer(codec, index, false)
        });
    }

    /// [`Screen::show_at`] の基準の時計（`CLOCK_MONOTONIC`）。
    /// `i64::from` は armv7 で要る（`timespec` のフィールドが `i32`）。
    #[allow(clippy::useless_conversion)]
    pub fn now() -> i64 {
        let mut when = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `when` は正しい型のローカル変数で、`CLOCK_MONOTONIC` は常にある。
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut when) };
        i64::from(when.tv_sec) * 1_000_000_000 + i64::from(when.tv_nsec)
    }

    fn give(
        &self,
        what: impl FnOnce(*mut ndk_sys::AMediaCodec, usize) -> ndk_sys::media_status_t,
    ) -> Result<(), Error> {
        if self.done.replace(true) {
            return Err(Error::Unavailable(String::from(
                "このフレームはもう返してあります",
            )));
        }
        let status = what(self.codec.as_ptr(), self.index);
        ok(status).map_err(|status| {
            Error::Unavailable(self::status("AMediaCodec_releaseOutputBuffer", status))
        })
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if !self.done.get() {
            self.drop_it();
        }
    }
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Screen(インデックス {}, 返却済 {})",
            self.index,
            self.done.get()
        )
    }
}

/// 16 ビットのサンプルのプレーンを詰めてコピーする。`step` はバイト。
fn read_wide(
    bytes: &[u8],
    at: usize,
    pitch: usize,
    width: usize,
    height: usize,
    step: usize,
) -> Option<Plane> {
    if pitch == 0 || width == 0 || height == 0 {
        return None;
    }
    let mut data = Vec::with_capacity(width * height * 2);
    for y in 0..height {
        let row = at + y * pitch;
        if step == 2 {
            data.extend_from_slice(bytes.get(row..row + width * 2)?);
            continue;
        }
        for x in 0..width {
            let sample = row + x * step;
            data.push(*bytes.get(sample)?);
            data.push(*bytes.get(sample + 1)?);
        }
    }
    Some(Plane {
        data,
        stride: width * 2,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codings_are_named_the_way_the_framework_names_them() {
        assert_eq!(mime(Codec::Mpeg2), "video/mpeg2");
        assert_eq!(mime(Codec::H264), "video/avc");
        assert_eq!(mime(Codec::Hevc), "video/hevc");
    }

    #[test]
    fn lengths_become_start_codes() {
        let data = [0, 0, 0, 2, 0x40, 0x01, 0, 0, 0, 1, 0x42];
        let mut out = Vec::new();
        to_start_codes(&data, &mut out);
        assert_eq!(out, [0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42]);
    }

    #[test]
    fn a_short_last_unit_is_taken_as_far_as_it_goes() {
        let mut out = Vec::new();
        to_start_codes(&[0, 0, 0, 8, 0x40, 0x01], &mut out);
        assert_eq!(out, [0, 0, 0, 1, 0x40, 0x01]);
    }

    #[test]
    fn the_codec_specific_data_is_the_sequence_header_and_its_extensions() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 1, 0xb3, 1, 2, 3]); // sequence header
        data.extend_from_slice(&[0, 0, 1, 0xb5, 0x14]); // sequence extension
        let end = data.len();
        data.extend_from_slice(&[0, 0, 1, 0xb8, 9]); // group header
        data.extend_from_slice(&[0, 0, 1, 0x00, 9]); // picture
        assert_eq!(mpeg2_csd(&data), Some(0..end));
    }

    #[test]
    fn a_picture_ends_the_codec_specific_data() {
        let mut data = vec![0, 0, 1, 0xb3, 1, 2, 3];
        let end = data.len();
        data.extend_from_slice(&[0, 0, 1, 0x00, 9]);
        assert_eq!(mpeg2_csd(&data), Some(0..end));
    }

    #[test]
    fn the_codec_specific_data_begins_at_the_sequence_header() {
        let mut data = vec![0, 0, 1, 0xb8, 7];
        let at = data.len();
        data.extend_from_slice(&[0, 0, 1, 0xb3, 1]);
        let end = data.len();
        data.extend_from_slice(&[0, 0, 1, 0x00, 9]);
        assert_eq!(mpeg2_csd(&data), Some(at..end));
    }

    #[test]
    fn no_sequence_header_means_no_codec_specific_data() {
        assert_eq!(mpeg2_csd(&[0, 0, 1, 0x00, 9]), None);
        assert_eq!(mpeg2_csd(&[]), None);
    }

    #[test]
    fn a_semi_planar_chroma_plane_comes_out_as_one_plane() {
        let bytes = [1, 9, 2, 9, 3, 9, 0, 0, 4, 9, 5, 9, 6, 9, 0, 0];
        let plane = read_plane(&bytes, 0, 8, 3, 2, 2).expect("a plane");
        assert_eq!(plane.data, [1, 2, 3, 4, 5, 6]);
        assert_eq!(plane.stride, 3);
    }

    #[test]
    fn a_row_at_a_time_gives_what_a_sample_at_a_time_gave() {
        let bytes: Vec<u8> = (0..64).collect();
        let quick = read_plane(&bytes, 3, 10, 6, 4, 1).expect("a plane");
        let mut slow = Vec::new();
        for y in 0..4 {
            for x in 0..6 {
                slow.push(bytes[3 + y * 10 + x]);
            }
        }
        assert_eq!(quick.data, slow);
        assert_eq!(quick.stride, 6);
    }

    #[test]
    fn woven_chroma_is_handed_over_as_it_lies() {
        let bytes = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        let plane = read_woven(&bytes, 0, 8, 3, 2, 2).expect("a plane");
        assert_eq!(plane.data, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(plane.stride, 6);
        assert_eq!(plane.width, 3);
    }

    #[test]
    fn a_stride_that_is_the_row_is_one_copy() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let plane = read_woven(&bytes, 0, 6, 3, 2, 2).expect("a plane");
        assert_eq!(plane.data, bytes);
    }

    #[test]
    fn an_empty_third_plane_is_what_says_the_chroma_is_woven() {
        use mediadecoder::video::Plane;
        let frame = mediadecoder::video::Frame {
            tag: 0,
            format: Format::default(),
            planes: [Plane::none(), Plane::none(), Plane::none()],
            structure: Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
        };
        assert!(frame.chroma_woven());
    }
}
