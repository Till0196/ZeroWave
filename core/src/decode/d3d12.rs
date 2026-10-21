// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! D3D12 Video Decode. The decoder writes into wgpu's own device, so nothing is copied.
//! D3D11VA ([`d3d11`]) is the fallback, by default only on AMD; `ZW_DECODE_API` pins either.

pub(crate) mod d3d11;
pub mod dx;
mod dxva;
mod h264;
mod hevc;
mod mpeg2;
pub(crate) mod pool;
mod process;
pub(crate) mod share;

use mediadecoder::es::{Codec, Format, Framing, Picture, Structure};
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010};
use windows::Win32::Media::MediaFoundation::{
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE, D3D12_VIDEO_DECODE_FRAME_ARGUMENT,
    D3D12_VIDEO_DECODE_PROFILE_H264, D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN,
    D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN10, D3D12_VIDEO_DECODE_PROFILE_MPEG1_AND_MPEG2,
    D3D12_VIDEO_DECODE_PROFILE_MPEG2, D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_FIELD_BASED,
    D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE, ID3D12VideoDecoder, ID3D12VideoDecoderHeap,
};
use windows::core::GUID;

use crate::decode::each_front;
use crate::decode::{Decode, Decoded, Error, Held as Where, Readback, Surface};
use pool::Pool;

pub struct Frame {
    /// Compared against wgpu's by the importer: another device's picture draws as noise.
    pub device: ID3D12Device,
    pub texture: ID3D12Resource,
    /// The texture is whole coding blocks with the picture in the corner.
    pub width: u32,
    pub height: u32,
    pub ten_bit: bool,
    /// このフレームがまだどこかで保持されていることを示すリース。実際に解放されたかで判定する（数え合わせだと別のフレームが出る）。
    /// `None` は DXVA のリング。
    pub lease: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// 書き終わるフェンスと値。読む側は自分のキューで GPU の上で待つ。`None` は渡す前に CPU で待ってある。
    pub ready: Option<(windows::Win32::Graphics::Direct3D12::ID3D12Fence, u64)>,
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Self {
            device: self.device.clone(),
            texture: self.texture.clone(),
            width: self.width,
            height: self.height,
            ten_bit: self.ten_bit,
            lease: self.lease.clone(),
            ready: self.ready.clone(),
        }
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Frame({}x{}, {})",
            self.width,
            self.height,
            if self.ten_bit { "P010" } else { "NV12" }
        )
    }
}

pub type Held = crate::decode::frames::Held<usize>;

/// One argument of a decode call. The pointer is not kept past the submission.
fn argument<T>(
    kind: D3D12_VIDEO_DECODE_ARGUMENT_TYPE,
    value: &T,
) -> D3D12_VIDEO_DECODE_FRAME_ARGUMENT {
    D3D12_VIDEO_DECODE_FRAME_ARGUMENT {
        Type: kind,
        Size: size_of::<T>() as u32,
        pData: std::ptr::from_ref(value).cast_mut().cast(),
    }
}

fn slice_argument<T>(
    kind: D3D12_VIDEO_DECODE_ARGUMENT_TYPE,
    values: &[T],
) -> D3D12_VIDEO_DECODE_FRAME_ARGUMENT {
    D3D12_VIDEO_DECODE_FRAME_ARGUMENT {
        Type: kind,
        Size: std::mem::size_of_val(values) as u32,
        pData: values.as_ptr().cast_mut().cast(),
    }
}

/// How a picture's textures are named to the driver: an array as long as the pool, indexed by slot.
#[derive(Debug, Clone, Copy)]
enum References<'a> {
    BySlot(&'a [usize]),
}

#[derive(Debug)]
pub struct Session {
    api: Api,
    pub pool: Pool,
    format: Format,
    held: usize,
    /// MPEG-2 の初期化用ピクチャーをデコードさせたか（[`mpeg2::Mpeg2::prime`]）。開くたびに一度。
    primed: bool,
}

#[derive(Debug)]
enum Api {
    D3d12 {
        decoder: ID3D12VideoDecoder,
        heap: ID3D12VideoDecoderHeap,
    },
    D3d11(Box<d3d11::Video>),
}

