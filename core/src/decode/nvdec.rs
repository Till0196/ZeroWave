// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! NVDEC: ドライバー同梱の `libnvcuvid.so.1` で MPEG-2・H.264・HEVC をデコードする。
//! マップしたアドレスはエクスポートできないので、Linux では線形の割り当てへ一回コピーして DMA-BUF で渡す。
//! ヘッダーの解析は自前で行い、NVIDIA のパーサーは使わない（出力順と `deck --check` のため）。
//! スタートコードは 3 バイト。4 バイトだと HEVC はフレームを一枚も出さない。

pub mod cu;
#[cfg(all(feature = "d3d12", windows))]
mod d3d12;
mod h264;
mod hevc;
mod mpeg2;
mod pool;

use std::sync::Arc;

use mediadecoder::es::{Codec, Format, Framing, Picture};

use crate::decode::each_front;
use crate::decode::{About, Decode, Error, Readback};
#[cfg(unix)]
use crate::decode::{Decoded, DmaBuf, DmaBufPlane, Held as Where, Surface};
use crate::trace::Trace;
use pool::Pool;

pub type Held = crate::decode::frames::Held<usize>;

#[derive(Debug)]
enum Front {
    Mpeg2(Box<mpeg2::Mpeg2>),
    H264(Box<h264::H264>),
    Hevc(Box<hevc::Hevc>),
}

/// コピー先の枚数。出したフレームは同時に高々一枚しか使われないので四枚で足りる。
#[cfg(unix)]
const STAGING: usize = 4;

struct Session {
    decoder: cu::Decoder,
    pool: Pool,
    format: Format,
    #[cfg(unix)]
    staging: Vec<cu::Linear>,
    #[cfg(unix)]
    next: usize,
    #[cfg(all(feature = "d3d12", windows))]
    bridge: Option<d3d12::Bridge>,
    #[cfg(all(feature = "d3d12", windows))]
    bridge_refused: bool,
    /// サーフェスの高さ。色差のプレーンはここから始まる。
    surface_height: usize,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Session({}x{})", self.format.width, self.format.height)
    }
}

#[derive(Debug)]
pub struct Nvdec {
    context: Arc<cu::Context>,
    codec: std::ffi::c_int,
    front: Front,
    session: Option<Session>,
    format: Format,
    #[cfg(any(unix, all(feature = "d3d12", windows)))]
    said_staging: bool,
    said_read_back: bool,
    deinterlace: Option<crate::decode::HwDeinterlace>,
    /// 一枚目のフィールドを出したフレーム。二枚目を出してからスロットを返す。
    second: Option<Held>,
    #[cfg(unix)]
    said_memory: bool,
}

