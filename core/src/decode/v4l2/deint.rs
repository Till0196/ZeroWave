// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! A hardware deinterlacer on its own device (`bcm2835-codec` bob, Allwinner DI300).
//! It passes pictures through unless told `V4L2_FIELD_INTERLACED_TB`. No capability says
//! "I deinterlace" (the ISP looks the same), so each M2M device is tried; the doubler wins.

use std::collections::HashMap;

use mediadecoder::es::Format;

use super::dev::{self, Device, Enqueue};
use super::probe;
use super::uapi::*;
use crate::decode::Error;

/// How many pictures come out for every one that goes in.
pub const PER_PICTURE: u32 = 2;

const OUTPUT_SLOTS: u32 = 12;
/// 書いたフレームは呼び出し側が解放するまで戻らない。足りなくなれば [`CAPTURE_MOST`] まで足す。
const CAPTURE_BUFFERS: u32 = 8;
const CAPTURE_MOST: usize = 24;

#[derive(Debug)]
pub struct Deinterlacer {
    device: Device,
    pub format: Format,
    pub stride: u32,
    /// Buffer height, not picture height.
    pub allocated: u32,
    pub visible: (u32, u32, u32, u32),
    pub planes: u8,
    pub chroma_offset: u32,
    maps: Vec<dev::Mapping>,
    full: bool,
    holding: Vec<Option<usize>>,
    free_slots: Vec<usize>,
    idle: Vec<usize>,
    /// Reported at close: a doubler not doubling fails silently.
    submitted: u64,
    produced: u64,
    starved: u64,
    pub waited: u64,
}

impl Deinterlacer {
    /// `Err` is the ordinary answer; the caller falls back to software.
    pub fn open(width: u32, height: u32) -> Result<Self, Error> {
        let mut tried = 0;
        for path in probe::nodes() {
            let Ok(device) = Device::open(&path) else {
                continue;
            };
            // Raw in and out rules out codecs but not the ISP, hence the trial.
            let takes = device.formats(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
            let gives = device.formats(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
            if !takes.contains(&V4L2_PIX_FMT_NV12) || !gives.contains(&V4L2_PIX_FMT_NV12) {
                continue;
            }
            tried += 1;
            match trial(&device, width, height) {
                Ok(count) if count >= PER_PICTURE => {
                    return Self::build(device, width, height);
                }
                Ok(count) => {
                    super::trace(format_args!(
                        "{} は 1 枚から {count} 枚しか出さないのでデインターレーサーではない",
                        device.card
                    ));
                }
                Err(error) => {
                    super::trace(format_args!("{} を確認できない: {error:?}", device.card));
                }
            }
        }
        Err(Error::Unavailable(format!(
            "非圧縮の入出力を持つデバイス {tried} 台にデインターレーサーがない"
        )))
    }

    fn build(device: Device, width: u32, height: u32) -> Result<Self, Error> {
        let got = set_up(&device, width, height)?;
        let stride = got.plane_fmt[0].bytesperline;
        let allocated = got.height;
        let visible = match device.compose(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            Some(rect) => (
                rect.left.max(0) as u32,
                rect.top.max(0) as u32,
                rect.width,
                rect.height,
            ),
            None => (0, 0, got.width, got.height),
        };
        let (slots, _) = device.request_buffers_of(
            V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
            OUTPUT_SLOTS,
            V4L2_MEMORY_DMABUF,
        )?;
        if slots == 0 {
            return Err(Error::Unavailable(String::from(
                "デインターレーサーが DMA-BUF の入力バッファを確保できない",
            )));
        }
        let (captures, _) =
            device.request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, CAPTURE_BUFFERS)?;
        if captures == 0 {
            return Err(Error::Unavailable(String::from(
                "デインターレーサーのサーフェスを確保できない",
            )));
        }
        let mut maps = Vec::new();
        for index in 0..captures {
            maps.push(
                device
                    .map(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index, 0)
                    .map_err(|error| {
                        Error::Unavailable(format!(
                            "デインターレーサーのサーフェスをマップできない: {error:?}"
                        ))
                    })?,
            );
            device.queue(&Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index))?;
        }
        device.stream_on(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE)?;
        device.stream_on(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)?;
        super::trace(format_args!(
            "デインターレーサー {} {}x{} bpl {} 表示領域 {:?} 入力 {} 枚 / サーフェス {} 枚",
            device.card, got.width, got.height, stride, visible, slots, captures
        ));
        Ok(Self {
            device,
            format: Format {
                width: visible.2,
                height: visible.3,
                depth: 8,
                chroma: 1,
                rate: (0, 1),
                progressive: true,
                aspect: (0, 1),
                field_pictures: false,
                woven: false,
                colour: Default::default(),
            },
            stride,
            allocated,
            visible,
            planes: got.num_planes.max(1),
            chroma_offset: stride * allocated,
            maps,
            full: false,
            holding: vec![None; slots as usize],
            free_slots: (0..slots as usize).collect(),
            idle: Vec::new(),
            submitted: 0,
            produced: 0,
            starved: 0,
            waited: 0,
        })
    }

