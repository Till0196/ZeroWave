// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! AMF のフレームを同じデバイスの D3D11 Video Processor でデインターレースする（AMF には無い）。
//! ドライバーが後ろのフレームも要るので、一枚遅らせてデインターレースする。
//! 出力はレンダラーへ GPU の上で渡す（[`super::Sharer`]）か、渡せなければ主記憶へ読む。

use std::collections::VecDeque;

use mediadecoder::es::Structure;
use mediadecoder::video::Frame;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::{GUID, Interface};

use super::{Sharer, api};
use crate::decode::{About, Decoded, Error, HwDeinterlace};

fn failed(what: &str, error: windows::core::Error) -> Error {
    Error::Unavailable(format!("D3D11 ビデオプロセッサー {what}: {error}"))
}

fn flag(mode: HwDeinterlace) -> u32 {
    match mode {
        HwDeinterlace::Bob => D3D11_VIDEO_PROCESSOR_PROCESSOR_CAPS_DEINTERLACE_BOB.0 as u32,
        HwDeinterlace::Adaptive => {
            D3D11_VIDEO_PROCESSOR_PROCESSOR_CAPS_DEINTERLACE_ADAPTIVE.0 as u32
        }
        HwDeinterlace::MotionCompensated => {
            D3D11_VIDEO_PROCESSOR_PROCESSOR_CAPS_DEINTERLACE_MOTION_COMPENSATION.0 as u32
        }
    }
}

/// ビデオプロセッサーのケイパビリティの一つ：(インデックス、前に要る枚数、後に要る枚数、フラグ)。
type Rate = (u32, u32, u32, u32);

fn rate_caps(
    video: &ID3D11VideoDevice,
    width: u32,
    height: u32,
) -> Result<(ID3D11VideoProcessorEnumerator, Vec<Rate>), Error> {
    let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_INTERLACED_TOP_FIELD_FIRST,
        InputFrameRate: DXGI_RATIONAL {
            Numerator: 30000,
            Denominator: 1001,
        },
        InputWidth: width,
        InputHeight: height,
        OutputFrameRate: DXGI_RATIONAL {
            Numerator: 60000,
            Denominator: 1001,
        },
        OutputWidth: width,
        OutputHeight: height,
        Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    };
    // SAFETY: 記述は生きたローカル変数。
    let enumerator = unsafe { video.CreateVideoProcessorEnumerator(&desc) }
        .map_err(|error| failed("CreateVideoProcessorEnumerator", error))?;
    let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
    // SAFETY: 出力は生きたローカル変数。
    unsafe { enumerator.GetVideoProcessorCaps(&mut caps) }
        .map_err(|error| failed("GetVideoProcessorCaps", error))?;
    let mut found = Vec::new();
    for index in 0..caps.RateConversionCapsCount {
        let mut rate = D3D11_VIDEO_PROCESSOR_RATE_CONVERSION_CAPS::default();
        // SAFETY: 出力は生きたローカル変数、インデックスは個数の範囲内。
        if unsafe { enumerator.GetVideoProcessorRateConversionCaps(index, &mut rate) }.is_ok() {
            found.push((
                index,
                rate.PastFrames,
                rate.FutureFrames,
                rate.ProcessorCaps,
            ));
        }
    }
    Ok((enumerator, found))
}

pub fn modes(device: &ID3D11Device) -> Vec<HwDeinterlace> {
    let Ok(video) = device.cast::<ID3D11VideoDevice>() else {
        return Vec::new();
    };
    let Ok((_, caps)) = rate_caps(&video, 1440, 1080) else {
        return Vec::new();
    };
    [
        HwDeinterlace::Adaptive,
        HwDeinterlace::MotionCompensated,
        HwDeinterlace::Bob,
    ]
    .into_iter()
    .filter(|mode| caps.iter().any(|(_, _, _, bits)| bits & flag(*mode) != 0))
    .collect()
}