impl Session {
    /// 代わりのピクチャー（規格 8.3.3）を埋めて参照に使えるようにする。書けないとき（参照専用のコピー、
    /// D3D11VA）は中身を決めない。
    fn fill_stand_ins(&mut self, device: &mut dx::Device) {
        for slot in self.pool.take_stand_ins() {
            if matches!(self.api, Api::D3d12 { .. })
                && let Some((texture, planes)) = self.pool.writable(slot)
                && let Err(error) = device.gray(&texture, planes)
            {
                tracing::warn!("D3D12: 代わりのピクチャーを埋められない: {error:?}");
            }
            self.pool.stood_in(slot);
        }
    }

    fn submit(
        &mut self,
        device: &mut dx::Device,
        slot: usize,
        used: References<'_>,
        bytes: &[u8],
        arguments: &[D3D12_VIDEO_DECODE_FRAME_ARGUMENT],
        macroblocks: u32,
    ) -> Result<(), Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Api::D3d11(video) = &mut self.api {
            let References::BySlot(used) = used;
            if used
                .iter()
                .any(|named| *named != slot && self.pool.written(*named) == 0)
            {
                return Ok(());
            }
            let value = video.submit(slot, bytes, arguments, macroblocks)?;
            self.pool.wrote(slot, value);
            return Ok(());
        }
        let Api::D3d12 { decoder, heap } = &self.api else {
            return Ok(());
        };
        let (decoder, heap) = (decoder.clone(), heap.clone());
        let handed = self
            .pool
            .texture(slot)
            .ok_or(Error::Damaged("a picture with no texture"))?
            .clone();
        // 変換の出力も渡すテクスチャへ直接書かせる（AMD の SDMA はテクスチャ間のコピーを取りこぼす）。
        let direct = self.pool.direct(slot).is_some();
        // 一度も書いていないテクスチャは先に黒で埋める（全部 0 の NV12 は真緑）。
        // 参照専用のコピーは埋めない：`VIDEO_DECODE_REFERENCE_ONLY` はコピー先にできず `Close` が失敗する。
        if self.pool.blank(slot)
            && !direct
            && let Err(error) = device.blacken(&handed, 2)
        {
            tracing::warn!("D3D12: テクスチャを黒で初期化できない: {error:?}");
        }
        let References::BySlot(used) = used;
        // まだ誰も書いていないテクスチャからは予測させない（緑が残る）。自分自身は除く
        // （MPEG-2 と H.264 の二つ目のフィールドは現在のスロットを指す）。
        if used
            .iter()
            .any(|named| *named != slot && self.pool.written(*named) == 0)
        {
            return Ok(());
        }
        let (references, subresources, twin) = self.pool.references(used, slot);
        let value = device.decode(
            &decoder,
            &heap,
            &handed,
            twin,
            references,
            subresources,
            bytes,
            arguments,
            slot,
        )?;
        self.pool.wrote(slot, value);
        if let Some((array, planes)) = self.pool.direct(slot) {
            match device.copy_planes(&array, planes, &handed, value) {
                Ok(copied) => self.pool.was_copied(slot, copied),
                Err(error) => tracing::warn!("D3D12: フレームをコピーできない: {error:?}"),
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
enum Front {
    Mpeg2(Box<mpeg2::Mpeg2>),
    H264(Box<h264::H264>),
    Hevc(Box<hevc::Hevc>),
}

#[derive(Debug)]
pub struct D3d12 {
    device: dx::Device,
    codec: Codec,
    front: Front,
    session: Option<Session>,
    format: Format,
    said_unwritten: bool,
    skipped_by_driver: u64,
    framing: Framing,
    buffering: Option<usize>,
    d3d11: bool,
    /// レンダリングと違う GPU でデコードしている。別デバイスのテクスチャは読めないのでメインメモリへ読み戻す。
    transfer: bool,
    /// ビデオプロセスでデインターレースするか（開いたときの設定）。
    wants: crate::decode::VideoProcess,
    deinterlacer: Option<process::Deinterlacer>,
    processed: std::collections::VecDeque<Decoded>,
    /// 一枚が一フィールドのピクチャーのパリティー（tag ごと、トップなら真）。
    parities: std::collections::BTreeMap<u64, bool>,
}

/// 固定の指定。環境変数 `ZW_DECODE_API` が優先で、無ければ設定（[`crate::decode::api`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forced {
    D3d11,
    D3d12,
}

fn forced() -> Option<Forced> {
    let said = std::env::var("ZW_DECODE_API")
        .ok()
        .map(|value| value.to_ascii_lowercase());
    match said.as_deref() {
        Some("d3d11" | "d3d11va") => return Some(Forced::D3d11),
        Some("d3d12") => return Some(Forced::D3d12),
        _ => {}
    }
    match crate::decode::api() {
        crate::decode::Api::Auto => None,
        crate::decode::Api::D3d12 => Some(Forced::D3d12),
        crate::decode::Api::D3d11 => Some(Forced::D3d11),
    }
}

impl D3d12 {
    /// Opens a decoder for a coding, or says the machine has none. The pool waits for the
    /// sequence header.
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_in(codec, framing, dx::Device::open()?, forced())
    }

    pub fn open_pick(
        codec: Codec,
        framing: Framing,
        device: &str,
        way: crate::decode::Way,
    ) -> Result<Self, Error> {
        let force = match way {
            crate::decode::Way::D3d12 => Some(Forced::D3d12),
            crate::decode::Way::D3d11 => Some(Forced::D3d11),
            _ => forced(),
        };
        Self::open_in(codec, framing, dx::Device::open_on(device)?, force)
    }

    fn open_in(
        codec: Codec,
        framing: Framing,
        device: dx::Device,
        force: Option<Forced>,
    ) -> Result<Self, Error> {
        let mut refusal = device.video_error.clone().map(Error::Unavailable);
        let supported = force != Some(Forced::D3d11)
            && device.video_error.is_none()
            && profiles(codec, 8).iter().any(|(profile, format)| {
                match device.supports(
                    *profile,
                    *format,
                    1920,
                    1080,
                    D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE,
                ) {
                    Ok(_) => true,
                    Err(error) => {
                        refusal.get_or_insert(error);
                        false
                    }
                }
            });
        // D3D12 が対応していなければ D3D11 に問い合わせる（MPEG-2 を D3D12 では扱わないドライバーがある）。
        let d3d11 = !supported
            && force != Some(Forced::D3d12)
            && profiles(codec, 8).iter().any(|(profile, format)| {
                match d3d11::supports(&device.raw, *profile, *format, 1920, 1088) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!("D3D11VA: {error}");
                        false
                    }
                }
            });
        if d3d11 {
            tracing::warn!(
                "デコーダー {codec:?}: D3D11VA を使う（{}）",
                if force == Some(Forced::D3d11) {
                    "設定で固定"
                } else {
                    "D3D12 は非対応"
                }
            );
        }
        if !supported && !d3d11 {
            let why = refusal
                .map(|error| error.to_string())
                .unwrap_or_else(|| format!("{codec:?} をデコードできません"));
            return Err(Error::Unavailable(format!(
                "{why}。ドライバーが挙げるプロファイル: {}",
                device.profiles()
            )));
        }
        let front = match codec {
            Codec::Mpeg2 => Front::Mpeg2(Box::default()),
            Codec::H264 => Front::H264(Box::new(h264::H264::new(framing))),
            Codec::Hevc => Front::Hevc(Box::new(hevc::Hevc::new(framing))),
        };
        let transfer = !device.shared && !device.adapter.is_empty();
        Ok(Self {
            transfer,
            device,
            codec,
            front,
            session: None,
            format: Format::default(),
            said_unwritten: false,
            skipped_by_driver: 0,
            framing,
            buffering: None,
            d3d11,
            wants: crate::decode::video_process(),
            deinterlacer: None,
            processed: std::collections::VecDeque::new(),
            parities: std::collections::BTreeMap::new(),
        })
    }

    pub fn name(&self) -> String {
        let name = if self.d3d11 {
            self.device.name().replacen("D3D12", "D3D11VA", 1)
        } else {
            self.device.name()
        };
        match self.wants {
            _ if !self.processes() => name,
            crate::decode::VideoProcess::Bob => format!("{name}・デインターレース bob"),
            _ => format!("{name}・デインターレース 適応型"),
        }
    }

    /// デコードと同じスレッドでビデオプロセスに通してデインターレースする。
    fn processes(&self) -> bool {
        self.wants != crate::decode::VideoProcess::Off
            && !self.transfer
            && !self
                .deinterlacer
                .as_ref()
                .is_some_and(process::Deinterlacer::refused)
    }

    /// 一枚が一フィールドのピクチャーに、どちらのフィールドかを付ける（組まずに渡し、レンダラーが半行ずらして
    /// 縦に伸ばす）。
    fn mark_field(&mut self, decoded: &mut Decoded) {
        let Decoded::Surface(surface) = decoded else {
            return;
        };
        if !surface.about.format.field_pictures {
            return;
        }
        if let Some(top) = self.parities.remove(&surface.about.tag) {
            surface.about.structure = if top {
                Structure::Top
            } else {
                Structure::Bottom
            };
        }
    }

    /// インターレースのフレームと画面よりずっと大きいフレームをビデオプロセスへ渡し、出来たフレームを `processed` に
    /// 積む。渡さなければ返す。
    fn process(&mut self, decoded: Decoded) -> Option<Decoded> {
        let Decoded::Surface(Surface {
            about,
            held: Where::D3d12(frame),
        }) = decoded
        else {
            return Some(decoded);
        };
        // インターレースはデインターレースする。プログレッシブは画面の二倍以上あるときだけ半分に縮める。
        let output = if about.format.progressive {
            crate::decode::display_size()
                .filter(|[wide, tall]| frame.width >= wide * 2 && frame.height >= tall * 2)
                .map(|_| [(frame.width / 2) & !1, (frame.height / 2) & !1])
        } else {
            Some([frame.width, frame.height])
        };
        let output = output.filter(|_| self.processes() && !about.format.field_pictures);
        let Some(output) = output else {
            return Some(Decoded::Surface(Surface {
                about,
                held: Where::D3d12(frame),
            }));
        };
        if self.deinterlacer.is_none() {
            match process::Deinterlacer::new(
                &frame.device,
                self.wants == crate::decode::VideoProcess::Bob,
            ) {
                Ok(made) => self.deinterlacer = Some(made),
                Err(error) => {
                    tracing::info!("D3D12: ビデオプロセスが使えない: {error:?}");
                    self.wants = crate::decode::VideoProcess::Off;
                    return Some(Decoded::Surface(Surface {
                        about,
                        held: Where::D3d12(frame),
                    }));
                }
            }
        }
        let deinterlacer = self.deinterlacer.as_mut()?;
        match deinterlacer.push(frame, about, output) {
            Ok(made) => self.processed.extend(made),
            Err(error) => {
                tracing::warn!(
                    "D3D12: ビデオプロセスに失敗したので、シェーダーで処理する: {error:?}"
                );
                self.wants = crate::decode::VideoProcess::Off;
            }
        }
        None
    }

    fn wait_written(&self, value: u64) -> Result<(), Error> {
        match self.session.as_ref().map(|session| &session.api) {
            Some(Api::D3d11(video)) => video.wait(value),
            _ => self.device.wait(value),
        }
    }

    fn configure_d3d11(&mut self, format: Format, held: usize) -> Result<(), Error> {
        // アラインメント（MPEG-2 はフィールドの高さも 16 の倍数になるよう 32）。
        let align = match self.codec {
            Codec::Hevc => 128,
            Codec::Mpeg2 => 32,
            Codec::H264 => 16,
        };
        let width = format.width.next_multiple_of(align);
        let height = format.height.next_multiple_of(align);
        let count = held + pool::SLACK;
        let mut refusal = None;
        for (profile, dxgi) in profiles(self.codec, format.depth) {
            match d3d11::Video::open(&self.device.raw, *profile, *dxgi, width, height, count) {
                Ok((video, textures)) => {
                    self.session = Some(Session {
                        api: Api::D3d11(Box::new(video)),
                        pool: Pool::new(textures, pool::Twins::None, pool::SLACK, count, 0),
                        format,
                        held,
                        primed: false,
                    });
                    self.format = format;
                    self.d3d11 = true;
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!("D3D11VA: {error}");
                    refusal.get_or_insert(error);
                }
            }
        }
        Err(refusal.unwrap_or(Error::Unsupported("a picture nothing decodes")))
    }

    fn configure(&mut self, format: Format, data: &[u8]) -> Result<(), Error> {
        if self.codec == Codec::Hevc
            && let Some(buffering) = hevc::buffering(data, self.framing)
        {
            self.buffering = Some(buffering);
        }
        let held = match &self.front {
            Front::Mpeg2(_) => 4,
            Front::H264(_) => 17,
            Front::Hevc(_) => self
                .buffering
                .map_or(16, |buffering| (buffering + 1).min(17)),
        };
        if let Some(session) = &self.session
            && crate::decode::session_fits(session.format, format)
            && session.held >= held
        {
            self.format = format;
            return Ok(());
        }
        self.teardown();
        crate::decode::openable(format)?;
        if self.d3d11 {
            return self.configure_d3d11(format, held);
        }
        let align = if self.codec == Codec::Hevc { 64 } else { 16 };
        let width = format.width.next_multiple_of(align);
        let mut height = format.height.next_multiple_of(align);
        // Profiles in order, so 8-bit HEVC still decodes on a Main10-only driver.
        // Interlaced MPEG-2 and H.264 must be declared `FIELD_BASED` (the DDI says so); with `NONE`
        // some drivers silently skip the second field. HEVC stays `NONE`.
        let interlace = if !format.progressive && matches!(self.codec, Codec::Mpeg2 | Codec::H264) {
            D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_FIELD_BASED
        } else {
            D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE
        };
        let mut chosen = None;
        let mut refusal = None;
        for (profile, format) in profiles(self.codec, format.depth) {
            match self
                .device
                .supports(*profile, *format, width, height, interlace)
            {
                Ok(terms) => {
                    tracing::info!(
                        "D3D12: プロファイル {}、{}",
                        dx::profile_name(*profile),
                        if interlace == D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE {
                            "フレーム構造"
                        } else {
                            "フィールド構造"
                        },
                    );
                    chosen = Some((terms, *format));
                    break;
                }
                Err(error) => {
                    refusal.get_or_insert(error);
                }
            }
        }
        let Some((terms, dxgi)) = chosen else {
            return Err(refusal.unwrap_or(Error::Unsupported("a picture nothing decodes")));
        };
        let configuration = terms.configuration;
        if terms.rows_of_32 {
            height = height.next_multiple_of(32);
        }
        // HEVC の代わりのピクチャー（規格 8.3.3）は参照ピクチャーセットにある間だけ残るので、同時に要るのは
        // 参照の数（`sps_max_dec_pic_buffering_minus1`）まで。
        let stand_ins = match &self.front {
            Front::Hevc(_) => held.saturating_sub(2),
            _ => 0,
        };
        let count = held + pool::SLACK + stand_ins;
        // DXVA の番号（`Index7Bits`）の数は DPB の枚数にする（[`Pool`]）。デコーダーヒープの
        // `MaxDecodePictureBufferCount` と `ReferenceFrames` の長さ、Tier 1 の配列のスライスの数も同じ。
        // テクスチャは渡したフレームの分だけ多い。
        let slices = held;
        let mut textures = Vec::with_capacity(count);
        for at in 0..count {
            let texture = self.device.texture(dxgi, width, height, false, 1)?;
            dx::name(&texture, &format!("zerowave サーフェス{at}"));
            textures.push(texture);
        }
        let twins = if terms.arrayed {
            let array =
                self.device
                    .texture(dxgi, width, height, terms.reference_only, slices as u32)?;
            dx::name(&array, "zerowave 参照の配列");
            pool::Twins::Array {
                array,
                slices,
                direct: !terms.reference_only,
            }
        } else if terms.reference_only {
            let mut each = Vec::with_capacity(count);
            for at in 0..count {
                let twin = self.device.texture(dxgi, width, height, true, 1)?;
                dx::name(&twin, &format!("zerowave 参照用コピー{at}"));
                each.push(twin);
            }
            pool::Twins::Each(each)
        } else {
            pool::Twins::None
        };
        let (decoder, heap) =
            self.device
                .decoder(configuration, dxgi, width, height, slices as u32)?;
        dx::name(&decoder, "zerowave デコーダー");
        dx::name(&heap, "zerowave デコーダーのヒープ");
        if terms.reference_only {
            let (ok, said) = self.device.converts(configuration, dxgi, width, height);
            if !ok {
                tracing::info!("D3D12 出力変換: {said}");
            }
        }
        self.session = Some(Session {
            api: Api::D3d12 { decoder, heap },
            pool: Pool::new(textures, twins, pool::SLACK, slices, stand_ins),
            format,
            held,
            primed: false,
        });
        self.format = format;
        Ok(())
    }

    fn teardown(&mut self) {
        // デインターレースを待つフレームはデコーダーのサーフェスを持っている。先に放す。
        self.processed.clear();
        self.parities.clear();
        if let Some(deinterlacer) = self.deinterlacer.as_mut() {
            deinterlacer.reset();
        }
        let Some(mut session) = self.session.take() else {
            return;
        };
        self.device.wait_idle();
        each_front!(&mut self.front, |front| front.reset(&mut session.pool))
    }

    fn next_held(&mut self) -> Option<Held> {
        each_front!(&mut self.front, |front| front.take())
    }
}