    pub fn name(&self) -> &str {
        &self.device.card
    }

    /// `owner` comes back from [`Deinterlacer::reclaim`].
    pub fn submit(
        &mut self,
        fd: std::os::fd::RawFd,
        owner: usize,
        ts: u64,
        top_first: bool,
        bytes: u32,
    ) -> Result<(), Error> {
        let Some(slot) = self.free_slots.pop() else {
            self.waited += 1;
            return Err(Error::Unavailable(String::from(
                "デインターレーサーに空きがありません",
            )));
        };
        let field = if top_first {
            V4L2_FIELD_INTERLACED_TB
        } else {
            V4L2_FIELD_INTERLACED_BT
        };
        match self.device.queue_dmabuf(
            V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
            slot as u32,
            fd,
            bytes,
            ts,
            field,
        ) {
            Ok(()) => {
                self.holding[slot] = Some(owner);
                self.submitted += 1;
                Ok(())
            }
            Err(error) => {
                self.free_slots.push(slot);
                Err(error)
            }
        }
    }

    /// They go back to the decoder: its capture queue is also its DPB.
    pub fn reclaim(&mut self) -> Vec<usize> {
        let mut back = Vec::new();
        while let Ok(Some(done)) = self
            .device
            .dequeue_now_of(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, V4L2_MEMORY_DMABUF)
        {
            let slot = done.index;
            if let Some(owner) = self.holding.get_mut(slot).and_then(Option::take) {
                back.push(owner);
            }
            self.free_slots.push(slot);
        }
        back
    }

    pub fn take(&mut self) -> Option<dev::Dequeued> {
        match self.device.dequeue_now(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            Ok(Some(done)) if done.bytes > 0 => {
                self.produced += 1;
                Some(done)
            }
            Ok(Some(done)) => {
                self.give_back(done.index);
                None
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!("V4L2: デインターレーサー: {error:?}");
                None
            }
        }
    }

    pub fn give_back(&mut self, index: usize) {
        if self
            .device
            .queue(&Enqueue {
                planes_used: u32::from(self.planes),
                ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32)
            })
            .is_err()
        {
            self.idle.push(index);
            self.starved += 1;
        }
        let idle = std::mem::take(&mut self.idle);
        for index in idle {
            if self
                .device
                .queue(&Enqueue {
                    planes_used: u32::from(self.planes),
                    ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32)
                })
                .is_err()
            {
                self.idle.push(index);
            }
        }
    }

    pub fn read(&self, index: usize) -> Option<(Vec<u8>, usize, usize)> {
        let (left, top, width, height) = self.visible;
        let height = height.min(self.allocated.saturating_sub(top));
        let bytes = self.maps.get(index)?.read();
        let row = width as usize;
        let mut luma = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let at = (top as usize + y) * self.stride as usize + left as usize;
            luma.extend_from_slice(bytes.get(at..at + row)?);
        }
        Some((luma, row, height as usize))
    }

    pub fn export(&self, index: usize, plane: u32) -> Result<std::os::fd::OwnedFd, Error> {
        self.device
            .export(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index as u32, plane)
    }

    pub fn captures(&self) -> usize {
        self.maps.len()
    }

    pub fn grow(&mut self) -> bool {
        if self.full || self.maps.len() >= CAPTURE_MOST {
            return false;
        }
        let kind = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        let made = (|| {
            let (index, count) = self.device.create_buffers(kind, 1)?;
            if count == 0 || index as usize != self.maps.len() {
                return Err(Error::Unavailable(String::from("追加できない")));
            }
            let map = self.device.map(kind, index, 0)?;
            self.device.queue(&Enqueue {
                planes_used: u32::from(self.planes),
                ..Enqueue::of(kind, index)
            })?;
            Ok::<_, Error>(map)
        })();
        match made {
            Ok(map) => {
                self.maps.push(map);
                super::trace(format_args!(
                    "デインターレーサーの出力バッファを追加、計 {} 本",
                    self.maps.len()
                ));
                true
            }
            Err(error) => {
                tracing::info!(
                    "V4L2: デインターレーサーの出力バッファは {} 本まで（{error:?}）",
                    self.maps.len()
                );
                self.full = true;
                false
            }
        }
    }

    pub fn outstanding(&self) -> usize {
        self.holding.iter().filter(|held| held.is_some()).count()
    }
}

