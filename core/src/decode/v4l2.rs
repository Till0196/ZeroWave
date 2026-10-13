// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! V4L2 decoders, stateless (Request API) and stateful (M2M), chosen per device.
//! Pictures come back as DMA-BUF; the decoder does not deinterlace.

pub mod ctrl;
pub mod deint;
pub mod dev;
mod h264;
mod hevc;
pub mod modifier;
mod mpeg2;
pub mod pool;
pub mod probe;
mod stateful;
pub mod uapi;

use std::fs::File;

use mediadecoder::es::{Codec, Format, Framing, Picture, Structure};

use crate::decode::each_front;
use crate::decode::takes;
use crate::decode::{
    About, Decode, Decoded, DeinterlaceBy, DmaBuf, DmaBufPlane, Error, Held as Where, Readback,
    Surface,
};
use crate::trace::Trace;
use ctrl::{Ctl, Naming};
use dev::{Device, Enqueue, Request};
use pool::Pool;
use uapi::*;

mod parsed;
mod setup;
mod stateless;

/// Named by its output buffer's timestamp (how V4L2 names a reference).
pub type Held = crate::decode::frames::Held<u64>;

/// How much of a picture goes into one request; the driver decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Slice,
    Frame,
}

/// One request's worth of work: ranges of the caller's access unit.
#[derive(Debug)]
pub struct Unit {
    pub controls: Vec<Ctl>,
    pub pieces: Vec<std::ops::Range<usize>>,
    pub zeroes: usize,
}

#[derive(Debug)]
pub struct Submission {
    pub ts: u64,
    pub units: Vec<Unit>,
    /// The second field of a pair follows: the capture buffer is held open.
    pub more: bool,
}

#[derive(Debug)]
enum Front {
    Mpeg2(Box<mpeg2::Mpeg2>),
    H264(Box<h264::H264>),
    Hevc(Box<hevc::Hevc>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Stateless,
    Stateful,
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    planes: u8,
    modifier: u64,
    drm: [u32; 2],
    chroma_offset: u32,
    /// Not `stride` for SAND.
    export_stride: u32,
    stride: u32,
    /// Buffer height, not picture height (1088 for 1080).
    allocated: u32,
    visible: (u32, u32, u32, u32),
    /// 1 サンプルのビット数。10 ビットのストリームでも 8 ビットの形式で受けることがあり、出力のフレームの深さはこれにする。
    depth: u8,
}

#[derive(Debug)]
struct Session {
    outputs: Vec<dev::Mapping>,
    free_outputs: Vec<usize>,
    requests: Vec<Request>,
    format: Format,
    /// `None` until the driver says what it produces (stateful: several AUs in).
    capture: Option<Capture>,
}

#[derive(Debug)]
struct Capture {
    layout: Layout,
    pool: Pool,
    /// Given back but not requeueable at the time.
    idle: Vec<usize>,
    fds: Vec<std::os::fd::OwnedFd>,
    size: u32,
    maps: Vec<dev::Mapping>,
    /// 色差が別プレーンの形（NV12M）の、各バッファの二つ目のプレーン。読み戻しのときだけ使う。
    chroma_maps: Vec<dev::Mapping>,
}

#[derive(Debug)]
pub struct V4l2 {
    device: Device,
    media: Option<File>,
    flavour: Flavour,
    naming: Naming,
    codec: Codec,
    compressed: u32,
    mode: Mode,
    prefix: u32,
    front: Front,
    stateful: Option<stateful::Stateful>,
    session: Option<Session>,
    format: Format,
    /// HEVC's coded size from the last SPS, given to the output format.
    coded: Option<(u32, u32)>,
    /// 最後の SPS の制御。キャプチャーの形式を決める前に渡す（[`Self::start_capture`]）。
    sequence: Option<Ctl>,
    /// HEVC の SPS の DPB の大きさ（`sps_max_dec_pic_buffering_minus1 + 1`、いちばん上の時間レイヤー）。
    /// 参照に取っておく capture バッファの数になる。
    dpb: Option<u32>,
    hold: bool,
    /// No hold (cedrus MPEG-2): one capture buffer queued per picture so the second
    /// field goes into the first field's buffer.
    by_hand: bool,
    open_field: Option<(u64, usize)>,
    finished: std::collections::VecDeque<(Held, usize)>,
    saw_field: u32,
    /// The `V4L2_BUF_FLAG_LAST` buffer came back: the only sign the drain is over.
    saw_last: bool,
    saw_eos: bool,
    by: DeinterlaceBy,
    deint: Option<deint::Deinterlacer>,
    /// A doubling filter answers twice per timestamp; keep records past the first match.
    woven: std::boxed::Box<deint::Woven>,
    /// Waiting for a deinterlacer slot inside decode would deadlock, so it is deferred.
    waiting: std::collections::VecDeque<(usize, Held)>,
    issued: std::collections::VecDeque<(usize, std::sync::Arc<crate::decode::slots::Lease>)>,
}

const START_CODE: [u8; 3] = [0, 0, 1];

pub(crate) fn trace(what: std::fmt::Arguments<'_>) {
    if SAY.on() {
        tracing::info!("V4L2: {what}");
    }
}

pub(crate) static SAY: Trace = Trace::of("v4l2");

/// A stateful decoder reads several access units before saying the size.
const OUTPUT_BUFFERS: u32 = 6;

/// Small on purpose: each one held is a buffer the deinterlacer cannot write into.
const WOVEN_KEEP: usize = 3;

pub fn open(codec: Codec, framing: Framing) -> Result<V4l2, Error> {
    V4l2::open(codec, framing)
}

impl V4l2 {
    /// Opens only the device; queues wait for the sequence header.
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_with(codec, framing, DeinterlaceBy::default())
    }