fn profiles(codec: Codec, depth: u8) -> &'static [(GUID, DXGI_FORMAT)] {
    match (codec, depth > 8) {
        // `MPEG1_AND_MPEG2` first: some drivers claim both GUIDs but refuse every decode under
        // `MPEG2` at `Close` with `E_INVALIDARG`.
        (Codec::Mpeg2, _) => &[
            (D3D12_VIDEO_DECODE_PROFILE_MPEG1_AND_MPEG2, DXGI_FORMAT_NV12),
            (D3D12_VIDEO_DECODE_PROFILE_MPEG2, DXGI_FORMAT_NV12),
        ],
        (Codec::H264, _) => &[(D3D12_VIDEO_DECODE_PROFILE_H264, DXGI_FORMAT_NV12)],
        (Codec::Hevc, false) => &[
            (D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN, DXGI_FORMAT_NV12),
            (D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN10, DXGI_FORMAT_P010),
        ],
        (Codec::Hevc, true) => &[(D3D12_VIDEO_DECODE_PROFILE_HEVC_MAIN10, DXGI_FORMAT_P010)],
    }
}

impl Drop for D3d12 {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl Decode for D3d12 {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        self.configure(format, data)?;
        if format.field_pictures
            && let Some(field) = picture.lone_field
        {
            self.parities.insert(tag, field == Structure::Top);
            while self.parities.len() > 64 {
                self.parities.pop_first();
            }
        }
        let Some(session) = self.session.as_mut() else {
            return Err(Error::Unavailable(String::from("サーフェスがありません")));
        };
        each_front!(&mut self.front, |front| {
            front.decode(&mut self.device, session, tag, picture, format, data)
        })
    }

