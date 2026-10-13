// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MediaCodec によるデコード。ヘッダーも自分で解析するデコーダーなので、こちらでの解析は無い。
//! 端末のデコーダーは MediaCodecList に問い合わせるだけで、端末名や API バージョンで分けない。
//! 宣言が正しくても出力が正しいとは限らないので、ソフトウェアと比べて確かめる。

mod media;

use std::collections::VecDeque;
use std::sync::Arc;

use mediadecoder::es::{Codec, Format, Framing, Picture, Structure};
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;

use super::{About, Decode, Decoded, Error, Held as Where, Readback, Surface};
use media::{Images, Send1};

mod drain;
mod feed;
mod session;

pub use media::{Frame, Screen};

/// 待つしかないときに待つ長さ。59.94 では一枚に 16 ms しかない。
const WAIT: std::time::Duration = std::time::Duration::from_millis(4);

const POLL: std::time::Duration = std::time::Duration::ZERO;

const TRIES: usize = 8;

/// 入力バッファを待ち続ける長さ。始まったばかりのデコーダーは並べ替えバッファが埋まるまで返さない。
const FEEDING: std::time::Duration = std::time::Duration::from_millis(750);

/// まだ一枚も出していないデコーダーに対する同じ長さ（最初の出力まで 1 秒を超えることがある）。
const STARTING: std::time::Duration = std::time::Duration::from_secs(2);

const LATENCY: std::time::Duration = std::time::Duration::from_millis(250);

/// AImageReader が一度に保持できるフレームの数。[`HOLD`] と [`AHEAD`] の和より多くないと MediaCodec が止まる。
const IMAGES: i32 = 8;

/// 渡した後も保持しておくフレームの数（時間方向のインタレース解除は三枚読む）。
const HOLD: usize = 3;

const AHEAD: usize = 3;

const TURNS: usize = IMAGES as usize * 4;

const DRAINING: std::time::Duration = std::time::Duration::from_millis(250);

/// end of stream を送った後に集め続ける上限。並べ替えバッファを丸ごと出すので秒の単位が要る。
const ENDING: std::time::Duration = std::time::Duration::from_secs(5);

const ENDING_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

/// これからする処理を記録する先。どれもセッションに一度だけ出す。
pub type Trace = Box<dyn FnMut(&str) + Send>;

mod step {
    pub const INPUT: u32 = 1 << 3;
    pub const FED: u32 = 1 << 4;
    pub const OUTPUT: u32 = 1 << 5;
    pub const GOT: u32 = 1 << 6;
    pub const RENDER: u32 = 1 << 7;
    pub const IMAGE: u32 = 1 << 8;
    pub const PICTURE: u32 = 1 << 9;
    pub const UNREADABLE: u32 = 1 << 10;
    pub const LAYOUT: u32 = 1 << 11;
    pub const SENT: u32 = 1 << 12;
    pub const ENDED: u32 = 1 << 13;
    pub const LATE: u32 = 1 << 14;
    pub const PAIRED: u32 = 1 << 15;
    pub const ACQUIRE_LATE: u32 = 1 << 16;
    /// AImageReader が「無い」でもフレームでもない物を返した。[`LATE`] と分けないと片方が記録されない。
    pub const ACQUIRE_FAILED: u32 = 1 << 17;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Buffers,
    Memory,
    Direct,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Self::Buffers => "AHardwareBuffer",
            Self::Memory => "メモリ",
            Self::Direct => "直接出力",
        }
    }
}

struct Session {
    /// [`Route::Direct`] では `None`（二つ目のインスタンスを開けない端末がある）。
    codec: Option<Send1<MediaCodec>>,
    direct: Option<media::Direct>,
    images: Option<Images>,
    route: Route,
    format: Format,
}

impl Session {
    fn give(&self, what: impl FnOnce(&MediaCodec)) {
        match (&self.codec, &self.direct) {
            (Some(codec), _) => what(codec),
            (None, Some(_)) => {}
            (None, None) => {}
        }
    }

    fn direct(&self) -> Option<&media::Direct> {
        self.direct.as_ref()
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Session({:?} {:?})", self.route, self.format)
    }
}

enum Taken {
    Yes,
    Full,
    Late,
}

/// 出力を一回取り出した結果。デコーダーの借用をフレームを格納する前に終わらせるためにある。
enum Said {
    Nothing,
    Again,
    Acquire(u64),
    Frame(Box<mediadecoder::video::Frame>, String),
    Ended,
    Screen(media::Screen, u64),
    /// 読めないレイアウトの出力。「何も無い」と同じに扱うと動いているデコーダーが何も出さないように見える。
    Unreadable(Error, String),
}