struct Past {
    // プレーンの参照はビューより先に解放する（宣言の順）。
    _surface: api::Surface,
    view: ID3D11VideoProcessorInputView,
}

struct Held {
    past: Past,
    about: About,
}

pub struct Deinterlacer {
    mode: HwDeinterlace,
    context: ID3D11DeviceContext,
    video: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
    /// 共有テクスチャ（[`super::Sharer`]）へ直に書く出力のビュー。テクスチャも持ち、アドレスを使い回させない。
    shared_views: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    width: u32,
    height: u32,
    wide: bool,
    past_frames: u32,
    past: VecDeque<Past>,
    future_frames: u32,
    held: Option<Held>,
    frame: u32,
    said: bool,
}

impl std::fmt::Debug for Deinterlacer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Deinterlacer({:?} {}x{})",
            self.mode, self.width, self.height
        )
    }
}

impl Deinterlacer {
    pub fn open(
        device: &ID3D11Device,
        mode: HwDeinterlace,
        width: u32,
        height: u32,
        wide: bool,
    ) -> Result<Self, Error> {
        let video: ID3D11VideoDevice = device
            .cast()
            .map_err(|error| failed("ID3D11VideoDevice", error))?;
        // SAFETY: デバイスは生きている。
        let context = unsafe { device.GetImmediateContext() }
            .map_err(|error| failed("GetImmediateContext", error))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|error| failed("ID3D11VideoContext", error))?;
        let (enumerator, caps) = rate_caps(&video, width, height)?;
        let Some(&(index, past_frames, future_frames, _)) =
            caps.iter().find(|(_, _, _, bits)| bits & flag(mode) != 0)
        else {
            return Err(Error::Unavailable(format!(
                "この GPU の D3D11 ビデオプロセッサーに {} のデインターレースはありません",
                mode.name()
            )));
        };
        // SAFETY: 列挙子は生きていて、インデックスはケイパビリティのテーブルの物。
        let processor = unsafe { video.CreateVideoProcessor(&enumerator, index) }
            .map_err(|error| failed("CreateVideoProcessor", error))?;
        let format: DXGI_FORMAT = if wide {
            DXGI_FORMAT_P010
        } else {
            DXGI_FORMAT_NV12
        };
        let texture = |usage, bind: u32, cpu: u32| -> Result<ID3D11Texture2D, Error> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: usage,
                BindFlags: bind,
                CPUAccessFlags: cpu,
                MiscFlags: 0,
            };
            let mut made = None;
            // SAFETY: 記述と出力は生きたローカル変数。
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut made)) }
                .map_err(|error| failed("CreateTexture2D", error))?;
            made.ok_or_else(|| Error::Unavailable(String::from("テクスチャが作れません")))
        };
        let output = texture(D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let staging = texture(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
        let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view = None;
        // SAFETY: 記述と出力は生きたローカル変数、テクスチャと列挙子は生きている。
        unsafe {
            video.CreateVideoProcessorOutputView(
                &output,
                &enumerator,
                &view_desc,
                Some(&mut output_view),
            )
        }
        .map_err(|error| failed("CreateVideoProcessorOutputView", error))?;
        let output_view = output_view
            .ok_or_else(|| Error::Unavailable(String::from("出力のビューが作れません")))?;
        // SAFETY: ビデオプロセッサーは生きている。出力はフィールドレート。
        unsafe {
            video_context.VideoProcessorSetStreamOutputRate(
                &processor,
                0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL,
                false,
                None,
            );
        }
        // 入力の矩形をフレームの大きさに限る。AMF のサーフェスはアラインした大きさで、指定しないと全体を縮めて出す。
        let rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        // 色は変換しない。入出力に同じ色空間（BT.709・0〜255）を指定し、自動の補正も止める。
        // 範囲を 16〜235 と指定すると出力をその範囲にクリップするドライバーがある。
        let space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
            // YCbCr_Matrix＝1（BT.709）が 2 ビット目、Nominal_Range＝2（0〜255）が 4〜5 ビット目。
            _bitfield: 1 << 2 | 2 << 4,
        };
        // SAFETY: ビデオプロセッサーは生きていて、矩形と色空間は生きたローカル変数。
        unsafe {
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&rect));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&rect));
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&rect));
            video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &space);
            video_context.VideoProcessorSetOutputColorSpace(&processor, &space);
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        }
        if super::say() {
            tracing::info!(
                "AMF デインターレース: {}、ケイパビリティ {index}（過去 {past_frames} 枚、未来 {future_frames} 枚）、一覧 {caps:x?}",
                mode.name()
            );
        }
        Ok(Self {
            mode,
            context,
            video,
            video_context,
            enumerator,
            processor,
            output,
            output_view,
            staging,
            shared_views: Vec::new(),
            width,
            height,
            wide,
            past_frames,
            past: VecDeque::new(),
            future_frames,
            held: None,
            frame: 0,
            said: false,
        })
    }

    pub fn fits(&self, width: u32, height: u32, wide: bool) -> bool {
        self.width == width && self.height == height && self.wide == wide
    }

    pub fn reset(&mut self) {
        self.past.clear();
        self.held = None;
    }

    /// 一枚前のフレームをデインターレースして二枚にする。終わりの一枚は [`Self::drain`] が出す。
    pub fn process(
        &mut self,
        surface: &api::Surface,
        about: About,
        sharer: Option<&mut Sharer>,
    ) -> Result<Vec<Decoded>, String> {
        let texture = texture_of(surface)?;
        let slice = self.array_slice(&texture);
        let view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: slice,
                },
            },
        };
        let mut view = None;
        // SAFETY: 記述と出力は生きたローカル変数。
        unsafe {
            self.video.CreateVideoProcessorInputView(
                &texture,
                &self.enumerator,
                &view_desc,
                Some(&mut view),
            )
        }
        .map_err(|error| format!("CreateVideoProcessorInputView: {error}"))?;
        let view = view.ok_or_else(|| String::from("入力のビューが作れません"))?;
        // 次のフレームをデインターレースするまで持つ。
        let next = Held {
            past: Past {
                _surface: surface.share(),
                view: view.clone(),
            },
            about,
        };
        let Some(held) = self.held.replace(next) else {
            return Ok(Vec::new());
        };
        self.solve(held, Some(view), sharer)
    }

    pub fn drain(&mut self, sharer: Option<&mut Sharer>) -> Result<Vec<Decoded>, String> {
        match self.held.take() {
            Some(held) => self.solve(held, None, sharer),
            None => Ok(Vec::new()),
        }
    }

    fn solve(
        &mut self,
        held: Held,
        future: Option<ID3D11VideoProcessorInputView>,
        mut sharer: Option<&mut Sharer>,
    ) -> Result<Vec<Decoded>, String> {
        let Held { past: this, about } = held;
        let interlaced = !about.progressive || about.structure.field();
        let top_first = if about.structure.field() {
            about.structure == Structure::Top
        } else {
            about.top_field_first
        };
        let format = if !interlaced {
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE
        } else if top_first {
            D3D11_VIDEO_FRAME_FORMAT_INTERLACED_TOP_FIELD_FIRST
        } else {
            D3D11_VIDEO_FRAME_FORMAT_INTERLACED_BOTTOM_FIELD_FIRST
        };
        // SAFETY: ビデオプロセッサーは生きている。
        unsafe {
            self.video_context
                .VideoProcessorSetStreamFrameFormat(&self.processor, 0, format);
        }
        let woven = About {
            structure: Structure::Frame,
            progressive: true,
            repeat_first_field: false,
            ..about
        };
        let mut frames: Vec<Decoded> = Vec::with_capacity(2);
        let mut failure = None;
        // 共有テクスチャへ渡すときは、出力ごとに空いた共有テクスチャへ直に書く。一枚の出力へ書いて写すと、
        // 写し終わる前に次の出力が上書きする（D3D11 はビデオの回路とコピーの間の順を守らない）。
        let mut last = None;
        for field in 0..2u32 {
            if !interlaced && field == 1 {
                let again = match (sharer.as_deref_mut(), last) {
                    (Some(sharer), Some(at)) => super::give(sharer, at, woven, self.wide),
                    _ => self.read(woven).map(Decoded::Planes),
                };
                match again {
                    Ok(again) => frames.push(again),
                    Err(why) => failure = Some(why),
                }
                break;
            }
            let target = match sharer.as_deref_mut() {
                Some(sharer) => match super::take(sharer, self.width, self.height, self.wide)
                    .and_then(|(at, texture)| Ok((at, self.shared_view(&texture)?)))
                {
                    Ok(target) => Some(target),
                    Err(why) => {
                        failure = Some(why);
                        break;
                    }
                },
                None => None,
            };
            let output_view = target
                .as_ref()
                .map_or(&self.output_view, |(_, view)| view)
                .clone();
            let mut past: Vec<Option<ID3D11VideoProcessorInputView>> = self
                .past
                .iter()
                .rev()
                .take(self.past_frames as usize)
                .map(|past| Some(past.view.clone()))
                .collect();
            let mut future: Vec<Option<ID3D11VideoProcessorInputView>> = future
                .iter()
                .take(self.future_frames as usize)
                .map(|future| Some(future.clone()))
                .collect();
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: if interlaced { field } else { 0 },
                InputFrameOrField: self.frame,
                PastFrames: past.len() as u32,
                FutureFrames: future.len() as u32,
                ppPastSurfaces: if past.is_empty() {
                    std::ptr::null_mut()
                } else {
                    past.as_mut_ptr()
                },
                ppFutureSurfaces: if future.is_empty() {
                    std::ptr::null_mut()
                } else {
                    future.as_mut_ptr()
                },
                pInputSurface: std::mem::ManuallyDrop::new(Some(this.view.clone())),
                ..Default::default()
            };
            let streams = [stream];
            // SAFETY: ビデオプロセッサー・出力・入力は生きていて、前後のビューは呼び出しの間持つ。
            let blt = unsafe {
                self.video_context.VideoProcessorBlt(
                    &self.processor,
                    &output_view,
                    self.frame * 2 + field,
                    &streams,
                )
            };
            // 渡したビューの参照を解放する（`ManuallyDrop` に入れたコピー）。
            let [stream] = streams;
            drop(std::mem::ManuallyDrop::into_inner(stream.pInputSurface));
            match blt
                .map_err(|error| format!("VideoProcessorBlt: {error}"))
                .and_then(|()| match (sharer.as_deref_mut(), &target) {
                    (Some(sharer), Some((at, _))) => {
                        last = Some(*at);
                        // AMF は DX11 のサーフェスの書き終わりを知らせない。前後のフレームを読む処理が
                        // 次の処理やデコードと重なると書きかけを読むので、出力ごとに書き終わりを待つ。
                        super::give(sharer, *at, woven, self.wide).and_then(|frame| {
                            super::finish(sharer)?;
                            Ok(frame)
                        })
                    }
                    _ => self.read(woven).map(Decoded::Planes),
                }) {
                Ok(frame) => frames.push(frame),
                Err(why) => {
                    failure = Some(why);
                    break;
                }
            }
        }
        self.frame = self.frame.wrapping_add(1);
        // デインターレースできなくても前のフレームとしては持つ（次のフレームの前後が崩れないように）。
        self.past.push_back(this);
        while self.past.len() > self.past_frames.max(1) as usize {
            self.past.pop_front();
        }
        match failure {
            Some(why) => Err(why),
            None => Ok(frames),
        }
    }

    fn array_slice(&mut self, texture: &ID3D11Texture2D) -> u32 {
        let (count, index) = array_index(texture);
        if count > 1 && !self.said {
            self.said = true;
            if super::say() {
                tracing::info!(
                    "AMF デインターレース: テクスチャ配列 {count} 枚、配列番号のプライベートデータ → {index:?}"
                );
            }
        }
        index.unwrap_or(0)
    }

    fn shared_view(
        &mut self,
        texture: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorOutputView, String> {
        if let Some((_, view)) = self
            .shared_views
            .iter()
            .find(|(known, _)| known.as_raw() == texture.as_raw())
        {
            return Ok(view.clone());
        }
        let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view = None;
        // SAFETY: 記述と出力は生きたローカル変数、テクスチャと列挙子は生きている。
        unsafe {
            self.video.CreateVideoProcessorOutputView(
                texture,
                &self.enumerator,
                &view_desc,
                Some(&mut view),
            )
        }
        .map_err(|error| format!("CreateVideoProcessorOutputView: {error}"))?;
        let view = view.ok_or_else(|| String::from("出力のビューが作れません"))?;
        self.shared_views.push((texture.clone(), view.clone()));
        Ok(view)
    }

    fn read(&self, about: About) -> Result<Frame, String> {
        // SAFETY: どちらのテクスチャも生きていて、同じ形。
        unsafe { self.context.CopyResource(&self.staging, &self.output) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: ステージングのテクスチャを読むために Map する。下で Unmap する。
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| format!("Map: {error}"))?;
        let pitch = mapped.RowPitch as usize;
        let rows = self.height as usize;
        // SAFETY: NV12・P010 のステージングは連続したメモリで、`Map` の間は読める。
        let result = unsafe {
            let base = mapped.pData.cast::<u8>().cast_const();
            let luma = std::slice::from_raw_parts(base, pitch * rows);
            let chroma =
                std::slice::from_raw_parts(base.add(pitch * rows), pitch * rows.div_ceil(2));
            crate::decode::semi_planar(
                (luma, pitch),
                (chroma, pitch),
                about.format.width as usize,
                about.format.height as usize,
                self.wide,
            )
        };
        // SAFETY: 上で Map した物を Unmap する。
        unsafe { self.context.Unmap(&self.staging, 0) };
        let planes = result.ok_or_else(|| String::from("フレームが出力より大きい"))?;
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

/// DX11 のサーフェスの D3D11 のテクスチャ（配列のことがある、[`array_index`]）。
pub(super) fn texture_of(surface: &api::Surface) -> Result<ID3D11Texture2D, String> {
    let memory = surface.memory_type();
    if memory != api::AMF_MEMORY_DX11 {
        return Err(format!(
            "AMF のフレームが DX11 ではありません（メモリの種類 {memory}）"
        ));
    }
    let plane = surface.plane(0).ok_or("プレーンがありません")?;
    let raw = plane.native();
    // SAFETY: DX11 のプレーンの `GetNative` は、プレーンが生きている間の `ID3D11Texture2D*`。
    unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }
        .cloned()
        .ok_or_else(|| String::from("テクスチャがありません"))
}

/// AMF のテクスチャ配列の枚数と、このサーフェスの配列番号（プライベートデータ、読めなければ `None`）。
pub(super) fn array_index(texture: &ID3D11Texture2D) -> (u32, Option<u32>) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: 出力は生きたローカル変数。
    unsafe { texture.GetDesc(&mut desc) };
    if desc.ArraySize <= 1 {
        return (desc.ArraySize, Some(0));
    }
    const ARRAY_INDEX: GUID = GUID::from_u128(0x28115527_e7c3_4b66_99d3_4f2ae6b47faf);
    let mut index = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: 出力は生きたローカル変数で、大きさを指定している。
    let read = unsafe {
        texture.GetPrivateData(
            &ARRAY_INDEX,
            &mut size,
            Some(std::ptr::from_mut(&mut index).cast()),
        )
    };
    (desc.ArraySize, read.ok().map(|()| index))
}