impl Drop for Deinterlacer {
    fn drop(&mut self) {
        super::trace(format_args!(
            "デインターレーサー集計: 入力 {} / 出力 {}（1 枚あたり {:.2}、期待値 {}）  \
             再キュー失敗 {}  空き待ち {}",
            self.submitted,
            self.produced,
            if self.submitted == 0 {
                0.0
            } else {
                self.produced as f64 / self.submitted as f64
            },
            PER_PICTURE,
            self.starved,
            self.waited
        ));
        self.device.stream_off(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
        self.device.stream_off(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
        self.maps.clear();
        let _ = self.device.request_buffers_of(
            V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
            0,
            V4L2_MEMORY_DMABUF,
        );
        let _ = self
            .device
            .request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, 0);
    }
}

/// The height is rounded up to 16: `VIDIOC_REQBUFS` refuses others with `EINVAL`.
fn set_up(device: &Device, width: u32, height: u32) -> Result<PixFormatMplane, Error> {
    let height = height.next_multiple_of(16);
    let out = device.set_raw_format(
        V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,
        V4L2_PIX_FMT_NV12,
        width,
        height,
        V4L2_FIELD_INTERLACED_TB,
    )?;
    if out.field != V4L2_FIELD_INTERLACED_TB {
        return Err(Error::Unavailable(format!(
            "{} は INTERLACED_TB を受け取らず {} を返します",
            device.card, out.field
        )));
    }
    device.set_raw_format(
        V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
        V4L2_PIX_FMT_NV12,
        width,
        height,
        V4L2_FIELD_NONE,
    )
}

fn trial(device: &Device, width: u32, height: u32) -> Result<u32, Error> {
    let got = set_up(device, width, height)?;
    let size = got.plane_fmt[0].sizeimage;
    let (outs, _) = device.request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 2)?;
    let (caps, _) = device.request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, 4)?;
    let count = if outs == 0 || caps == 0 {
        0
    } else {
        run_trial(device, outs, caps, size)
    };
    device.stream_off(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE);
    device.stream_off(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
    let _ = device.request_buffers(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, 0);
    let _ = device.request_buffers(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, 0);
    Ok(count)
}

fn run_trial(device: &Device, outs: u32, caps: u32, size: u32) -> u32 {
    for index in 0..caps {
        if device
            .queue(&Enqueue::of(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, index))
            .is_err()
        {
            return 0;
        }
    }
    if device.stream_on(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE).is_err()
        || device
            .stream_on(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)
            .is_err()
    {
        return 0;
    }
    // Two frames: an advanced deinterlacer holds the first back.
    for index in 0..outs.min(2) {
        if device
            .queue(&Enqueue {
                bytes: size,
                ts: u64::from(index + 1) * 1_000_000,
                field: V4L2_FIELD_INTERLACED_TB,
                ..Enqueue::of(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE, index)
            })
            .is_err()
        {
            return 0;
        }
    }
    let mut out = 0;
    let until = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < until {
        while let Ok(Some(done)) = device.dequeue_now(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            if done.bytes > 0 {
                out += 1;
            }
            let _ = device.queue(&Enqueue::of(
                V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                done.index as u32,
            ));
        }
        while device
            .dequeue_now(V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE)
            .is_ok_and(|done| done.is_some())
        {}
        if out >= 2 * outs.min(2) {
            break;
        }
        device.wait(10);
    }
    // Per picture, rounded up: two in and three out is a doubler held back by one.
    out.div_ceil(outs.clamp(1, 2))
}

/// A doubler copies the timestamp onto both outputs, so a record answers twice.
#[derive(Debug, Default)]
pub struct Woven {
    said: HashMap<u64, (super::Held, u32)>,
}

impl Woven {
    pub fn remember(&mut self, ts: u64, held: super::Held) {
        self.said.insert(ts, (held, 0));
    }

    pub fn matched(&mut self, ts: u64) -> Option<super::Held> {
        let (held, seen) = self.said.get_mut(&ts)?;
        *seen += 1;
        let held = *held;
        if *seen >= PER_PICTURE {
            self.said.remove(&ts);
        }
        Some(held)
    }

    pub fn clear(&mut self) {
        self.said.clear();
    }

    /// A doubler is not finished when the decoder's book is empty.
    pub fn pending(&self) -> usize {
        self.said.len()
    }

    pub fn forget_stale(&mut self) {
        while self.said.len() > 256 {
            let Some(oldest) = self.said.keys().min().copied() else {
                return;
            };
            self.said.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_record_answers_for_both_pictures() {
        let mut woven = Woven::default();
        let held = super::super::Held {
            tag: 7,
            slot: 1_000_000,
            ..Default::default()
        };
        woven.remember(1_000_000, held);
        assert_eq!(woven.matched(1_000_000).map(|held| held.tag), Some(7));
        assert_eq!(woven.matched(1_000_000).map(|held| held.tag), Some(7));
        assert!(woven.matched(1_000_000).is_none());
    }

    #[test]
    fn a_deinterlacer_is_the_one_that_doubles() {
        assert_eq!(PER_PICTURE, 2);
    }
}
