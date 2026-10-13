// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Windows のソフトウェアデコーダー（Media Foundation の同期 MFT）。
//! H.264・HEVC の入力の時刻は表示順に増える値にする（HEVC Video Extensions は入力の時刻を
//! 昇順に付け直すので、デコード順の `tag` だとフレームと記録がずれる）。MPEG-2 は `tag`。

use std::collections::{BTreeMap, VecDeque};

use mediadecoder::es::{Codec, Format, Framing, Nals, Picture, Structure};
use mediadecoder::video::{Frame, Plane};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFSample, IMFTransform, MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_E_TRANSFORM_TYPE_NOT_SET, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType,
    MFMediaType_Video, MFSTARTUP_LITE, MFSampleExtension_BottomFieldFirst,
    MFSampleExtension_Interlaced, MFSampleExtension_RepeatFirstField, MFStartup,
    MFT_CATEGORY_VIDEO_DECODER, MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_COMMAND_FLUSH, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFVideoFormat_H264, MFVideoFormat_HEVC,
    MFVideoFormat_MPEG2, MFVideoFormat_NV12, MFVideoFormat_P010,
    MFVideoInterlace_MixedInterlaceOrProgressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::{GUID, PWSTR};

use super::order::{Key, Keys};
use super::{About, Decode, Decoded, Error};
use crate::trace::Trace;

static SAY: Trace = Trace::of("mediafoundation");

/// 出てきたフレームの記録をいくつまで持つか（フレームにならずに残る記録の上限）。
const ABOUTS: usize = 64;

/// シーケンスが替わるたびに時刻の基準を繰り上げる幅。
const SEQUENCE: i64 = 1 << 32;

fn failed(what: &str, error: windows::core::Error) -> Error {
    Error::Unavailable(format!("{what}: {error}"))
}

/// Media Foundation を使える状態にする。`MFStartup` はプロセスに一度、COM はスレッドごと。
pub(crate) fn start() -> Result<(), Error> {
    static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    // SAFETY: 予約の引数は NULL。既に初期化済みでも続けてよい。
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    STARTED
        .get_or_init(|| {
            // SAFETY: バージョンはヘッダーで定義された値。
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|error| error.to_string())
        })
        .clone()
        .map_err(|why| Error::Unavailable(format!("MFStartup: {why}")))
}

fn subtype(codec: Codec) -> GUID {
    match codec {
        Codec::Mpeg2 => MFVideoFormat_MPEG2,
        Codec::H264 => MFVideoFormat_H264,
        Codec::Hevc => MFVideoFormat_HEVC,
    }
}

pub(crate) fn friendly(activate: &IMFActivate) -> String {
    let mut text = PWSTR::null();
    let mut length = 0u32;
    // SAFETY: 出力は生きたローカル変数。返った文字列は読んだら解放する。
    unsafe {
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut length)
            .is_err()
            || text.is_null()
        {
            return String::from("名前の無い MFT");
        }
        let name = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0.cast_const().cast()));
        name
    }
}

fn candidates(codec: Codec) -> Result<Vec<IMFActivate>, Error> {
    Ok(mft::enumerate(
        MFT_CATEGORY_VIDEO_DECODER,
        MFMediaType_Video,
        subtype(codec),
    )?)
}

/// このコーデックに使える OS のデコーダーの名前。一覧に出ても拒否するものがあるので、入力の型を渡すまで試す。
pub fn usable(codec: Codec) -> Vec<String> {
    if start().is_err() {
        return Vec::new();
    }
    let Ok(found) = candidates(codec) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for activate in &found {
        let name = friendly(activate);
        if names.contains(&name) {
            continue;
        }
        let usable = mft::activate(activate)
            .is_ok_and(|transform| set_input(&transform, codec, None).is_ok());
        mft::shutdown(activate);
        if usable {
            names.push(name);
        }
    }
    names
}

