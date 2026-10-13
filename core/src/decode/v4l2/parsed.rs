// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The stateful path: the decoder parses the stream, keeps its own DPB and
//! returns pictures in display order.

use super::*;

impl V4l2 {
    pub(super) fn feed_stateful(
        &mut self,
        tag: u64,
        picture: &Picture,
        data: &[u8],
    ) -> Result<(), Error> {
        let ts = match self.stateful.as_mut() {
            Some(stateful) => stateful.begin(tag, picture),
            None => return Ok(()),
        };
        let index = self.free_output()?;
        let Some(session) = self.session.as_mut() else {
            return Err(Error::Unavailable(String::from("バッファがありません")));
        };
        let written = session.outputs[index].write(data) as u32;
        trace(format_args!(
            "入力 tag={tag} ts={ts} {written}/{} バイト index={index}",
            data.len()
        ));
        self.device.queue(&Enqueue {
            bytes: written,
            ts,
            ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, index as u32)
        })?;
        // Events first: until the size is announced there is no capture queue.
        self.pump_events();
        self.collect_stateful();
        if let Some(stateful) = self.stateful.as_mut() {
            stateful.forget_stale();
        }
        Ok(())
    }

    pub(super) fn collect_stateful(&mut self) {
        let Some(planes) = self
            .capture()
            .map(|capture| u32::from(capture.layout.planes))
        else {
            // No capture queue yet: DQBUF would answer EINVAL, not EAGAIN.
            return;
        };
        loop {
            let done = match self.device.dequeue_now(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
                Ok(Some(done)) => done,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!("V4L2: capture の DQBUF に失敗: {error:?}");
                    return;
                }
            };
            trace(format_args!(
                "出力 index={} ts={} field={} bytes={} last={} error={}",
                done.index, done.ts, done.field, done.bytes, done.last, done.error
            ));
            if done.last {
                self.saw_last = true;
            }
            self.saw_field = done.field;
            if done.bytes == 0 && done.last {
                return;
            }
            let held = self
                .stateful
                .as_mut()
                .and_then(|stateful| stateful.finish(done.ts));
            // Requeued only once read: it is also the decoder's reference.
            if self.deint.is_some() {
                match held {
                    Some(held) if !done.error => self.weave(held, done.index),
                    _ => {
                        let _ = self.device.queue(&Enqueue {
                            planes_used: planes,
                            ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, done.index as u32)
                        });
                    }
                }
                continue;
            }
            match held {
                Some(held) if !done.error => self.finished.push_back((held, done.index)),
                _ => {
                    let _ = self.device.queue(&Enqueue {
                        planes_used: planes,
                        ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, done.index as u32)
                    });
                }
            }
        }
    }

    /// Field order comes from the demuxer, not from the driver's buffer flags.
    pub(super) fn weave(&mut self, held: Held, index: usize) {
        self.woven.remember(held.slot, held);
        self.waiting.push_back((index, held));
        self.pump_waiting();
    }

    pub(super) fn pump_waiting(&mut self) {
        self.collect_woven();
        while let Some(&(index, held)) = self.waiting.front() {
            let Some(fd) = self
                .session
                .as_ref()
                .and_then(|session| session.capture.as_ref())
                .and_then(|capture| capture.fds.get(index))
                .map(std::os::fd::AsRawFd::as_raw_fd)
            else {
                self.waiting.pop_front();
                continue;
            };
            let size = self.capture().map_or(0, |capture| capture.size);
            let Some(filter) = self.deint.as_mut() else {
                return;
            };
            match filter.submit(fd, index, held.slot, held.top_field_first, size) {
                Ok(()) => {
                    self.waiting.pop_front();
                }
                Err(_) => return,
            }
        }
    }

    pub(super) fn collect_woven(&mut self) {
        let back = match self.deint.as_mut() {
            Some(filter) => filter.reclaim(),
            None => return,
        };
        // A stateless decoder's buffer is the pool's (it may still be a reference).
        if self.stateful.is_some() {
            for index in back {
                let _ = self.device.queue(&Enqueue::of(
                    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                    index as u32,
                ));
            }
        }
        loop {
            if self.finished.len() >= WOVEN_KEEP {
                break;
            }
            let Some(done) = self.deint.as_mut().and_then(deint::Deinterlacer::take) else {
                break;
            };
            match self.woven.matched(done.ts) {
                Some(held) => {
                    let held = Held {
                        structure: Structure::Frame,
                        progressive: true,
                        repeat_first_field: false,
                        ..held
                    };
                    self.finished.push_back((held, done.index));
                }
                None => {
                    if let Some(filter) = self.deint.as_mut() {
                        filter.give_back(done.index);
                    }
                }
            }
        }
        self.woven.forget_stale();
    }

    pub(super) fn release_woven(&mut self, index: usize) {
        if let Some(filter) = self.deint.as_mut() {
            filter.give_back(index);
        }
    }

    pub(super) fn lend_woven(
        &mut self,
        index: usize,
    ) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {
        self.return_woven();
        let lease = std::sync::Arc::new(crate::decode::slots::Lease);
        self.issued.push_back((index, lease.clone()));
        // 空きが 2 本を切ったら足す。待たせると間に合わないフレームが捨てられてカクつく。
        if let Some(filter) = self.deint.as_mut()
            && self.issued.len() + 2 >= filter.captures()
        {
            filter.grow();
        }
        Some(lease)
    }

    pub(super) fn return_woven(&mut self) {
        let mut back = Vec::new();
        self.issued.retain(|(index, lease)| {
            let held = std::sync::Arc::strong_count(lease) > 1;
            if !held {
                back.push(*index);
            }
            held
        });
        for index in back {
            if let Some(filter) = self.deint.as_mut() {
                filter.give_back(index);
            }
        }
    }
}