    fn take(&mut self) -> Option<Decoded> {
        if self.transfer {
            while let Some(read) = Decode::read_back(self) {
                if let Some(frame) = crate::decode::frame_of(&read) {
                    return Some(Decoded::Planes(frame));
                }
            }
            return None;
        }
        loop {
            if let Some(made) = self.processed.pop_front() {
                return Some(made);
            }
            match self.take_one() {
                Taken::Picture(mut decoded) => {
                    self.mark_field(&mut decoded);
                    if let Some(decoded) = self.process(decoded) {
                        return Some(decoded);
                    }
                }
                Taken::Skipped => {}
                Taken::Empty => return None,
            }
        }
    }

    fn name(&self) -> String {
        D3d12::name(self)
    }

    fn flush(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        each_front!(&mut self.front, |front| front.flush(&mut session.pool));
        // 後ろの参照を待っているフレームも出す。
        if self.deinterlacer.is_some() {
            loop {
                match self.take_one() {
                    Taken::Picture(mut decoded) => {
                        self.mark_field(&mut decoded);
                        if let Some(decoded) = self.process(decoded) {
                            self.processed.push_back(decoded);
                        }
                    }
                    Taken::Skipped => {}
                    Taken::Empty => break,
                }
            }
            if let Some(deinterlacer) = self.deinterlacer.as_mut() {
                self.processed.extend(deinterlacer.flush());
            }
        }
    }

