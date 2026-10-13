// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The stateless path: one request per picture, and the pictures back out.

use super::*;

impl V4l2 {
    /// Order: controls into the request, QBUF against it, then queue the request.
    pub(super) fn submit(&mut self, submission: Submission, data: &[u8]) -> Result<(), Error> {
        let last = submission.units.len().saturating_sub(1);
        if !self.hold && (submission.units.len() > 1 || submission.more && !self.by_hand) {
            return Err(Error::Unsupported(
                "a picture in several requests on a queue with no hold capture buffer",
            ));
        }
        if self.by_hand {
            self.queue_capture_for(submission.ts)?;
        }
        let prefix: &[u8] = if self.prefix == 0 { &[] } else { &START_CODE };
        for (at, mut unit) in submission.units.into_iter().enumerate() {
            let index = self.free_output()?;
            let hold = self.hold && (at != last || submission.more);
            let (written, request) = {
                let Some(session) = self.session.as_mut() else {
                    return Err(Error::Unavailable(String::from("バッファがありません")));
                };
                let written =
                    session.outputs[index].fill(prefix, data, &unit.pieces, unit.zeroes) as u32;
                (written, session.requests.get(index).map(Request::fd))
            };
            // 拒否されたらインデックスを空きへ戻す（戻さないと出力バッファが足りなくなって止まる）。
            let queued = (|| -> Result<(), Error> {
                if let Some(session) = self.session.as_ref()
                    && let Some(request) = session.requests.get(index)
                {
                    request.reinit()?;
                }
                self.device
                    .set_controls(self.naming, &mut unit.controls, request)?;
                self.device.queue(&Enqueue {
                    bytes: written,
                    ts: submission.ts,
                    request,
                    hold,
                    ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, index as u32)
                })
            })();
            if let Err(error) = queued {
                if let Some(session) = self.session.as_mut() {
                    session.free_outputs.push(index);
                }
                return Err(error);
            }
            if let Some(session) = self.session.as_ref()
                && let Some(request) = session.requests.get(index)
            {
                request.queue()?;
            }
        }
        if submission.more && !self.by_hand {
            return Ok(());
        }
        // Decode order, one per picture: a stateless decoder does not reorder.
        let done = self.wait_capture()?;
        trace(format_args!(
            "リクエスト ts {} → capture {}{}{}",
            submission.ts,
            done.index,
            if submission.more {
                "（第 1 フィールド）"
            } else {
                ""
            },
            if done.error { " エラー" } else { "" }
        ));
        if submission.more && !done.error {
            self.open_field = Some((submission.ts, done.index));
            return Ok(());
        }
        if done.error {
            // Straight back to the driver, or every bad picture costs a capture buffer.
            if let Some(capture) = self.capture_mut() {
                capture.idle.push(done.index);
            }
            self.recycle();
            return Err(Error::Damaged("the decoder marked the picture bad"));
        }
        if done.ts != submission.ts {
            tracing::debug!(
                "V4L2: タイムスタンプが一致しない（送信 {}、返却 {}）",
                submission.ts,
                done.ts
            );
        }
        self.saw_field = done.field;
        if let Some(capture) = self.capture_mut() {
            capture.pool.bind(done.ts, done.index);
        }
        Ok(())
    }

    /// Two seconds: a wedged driver becomes a message, not a hung player.
    pub(super) fn wait_capture(&self) -> Result<dev::Dequeued, Error> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(done) = self
                .device
                .dequeue_now(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)?
            {
                return Ok(done);
            }
            if std::time::Instant::now() >= until {
                return Err(Error::Unavailable(String::from(
                    "デコーダーが 2 秒のあいだフレームを返しませんでした",
                )));
            }
            self.device.wait(10);
        }
    }

    pub(super) fn wait_for_output_drained(&mut self) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            while let Ok(Some(done)) = self.device.dequeue_now(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
                if let Some(session) = self.session.as_mut() {
                    session.free_outputs.push(done.index);
                }
            }
            let all_back = self
                .session
                .as_ref()
                .is_some_and(|session| session.free_outputs.len() >= session.outputs.len());
            if all_back || std::time::Instant::now() >= until {
                return;
            }
            self.pump_events();
            self.collect_stateful();
            self.device.wait(10);
        }
    }

    /// Waiting here also empties the capture queue, or the two queues deadlock when it fills.
    pub(super) fn free_output(&mut self) -> Result<usize, Error> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(session) = self.session.as_mut()
                && let Some(index) = session.free_outputs.pop()
            {
                return Ok(index);
            }
            match self.device.dequeue_now(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE)? {
                Some(done) => {
                    if let Some(session) = self.session.as_mut() {
                        session.free_outputs.push(done.index);
                    }
                    continue;
                }
                None => {
                    // Before its capture queue exists a stateful decoder waits for events.
                    self.pump_events();
                    if self.stateful.is_some() {
                        self.collect_stateful();
                    }
                }
            }
            if std::time::Instant::now() >= until {
                return Err(Error::Unavailable(String::from(
                    "出力バッファが 2 秒のあいだ空きませんでした",
                )));
            }
            self.device.wait(10);
        }
    }

    /// The first field's buffer if this is the second, else a free one.
    fn queue_capture_for(&mut self, ts: u64) -> Result<(), Error> {
        self.recycle();
        let reopened = match self.open_field.take() {
            Some((open, index)) if open == ts => Some(index),
            Some((_, index)) => {
                if let Some(capture) = self.capture_mut() {
                    capture.idle.push(index);
                }
                None
            }
            None => None,
        };
        // 空きが無ければ足し、足せなければ一番古い貸し出しを取り戻す（止まるよりまし）。
        let short = self
            .capture_mut()
            .is_some_and(|capture| reopened.is_none() && capture.idle.is_empty());
        if short {
            let grown = self
                .capture_mut()
                .is_some_and(|capture| capture.maps.len() < capture.pool.capacity())
                .then(|| self.grow_capture())
                .flatten();
            if let Some(capture) = self.capture_mut() {
                match grown {
                    Some(index) => capture.idle.push(index),
                    None if capture.pool.steal() => {
                        let mut back = capture.pool.reclaim();
                        capture.idle.append(&mut back);
                    }
                    None => {}
                }
            }
        }
        let Some(capture) = self.capture_mut() else {
            return Err(Error::Unavailable(String::from("バッファがありません")));
        };
        let planes = u32::from(capture.layout.planes);
        let Some(index) = reopened.or_else(|| capture.idle.pop()) else {
            return Err(Error::Unavailable(String::from(
                "デコーダーに渡せる capture のバッファがありません",
            )));
        };
        self.device.queue(&Enqueue {
            planes_used: planes,
            ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32)
        })
    }

    pub(super) fn recycle(&mut self) {
        let by_hand = self.by_hand;
        let (freed, planes) = match self.capture_mut() {
            Some(capture) => {
                let mut freed = capture.pool.reclaim();
                if by_hand {
                    capture.idle.append(&mut freed);
                    return;
                }
                freed.append(&mut capture.idle);
                (freed, u32::from(capture.layout.planes))
            }
            None => return,
        };
        for index in freed {
            if let Err(error) = self.device.queue(&Enqueue {
                planes_used: planes,
                ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32)
            }) {
                tracing::warn!("V4L2: capture バッファを再キューできない: {error:?}");
                if let Some(capture) = self.capture_mut() {
                    capture.idle.push(index);
                }
            }
        }
    }

    pub(super) fn export(&mut self, held: Held, index: usize) -> Option<Decoded> {
        if self.deint.is_some() {
            return self.export_woven(held, index);
        }
        let layout = self.capture()?.layout;
        let mut planes = Vec::new();
        for plane in 0..2usize {
            let which = if layout.planes == 2 { plane as u32 } else { 0 };
            let fd =
                match self
                    .device
                    .export(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32, which)
                {
                    Ok(fd) => fd,
                    Err(error) => {
                        tracing::warn!("V4L2: DMA-BUF をエクスポートできない: {error:?}");
                        break;
                    }
                };
            planes.push(DmaBufPlane {
                fd,
                offset: if layout.planes == 2 || plane == 0 {
                    0
                } else {
                    layout.chroma_offset
                },
                stride: layout.export_stride,
                format: layout.drm[plane],
            });
        }
        // 受け取った側が全部解放するまで、このバッファは再キューしない。
        let lease = self
            .capture_mut()
            .map(|capture| capture.pool.leased(held.slot));
        self.recycle();
        if planes.len() < 2 {
            return None;
        }
        Some(Decoded::Surface(Surface {
            about: held.about(self.produced()),
            held: Where::DmaBuf(DmaBuf {
                width: self.format.width,
                height: self.format.height,
                modifier: layout.modifier,
                planes,
                lease: lease.map(|lease| lease as std::sync::Arc<dyn std::any::Any + Send + Sync>),
            }),
        }))
    }

    /// メモリーに読み戻す（一枚が一フィールドの HEVC を組むときと `deck --check`・`--dump`）。行は
    /// `VIDIOC_G_SELECTION` で切り取る。デインターレースしない経路は輝度に続けて色差（UV を交互に並べた
    /// 一プレーン）も返す。
    pub(super) fn read_one(&mut self, held: Held, index: usize) -> Option<Readback> {
        if self.deint.is_some() {
            let filter = self.deint.as_ref()?;
            let format = filter.format;
            let read = filter.read(index);
            let about = About {
                tag: held.tag,
                format: Format {
                    rate: self.format.rate,
                    aspect: self.format.aspect,
                    ..format
                },
                structure: Structure::Frame,
                top_field_first: held.top_field_first,
                repeat_first_field: false,
                progressive: true,
            };
            self.release_woven(index);
            let (luma, pitch, rows) = read?;
            return Some(Readback {
                about,
                planes: vec![(luma, pitch, rows)],
            });
        }
        let layout = self.capture()?.layout;
        let (left, top, width, height) = layout.visible;
        let height = height.min(layout.allocated.saturating_sub(top));
        let wide = usize::from(self.produced().depth > 8) + 1;
        let stride = layout.stride as usize;
        let row = width as usize * wide;
        // 4:2:0 の色差は輝度の半分の行で、U と V が交互に並ぶので行の長さは輝度と同じ（奇数の幅は切り上げ）。
        let chroma_row = (width as usize).div_ceil(2) * 2 * wide;
        let chroma_rows = (height as usize).div_ceil(2);
        let rows = |bytes: &[u8], from: usize, first: usize, count: usize, length: usize| {
            let mut out = Vec::with_capacity(length * count);
            for y in 0..count {
                let at = from + (first + y) * stride + left as usize * wide;
                out.extend_from_slice(bytes.get(at..at + length)?);
            }
            Some(out)
        };
        let planes = self.capture().and_then(|capture| {
            let bytes = capture.maps.get(index)?.read();
            let luma = rows(bytes, 0, top as usize, height as usize, row)?;
            // 色差を線形に読めるのは、色差が同じバッファの後ろにある形（NV12・P010）と別プレーンの NV12M だけ。
            // SAND・NV15 は並びが違うので輝度だけ返す。
            let chroma = match layout.planes {
                2 => capture
                    .chroma_maps
                    .get(index)
                    .and_then(|map| rows(map.read(), 0, top as usize / 2, chroma_rows, chroma_row)),
                _ if layout.modifier == DRM_FORMAT_MOD_LINEAR
                    && matches!(layout.drm[1], DRM_FORMAT_GR88 | DRM_FORMAT_GR1616) =>
                {
                    rows(
                        bytes,
                        layout.chroma_offset as usize,
                        top as usize / 2,
                        chroma_rows,
                        chroma_row,
                    )
                }
                _ => None,
            };
            let mut planes = vec![(luma, row, height as usize)];
            if let Some(chroma) = chroma {
                planes.push((chroma, chroma_row, chroma_rows));
            }
            Some(planes)
        });
        let about = held.about(self.produced());
        // Straight back, not through the ring: the capture queue is the decoder's DPB.
        if let Some(capture) = self.capture_mut() {
            capture.pool.release(held.slot);
        }
        self.recycle();
        Some(Readback {
            about,
            planes: planes?,
        })
    }

    pub(super) fn next_picture(&mut self) -> Option<(Held, usize)> {
        if self.stateful.is_some() {
            self.pump_events();
            self.collect_stateful();
            self.pump_waiting();
            // While draining, the decoder gives up its pictures one buffer at a time.
            if self.finished.is_empty()
                && !self.saw_last
                && self
                    .stateful
                    .as_ref()
                    .is_some_and(stateful::Stateful::draining)
            {
                let until = std::time::Instant::now() + std::time::Duration::from_millis(200);
                while self.finished.is_empty() && std::time::Instant::now() < until {
                    self.device.wait(10);
                    self.pump_events();
                    self.collect_stateful();
                }
            }
            let (held, index) = self.finished.pop_front()?;
            // With a deinterlacer the index is that device's buffer, unknown to the pool.
            if self.deint.is_some() {
                return Some((held, index));
            }
            if let Some(capture) = self.capture_mut() {
                capture.pool.adopt(held.slot, index);
            }
            return Some((held, index));
        }
        if self.deint.is_some() {
            return self.next_woven();
        }
        let held = self.next_held()?;
        let index = self.capture()?.pool.index(held.slot)?;
        trace(format_args!(
            "出力 ts {} tag {} ← capture {}",
            held.slot, held.tag, index
        ));
        Some((held, index))
    }

    /// The deinterlacer reads the decoder's buffer well inside [`pool::SLACK`].
    fn next_woven(&mut self) -> Option<(Held, usize)> {
        loop {
            self.return_woven();
            self.pump_waiting();
            if let Some(out) = self.finished.pop_front() {
                return Some(out);
            }
            let held = self.next_held()?;
            let index = self.capture()?.pool.index(held.slot)?;
            self.weave(held, index);
            if let Some(capture) = self.capture_mut() {
                capture.pool.issued(held.slot);
            }
            self.recycle();
        }
    }

    pub(super) fn export_woven(&mut self, held: Held, index: usize) -> Option<Decoded> {
        let (stride, planes_count, chroma_offset, format) = {
            let filter = self.deint.as_ref()?;
            (
                filter.stride,
                filter.planes,
                filter.chroma_offset,
                filter.format,
            )
        };
        let mut planes = Vec::new();
        for plane in 0..2usize {
            let which = if planes_count >= 2 { plane as u32 } else { 0 };
            let fd = match self.deint.as_ref()?.export(index, which) {
                Ok(fd) => fd,
                Err(error) => {
                    tracing::warn!(
                        "V4L2: デインターレース後のフレームをエクスポートできない: {error:?}"
                    );
                    break;
                }
            };
            planes.push(DmaBufPlane {
                fd,
                offset: if planes_count >= 2 || plane == 0 {
                    0
                } else {
                    chroma_offset
                },
                stride,
                format: if plane == 0 {
                    DRM_FORMAT_R8
                } else {
                    DRM_FORMAT_GR88
                },
            });
        }
        let lease = self.lend_woven(index);
        if planes.len() < 2 {
            return None;
        }
        // デインターレーサーが高さを 16 の倍数で返すことがある。余りを表示すると下に灰色の領域が出る。
        let format = Format {
            width: format.width.min(self.format.width),
            height: format.height.min(self.format.height),
            ..format
        };
        Some(Decoded::Surface(Surface {
            about: About {
                tag: held.tag,
                format: Format {
                    rate: self.format.rate,
                    aspect: self.format.aspect,
                    ..format
                },
                structure: Structure::Frame,
                top_field_first: held.top_field_first,
                repeat_first_field: false,
                progressive: true,
            },
            held: Where::DmaBuf(DmaBuf {
                width: format.width,
                height: format.height,
                modifier: DRM_FORMAT_MOD_LINEAR,
                planes,
                lease,
            }),
        }))
    }

    pub(super) fn next_held(&mut self) -> Option<Held> {
        each_front!(&mut self.front, |front| front.take())
    }
}
