// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたフレームを、専用の VIDEO_PROCESS のキューで D3D12 のビデオプロセスに通してデインターレースする。
//! GPU のビデオ処理の回路で動くので、シェーダーでするより一桁軽い。一枚から二枚のプログレッシブのフレームを出す。
//! 画面よりずっと大きいプログレッシブのフレームは、同じ処理で縮めて一枚出す。
//!
//! デコードと同じスレッドで呼ぶ。Intel のドライバーはデコードのスレッドでテクスチャのロックを持ったまま返ることがあり、
//! 別のスレッドの `ProcessFrames` がそれを待って止まる。

use std::collections::VecDeque;
use std::sync::Arc;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::core::Interface;

use super::Frame;
use super::dx::{self, Owned, failed};
use crate::decode::{About, Decoded, Error, Held, Surface};
use mediadecoder::es::Structure;

/// 前後の参照の上限。後ろを多く持つとデコーダーのサーフェスが足りなくなり、映像も遅れる。
const MOST_PAST: u32 = 1;
const MOST_FUTURE: u32 = 2;

pub(super) struct Deinterlacer {
    raw: ID3D12Device,
    video: ID3D12VideoDevice,
    queue: ID3D12CommandQueue,
    list: ID3D12VideoProcessCommandList,
    /// リストを作ったアロケーター。リストより先に放すと、ドライバーによっては次の `Reset` で落ちる。
    _made_with: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    value: u64,
    event: Owned,
    bob: bool,
    made: Option<Made>,
    refused: bool,
    outs: Vec<Out>,
    /// 処理を待つフレーム（先頭が次に出すフレーム、後ろは参照）。
    waiting: VecDeque<(Frame, About)>,
    /// 出し終えた前のフレーム（参照）。
    past: VecDeque<Frame>,
    count: u32,
}

struct Made {
    size: [u32; 2],
    /// 出力の大きさ。
    output: [u32; 2],
    /// プログレッシブの入力を縮めるだけ（デインターレースしない）。
    progressive: bool,
    /// `NV12` か、10 ビットなら `P010`（入力と出力で同じ）。
    format: DXGI_FORMAT,
    top_first: bool,
    processor: ID3D12VideoProcessor,
    past: u32,
    future: u32,
}

/// 出力の一枚。受け取る側が全部持っていれば足す。
struct Out {
    texture: ID3D12Resource,
    allocator: ID3D12CommandAllocator,
    /// レンダラーが持っている間は 2 以上。
    lease: Arc<()>,
    /// この出力を書き終えるフェンスの値。
    written: u64,
    /// この出力を書く処理が読む入力。書き終えるまでデコーダーに返さない。
    holding: Vec<Frame>,
}

impl std::fmt::Debug for Deinterlacer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Deinterlacer(bob: {})", self.bob)
    }
}