    fn read_back(&mut self) -> Option<Readback> {
        // 回復中にデコードしたフレーム（[`Pool::hide`]）は渡さない。
        let held = loop {
            let held = self.next_held()?;
            let session = self.session.as_mut()?;
            if !session.pool.hidden(held.slot) {
                break held;
            }
            session.pool.issued(held.slot);
        };
        let about = held.about(self.format);
        let texture = self.session.as_ref()?.pool.texture(held.slot)?.clone();
        let value = self.session.as_ref()?.pool.written(held.slot);
        if let Err(error) = self.wait_written(value) {
            tracing::warn!("D3D12: デコード完了の待機に失敗: {error:?}");
            return None;
        }
        let read = self.device.read_back(&texture, 2);
        if let Some(session) = self.session.as_mut() {
            session.pool.issued(held.slot);
        }
        match read {
            Ok(read) => Some(Readback {
                about,
                planes: read.planes,
            }),
            Err(error) => {
                tracing::warn!("D3D12: 読み戻せない: {error:?}");
                None
            }
        }
    }

    fn reset(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        each_front!(&mut self.front, |front| front.reset(&mut session.pool));
        session.pool.forget_recent();
        self.processed.clear();
        if let Some(deinterlacer) = self.deinterlacer.as_mut() {
            deinterlacer.reset();
        }
    }