    pub fn open_with(codec: Codec, framing: Framing, by: DeinterlaceBy) -> Result<Self, Error> {
        let found = probe::find(codec)?;
        let device = found.device;
        let naming = match (codec, found.flavour) {
            (Codec::Hevc, Flavour::Stateless) => device.hevc_naming()?,
            _ => Naming::Stateless,
        };
        let (mode, prefix) = match found.flavour {
            Flavour::Stateless => {
                device.check_controls(naming, wanted_controls(codec))?;
                probe::mode_of(&device, codec, naming)?
            }
            Flavour::Stateful => (Mode::Frame, 0),
        };
        let media = match found.flavour {
            Flavour::Stateless => Some(dev::media_for(&device)?),
            Flavour::Stateful => None,
        };
        let stateful = match found.flavour {
            Flavour::Stateful => {
                let stateful = stateful::Stateful::new(framing);
                if !stateful.takes() {
                    return Err(Error::Unavailable(String::from(
                        "stateful のデコーダーは Annex B しか読めませんが、\
                         このストリームは長さプレフィックス形式です",
                    )));
                }
                Some(stateful)
            }
            Flavour::Stateless => None,
        };
        let mut front = match codec {
            Codec::Mpeg2 => Front::Mpeg2(Box::default()),
            Codec::H264 => Front::H264(Box::new(h264::H264::new(framing))),
            Codec::Hevc => Front::Hevc(Box::new(hevc::Hevc::new(framing))),
        };
        match &mut front {
            Front::Mpeg2(front) => {
                front.configure(mode, prefix);
                front.take_field_height(device.driver == "cedrus");
            }
            Front::H264(front) => front.configure(mode, prefix),
            Front::Hevc(front) => {
                front.configure(mode, prefix);
                front.take_entry_points(
                    found.flavour == Flavour::Stateless
                        && device
                            .control_size(ctrl::Kind::HevcEntryPointOffsets.id_for(naming))
                            .is_some(),
                );
            }
        }
        Ok(Self {
            device,
            media,
            flavour: found.flavour,
            naming,
            codec,
            compressed: found.compressed,
            mode,
            prefix,
            front,
            stateful,
            session: None,
            format: Format::default(),
            coded: None,
            sequence: None,
            dpb: None,
            hold: false,
            by_hand: false,
            open_field: None,
            finished: std::collections::VecDeque::new(),
            saw_field: V4L2_FIELD_ANY,
            saw_last: false,
            saw_eos: false,
            by,
            deint: None,
            woven: Box::default(),
            waiting: std::collections::VecDeque::new(),
            issued: std::collections::VecDeque::new(),
        })
    }