/// 入力の型を設定する。開くときはサイズ無しで、最初のフレームの手前でサイズ付きで設定し直す。
/// HEVC Video Extensions はサイズの無いまま出力の型を問い合わせるとプロセスごと落ちる。
fn set_input(
    transform: &IMFTransform,
    codec: Codec,
    size: Option<(u32, u32)>,
) -> windows::core::Result<()> {
    // SAFETY: 作った型はローカル変数で、キーと値はどれも Media Foundation の定数。
    unsafe {
        let kind = MFCreateMediaType()?;
        kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        kind.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
        kind.SetUINT32(
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_MixedInterlaceOrProgressive.0 as u32,
        )?;
        if let Some((width, height)) = size {
            kind.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(width) << 32) | u64::from(height),
            )?;
        }
        transform.SetInputType(0, &kind, 0)
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

/// 出力の型。MFT が返すサイズはパディング込み（1080 が 1088）。
#[derive(Debug, Clone, Copy)]
struct Shape {
    wide: bool,
    width: usize,
    height: usize,
}

pub struct MediaFoundation {
    transform: IMFTransform,
    /// 変換を作成した IMFActivate。終了させるのはこちら（`ShutdownObject`）。
    activate: IMFActivate,
    codec: Codec,
    framing: Framing,
    name: String,
    shape: Option<Shape>,
    /// 入力の型に設定したサイズ。`None` のあいだは出力の型を問い合わせない（[`set_input`]）。
    sized: Option<(u32, u32)>,
    format: Format,
    abouts: BTreeMap<i64, About>,
    /// 表示順のキー。MPEG-2 は表示順の番号を持たないので `tag` をそのまま時刻にする。
    keys: Keys,
    base: i64,
    ready: VecDeque<Frame>,
    unreadable: usize,
    why: Option<String>,
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // 終了させるのは一度だけ。変換の参照はこの後に解放される。
        mft::shutdown(&self.activate);
    }
}

impl std::fmt::Debug for MediaFoundation {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "MediaFoundation({:?} {})", self.codec, self.name)
    }
}

// SAFETY: 一度に一つの呼び出しでしか触らない。同期の MFT は自由スレッド。
unsafe impl Send for MediaFoundation {}