pub struct Android {
    codec: Codec,
    framing: Framing,
    name: String,
    chosen: Option<String>,
    wanted: Route,
    session: Option<Session>,
    ready: VecDeque<Decoded>,
    /// AImageReader に出してまだ取っていないフレームのタグ。空でない間はデコーダーに他を要求しない。
    pending: VecDeque<u64>,
    holding: VecDeque<Arc<media::Image>>,
    fed: bool,
    produced: bool,
    /// AImageReader に出して戻らなかったフレームの数（Android では eprintln! が見えない）。
    missed: u64,
    spun: u64,
    unreadable: u64,
    why: Option<String>,
    ending: bool,
    /// end of stream のフラグが出力に戻ったか。出力が終わったことを示すのはこれだけ。
    ended: bool,
    /// 開いたときのデコーダーを取っておく（一インスタンスしか許さないデコーダーがある）。
    spare: Option<Send1<MediaCodec>>,
    csd: Option<Vec<u8>>,
    half: Option<(u64, Vec<u8>)>,
    parity: Option<Structure>,
    /// 直接出力のフレームの出力先。最初の単位の前に [`Android::onto`] で。
    window: Option<std::sync::Arc<ndk::native_window::NativeWindow>>,
    /// メモリーのフレームで色差をインターリーブのままにしてよいか。
    weave: bool,
    /// configure に足すキー。知らないキーは黙って捨てられる。
    extra: Vec<(String, i32)>,
    trace: Option<Trace>,
    said: u32,
    software: bool,
}

impl std::fmt::Debug for Android {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Android({} {:?})", self.name, self.session)
    }
}

impl Android {
    /// 端末のデコーダーをメモリー経路で開く（AImageReader 経路は Vulkan が受け付けないフレームを返す端末がある）。
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_named(codec, framing, None, Route::Memory)
    }

    pub fn open_named(
        codec: Codec,
        framing: Framing,
        name: Option<&str>,
        wanted: Route,
    ) -> Result<Self, Error> {
        let mime = media::mime(codec);
        let opened = match name {
            Some(name) => MediaCodec::from_codec_name(name),
            None => MediaCodec::from_decoder_type(mime),
        };
        let opened = opened.ok_or_else(|| {
            Error::Unavailable(format!(
                "この端末に {mime} のデコーダーがありません（MediaCodecList に無い）"
            ))
        })?;
        // `AMediaCodec_getName` は API 28 なので、名前は指定した物。
        let found = name.map_or_else(|| format!("既定 ({mime})"), str::to_owned);
        Ok(Self {
            codec,
            framing,
            name: found,
            chosen: name.map(str::to_owned),
            wanted,
            session: None,
            ready: VecDeque::new(),
            pending: VecDeque::new(),
            holding: VecDeque::new(),
            fed: false,
            produced: false,
            missed: 0,
            spun: 0,
            unreadable: 0,
            why: None,
            ending: false,
            ended: false,
            // SAFETY: `Send1` を見よ。
            spare: Some(unsafe { Send1::new(opened) }),
            csd: None,
            half: None,
            parity: None,
            weave: false,
            window: None,
            extra: Vec::new(),
            trace: None,
            said: 0,
            software: false,
        })
    }

    /// AOSP のソフトウェアデコーダーを名前で開く（[`crate::decode::Prefer::System`]）。
    /// 入っているとは限らない。フレームはメモリーで受ける。
    pub fn open_software(codec: Codec, framing: Framing) -> Result<Self, Error> {
        let names: &[&str] = match codec {
            Codec::Mpeg2 => &["c2.android.mpeg2.decoder", "OMX.google.mpeg2.decoder"],
            Codec::H264 => &["c2.android.avc.decoder", "OMX.google.h264.decoder"],
            Codec::Hevc => &["c2.android.hevc.decoder", "OMX.google.hevc.decoder"],
        };
        for name in names {
            if let Ok(mut decoder) = Self::open_named(codec, framing, Some(name), Route::Memory) {
                decoder.software = true;
                return Ok(decoder);
            }
        }
        Err(Error::Unavailable(format!(
            "この端末に AOSP のソフトウェアデコーダーがありません（{}）",
            names.join("・")
        )))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 直接出力の出力先。ウィンドウはデコーダーより長く生きること。最初の単位の前に呼ぶ。
    pub fn onto(&mut self, window: std::sync::Arc<ndk::native_window::NativeWindow>) {
        self.window = Some(window);
    }

    pub fn weaving(&mut self, yes: bool) {
        self.weave = yes;
    }

    pub fn also(&mut self, key: &str, value: i32) {
        self.extra.push((key.to_owned(), value));
    }

    pub fn declared(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        match (&session.codec, &session.direct) {
            (Some(codec), _) => Some(codec.output_format().to_string()),
            (None, Some(direct)) => direct.declared().map(|shape| shape.to_string()),
            (None, None) => None,
        }
    }

    pub fn tracing(&mut self, trace: Trace) {
        self.trace = Some(trace);
    }

    /// 試すたびに記録する段階（[`Android::step`] は一度しか記録しない）。
    fn note(&mut self, what: &str) {
        if let Some(trace) = self.trace.as_mut() {
            trace(what);
        }
    }

    fn step(&mut self, bit: u32, what: &str) {
        if self.said & bit != 0 {
            return;
        }
        self.said |= bit;
        if let Some(trace) = self.trace.as_mut() {
            trace(what);
        }
    }

    pub fn missed(&self) -> u64 {
        self.missed
    }

    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub fn unreadable(&self) -> u64 {
        self.unreadable
    }

    pub fn why(&self) -> Option<&str> {
        self.why.as_deref()
    }

    pub fn finished(&self) -> bool {
        self.ended
    }

    pub fn spun(&self) -> u64 {
        self.spun
    }

    pub fn route(&self) -> Route {
        self.session.as_ref().map_or(self.wanted, |it| it.route)
    }

    pub fn format(&self) -> Format {
        self.session
            .as_ref()
            .map_or(Format::default(), |it| it.format)
    }
}