    pub fn driver(&self) -> &str {
        &self.device.driver
    }

    pub fn flavour(&self) -> Flavour {
        self.flavour
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn field_seen(&self) -> u32 {
        self.saw_field
    }
}

pub(crate) fn wanted_controls(codec: Codec) -> &'static [ctrl::Kind] {
    use ctrl::Kind::{
        H264DecodeParams, H264Pps, H264PredWeights, H264ScalingMatrix, H264SliceParams, H264Sps,
        HevcDecodeParams, HevcPps, HevcSliceParams, HevcSps, Mpeg2Picture, Mpeg2Quantisation,
        Mpeg2Sequence,
    };
    match codec {
        Codec::Mpeg2 => &[Mpeg2Sequence, Mpeg2Picture, Mpeg2Quantisation],
        Codec::H264 => &[
            H264Sps,
            H264Pps,
            H264ScalingMatrix,
            H264PredWeights,
            H264SliceParams,
            H264DecodeParams,
        ],
        Codec::Hevc => &[HevcSps, HevcPps, HevcSliceParams, HevcDecodeParams],
    }
}

impl Drop for V4l2 {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl Decode for V4l2 {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if let Front::Hevc(front) = &self.front
            && let Some(sps) = front.sequence(data)
        {
            self.coded = Some((sps.width as u32, sps.height as u32));
            self.dpb = Some(sps.max_dec_pic_buffering as u32);
            self.sequence = Some(hevc::sequence_control(&sps));
        }
        self.configure(format)?;
        if self.stateful.is_some() {
            return self.feed_stateful(tag, picture, data);
        }
        let submission = {
            let Some(pool) = self
                .session
                .as_mut()
                .and_then(|session| session.capture.as_mut())
                .map(|capture| &mut capture.pool)
            else {
                return Err(Error::Unavailable(String::from("バッファがありません")));
            };
            each_front!(&mut self.front, |front| front
                .decode(pool, tag, picture, format, data))?
        };
        let outcome = match submission {
            Some(submission) => self.submit(submission, data),
            None => Ok(()),
        };
        if let Some(pool) = self
            .session
            .as_mut()
            .and_then(|session| session.capture.as_mut())
            .map(|capture| &mut capture.pool)
        {
            each_front!(&mut self.front, |front| front.finished(pool));
        }
        self.recycle();
        outcome
    }

    fn take(&mut self) -> Option<Decoded> {
        let (held, index) = self.next_picture()?;
        self.export(held, index)
    }

    /// An alternative to [`Decode::take`], not a step before it.
    fn read_back(&mut self) -> Option<Readback> {
        let (held, index) = self.next_picture()?;
        self.read_one(held, index)
    }