impl MediaFoundation {
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_named(codec, framing, None)
    }

    pub fn open_named(codec: Codec, framing: Framing, wanted: Option<&str>) -> Result<Self, Error> {
        start()?;
        let found = candidates(codec)?;
        if found.is_empty() {
            return Err(Error::Unavailable(format!(
                "この Windows に {codec:?} のソフトウェアデコーダー（同期の MFT）がありません"
            )));
        }
        let mut refusals = Vec::new();
        for activate in &found {
            let name = friendly(activate);
            if wanted.is_some_and(|wanted| wanted != name) {
                continue;
            }
            let transform = match mft::activate(activate) {
                Ok(transform) => transform,
                Err(error) => {
                    refusals.push(format!("{name}: {error}"));
                    continue;
                }
            };
            if SAY.on() {
                tracing::info!("Media Foundation {codec:?}: {name} を開いた");
            }
            if let Err(error) = set_input(&transform, codec, None) {
                refusals.push(format!("{name}: {error}"));
                mft::shutdown(activate);
                continue;
            }
            let decoder = Self {
                transform,
                activate: activate.clone(),
                codec,
                framing,
                name,
                shape: None,
                sized: None,
                keys: match codec {
                    Codec::Mpeg2 => Keys::Mpeg2,
                    Codec::H264 => Keys::H264(Box::default()),
                    Codec::Hevc => Keys::Hevc(Box::default()),
                },
                base: 0,
                format: Format::default(),
                abouts: BTreeMap::new(),
                ready: VecDeque::new(),
                unreadable: 0,
                why: None,
            };
            return Ok(decoder);
        }
        if refusals.is_empty()
            && let Some(wanted) = wanted
        {
            return Err(Error::Unavailable(format!(
                "{wanted} がありません（再インストールまたは削除された）"
            )));
        }
        Err(Error::Unavailable(format!(
            "{codec:?} の MFT がどれも入力の型を受け付けません（{}）",
            refusals.join("、")
        )))
    }

    fn set_output(&mut self) -> Result<(), Error> {
        let wanted = if self.format.depth > 8 {
            MFVideoFormat_P010
        } else {
            MFVideoFormat_NV12
        };
        let chosen = mft::output_types(&self.transform)
            .find(|kind| mft::guid(kind, &MF_MT_SUBTYPE).ok() == Some(wanted));
        let Some(kind) = chosen else {
            self.shape = None;
            return Err(Error::Unavailable(format!(
                "{} が {} を出しません",
                self.name,
                if wanted == MFVideoFormat_P010 {
                    "P010"
                } else {
                    "NV12"
                }
            )));
        };
        // SAFETY: MFT が挙げた型をそのまま返す。
        unsafe { self.transform.SetOutputType(0, &kind, 0) }
            .map_err(|error| failed("SetOutputType", error))?;
        let size = mft::uint64(&kind, &MF_MT_FRAME_SIZE)
            .map_err(|error| failed("MF_MT_FRAME_SIZE", error))?;
        self.shape = Some(Shape {
            wide: wanted == MFVideoFormat_P010,
            width: (size >> 32) as usize,
            height: (size & 0xffff_ffff) as usize,
        });
        if SAY.on() {
            tracing::info!(
                "Media Foundation {:?}: 出力形式 {:?}",
                self.codec,
                self.shape
            );
        }
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

    fn resize(&mut self, size: (u32, u32)) -> Result<(), Error> {
        let first = self.sized.is_none();
        if SAY.on() {
            tracing::info!(
                "Media Foundation {:?}: 入力形式を {}x{} に設定",
                self.codec,
                size.0,
                size.1
            );
        }
        if !first {
            // 保持しているフレームは前のサイズのもの。入力の型を替える前に捨てる。
            mft::message(&self.transform, MFT_MESSAGE_COMMAND_FLUSH);
            self.abouts.clear();
        }
        set_input(&self.transform, self.codec, Some(size))
            .map_err(|error| failed("SetInputType", error))?;
        self.sized = Some(size);
        self.shape = None;
        self.set_output()?;
        if first {
            mft::begin(&self.transform);
        }
        Ok(())
    }

    fn pull(&mut self) -> Result<(), Error> {
        loop {
            if self.shape.is_none() {
                self.set_output()?;
            }
            let info = mft::output_info(&self.transform)?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0;
            if SAY.on() {
                tracing::info!(
                    "Media Foundation {:?}: 出力ストリーム フラグ {:#x} サイズ {} アライメント {}",
                    self.codec,
                    info.dwFlags,
                    info.cbSize,
                    info.cbAlignment
                );
            }
            let given = if provides {
                None
            } else {
                Some(mft::empty(info.cbSize)?)
            };
            let (result, status, sample) = mft::output(&self.transform, given);
            if SAY.on() {
                tracing::info!(
                    "Media Foundation {:?}: ProcessOutput {:?} ステータス {status:#x}",
                    self.codec,
                    result.as_ref().map_err(windows::core::Error::code)
                );
            }
            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        self.receive(&sample);
                    }
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(error)
                    if error.code() == MF_E_TRANSFORM_STREAM_CHANGE
                        || error.code() == MF_E_TRANSFORM_TYPE_NOT_SET =>
                {
                    self.set_output()?;
                }
                Err(error) => return Err(failed("ProcessOutput", error)),
            }
        }
    }

    fn receive(&mut self, sample: &IMFSample) {
        match self.read(sample) {
            Ok(frame) => self.ready.push_back(frame),
            Err(why) => {
                self.unreadable += 1;
                if self.why.is_none() {
                    tracing::warn!("Media Foundation {:?}: {why}", self.codec);
                    self.why = Some(why);
                }
            }
        }
    }

    fn read(&mut self, sample: &IMFSample) -> Result<Frame, String> {
        let shape = self.shape.ok_or("出力の型が決まっていません")?;
        let time = mft::sample_time(sample).unwrap_or(-1);
        let flag = |key| mft::uint32(sample, key).unwrap_or(0) != 0;
        let interlaced = flag(&MFSampleExtension_Interlaced);
        let bottom_first = flag(&MFSampleExtension_BottomFieldFirst);
        let repeat = flag(&MFSampleExtension_RepeatFirstField);
        if SAY.on() {
            tracing::info!(
                "Media Foundation {:?}: 出力 時刻 {time} 記録 {}",
                self.codec,
                if self.abouts.contains_key(&time) {
                    "あり"
                } else {
                    "なし"
                }
            );
        }
        let about = match self.abouts.remove(&time) {
            // フィールドピクチャーは二枚が一つになって出る。順序は一枚目のものを使う。
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
                top_field_first: !bottom_first,
                repeat_first_field: repeat,
                progressive: !interlaced,
            },
        };
        let buffer = mft::contiguous(sample)?;
        let step = if shape.wide { 2 } else { 1 };
        let locked = mft::Locked::picture(&buffer)?;
        let pitch = locked.pitch().unwrap_or((shape.width * step) as i32);
        let length = locked.length();
        let result = (|| {
            let pitch = usize::try_from(pitch).map_err(|_| format!("ピッチが負です（{pitch}）"))?;
            let needed = pitch * shape.height + pitch * shape.height.div_ceil(2);
            let bytes = locked.bytes(needed).ok_or_else(|| {
                format!(
                    "バッファが短すぎます（{length} バイト、{}x{} に {needed} バイト要る）",
                    shape.width, shape.height
                )
            })?;
            let width = (about.format.width as usize).min(shape.width);
            let height = (about.format.height as usize).min(shape.height);
            planes(bytes, pitch, shape, width, height)
                .ok_or_else(|| String::from("バッファがフレームのサイズに足りません"))
        })();
        drop(locked);
        let planes = result?;
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
}