impl Nvdec {
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_with(codec, framing, None)
    }

    pub fn open_with(
        codec: Codec,
        framing: Framing,
        deinterlace: Option<crate::decode::HwDeinterlace>,
    ) -> Result<Self, Error> {
        if deinterlace == Some(crate::decode::HwDeinterlace::MotionCompensated) {
            return Err(Error::Unavailable(String::from(
                "NVDEC に動き補償のデインターレースはありません",
            )));
        }
        // D3D12 のデバイスと同じ GPU の CUDA を開く（外部メモリーは同じデバイスでしかインポートできない）。
        #[cfg(all(feature = "d3d12", windows))]
        let context = Arc::new(cu::Context::open_matching(d3d12::adopted_luid())?);
        #[cfg(not(all(feature = "d3d12", windows)))]
        let context = Arc::new(cu::Context::open()?);
        let which = match codec {
            Codec::Mpeg2 => cu::CUDA_VIDEO_CODEC_MPEG2,
            Codec::H264 => cu::CUDA_VIDEO_CODEC_H264,
            Codec::Hevc => cu::CUDA_VIDEO_CODEC_HEVC,
        };
        let caps = context.caps(which, 8)?;
        if caps.supported == 0 {
            return Err(Error::Unavailable(format!(
                "{} は {codec:?} をデコードできません",
                context.name
            )));
        }
        let front = match codec {
            Codec::Mpeg2 => Front::Mpeg2(Box::default()),
            Codec::H264 => Front::H264(Box::new(h264::H264::new(framing))),
            Codec::Hevc => Front::Hevc(Box::new(hevc::Hevc::new(framing))),
        };
        let live = LIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        match context.memory() {
            Some((free, total)) => tracing::debug!(
                "NVDEC: デコーダーを開いた（使用中 {live} 個、GPU メモリー空き {free} / {total} MiB）"
            ),
            None => tracing::debug!("NVDEC: デコーダーを開いた（使用中 {live} 個）"),
        }
        Ok(Self {
            context,
            codec: which,
            front,
            session: None,
            format: Format::default(),
            #[cfg(any(unix, all(feature = "d3d12", windows)))]
            said_staging: false,
            said_read_back: false,
            deinterlace,
            second: None,
            #[cfg(unix)]
            said_memory: false,
        })
    }

    pub fn name(&self) -> &str {
        &self.context.name
    }

    fn configure(&mut self, format: Format) -> Result<(), Error> {
        if let Some(session) = &self.session
            && crate::decode::session_fits(session.format, format)
        {
            self.format = format;
            return Ok(());
        }
        self.teardown();
        crate::decode::openable(format)?;
        let wide = format.depth > 8;
        let caps = self.context.caps(self.codec, u32::from(format.depth))?;
        if caps.supported == 0 {
            return Err(Error::Unsupported(
                "this depth of this coding, on this hardware",
            ));
        }
        let width = format.width.next_multiple_of(16);
        let height = format.height.next_multiple_of(16);
        if caps.max_width != 0 && (width > caps.max_width || height > caps.max_height) {
            return Err(Error::Unsupported("a picture larger than the hardware's"));
        }
        let count = match &self.front {
            Front::Mpeg2(_) => 3,
            Front::H264(_) => 17,
            Front::Hevc(_) => 16,
        } + pool::SLACK;
        let info = cu::DecodeCreateInfo {
            width: cu::CuUlong::from(width),
            height: cu::CuUlong::from(height),
            num_decode_surfaces: count as cu::CuUlong,
            codec: self.codec,
            chroma_format: cu::CUDA_VIDEO_CHROMA_420,
            creation_flags: 0,
            bit_depth_minus8: cu::CuUlong::from(format.depth.saturating_sub(8)),
            // 0。値を入れると再設定できる形（`cuvidReconfigureDecoder`）で作ろうとする。
            max_width: 0,
            max_height: 0,
            display_area: [0; 4],
            output_format: if wide {
                cu::CUDA_VIDEO_SURFACE_P016
            } else {
                cu::CUDA_VIDEO_SURFACE_NV12
            },
            deinterlace_mode: match self.deinterlace {
                Some(crate::decode::HwDeinterlace::Adaptive) => cu::CUDA_VIDEO_DEINTERLACE_ADAPTIVE,
                Some(crate::decode::HwDeinterlace::Bob) => cu::CUDA_VIDEO_DEINTERLACE_BOB,
                _ => cu::CUDA_VIDEO_DEINTERLACE_WEAVE,
            },
            target_width: cu::CuUlong::from(width),
            target_height: cu::CuUlong::from(height),
            num_output_surfaces: 1,
            ..cu::DecodeCreateInfo::default()
        };
        if say() {
            tracing::info!(
                "NVDEC: {}x{} depth {} → 出力 {} (mask 0x{:04x}) / サーフェス {count} / 最大 {}x{}",
                width,
                height,
                format.depth,
                info.output_format,
                caps.output_format_mask,
                caps.max_width,
                caps.max_height,
            );
        }
        let decoder = self.context.decoder(info)?;
        self.session = Some(Session {
            decoder,
            pool: Pool::new(count),
            format,
            #[cfg(unix)]
            staging: Vec::new(),
            #[cfg(unix)]
            next: 0,
            #[cfg(all(feature = "d3d12", windows))]
            bridge: None,
            #[cfg(all(feature = "d3d12", windows))]
            bridge_refused: false,
            surface_height: height as usize,
        });
        self.format = format;
        Ok(())
    }

    fn teardown(&mut self) {
        self.second = None;
        let Some(mut session) = self.session.take() else {
            return;
        };
        each_front!(&mut self.front, |front| front.reset(&mut session.pool))
    }

    fn next_held(&mut self) -> Option<Held> {
        each_front!(&mut self.front, |front| front.take())
    }

    fn next_output(&mut self) -> Option<(Held, cu::ProcParams, bool, About)> {
        if let Some(held) = self.second.take() {
            let proc = Self::field(&held, 1);
            return Some((held, proc, true, Self::woven(held.about(self.format))));
        }
        let held = self.next_held()?;
        if self.deinterlace.is_none() {
            return Some((held, Self::untouched(&held), true, held.about(self.format)));
        }
        self.second = Some(held);
        let proc = Self::field(&held, 0);
        Some((held, proc, false, Self::woven(held.about(self.format))))
    }

    fn field(held: &Held, second: i32) -> cu::ProcParams {
        if held.progressive {
            return Self::untouched(held);
        }
        cu::ProcParams {
            progressive_frame: 0,
            second_field: second,
            top_field_first: i32::from(held.top_field_first),
            unpaired_field: 0,
            ..cu::ProcParams::default()
        }
    }

    fn woven(about: About) -> About {
        About {
            structure: mediadecoder::es::Structure::Frame,
            progressive: true,
            repeat_first_field: false,
            ..about
        }
    }

    fn untouched(held: &Held) -> cu::ProcParams {
        cu::ProcParams {
            progressive_frame: 1,
            second_field: 0,
            top_field_first: i32::from(held.top_field_first),
            unpaired_field: 0,
            ..cu::ProcParams::default()
        }
    }

    fn read_back_now(&mut self) -> Option<(About, Readback)> {
        let held = self.next_held()?;
        let about = held.about(self.format);
        let format = self.format;
        let read = {
            let session = self.session.as_mut()?;
            let read = read_into_memory(session, &held, format, Self::untouched(&held));
            session.pool.issued(held.slot);
            read
        };
        match read {
            Ok(planes) => Some((about, Readback { about, planes })),
            Err(error) => {
                if !self.said_read_back {
                    self.said_read_back = true;
                    tracing::warn!("NVDEC: 読み戻せない: {error:?}");
                }
                None
            }
        }
    }
}