impl Drop for Android {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl Decode for Android {
    fn name(&self) -> String {
        format!("MediaCodec {}", self.name)
    }

    fn hardware(&self) -> bool {
        !self.software
    }

    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if self.codec == Codec::Mpeg2 && self.csd.is_none() {
            self.csd = media::mpeg2_csd(data).map(|range| data[range].to_vec());
        }
        self.configure(format)?;
        // MMT は長さ付き、MediaCodec はスタートコードを取るので書き直す。
        let mut rewritten = Vec::new();
        if self.framing == Framing::Lengths {
            media::to_start_codes(data, &mut rewritten);
        }
        let bytes: &[u8] = match self.framing {
            Framing::StartCodes => data,
            Framing::Lengths => &rewritten,
        };
        if self.codec == Codec::Mpeg2 && picture.structure.field() {
            return self.pair(tag, picture.structure, bytes);
        }
        // 対の片方の後にフレームが来たら、その片方は壊れている。
        if let Some((held, bytes)) = self.half.take() {
            self.submit(held, &bytes)?;
        }
        self.submit(tag, bytes)
    }
    fn take(&mut self) -> Option<Decoded> {
        if self.ready.is_empty() {
            self.drain(POLL);
            self.drain_ending();
        }
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        if self.session.is_none() || self.ending {
            return;
        }
        if let Some((tag, bytes)) = self.half.take() {
            self.parity = None;
            let _ = self.submit(tag, &bytes);
        }
        if self.fed {
            // end of stream だけが並べ替えバッファに残ったフレームを出させる。
            self.step(step::SENT, "end of stream を送ります");
            // end of stream にも入力バッファが要る。出力が詰まることがあるので取り出しながら待つ。
            // `ending` を先に立て、[`Android::room`] に広い上限を使わせる。
            self.ending = true;
            let began = std::time::Instant::now();
            let mut sent = false;
            for attempt in 0.. {
                if attempt >= TRIES && began.elapsed() > FEEDING {
                    break;
                }
                self.drain(if attempt == 0 { POLL } else { WAIT });
                let Some(session) = self.session.as_ref() else {
                    break;
                };
                if let Some(direct) = session.direct() {
                    if let Some(index) = direct.input(WAIT) {
                        sent = direct
                            .feed(index, &[], 0, media::FLAG_END_OF_STREAM)
                            .is_ok();
                        break;
                    }
                    continue;
                }
                let Some(codec) = session.codec.as_deref() else {
                    break;
                };
                match codec.dequeue_input_buffer(WAIT) {
                    Ok(DequeuedInputBufferResult::Buffer(input)) => {
                        sent = codec
                            .queue_input_buffer(input, 0, 0, 0, media::FLAG_END_OF_STREAM)
                            .is_ok();
                        break;
                    }
                    Ok(DequeuedInputBufferResult::TryAgainLater) => continue,
                    Err(_) => break,
                }
            }
            if !sent {
                self.step(
                    step::LATE,
                    "end of stream を渡す入力バッファが空きませんでした",
                );
            }
        }
        self.ending = true;
        self.ended = false;
        // `AMediaCodec_flush` は呼ばない。end of stream で出すフレームを捨ててしまう。
        self.drain_ending();
    }

    fn reset(&mut self) {
        self.ready.clear();
        self.holding.clear();
        // AImageReader に出したフレームは解放してから忘れる。忘れるだけだと空きが減り、タグとの対応がずれる。
        while let Some(tag) = self.pending.front().copied() {
            if matches!(self.acquire(tag), Taken::Full) {
                break;
            }
            self.pending.pop_front();
        }
        self.ready.clear();
        self.pending.clear();
        self.half = None;
        self.parity = None;
        self.ending = false;
        self.ended = false;
        if let Some(session) = &self.session {
            session.give(|codec| {
                let _ = codec.flush();
            });
        }
        self.fed = false;
    }

    fn read_back(&mut self) -> Option<Readback> {
        if self.ready.is_empty() {
            self.drain(POLL);
            self.drain_ending();
        }
        self.readback()
    }
}