/// `NV12` / `P010` のバッファを三つのプレーンに分ける。色差は輝度に確保した行数（パディング込み）の後ろから。
fn planes(
    bytes: &[u8],
    pitch: usize,
    shape: Shape,
    width: usize,
    height: usize,
) -> Option<[Plane; 3]> {
    let after = pitch * shape.height;
    crate::decode::semi_planar(
        (bytes, pitch),
        (bytes.get(after..)?, pitch),
        width,
        height,
        shape.wide,
    )
}

impl Decode for MediaFoundation {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        super::openable(format)?;
        let deeper = (format.depth > 8) != (self.format.depth > 8);
        self.format = format;
        let size = (format.width, format.height);
        if self.sized != Some(size) {
            self.resize(size)?;
        } else if deeper && self.shape.is_some() {
            let _ = self.set_output();
        }
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
        // フィールド対は二枚が同じ表示順の番号になりうる。一枚目のものを残す。
        self.abouts.entry(time).or_insert(about);
        while self.abouts.len() > ABOUTS {
            self.abouts.pop_first();
        }
        let length =
            u32::try_from(data.len()).map_err(|_| Error::Damaged("an access unit over 4 GB"))?;
        let sample = mft::filled(length, data, time, picture.random_access)?;
        if SAY.on() {
            tracing::info!(
                "Media Foundation {:?}: ProcessInput {tag} {} バイト",
                self.codec,
                length
            );
        }
        match mft::input(&self.transform, &sample) {
            Ok(()) => {}
            // 取り出していないフレームで詰まっている。受け取ってから、もう一度だけ渡す。
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                self.pull()?;
                mft::input(&self.transform, &sample)
                    .map_err(|error| failed("ProcessInput", error))?;
            }
            Err(error) => return Err(failed("ProcessInput", error)),
        }
        self.pull()
    }

    fn take(&mut self) -> Option<Decoded> {
        self.ready.pop_front().map(Decoded::Planes)
    }

    fn flush(&mut self) {
        mft::message(&self.transform, MFT_MESSAGE_COMMAND_DRAIN);
        if let Err(error) = self.pull() {
            tracing::warn!("Media Foundation {:?}: {error}", self.codec);
        }
    }

    fn name(&self) -> String {
        format!("Media Foundation（{}）", self.name)
    }

    fn hardware(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        mft::message(&self.transform, MFT_MESSAGE_COMMAND_FLUSH);
        self.ready.clear();
        self.abouts.clear();
        self.keys.reset();
        self.base += SEQUENCE;
    }
}