pub(crate) fn say() -> bool {
    SAY.on()
}

static SAY: Trace = Trace::of("nvdec");

/// 一枚をメインメモリへ、プレーンごとに詰めたストライドで（ソフトウェアデコーダーと同じ配置）。
fn read_into_memory(
    session: &mut Session,
    held: &Held,
    format: Format,
    mut proc: cu::ProcParams,
) -> Result<Vec<(Vec<u8>, usize, usize)>, Error> {
    let wide = usize::from(format.depth > 8) + 1;
    let mapped = session.decoder.map(session.pool.id(held.slot), &mut proc)?;
    if say() {
        // デコードの結果はマップした後に問い合わせる。前だと「まだ途中」しか返らない。
        let status = session
            .decoder
            .status(session.pool.id(held.slot))
            .map_or_else(|| String::from("取得失敗"), |code| code.to_string());
        tracing::info!(
            "NVDEC: サーフェス {} のステータス {status}（成功は {}）/ ストライド {} / 高さ {}",
            session.pool.id(held.slot),
            cu::CUVID_DECODE_STATUS_SUCCESS,
            mapped.pitch,
            session.surface_height,
        );
    }
    let width = format.width as usize * wide;
    let rows = format.height as usize;
    let mut planes = Vec::with_capacity(2);
    for plane in 0..2 {
        let rows = if plane == 0 { rows } else { rows.div_ceil(2) };
        let mut out = vec![0u8; width * rows];
        let from = mapped.pointer
            + if plane == 0 {
                0
            } else {
                (mapped.pitch * session.surface_height) as u64
            };
        session.decoder.context().copy(&cu::Memcpy2D {
            src_memory_type: cu::CU_MEMORYTYPE_DEVICE,
            src_device: from,
            src_pitch: mapped.pitch,
            dst_memory_type: cu::CU_MEMORYTYPE_HOST,
            dst_host: out.as_mut_ptr().cast(),
            dst_pitch: width,
            width_in_bytes: width,
            height: rows,
            ..cu::Memcpy2D::default()
        })?;
        planes.push((out, width, rows));
    }
    Ok(planes)
}