    fn deinterlaces(&self) -> bool {
        self.processes()
    }

    fn marks_fields(&self) -> bool {
        !self.transfer
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.processes() { 2 } else { 1 }
    }
}

enum Taken {
    Picture(Decoded),
    Skipped,
    Empty,
}

impl D3d12 {
    /// 一枚取り出す。書かれていない（fence の値が 0 の）テクスチャは渡さない（真緑に出る）。
    fn take_one(&mut self) -> Taken {
        let Some(held) = self.next_held() else {
            return Taken::Empty;
        };
        let about = held.about(self.format);
        let Some(session) = self.session.as_mut() else {
            return Taken::Empty;
        };
        let Some(texture) = session.pool.texture(held.slot).cloned() else {
            return Taken::Empty;
        };
        let value = session.pool.written(held.slot);
        let lease = session.pool.leased(held.slot);
        if session.pool.hidden(held.slot) {
            drop(lease);
            return Taken::Skipped;
        }
        if value == 0 {
            drop(lease);
            if !self.said_unwritten {
                self.said_unwritten = true;
                tracing::info!(
                    "D3D12: 参照がそろっていないフレームを捨てた（tag {}）",
                    held.tag
                );
            }
            return Taken::Skipped;
        }
        if let Err(error) = self.wait_written(value) {
            tracing::warn!("D3D12: デコード完了の待機に失敗: {error:?}");
            return Taken::Empty;
        }
        let copied = self
            .session
            .as_ref()
            .map_or(0, |session| session.pool.copied(held.slot));
        if copied != 0
            && let Err(error) = self.device.wait_copy(copied)
        {
            tracing::warn!("D3D12: コピー完了の待機に失敗: {error:?}");
            return Taken::Empty;
        }
        // ドライバーが表示しないよう指示したフレーム（`CONTINUE_SKIP_DISPLAY`）は出さない（何も書かれていない）。
        if self.device.told_to_skip(held.slot) {
            drop(lease);
            self.skipped_by_driver += 1;
            if self.skipped_by_driver == 1 {
                tracing::info!(
                    "D3D12: ドライバーが表示しないよう指示したフレームを捨てた（tag {}）",
                    held.tag
                );
            }
            return Taken::Skipped;
        }
        Taken::Picture(Decoded::Surface(Surface {
            about,
            held: Where::D3d12(Frame {
                device: self.device.raw.clone(),
                texture,
                width: self.format.width,
                height: self.format.height,
                ten_bit: self.format.depth > 8,
                lease: Some(lease),
                ready: None,
            }),
        }))
    }
}