/// 同期 MFT と Media Foundation のオブジェクトを扱う関数群。渡すのは生きた COM オブジェクトと
/// Media Foundation の定数だけなので、呼ぶ側に unsafe は要らない。
pub(crate) mod mft {
    use windows::Win32::Media::MediaFoundation::{
        IMF2DBuffer, IMFActivate, IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample,
        IMFTransform, MFCreateMemoryBuffer, MFCreateSample, MFSampleExtension_CleanPoint,
        MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
        MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_TYPE,
        MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::core::{GUID, Interface};

    /// 失敗した呼び出しの名前とエラー。`{名前}: {エラー}` と書く。
    #[derive(Debug)]
    pub(crate) struct Failed(pub &'static str, pub windows::core::Error);

    impl std::fmt::Display for Failed {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(out, "{}: {}", self.0, self.1)
        }
    }

    impl From<Failed> for crate::decode::Error {
        fn from(failed: Failed) -> Self {
            Self::Unavailable(failed.to_string())
        }
    }

    impl From<Failed> for String {
        fn from(failed: Failed) -> Self {
            failed.to_string()
        }
    }

    trait Named<T> {
        fn named(self, what: &'static str) -> Result<T, Failed>;
    }

    impl<T> Named<T> for windows::core::Result<T> {
        fn named(self, what: &'static str) -> Result<T, Failed> {
            self.map_err(|error| Failed(what, error))
        }
    }

    /// 同期・ローカルの MFT を優先度順に。
    pub(crate) fn enumerate(
        category: GUID,
        major: GUID,
        subtype: GUID,
    ) -> Result<Vec<IMFActivate>, Failed> {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: major,
            guidSubtype: subtype,
        };
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: 型の記述は生きたローカル変数。出力の配列は CoTaskMemAlloc のもの。
        unsafe {
            MFTEnumEx(
                category,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&raw const input),
                None,
                &mut list,
                &mut count,
            )
        }
        .named("MFTEnumEx")?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: 配列は `count` 個。要素を読み出して所有してから、配列を解放する。
        let found = unsafe {
            let found = (0..count as usize)
                .filter_map(|at| std::ptr::read(list.add(at)))
                .collect();
            CoTaskMemFree(Some(list.cast_const().cast()));
            found
        };
        Ok(found)
    }

    pub(crate) fn activate(activate: &IMFActivate) -> windows::core::Result<IMFTransform> {
        // SAFETY: 生きた IMFActivate。
        unsafe { activate.ActivateObject() }
    }

    /// 作成した変換を終了させる。
    pub(crate) fn shutdown(activate: &IMFActivate) {
        // SAFETY: 生きた IMFActivate。
        let _ = unsafe { activate.ShutdownObject() };
    }

    /// 引数を取らない命令と通知。
    pub(crate) fn message(transform: &IMFTransform, message: MFT_MESSAGE_TYPE) {
        // SAFETY: 生きた変換で、引数は 0。
        let _ = unsafe { transform.ProcessMessage(message, 0) };
    }

    /// ストリームの開始を通知する。
    pub(crate) fn begin(transform: &IMFTransform) {
        message(transform, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING);
        message(transform, MFT_MESSAGE_NOTIFY_START_OF_STREAM);
    }

    /// 出力ストリームが列挙する型を順に。
    pub(crate) fn output_types(transform: &IMFTransform) -> impl Iterator<Item = IMFMediaType> {
        (0..).map_while(move |at| {
            // SAFETY: 出力ストリームは一本。インデックスが範囲を超えるとエラーが返り、そこで止める。
            unsafe { transform.GetOutputAvailableType(0, at) }.ok()
        })
    }

    pub(crate) fn output_info(transform: &IMFTransform) -> Result<MFT_OUTPUT_STREAM_INFO, Failed> {
        // SAFETY: 出力ストリームは一本。
        unsafe { transform.GetOutputStreamInfo(0) }.named("GetOutputStreamInfo")
    }

    pub(crate) fn input(transform: &IMFTransform, sample: &IMFSample) -> windows::core::Result<()> {
        // SAFETY: 入力ストリームは一本で、サンプルは生きている。
        unsafe { transform.ProcessInput(0, sample, 0) }
    }