static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Drop for Nvdec {
    fn drop(&mut self) {
        self.teardown();
        let left = LIVE.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
        match self.context.memory() {
            Some((free, total)) => tracing::debug!(
                "NVDEC: デコーダーを閉じた（使用中 {left} 個、GPU メモリー空き {free} / {total} MiB）"
            ),
            None => tracing::debug!("NVDEC: デコーダーを閉じた（使用中 {left} 個）"),
        }
    }
}

impl Decode for Nvdec {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        self.configure(format)?;
        let Some(session) = self.session.as_mut() else {
            return Err(Error::Unavailable(String::from("サーフェスがありません")));
        };
        each_front!(&mut self.front, |front| front.decode(
            &session.decoder,
            &mut session.pool,
            tag,
            picture,
            format,
            data,
        ))
    }

    #[cfg(unix)]
    fn take(&mut self) -> Option<crate::decode::Decoded> {
        let (held, proc, release, about) = self.next_output()?;
        let format = self.format;
        let session = self.session.as_mut()?;
        // エクスポートできないデバイスはメモリーへ読み戻す。
        let made = if session.decoder.context().exports_dma_buf() && !crate::decode::memory_only() {
            copy_out(session, &held, format, proc).map(|buffer| {
                Decoded::Surface(Surface {
                    about,
                    held: Where::DmaBuf(buffer),
                })
            })
        } else {
            if !self.said_memory {
                self.said_memory = true;
                tracing::warn!(
                    "NVDEC: このデバイスは DMA-BUF に対応していないので、メモリーに読み戻す"
                );
            }
            read_into_memory(session, &held, format, proc).and_then(|planes| {
                crate::decode::frame_of(&Readback { about, planes })
                    .map(Decoded::Planes)
                    .ok_or_else(|| {
                        Error::Unavailable(String::from("読み戻したフレームが短すぎます"))
                    })
            })
        };
        if release {
            session.pool.issued(held.slot);
        }
        match made {
            Ok(decoded) => Some(decoded),
            Err(error) => {
                if !self.said_staging {
                    self.said_staging = true;
                    tracing::warn!("NVDEC: サーフェスを渡せない: {error:?}");
                }
                None
            }
        }
    }

    #[cfg(all(feature = "d3d12", windows))]
    fn take(&mut self) -> Option<crate::decode::Decoded> {
        let (held, proc, release, about) = self.next_output()?;
        let format = self.format;
        let made = {
            let session = self.session.as_mut()?;
            let made = hand_over(session, &held, format, proc);
            if release {
                session.pool.issued(held.slot);
            }
            made
        };
        match made {
            Ok(frame) => Some(crate::decode::Decoded::Surface(crate::decode::Surface {
                about,
                held: crate::decode::Held::D3d12(frame),
            })),
            Err(error) => {
                if !self.said_staging {
                    self.said_staging = true;
                    tracing::warn!("NVDEC: サーフェスを渡せない: {error:?}");
                }
                None
            }
        }
    }

    #[cfg(all(not(unix), not(all(feature = "d3d12", windows))))]
    fn take(&mut self) -> Option<crate::decode::Decoded> {
        let held = self.next_held()?;
        if let Some(session) = self.session.as_mut() {
            session.pool.issued(held.slot);
        }
        if !self.said_read_back {
            self.said_read_back = true;
            tracing::info!(
                "NVDEC: このビルドはサーフェスの受け渡しに対応していない（読み戻しのみ）"
            );
        }
        None
    }

    fn read_back(&mut self) -> Option<Readback> {
        self.read_back_now().map(|(_, read)| read)
    }

    fn name(&self) -> String {
        match self.deinterlace {
            Some(mode) => format!(
                "NVDEC ({}・デインターレース {})",
                self.context.name,
                mode.name()
            ),
            None => format!("NVDEC ({})", self.context.name),
        }
    }

    fn deinterlaces(&self) -> bool {
        self.deinterlace.is_some()
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.deinterlace.is_some() { 2 } else { 1 }
    }

    fn flush(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        each_front!(&mut self.front, |front| front.flush(&mut session.pool))
    }

    fn reset(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        each_front!(&mut self.front, |front| front.reset(&mut session.pool));
        // 二枚目のフィールドを待っていたフレームのスロットも返す。返さないとシークのたびに一つずつ減る。
        if let Some(held) = self.second.take() {
            session.pool.issued(held.slot);
        }
        session.pool.forget_recent();
    }
}