impl Deinterlacer {
    pub fn new(raw: &ID3D12Device, bob: bool) -> Result<Self, Error> {
        let video: ID3D12VideoDevice = raw
            .cast()
            .map_err(|error| failed("ID3D12VideoDevice", error))?;
        let allocator = dx::allocator(raw, D3D12_COMMAND_LIST_TYPE_VIDEO_PROCESS)?;
        // SAFETY: 生きているデバイスで、記述はどれもローカル。
        unsafe {
            let queue: ID3D12CommandQueue = raw
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_VIDEO_PROCESS,
                    ..Default::default()
                })
                .map_err(|error| failed("ビデオプロセスのキュー", error))?;
            let list: ID3D12VideoProcessCommandList = raw
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_VIDEO_PROCESS, &allocator, None)
                .map_err(|error| failed("CreateCommandList", error))?;
            list.Close().map_err(|error| failed("Close", error))?;
            let fence = raw
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(|error| failed("CreateFence", error))?;
            Ok(Self {
                raw: raw.clone(),
                video,
                queue,
                list,
                _made_with: allocator,
                fence,
                value: 0,
                event: Owned::event()?,
                bob,
                made: None,
                refused: false,
                outs: Vec::new(),
                waiting: VecDeque::new(),
                past: VecDeque::new(),
                count: 0,
            })
        }
    }

    pub fn refused(&self) -> bool {
        self.refused
    }

    /// 一枚受け取り、出せるようになった分を返す。`output` は出力の大きさ。
    pub fn push(
        &mut self,
        frame: Frame,
        about: About,
        output: [u32; 2],
    ) -> Result<Vec<Decoded>, Error> {
        let format = if frame.ten_bit {
            DXGI_FORMAT_P010
        } else {
            DXGI_FORMAT_NV12
        };
        let progressive = about.format.progressive;
        if self.made.as_ref().is_some_and(|made| {
            made.size != [frame.width, frame.height]
                || made.format != format
                || made.output != output
                || made.progressive != progressive
        }) {
            self.made = None;
            self.reset();
            // 大きさか形式の違う出力は使い回さない。
            self.outs.clear();
        }
        if self.made.is_none() {
            let first = match about.structure {
                Structure::Bottom => false,
                Structure::Top => true,
                Structure::Frame => about.top_field_first,
            };
            match self.make(
                [frame.width, frame.height],
                first,
                format,
                output,
                progressive,
            ) {
                Ok(made) => {
                    if progressive {
                        tracing::info!(
                            "D3D12: ビデオプロセスで縮小（{}x{} → {}x{}）",
                            frame.width,
                            frame.height,
                            output[0],
                            output[1]
                        );
                    } else {
                        tracing::info!(
                            "D3D12: ビデオプロセスでデインターレース（{}、参照 前 {} 後 {}）",
                            if self.bob { "bob" } else { "適応型" },
                            made.past,
                            made.future
                        );
                    }
                    self.made = Some(made);
                }
                Err(error) => {
                    self.refused = true;
                    return Err(error);
                }
            }
        }
        self.release_read();
        self.waiting.push_back((frame, about));
        let future = self.made.as_ref().map_or(0, |made| made.future) as usize;
        let mut out = Vec::new();
        while self.waiting.len() > future {
            out.extend(self.next()?);
        }
        Ok(out)
    }

    /// 残りを出す（後ろの参照は手元の一番後ろで埋める）。
    pub fn flush(&mut self) -> Vec<Decoded> {
        let mut out = Vec::new();
        while !self.waiting.is_empty() {
            match self.next() {
                Ok(made) => out.extend(made),
                Err(error) => {
                    tracing::warn!("D3D12: ビデオプロセスに失敗: {error:?}");
                    self.waiting.clear();
                }
            }
        }
        out
    }

    /// 前の番組のフレームを捨てる。書き終えるのを待って、出力が読んでいた入力もデコーダーに返す。
    pub fn reset(&mut self) {
        self.waiting.clear();
        self.past.clear();
        if self.event.wait_fence(&self.fence, self.value).is_ok() {
            for out in &mut self.outs {
                out.holding.clear();
            }
        }
    }

    /// 書き終えた出力が読んだ入力をデコーダーに返す（出力を使い回すまで持つと、デコーダーのサーフェスが尽きる）。
    fn release_read(&mut self) {
        // SAFETY: 読むだけ。
        let done = dx::completed(&self.fence);
        for out in &mut self.outs {
            if out.written <= done {
                out.holding.clear();
            }
        }
    }

    /// 先頭のフレームから二枚作る。
    fn next(&mut self) -> Result<Vec<Decoded>, Error> {
        let Some((frame, about)) = self.waiting.pop_front() else {
            return Ok(Vec::new());
        };
        let Some(made) = self.made.as_ref() else {
            return Ok(Vec::new());
        };
        let (past_wanted, future_wanted, top_first, format, output, progressive) = (
            made.past,
            made.future,
            made.top_first,
            made.format,
            made.output,
            made.progressive,
        );
        // ドライバーが求める枚数をそのまま渡す。持っていない分は一番近いフレームで埋める。
        let fill = |have: Vec<&ID3D12Resource>, wanted: u32| -> Vec<Option<ID3D12Resource>> {
            (0..wanted as usize)
                .map(|at| {
                    Some(
                        have.get(at)
                            .or(have.last())
                            .copied()
                            .unwrap_or(&frame.texture)
                            .clone(),
                    )
                })
                .collect()
        };
        let past = fill(
            self.past.iter().rev().map(|frame| &frame.texture).collect(),
            past_wanted,
        );
        let future = fill(
            self.waiting
                .iter()
                .map(|(frame, _)| &frame.texture)
                .collect(),
            future_wanted,
        );
        let first = match about.structure {
            Structure::Bottom => false,
            Structure::Top => true,
            Structure::Frame => about.top_field_first,
        };
        let mut made = Vec::with_capacity(2);
        for field in 0..if progressive { 1 } else { 2u32 } {
            // 処理のフィールドの順は最初のフレームのまま。順の違うフレームは出力の番号で合わせる。
            let output_index = if progressive || first == top_first {
                field
            } else {
                1 - field
            };
            let at = self.out_for(output, format)?;
            self.process(&frame, &past, &future, at, output_index)?;
            // 読んだ入力は、書き終えるまで持っておく（デコーダーが次のピクチャーを書かないように）。
            let mut holding = vec![frame.clone()];
            holding.extend(self.past.iter().cloned());
            holding.extend(self.waiting.iter().map(|(frame, _)| frame.clone()));
            self.outs[at].holding = holding;
            let out = &self.outs[at];
            made.push(Decoded::Surface(Surface {
                about: About {
                    format: mediadecoder::es::Format {
                        width: output[0],
                        height: output[1],
                        ..about.format
                    },
                    structure: Structure::Frame,
                    progressive: true,
                    repeat_first_field: false,
                    ..about
                },
                held: Held::D3d12(Frame {
                    device: self.raw.clone(),
                    texture: out.texture.clone(),
                    width: output[0],
                    height: output[1],
                    ten_bit: frame.ten_bit,
                    lease: Some(out.lease.clone()),
                    // レンダラーが自分のキューで書き終わりを待つ。
                    ready: Some((self.fence.clone(), out.written)),
                }),
            }));
        }
        self.count = self.count.wrapping_add(1);

        self.past.push_back(frame);
        while self.past.len() > MOST_PAST as usize {
            self.past.pop_front();
        }
        Ok(made)
    }

    fn process(
        &mut self,
        frame: &Frame,
        past: &[Option<ID3D12Resource>],
        future: &[Option<ID3D12Resource>],
        at: usize,
        output_index: u32,
    ) -> Result<(), Error> {
        let Some(made) = self.made.as_ref() else {
            return Ok(());
        };
        let (processor, size) = (made.processor.clone(), made.output);
        let out = &self.outs[at];
        let output = out.texture.clone();
        let allocator = out.allocator.clone();
        let rect = RECT {
            left: 0,
            top: 0,
            right: frame.width as i32,
            bottom: frame.height as i32,
        };
        let target = RECT {
            left: 0,
            top: 0,
            right: size[0] as i32,
            bottom: size[1] as i32,
        };
        let mut past_subresources = vec![0u32; past.len()];
        let mut future_subresources = vec![0u32; future.len()];
        // SAFETY: 記録するリソースはどれもこの関数の間生きていて、リストは閉じてから実行し、終わるまで待つ。
        unsafe {
            allocator.Reset().map_err(|error| failed("Reset", error))?;
            self.list
                .Reset(&allocator)
                .map_err(|error| failed("Reset", error))?;
            // デコーダーのテクスチャ（同時アクセス）にはバリアを打たず、COMMON からの暗黙の昇格で読む。
            self.list.ResourceBarrier(&[dx::transition(
                &output,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_VIDEO_PROCESS_WRITE,
            )]);
            let input = D3D12_VIDEO_PROCESS_INPUT_STREAM_ARGUMENTS {
                InputStream: [
                    D3D12_VIDEO_PROCESS_INPUT_STREAM {
                        pTexture2D: dx::borrow(&frame.texture),
                        Subresource: 0,
                        ReferenceSet: D3D12_VIDEO_PROCESS_REFERENCE_SET {
                            NumPastFrames: past.len() as u32,
                            ppPastFrames: if past.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                past.as_ptr().cast_mut()
                            },
                            pPastSubresources: if past.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                past_subresources.as_mut_ptr()
                            },
                            NumFutureFrames: future.len() as u32,
                            ppFutureFrames: if future.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                future.as_ptr().cast_mut()
                            },
                            pFutureSubresources: if future.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                future_subresources.as_mut_ptr()
                            },
                        },
                    },
                    D3D12_VIDEO_PROCESS_INPUT_STREAM::default(),
                ],
                Transform: D3D12_VIDEO_PROCESS_TRANSFORM {
                    SourceRectangle: rect,
                    DestinationRectangle: target,
                    Orientation: D3D12_VIDEO_PROCESS_ORIENTATION_DEFAULT,
                },
                Flags: D3D12_VIDEO_PROCESS_INPUT_STREAM_FLAG_NONE,
                RateInfo: D3D12_VIDEO_PROCESS_INPUT_STREAM_RATE {
                    OutputIndex: output_index,
                    InputFrameOrField: self.count,
                },
                FilterLevels: [0; 32],
                AlphaBlending: D3D12_VIDEO_PROCESS_ALPHA_BLENDING::default(),
            };
            let output_arguments = D3D12_VIDEO_PROCESS_OUTPUT_STREAM_ARGUMENTS {
                OutputStream: [
                    D3D12_VIDEO_PROCESS_OUTPUT_STREAM {
                        pTexture2D: dx::borrow(&output),
                        Subresource: 0,
                    },
                    D3D12_VIDEO_PROCESS_OUTPUT_STREAM::default(),
                ],
                TargetRectangle: target,
            };
            self.list
                .ProcessFrames(&processor, &output_arguments, std::slice::from_ref(&input));
            self.list.ResourceBarrier(&[dx::transition(
                &output,
                D3D12_RESOURCE_STATE_VIDEO_PROCESS_WRITE,
                D3D12_RESOURCE_STATE_COMMON,
            )]);
            self.list.Close().map_err(|error| failed("Close", error))?;
            let list: ID3D12CommandList = self
                .list
                .cast()
                .map_err(|error| failed("ID3D12CommandList", error))?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.value += 1;
            self.queue
                .Signal(&self.fence, self.value)
                .map_err(|error| failed("Signal", error))?;
        }
        self.outs[at].written = self.value;
        Ok(())
    }

    /// 空いている出力。全部持たれていれば増やす。
    fn out_for(&mut self, size: [u32; 2], format: DXGI_FORMAT) -> Result<usize, Error> {
        if let Some(at) = self
            .outs
            .iter()
            .position(|out| Arc::strong_count(&out.lease) == 1)
        {
            // 前の書き込みが終わるまでアロケーターも入力も手放さない（ふつうはとうに終わっている）。
            self.event.wait_fence(&self.fence, self.outs[at].written)?;
            self.outs[at].holding.clear();
            return Ok(at);
        }
        // デコーダーのテクスチャと同じく同時アクセスで作る（レンダラーは COMMON から読む）。
        let texture = dx::texture(&self.raw, format, size[0], size[1], false, 1)?;
        let allocator = dx::allocator(&self.raw, D3D12_COMMAND_LIST_TYPE_VIDEO_PROCESS)?;
        self.outs.push(Out {
            texture,
            allocator,
            lease: Arc::new(()),
            written: 0,
            holding: Vec::new(),
        });
        Ok(self.outs.len() - 1)
    }

    fn make(
        &self,
        size: [u32; 2],
        top_first: bool,
        format: DXGI_FORMAT,
        output_size: [u32; 2],
        progressive: bool,
    ) -> Result<Made, Error> {
        // 入力と出力を同じ形式・色空間にして、色の変換をさせない（色はレンダラーが変換する）。
        // 10 ビットも BT.709 と示す。BT.2020 と示すと、NVIDIA は縮小した出力に何も書かない（エラーも返さない）。
        let space = DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709;
        let field_type = if progressive {
            D3D12_VIDEO_FIELD_TYPE_NONE
        } else if top_first {
            D3D12_VIDEO_FIELD_TYPE_INTERLACED_TOP_FIELD_FIRST
        } else {
            D3D12_VIDEO_FIELD_TYPE_INTERLACED_BOTTOM_FIELD_FIRST
        };
        let input_rate = DXGI_RATIONAL {
            Numerator: if progressive { 60000 } else { 30000 },
            Denominator: 1001,
        };
        let output_rate = DXGI_RATIONAL {
            Numerator: 60000,
            Denominator: 1001,
        };
        let mut support = D3D12_FEATURE_DATA_VIDEO_PROCESS_SUPPORT {
            NodeIndex: 0,
            InputSample: D3D12_VIDEO_SAMPLE {
                Width: size[0],
                Height: size[1],
                Format: D3D12_VIDEO_FORMAT {
                    Format: format,
                    ColorSpace: space,
                },
            },
            InputFieldType: field_type,
            InputStereoFormat: D3D12_VIDEO_FRAME_STEREO_FORMAT_NONE,
            InputFrameRate: input_rate,
            OutputFormat: D3D12_VIDEO_FORMAT {
                Format: format,
                ColorSpace: space,
            },
            OutputStereoFormat: D3D12_VIDEO_FRAME_STEREO_FORMAT_NONE,
            OutputFrameRate: output_rate,
            ..Default::default()
        };
        // SAFETY: 大きさどおりのローカルを渡す。
        unsafe {
            self.video.CheckFeatureSupport(
                D3D12_FEATURE_VIDEO_PROCESS_SUPPORT,
                std::ptr::from_mut(&mut support).cast(),
                size_of::<D3D12_FEATURE_DATA_VIDEO_PROCESS_SUPPORT>() as u32,
            )
        }
        .map_err(|error| failed("CheckFeatureSupport", error))?;
        if support.SupportFlags.0 & D3D12_VIDEO_PROCESS_SUPPORT_FLAG_SUPPORTED.0 == 0 {
            return Err(Error::Unavailable(String::from(
                "ドライバーがこの入力と出力に対応していない",
            )));
        }
        let has = |flag: D3D12_VIDEO_PROCESS_DEINTERLACE_FLAGS| {
            support.DeinterlaceSupport.0 & flag.0 != 0
        };
        // 適応型（ドライバー独自）を、ドライバーの求める参照の枚数で（上限は MOST_*）。断られたら bob。
        let mut tries = Vec::new();
        if progressive {
            tries.push((D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_NONE, (0, 0)));
        } else if !self.bob && has(D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_CUSTOM) {
            let (past, future) = self.references(input_rate, output_rate);
            for counts in [
                (past.min(MOST_PAST), future.min(MOST_FUTURE)),
                (1, 1),
                (0, 0),
            ] {
                if !tries.contains(&(D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_CUSTOM, counts)) {
                    tries.push((D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_CUSTOM, counts));
                }
            }
        }
        if !progressive && has(D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_BOB) {
            tries.push((D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_BOB, (0, 0)));
        }
        let output = D3D12_VIDEO_PROCESS_OUTPUT_STREAM_DESC {
            Format: format,
            ColorSpace: space,
            AlphaFillMode: D3D12_VIDEO_PROCESS_ALPHA_FILL_MODE_OPAQUE,
            AlphaFillModeSourceStreamIndex: 0,
            BackgroundColor: [0.0, 0.0, 0.0, 1.0],
            FrameRate: output_rate,
            EnableStereo: false.into(),
        };
        let range = D3D12_VIDEO_SIZE_RANGE {
            MaxWidth: size[0],
            MaxHeight: size[1],
            MinWidth: size[0],
            MinHeight: size[1],
        };
        let mut last =
            Error::Unavailable(String::from("ドライバーがデインターレースに対応していない"));
        for (mode, (past, future)) in tries {
            let input = D3D12_VIDEO_PROCESS_INPUT_STREAM_DESC {
                Format: format,
                ColorSpace: space,
                SourceAspectRatio: DXGI_RATIONAL {
                    Numerator: 1,
                    Denominator: 1,
                },
                DestinationAspectRatio: DXGI_RATIONAL {
                    Numerator: 1,
                    Denominator: 1,
                },
                FrameRate: input_rate,
                SourceSizeRange: range,
                DestinationSizeRange: D3D12_VIDEO_SIZE_RANGE {
                    MaxWidth: output_size[0],
                    MaxHeight: output_size[1],
                    MinWidth: output_size[0],
                    MinHeight: output_size[1],
                },
                EnableOrientation: false.into(),
                FilterFlags: D3D12_VIDEO_PROCESS_FILTER_FLAG_NONE,
                StereoFormat: D3D12_VIDEO_FRAME_STEREO_FORMAT_NONE,
                FieldType: field_type,
                DeinterlaceMode: mode,
                EnableAlphaBlending: false.into(),
                LumaKey: D3D12_VIDEO_PROCESS_LUMA_KEY::default(),
                NumPastFrames: past,
                NumFutureFrames: future,
                EnableAutoProcessing: false.into(),
            };
            // SAFETY: 記述はどれもローカル。
            match unsafe { self.video.CreateVideoProcessor(0, &output, &[input]) } {
                Ok(processor) => {
                    return Ok(Made {
                        size,
                        output: output_size,
                        progressive,
                        format,
                        top_first,
                        processor,
                        past,
                        future,
                    });
                }
                Err(error) => last = failed("CreateVideoProcessor", error),
            }
        }
        Err(last)
    }

    /// ドライバーが適応型に求める前後の参照の枚数。
    fn references(&self, input_rate: DXGI_RATIONAL, output_rate: DXGI_RATIONAL) -> (u32, u32) {
        let mut info = D3D12_FEATURE_DATA_VIDEO_PROCESS_REFERENCE_INFO {
            NodeIndex: 0,
            DeinterlaceMode: D3D12_VIDEO_PROCESS_DEINTERLACE_FLAG_CUSTOM,
            Filters: D3D12_VIDEO_PROCESS_FILTER_FLAG_NONE,
            FeatureSupport: D3D12_VIDEO_PROCESS_FEATURE_FLAG_NONE,
            InputFrameRate: input_rate,
            OutputFrameRate: output_rate,
            EnableAutoProcessing: false.into(),
            ..Default::default()
        };
        // SAFETY: 大きさどおりのローカルを渡す。
        let asked = unsafe {
            self.video.CheckFeatureSupport(
                D3D12_FEATURE_VIDEO_PROCESS_REFERENCE_INFO,
                std::ptr::from_mut(&mut info).cast(),
                size_of::<D3D12_FEATURE_DATA_VIDEO_PROCESS_REFERENCE_INFO>() as u32,
            )
        };
        if asked.is_err() {
            return (1, 1);
        }
        (info.PastFrames, info.FutureFrames)
    }
}

impl Drop for Deinterlacer {
    fn drop(&mut self) {
        let _ = self.event.wait_fence(&self.fence, self.value);
    }
}