    /// 一つ受け取る。戻り値・状態・出てきたサンプル（渡したか、MFT が作った物）。
    pub(crate) fn output(
        transform: &IMFTransform,
        given: Option<IMFSample>,
    ) -> (windows::core::Result<()>, u32, Option<IMFSample>) {
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: std::mem::ManuallyDrop::new(given),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        // SAFETY: 配列は一つで、サンプルはこちらが参照を持って渡している。返ったら二つとも
        // 取り戻し、以後は配列に触らない。
        let (result, sample, _events) = unsafe {
            let result = transform.ProcessOutput(0, &mut buffers, &mut status);
            let [mut buffer] = buffers;
            (
                result,
                std::mem::ManuallyDrop::take(&mut buffer.pSample),
                std::mem::ManuallyDrop::take(&mut buffer.pEvents),
            )
        };
        (result, status, sample)
    }

    /// 空のバッファを一つ持ったサンプル。
    pub(crate) fn empty(size: u32) -> Result<IMFSample, Failed> {
        // SAFETY: 作ったものはローカル変数で、サンプルに一つ持たせる。
        unsafe {
            let buffer = MFCreateMemoryBuffer(size).named("MFCreateMemoryBuffer")?;
            let sample = MFCreateSample().named("MFCreateSample")?;
            sample.AddBuffer(&buffer).named("AddBuffer")?;
            Ok(sample)
        }
    }

