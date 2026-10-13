// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Opening the queues: formats, buffers, the capture side and tearing down.

use super::*;

/// バッファを自前で管理する経路で、参照に要る分に加えて最初に要求する本数。
const START_SLACK: u32 = 4;

impl V4l2 {
    /// A stateful decoder gets its capture queue only after `V4L2_EVENT_SOURCE_CHANGE`,
    /// or `bcm2835-codec` takes the buffers and never returns a picture.
    pub(super) fn configure(&mut self, format: Format) -> Result<(), Error> {
        if let Some(session) = &self.session
            && crate::decode::session_fits(session.format, format)
        {
            self.format = format;
            return Ok(());
        }
        self.teardown();
        crate::decode::openable(format)?;

        // HEVC's coded size is a multiple of 8, not necessarily of 16.
        let (width, height) = match (self.codec, self.coded) {
            (Codec::Hevc, Some(coded)) => coded,
            _ => (
                format.width.next_multiple_of(16),
                format.height.next_multiple_of(16),
            ),
        };
        match self.flavour {
            Flavour::Stateless => {
                self.device.set_format(
                    V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
                    self.compressed,
                    width,
                    height,
                    1 << 20,
                    1,
                )?;
            }
            Flavour::Stateful => {
                self.device.set_compressed(
                    V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
                    self.compressed,
                    1 << 20,
                )?;
            }
        }

        // Subscribe before streaming, or an early event is lost and the decoder stalls.
        if self.flavour == Flavour::Stateful {
            self.device.subscribe(V4L2_EVENT_SOURCE_CHANGE)?;
            let _ = self.device.subscribe(V4L2_EVENT_EOS);
        }

        let (outputs, out_caps) = self
            .device
            .request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, OUTPUT_BUFFERS)?;
        let made = (|| -> Result<_, Error> {
            if self.flavour == Flavour::Stateless && out_caps & V4L2_BUF_CAP_SUPPORTS_REQUESTS == 0
            {
                return Err(Error::Unavailable(format!(
                    "{} の出力キューは Request API を持ちません",
                    self.device.driver
                )));
            }
            let mut mappings = Vec::new();
            for index in 0..outputs {
                mappings.push(
                    self.device
                        .map(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, index, 0)?,
                );
            }
            let mut requests = Vec::new();
            if let Some(media) = &self.media {
                for _ in 0..outputs {
                    requests.push(Request::alloc(media)?);
                }
            }
            Ok((mappings, requests))
        })();
        let (mappings, requests) = match made {
            Ok(made) => made,
            Err(error) => {
                let _ = self
                    .device
                    .request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 0);
                return Err(error);
            }
        };
        self.hold = out_caps & V4L2_BUF_CAP_SUPPORTS_M2M_HOLD_CAPTURE_BUF != 0;
        self.by_hand = self.flavour == Flavour::Stateless && !self.hold;
        self.open_field = None;

        self.session = Some(Session {
            outputs: mappings,
            free_outputs: (0..outputs as usize).collect(),
            requests,
            format,
            capture: None,
        });
        self.format = format;
        self.saw_last = false;
        self.saw_eos = false;

        if self.flavour == Flavour::Stateless {
            self.start_capture(width, height)?;
        }
        self.device.stream_on(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE)?;
        Ok(())
    }

    pub(super) fn start_capture(&mut self, width: u32, height: u32) -> Result<(), Error> {
        let format = self.format;
        // ステートレスのデコーダーの初期化の手順では、キャプチャーの形式を決める前にヘッダーの制御を渡す。
        // ドライバーはそれでビット深度を知り、出せる形式とバッファの大きさを決める。
        if self.flavour == Flavour::Stateless
            && let Some(sequence) = self.sequence.clone()
            && let Err(error) = self.device.set_controls(self.naming, &mut [sequence], None)
        {
            tracing::info!("V4L2: キャプチャーの形式を決める前に SPS を渡せない: {error:?}");
        }
        let layout = self.negotiate_capture(format, width, height)?;
        // バッファを自前で管理する経路は少なく始めて追加する（上限まで取るとメモリの少ない機器では CMA を使い過ぎる）。
        let limit = self.buffer_count();
        let first = if self.by_hand {
            limit.min(self.references_count() + START_SLACK)
        } else {
            limit
        };
        let (captures, _) = self
            .device
            .request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, first)?;
        if captures == 0 {
            return Err(Error::Unavailable(String::from(
                "サーフェスを一つも確保できませんでした",
            )));
        }
        let mut maps = Vec::new();
        let mut chroma_maps = Vec::new();
        for index in 0..captures {
            maps.push(
                self.device
                    .map(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index, 0)?,
            );
            if layout.planes == 2 {
                chroma_maps.push(
                    self.device
                        .map(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index, 1)?,
                );
            }
        }
        let mut fds = Vec::new();
        if self.by == DeinterlaceBy::Hardware {
            let got = self.device.format(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)?;
            match deint::Deinterlacer::open(got.width, got.height) {
                Ok(filter) => {
                    for index in 0..captures {
                        fds.push(self.device.export(
                            V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                            index,
                            0,
                        )?);
                    }
                    tracing::info!(
                        "V4L2: デインターレーサー {}（1 枚から {} 枚）",
                        filter.name(),
                        deint::PER_PICTURE
                    );
                    self.deint = Some(filter);
                }
                Err(error) => {
                    tracing::warn!("V4L2: ハードウェアのデインターレースを使えない: {error:?}");
                }
            }
        }
        if !self.by_hand {
            for index in 0..captures {
                self.device.queue(&Enqueue {
                    planes_used: u32::from(layout.planes),
                    ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index)
                })?;
            }
        }
        self.device.stream_on(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)?;
        // ドライバーが減らした（vb2 は 32 が上限）なら、追加できる上限もそこまで。
        let limit = if captures < first {
            tracing::warn!("V4L2: capture バッファを {first} 本要求して {captures} 本だけ確保");
            captures
        } else {
            limit
        };
        let Some(session) = self.session.as_mut() else {
            return Err(Error::Unavailable(String::from("バッファがありません")));
        };
        trace(format_args!(
            "capture {captures} 枚 {}x{} bpl {} 表示領域 {:?} プレーン {}",
            layout.stride, layout.allocated, layout.stride, layout.visible, layout.planes
        ));
        session.capture = Some(Capture {
            layout,
            pool: Pool::growing(captures as usize, limit as usize),
            idle: if self.by_hand {
                (0..captures as usize).rev().collect()
            } else {
                Vec::new()
            },
            fds,
            size: layout.stride * layout.allocated * 3 / 2,
            maps,
            chroma_maps,
        });
        Ok(())
    }

    pub(super) fn capture_ready(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.capture.is_some())
    }

    pub(super) fn capture(&self) -> Option<&Capture> {
        self.session.as_ref()?.capture.as_ref()
    }

    /// 出力のフレームの形式。深さはキャプチャーの形式に合わせる（10 ビットのストリームを 8 ビットで受けたとき）。
    pub(super) fn produced(&self) -> Format {
        let mut format = self.format;
        if let Some(capture) = self.capture() {
            format.depth = capture.layout.depth;
        }
        format
    }

    pub(super) fn capture_mut(&mut self) -> Option<&mut Capture> {
        self.session.as_mut()?.capture.as_mut()
    }

    /// Called wherever this code might wait: only an event unblocks a stateful decoder.
    pub(super) fn pump_events(&mut self) {
        while let Some((kind, changes)) = self.device.event() {
            trace(format_args!("event {kind} changes {changes:#x}"));
            match kind {
                V4L2_EVENT_SOURCE_CHANGE if changes & V4L2_EVENT_SRC_CH_RESOLUTION != 0 => {
                    if self.capture_ready() {
                        continue;
                    }
                    let format = self.format;
                    if let Err(error) = self.start_capture(
                        format.width.next_multiple_of(16),
                        format.height.next_multiple_of(16),
                    ) {
                        tracing::warn!("V4L2: capture キューを作れない: {error:?}");
                    }
                }
                // Not `saw_last`: EOS only acknowledges the drain; `V4L2_BUF_FLAG_LAST` ends it.
                V4L2_EVENT_EOS => self.saw_eos = true,
                _ => {}
            }
        }
    }

    pub(super) fn references_count(&self) -> u32 {
        self.buffer_count() - pool::SLACK as u32
    }

    /// バッファを自前で管理する経路で capture バッファを 1 本追加する。追加できなければ以後は追加しない。
    pub(super) fn grow_capture(&mut self) -> Option<usize> {
        let kind = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        let made = (|| {
            let (index, count) = self.device.create_buffers(kind, 1)?;
            if count == 0 {
                return Err(Error::Unavailable(String::from("追加できない")));
            }
            let map = self.device.map(kind, index, 0)?;
            let chroma = match self.capture().map(|capture| capture.layout.planes) {
                Some(2) => Some(self.device.map(kind, index, 1)?),
                _ => None,
            };
            let fd = match self.deint {
                Some(_) => Some(self.device.export(kind, index, 0)?),
                None => None,
            };
            Ok::<_, Error>((index as usize, map, chroma, fd))
        })();
        let capture = self.capture_mut()?;
        match made {
            Ok((index, map, chroma, fd)) if index == capture.maps.len() => {
                capture.maps.push(map);
                capture.chroma_maps.extend(chroma);
                if let Some(fd) = fd {
                    capture.fds.push(fd);
                }
                trace(format_args!(
                    "capture バッファを追加、計 {} 本",
                    capture.maps.len()
                ));
                Some(index)
            }
            Ok((index, ..)) => {
                tracing::warn!(
                    "V4L2: 追加した capture バッファの番号が連続していない（{index}）。追加をやめる"
                );
                capture.pool.stop_growing(capture.maps.len());
                None
            }
            Err(error) => {
                tracing::info!(
                    "V4L2: capture バッファは {} 本まで（{error:?}）",
                    capture.maps.len()
                );
                capture.pool.stop_growing(capture.maps.len());
                None
            }
        }
    }

    pub(super) fn buffer_count(&self) -> u32 {
        // Two extra for a stateful decoder's hidden reordering.
        let asked = self
            .device
            .control_value(V4L2_CID_MIN_BUFFERS_FOR_CAPTURE)
            .unwrap_or(0)
            .max(0) as u32;
        // HEVC は SPS が言う DPB の大きさ（デコード中のピクチャーと出力を待つピクチャーを含む。H.265 C.5.2.2）。
        // 上限の 16 で取ると、下流で持つ分と合わせて V4L2 の 32 本を超え、表示中のバッファを奪うことになる。
        let references = match self.codec {
            Codec::Mpeg2 => 3,
            Codec::H264 => 17,
            Codec::Hevc => self.dpb.unwrap_or(16),
        };
        // Six is a floor: [`pool::SLACK`] buffers are held out of the driver's hands.
        references.max(asked + 2).max(6) + pool::SLACK as u32
    }

    /// Whether a format can be drawn is asked of [`takes`], else of Vulkan.
    pub(super) fn negotiate_capture(
        &self,
        format: Format,
        width: u32,
        height: u32,
    ) -> Result<Layout, Error> {
        let offered = self.device.formats(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
        let wide = format.depth > 8;
        // SPS を渡した後に列挙される形式は、このストリームで出せる形式。10 ビットのストリームで 8 ビットの形式
        // しか無ければ、ドライバーは上位 8 ビットを書く（cedrus）。
        let order: &[u32] = if wide {
            &[
                V4L2_PIX_FMT_P010,
                V4L2_PIX_FMT_NV12_10_COL128,
                V4L2_PIX_FMT_NV15,
                V4L2_PIX_FMT_NV12M,
                V4L2_PIX_FMT_NV12,
                V4L2_PIX_FMT_NV12_COL128,
            ]
        } else {
            &[
                V4L2_PIX_FMT_NV12M,
                V4L2_PIX_FMT_NV12,
                V4L2_PIX_FMT_NV12_COL128,
            ]
        };
        let named = || {
            offered
                .iter()
                .map(|code| probe::fourcc_name(*code))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut refused = Vec::new();
        let chosen = order
            .iter()
            .copied()
            .filter(|wanted| offered.contains(wanted))
            .find(|wanted| {
                let drawn = drawn(*wanted);
                if !drawn {
                    refused.push(probe::fourcc_name(*wanted));
                }
                drawn
            })
            .ok_or_else(|| {
                Error::Unavailable(if refused.is_empty() {
                    format!(
                        "{} が出せるのは {} で、{} ビットの 4:2:0 がありません",
                        self.device.driver,
                        named(),
                        format.depth
                    )
                } else {
                    format!(
                        "{} は {} ビットを {} で出しますが、レンダリング側がその形式を受け付けません",
                        self.device.driver,
                        format.depth,
                        refused.join("・")
                    )
                })
            })?;
        // Only the pixel format: `V4L2_FIELD_NONE` would ask a stateful decoder for progressive.
        let got = if self.flavour == Flavour::Stateful {
            self.device
                .set_pixelformat(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, chosen)?
        } else {
            let planes = if chosen == V4L2_PIX_FMT_NV12M { 2 } else { 1 };
            self.device.set_format(
                V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                chosen,
                width,
                height,
                0,
                planes,
            )?
        };
        let stride = got.plane_fmt[0].bytesperline;
        let allocated = got.height;
        // cedrus has no `G_SELECTION`: 1080 would otherwise read as the 1088 buffer rows.
        let visible = match self.device.compose(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            Some(rect) => (
                rect.left.max(0) as u32,
                rect.top.max(0) as u32,
                rect.width,
                rect.height,
            ),
            None => (
                0,
                0,
                format.width.min(got.width),
                format.height.min(got.height),
            ),
        };
        match got.pixelformat {
            V4L2_PIX_FMT_NV12M => Ok(Layout {
                planes: 2,
                modifier: DRM_FORMAT_MOD_LINEAR,
                drm: [DRM_FORMAT_R8, DRM_FORMAT_GR88],
                chroma_offset: 0,
                export_stride: stride,
                stride,
                allocated,
                visible,
                depth: 8,
            }),
            V4L2_PIX_FMT_NV12 => Ok(Layout {
                planes: 1,
                modifier: DRM_FORMAT_MOD_LINEAR,
                drm: [DRM_FORMAT_R8, DRM_FORMAT_GR88],
                chroma_offset: stride * got.height,
                export_stride: stride,
                stride,
                allocated,
                visible,
                depth: 8,
            }),
            V4L2_PIX_FMT_P010 => Ok(Layout {
                planes: 1,
                modifier: DRM_FORMAT_MOD_LINEAR,
                drm: [DRM_FORMAT_R16, DRM_FORMAT_GR1616],
                chroma_offset: stride * got.height,
                export_stride: stride,
                stride,
                allocated,
                visible,
                depth: 10,
            }),
            V4L2_PIX_FMT_NV12_COL128 => {
                let modifier = sand128_col_height(stride);
                if takes::accepts(DRM_FORMAT_NV12, modifier).is_none() {
                    modifier::require(modifier, false).map_err(|why| {
                        Error::Unavailable(format!(
                            "{} は SAND128（カラムの高さ {stride}）で出しますが、{why}",
                            self.device.driver
                        ))
                    })?;
                }
                Ok(Layout {
                    planes: 1,
                    modifier,
                    drm: [DRM_FORMAT_R8, DRM_FORMAT_GR88],
                    // In each column the luma rows come first, so chroma begins one column in.
                    chroma_offset: got.height * 128,
                    // The modifier carries the column height; the stride is the row width.
                    export_stride: got.width,
                    stride,
                    allocated,
                    visible,
                    depth: 8,
                })
            }
            V4L2_PIX_FMT_NV12_10_COL128 => Ok(Layout {
                planes: 1,
                modifier: sand128_col_height(stride),
                drm: [DRM_FORMAT_P030, DRM_FORMAT_P030],
                chroma_offset: got.height * 128,
                export_stride: got.width.div_ceil(3) * 4,
                stride,
                allocated,
                visible,
                depth: 10,
            }),
            V4L2_PIX_FMT_NV15 => Ok(Layout {
                planes: 1,
                modifier: DRM_FORMAT_MOD_LINEAR,
                drm: [DRM_FORMAT_NV15, DRM_FORMAT_NV15],
                chroma_offset: stride * got.height,
                export_stride: stride,
                stride,
                allocated,
                visible,
                depth: 10,
            }),
            _ => Err(Error::Unsupported(
                "the decoder chose a picture format this build cannot export",
            )),
        }
    }

    pub(super) fn teardown(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        if let Some(capture) = session.capture.as_mut() {
            each_front!(&mut self.front, |front| front.reset(&mut capture.pool));
            capture.pool.clear();
        }
        self.finished.clear();
        self.saw_last = false;
        self.saw_eos = false;
        self.deint = None;
        self.woven.clear();
        self.waiting.clear();
        self.issued.clear();
        if let Some(stateful) = self.stateful.as_mut() {
            stateful.reset();
        }
        self.device.stream_off(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
        self.device.stream_off(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
        // Mappings and requests first: `REQBUFS 0` fails while a buffer is mapped.
        session.outputs.clear();
        session.requests.clear();
        session.capture = None;
        let _ = self
            .device
            .request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 0);
        let _ = self
            .device
            .request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, 0);
    }
}

/// The drawing side's answer ([`takes`]) if given, else Vulkan's (no `P030` or `NV15`).
fn drawn(code: u32) -> bool {
    let sand = sand128_col_height(0);
    let (fourcc, modifier, vulkan) = match code {
        V4L2_PIX_FMT_NV12M | V4L2_PIX_FMT_NV12 => return true,
        V4L2_PIX_FMT_P010 => (DRM_FORMAT_P010, DRM_FORMAT_MOD_LINEAR, true),
        V4L2_PIX_FMT_NV12_COL128 => (DRM_FORMAT_NV12, sand, true),
        V4L2_PIX_FMT_NV12_10_COL128 => (DRM_FORMAT_P030, sand, false),
        V4L2_PIX_FMT_NV15 => (DRM_FORMAT_NV15, DRM_FORMAT_MOD_LINEAR, false),
        _ => return false,
    };
    takes::accepts(fourcc, modifier).unwrap_or(vulkan)
}