    fn flush(&mut self) {
        if let Some(stateful) = self.stateful.as_mut() {
            if !stateful.draining() {
                stateful.drain();
                // Some drivers lose what is still queued when the drain is asked for.
                self.wait_for_output_drained();
                let back = self.session.as_ref().map_or((0, 0), |session| {
                    (session.free_outputs.len(), session.outputs.len())
                });
                let stop = self.device.decoder_stop();
                trace(format_args!(
                    "ドレイン: output {}/{} 返却、capture {}、DECODER_CMD(STOP) {:?}",
                    back.0,
                    back.1,
                    if self.capture_ready() {
                        "あり"
                    } else {
                        "なし"
                    },
                    stop
                ));
                if let Err(error) = stop {
                    tracing::warn!("V4L2: DECODER_CMD(STOP) に失敗: {error:?}");
                }
            }
            let ring = std::mem::take(&mut self.issued);
            for (index, _) in ring {
                if let Some(filter) = self.deint.as_mut() {
                    filter.give_back(index);
                }
            }
            // The LAST flag, not the outstanding count, ends the drain (pictures may be dropped).
            let began = std::time::Instant::now();
            let until = began + std::time::Duration::from_secs(3);
            loop {
                self.pump_events();
                self.collect_stateful();
                self.pump_waiting();
                let empty = self
                    .stateful
                    .as_ref()
                    .is_none_or(|stateful| stateful.outstanding() == 0);
                // The caller takes nothing inside this call, so a full hand ends the wait.
                let full = if self.deint.is_some() {
                    self.finished.len() >= WOVEN_KEEP
                } else {
                    self.capture()
                        .is_some_and(|capture| self.finished.len() >= capture.maps.len())
                };
                let empty = empty
                    && self.waiting.is_empty()
                    && self.deint.as_ref().is_none_or(|f| f.outstanding() == 0)
                    && self.woven.pending() == 0;
                if self.saw_last
                    || (self.capture_ready() && empty)
                    || full
                    || std::time::Instant::now() >= until
                {
                    trace(format_args!(
                        "ドレイン完了 {:?}: last={} eos={} 残り {} 取得 {} capture={}",
                        began.elapsed(),
                        self.saw_last,
                        self.saw_eos,
                        self.stateful
                            .as_ref()
                            .map_or(0, stateful::Stateful::outstanding),
                        self.finished.len(),
                        self.capture_ready()
                    ));
                    break;
                }
                self.device.wait(10);
            }
            return;
        }
        let Some(pool) = self
            .session
            .as_mut()
            .and_then(|session| session.capture.as_mut())
            .map(|capture| &mut capture.pool)
        else {
            return;
        };
        each_front!(&mut self.front, |front| front.flush(pool))
    }

    fn deinterlaces(&self) -> bool {
        self.deint.is_some()
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.deint.is_some() {
            deint::PER_PICTURE
        } else {
            1
        }
    }

    fn reset(&mut self) {
        if let Some(stateful) = self.stateful.as_mut() {
            stateful.reset();
        }
        // 手元のバッファは DQBUF 済みなので、捨てずに所有者へ返す（捨てるとデコードが止まる）。
        let finished = std::mem::take(&mut self.finished);
        let issued = std::mem::take(&mut self.issued);
        let waiting = std::mem::take(&mut self.waiting);
        match self.deint.as_mut() {
            Some(filter) => {
                for index in finished
                    .into_iter()
                    .map(|(_, index)| index)
                    .chain(issued.into_iter().map(|(index, _)| index))
                {
                    filter.give_back(index);
                }
            }
            None => {
                if let Some(capture) = self.capture_mut() {
                    capture
                        .idle
                        .extend(finished.into_iter().map(|(_, index)| index));
                }
            }
        }
        if let Some(capture) = self.capture_mut() {
            capture
                .idle
                .extend(waiting.into_iter().map(|(index, _)| index));
        }
        if let Some((_, index)) = self.open_field.take()
            && let Some(capture) = self.capture_mut()
        {
            capture.idle.push(index);
        }
        self.saw_last = false;
        self.saw_eos = false;
        self.woven.clear();
        let Some(pool) = self
            .session
            .as_mut()
            .and_then(|session| session.capture.as_mut())
            .map(|capture| &mut capture.pool)
        else {
            return;
        };
        each_front!(&mut self.front, |front| front.reset(pool));
        if let Some(capture) = self.capture_mut() {
            capture.pool.forget_recent();
        }
        self.recycle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_coding_asks_for_the_buffers_its_reference_buffer_needs() {
        let count =
            |references: u32, asked: u32| references.max(asked + 2).max(6) + pool::SLACK as u32;
        let slack = pool::SLACK as u32;
        assert_eq!(count(3, 0), 6 + slack);
        assert_eq!(count(17, 0), 17 + slack);
        assert_eq!(count(16, 0), 16 + slack);
        assert_eq!(count(3, 1), 6 + slack);
        assert_eq!(count(3, 12), 14 + slack);
    }

    #[test]
    fn every_control_sent_is_one_that_can_be_checked() {
        for codec in [Codec::Mpeg2, Codec::H264, Codec::Hevc] {
            for kind in wanted_controls(codec) {
                assert!(kind.size() > 0, "{} has no size", kind.name());
            }
        }
    }

    #[test]
    fn the_start_code_is_three_bytes() {
        assert_eq!(START_CODE, [0, 0, 1]);
    }
}