    /// `data` をコピーしたバッファを持ち、時刻を付けたサンプル。バッファは `capacity` バイト確保する。
    pub(crate) fn filled(
        capacity: u32,
        data: &[u8],
        time: i64,
        clean_point: bool,
    ) -> Result<IMFSample, Failed> {
        let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
        // SAFETY: 作ったものはローカル変数。バッファはコピーしたらすぐアンロックする。確保は `data` より短くない。
        unsafe {
            let buffer = MFCreateMemoryBuffer(capacity).named("MFCreateMemoryBuffer")?;
            let mut base = std::ptr::null_mut();
            buffer.Lock(&mut base, None, None).named("Lock")?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), base, data.len());
            let _ = buffer.Unlock();
            buffer.SetCurrentLength(length).named("SetCurrentLength")?;
            let sample = MFCreateSample().named("MFCreateSample")?;
            sample.AddBuffer(&buffer).named("AddBuffer")?;
            sample.SetSampleTime(time).named("SetSampleTime")?;
            if clean_point {
                let _ = sample.SetUINT32(&MFSampleExtension_CleanPoint, 1);
            }
            Ok(sample)
        }
    }

    pub(crate) fn sample_time(sample: &IMFSample) -> windows::core::Result<i64> {
        // SAFETY: 生きたサンプル。
        unsafe { sample.GetSampleTime() }
    }

    pub(crate) fn contiguous(sample: &IMFSample) -> Result<IMFMediaBuffer, Failed> {
        // SAFETY: 生きたサンプル。
        unsafe { sample.ConvertToContiguousBuffer() }.named("ConvertToContiguousBuffer")
    }

    pub(crate) fn guid(attributes: &IMFAttributes, key: &GUID) -> windows::core::Result<GUID> {
        // SAFETY: 生きたオブジェクトと、生きたキー。
        unsafe { attributes.GetGUID(key) }
    }

    pub(crate) fn uint32(attributes: &IMFAttributes, key: &GUID) -> windows::core::Result<u32> {
        // SAFETY: 同上。
        unsafe { attributes.GetUINT32(key) }
    }

    pub(crate) fn uint64(attributes: &IMFAttributes, key: &GUID) -> windows::core::Result<u64> {
        // SAFETY: 同上。
        unsafe { attributes.GetUINT64(key) }
    }

    /// ロックしたバッファ。落とすとアンロックする。
    pub(crate) struct Locked<'a> {
        buffer: &'a IMFMediaBuffer,
        two: Option<IMF2DBuffer>,
        base: *mut u8,
        pitch: Option<i32>,
        length: usize,
    }

    impl<'a> Locked<'a> {
        /// フレームのバッファ。二次元でロックできればピッチも得る。長さは `GetCurrentLength`。
        pub(crate) fn picture(buffer: &'a IMFMediaBuffer) -> Result<Self, Failed> {
            let mut base = std::ptr::null_mut();
            let two = buffer.cast::<IMF2DBuffer>().ok();
            // SAFETY: 出力は生きたローカル変数。ロックはドロップ時に解除し、長さはロック中に取得する。
            let (pitch, length) = unsafe {
                let pitch = match &two {
                    Some(two) => {
                        let mut pitch = 0i32;
                        two.Lock2D(&mut base, &mut pitch).named("Lock2D")?;
                        Some(pitch)
                    }
                    None => {
                        buffer.Lock(&mut base, None, None).named("Lock")?;
                        None
                    }
                };
                (pitch, buffer.GetCurrentLength().unwrap_or(0) as usize)
            };
            Ok(Self {
                buffer,
                two,
                base,
                pitch,
                length,
            })
        }

        /// 連続したメモリとしてロックする。長さは `Lock` が返す。
        pub(crate) fn plain(buffer: &'a IMFMediaBuffer) -> Result<Self, Failed> {
            let mut base = std::ptr::null_mut();
            let mut length = 0u32;
            // SAFETY: 出力は生きたローカル変数。ドロップ時にアンロックする。
            unsafe { buffer.Lock(&mut base, None, Some(&mut length)) }.named("Lock")?;
            Ok(Self {
                buffer,
                two: None,
                base,
                pitch: None,
                length: length as usize,
            })
        }

        /// 二次元でロックした時のピッチ。
        pub(crate) fn pitch(&self) -> Option<i32> {
            self.pitch
        }

        pub(crate) fn length(&self) -> usize {
            self.length
        }

        /// 先頭から `length` バイト。アドレスが無いか、バッファが短ければ `None`。
        pub(crate) fn bytes(&self, length: usize) -> Option<&[u8]> {
            if self.base.is_null() || length > self.length {
                return None;
            }
            // SAFETY: ロックしたバッファで、長さは確かめた。参照はアンロックする前に破棄される。
            Some(unsafe { std::slice::from_raw_parts(self.base.cast_const(), length) })
        }
    }

    impl Drop for Locked<'_> {
        fn drop(&mut self) {
            // SAFETY: 掛けたロックを解除する。
            unsafe {
                match &self.two {
                    Some(two) => {
                        let _ = two.Unlock2D();
                    }
                    None => {
                        let _ = self.buffer.Unlock();
                    }
                }
            }
        }
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

    #[test]
    fn p010_comes_down_to_the_bottom_and_apart() {
        let shape = Shape {
            wide: true,
            width: 2,
            height: 2,
        };
        let up = |value: u16| (value << 6).to_le_bytes();
        let mut bytes = Vec::new();
        for value in [1, 2, 3, 1023] {
            bytes.extend_from_slice(&up(value));
        }
        bytes.extend_from_slice(&up(512));
        bytes.extend_from_slice(&up(100));
        let [luma, blue, red] = planes(&bytes, 4, shape, 2, 2).expect("足りる");
        assert_eq!(luma.data, [1, 0, 2, 0, 3, 0, 0xff, 0x03]);
        assert_eq!(blue.data, 512u16.to_le_bytes());
        assert_eq!(red.data, 100u16.to_le_bytes());
        assert_eq!((luma.stride, blue.width, blue.height), (4, 1, 1));
    }

    #[test]
    fn nv12_skips_the_padding() {
        let shape = Shape {
            wide: false,
            width: 4,
            height: 4,
        };
        let pitch = 4;
        let mut bytes: Vec<u8> = (0..16).collect();
        bytes.extend_from_slice(&[100, 200, 101, 201, 102, 202, 103, 203]);
        let [luma, blue, red] = planes(&bytes, pitch, shape, 2, 2).expect("足りる");
        assert_eq!(luma.data, [0, 1, 4, 5]);
        assert_eq!(blue.data, [100]);
        assert_eq!(red.data, [200]);
    }
}