/// 設定ウィンドウに並べる GPU。レンダリングに使う GPU を先に。
pub fn offers(codec: Codec) -> Vec<crate::decode::Offer> {
    use crate::decode::{Offer, Way};
    let mut offers = Vec::new();
    for adapter in dx::adapters() {
        if offers
            .iter()
            .any(|offer: &crate::decode::Offer| offer.device == adapter.name)
        {
            continue;
        }
        let Some(device) = dx::Device::asking(&adapter) else {
            continue;
        };
        let d3d12 = profiles(codec, 8).iter().any(|(profile, format)| {
            device
                .supports(
                    *profile,
                    *format,
                    1920,
                    1080,
                    D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE,
                )
                .is_ok()
        });
        let d3d11 = profiles(codec, 8).iter().any(|(profile, format)| {
            d3d11::supports(&device.raw, *profile, *format, 1920, 1088).is_ok()
        });
        let draws = dx::draws_on(&adapter.name);
        let nvdec = cfg!(feature = "nvdec") && adapter.vendor == 0x10de && draws;
        let mut ways = Vec::new();
        if d3d12 || d3d11 {
            ways.push(Way::Auto);
        }
        if d3d12 {
            ways.push(Way::D3d12);
        }
        if d3d11 {
            ways.push(Way::D3d11);
        }
        if nvdec {
            ways.push(Way::Nvdec);
        }
        if ways.is_empty() {
            continue;
        }
        offers.push(Offer {
            device: adapter.name,
            ways,
            gpu: true,
            draws,
        });
    }
    offers.sort_by_key(|offer| !offer.draws);
    offers
}