#[cfg(all(feature = "d3d12", windows))]
fn hand_over(
    session: &mut Session,
    held: &Held,
    format: Format,
    mut proc: cu::ProcParams,
) -> Result<crate::decode::d3d12::Frame, Error> {
    if session.bridge.is_none() {
        if session.bridge_refused {
            return Err(Error::Unavailable(String::from(
                "CUDA と D3D12 の共有を作れません",
            )));
        }
        match d3d12::Bridge::open(session.decoder.context(), format) {
            Ok(bridge) => session.bridge = Some(bridge),
            Err(error) => {
                session.bridge_refused = true;
                return Err(error);
            }
        }
    }
    let mapped = session.decoder.map(session.pool.id(held.slot), &mut proc)?;
    let surface_height = session.surface_height;
    let Some(bridge) = session.bridge.as_mut() else {
        return Err(Error::Unavailable(String::from(
            "CUDA と D3D12 の共有がありません",
        )));
    };
    bridge.write(mapped.pointer, mapped.pitch, surface_height)
}

#[cfg(unix)]
fn copy_out(
    session: &mut Session,
    held: &Held,
    format: Format,
    mut proc: cu::ProcParams,
) -> Result<DmaBuf, Error> {
    // 先に問い合わせる。コピーした後だと何が拒んだのか分からない `NOT_SUPPORTED` が返る。
    if !session.decoder.context().exports_dma_buf() {
        return Err(Error::Unavailable(String::from(
            "このデバイスは割り当てを DMA-BUF としてエクスポートできません\
             （CU_DEVICE_ATTRIBUTE_DMA_BUF_SUPPORTED が偽）",
        )));
    }
    let wide = usize::from(format.depth > 8) + 1;
    let mapped = session.decoder.map(session.pool.id(held.slot), &mut proc)?;
    let pitch = mapped.pitch;
    let rows = session.surface_height + session.surface_height.div_ceil(2);
    if session.staging.len() < STAGING
        || session.staging[0].pitch != pitch
        || session.staging[0].rows != rows
    {
        session.staging.clear();
        session.next = 0;
        for _ in 0..STAGING {
            session
                .staging
                .push(cu::Linear::new(session.decoder.context(), pitch, rows)?);
        }
    }
    let into = &session.staging[session.next];
    session.next = (session.next + 1) % STAGING;
    session.decoder.context().copy(&cu::Memcpy2D {
        src_memory_type: cu::CU_MEMORYTYPE_DEVICE,
        src_device: mapped.pointer,
        src_pitch: pitch,
        dst_memory_type: cu::CU_MEMORYTYPE_DEVICE,
        dst_device: into.pointer,
        dst_pitch: pitch,
        width_in_bytes: pitch,
        height: rows,
        ..cu::Memcpy2D::default()
    })?;
    let fd = into.export()?;
    let chroma = fd
        .try_clone()
        .map_err(|error| Error::Unavailable(format!("ファイル記述子を複製できません: {error}")))?;
    Ok(DmaBuf {
        width: format.width,
        height: format.height,
        modifier: DRM_FORMAT_MOD_LINEAR,
        planes: vec![
            DmaBufPlane {
                fd,
                offset: 0,
                stride: pitch as u32,
                format: if wide == 2 {
                    DRM_FORMAT_R16
                } else {
                    DRM_FORMAT_R8
                },
            },
            DmaBufPlane {
                fd: chroma,
                offset: (pitch * session.surface_height) as u32,
                stride: pitch as u32,
                format: if wide == 2 {
                    DRM_FORMAT_GR1616
                } else {
                    DRM_FORMAT_GR88
                },
            },
        ],
        lease: None,
    })
}

#[cfg(unix)]
const DRM_FORMAT_MOD_LINEAR: u64 = 0;
#[cfg(unix)]
const DRM_FORMAT_R8: u32 = u32::from_le_bytes(*b"R8  ");
#[cfg(unix)]
const DRM_FORMAT_R16: u32 = u32::from_le_bytes(*b"R16 ");
#[cfg(unix)]
const DRM_FORMAT_GR88: u32 = u32::from_le_bytes(*b"GR88");
#[cfg(unix)]
const DRM_FORMAT_GR1616: u32 = u32::from_le_bytes(*b"GR32");
