// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The H.264 software decoder: Main and High, 4:2:0, eight bits.
//! 124/128° CS codes every picture as a field; MBAFF is refused.

#[cfg(feature = "h264-decoder")]
mod cabac;
#[cfg(feature = "h264-decoder")]
mod deblock;
pub mod dpb;
#[cfg(feature = "h264-decoder")]
mod inter;
#[cfg(feature = "h264-decoder")]
mod intra;
#[cfg(feature = "h264-decoder")]
mod mb;
pub mod params;
#[cfg(feature = "h264-decoder")]
mod slice;
#[cfg_attr(not(feature = "h264-decoder"), allow(dead_code))]
mod tables;
#[cfg(test)]
#[cfg(feature = "h264-decoder")]
mod tests;
#[cfg(feature = "h264-decoder")]
mod transform;
#[cfg(feature = "h264-decoder")]
mod vlc;

#[cfg(feature = "h264-decoder")]
use std::collections::VecDeque;

#[cfg(feature = "h264-decoder")]
use crate::bits::unescape;
#[cfg(feature = "h264-decoder")]
use crate::es::{Format, Framing, Nals, Picture, Structure};
#[cfg(feature = "h264-decoder")]
use crate::video::{Decoder, Error, Frame, Plane};

#[cfg(feature = "h264-decoder")]
use dpb::{Dpb, Marked, Marking, Store};
#[cfg(feature = "h264-decoder")]
use mb::Mb;
#[cfg(feature = "h264-decoder")]
use params::{Header, Kind as SliceKind, Sps};
#[cfg(feature = "h264-decoder")]
use slice::{Colocated, Geometry, Reference};

#[cfg(feature = "h264-decoder")]
const SLICE: u8 = 1;
#[cfg(feature = "h264-decoder")]
const IDR: u8 = 5;
#[cfg(feature = "h264-decoder")]
const SEQUENCE: u8 = 7;
#[cfg(feature = "h264-decoder")]
const PICTURE: u8 = 8;

#[derive(Debug, Clone, Copy, Default)]
#[cfg(feature = "h264-decoder")]
struct Held {
    tag: u64,
    structure: Structure,
    top_field_first: bool,
    repeat_first_field: bool,
    progressive: bool,
}

#[cfg(feature = "h264-decoder")]
impl dpb::Ordered for Held {
    fn set_order(&mut self, _order: i32) {}
}

#[derive(Debug, Clone)]
#[cfg(feature = "h264-decoder")]
struct Coding {
    geometry: Geometry,
    poc: i32,
    slices: u16,
}

#[cfg(feature = "h264-decoder")]
struct Slots<'a> {
    planes: &'a [[Vec<u8>; 3]],
    format: Format,
    stride: [usize; 3],
    ready: &'a mut VecDeque<Frame>,
}

#[cfg(feature = "h264-decoder")]
impl dpb::Slots for Slots<'_> {
    type Id = usize;
    type Held = Held;

    fn take(&mut self, stores: &[Store<usize, Held>]) -> Option<usize> {
        (0..self.planes.len()).find(|at| !stores.iter().any(|store| store.slot == *at))
    }

    fn release(&mut self, _id: usize) {}

    fn emit(&mut self, store: &Store<usize, Held>) {
        let buffer = &self.planes[store.slot];
        if buffer[0].is_empty() {
            return;
        }
        let width = self.format.width as usize;
        let height = self.format.height as usize;
        let stride = self.stride;
        let planes = std::array::from_fn(|plane| {
            let divisor = if plane == 0 { 1 } else { 2 };
            Plane {
                data: buffer[plane].clone(),
                stride: stride[plane],
                width: width.div_ceil(divisor),
                height: height.div_ceil(divisor),
            }
        });
        self.ready.push_back(Frame {
            tag: store.held.tag,
            format: self.format,
            planes,
            structure: store.held.structure,
            top_field_first: store.held.top_field_first,
            repeat_first_field: store.held.repeat_first_field,
            progressive: store.held.progressive,
        });
    }
}

#[derive(Debug)]
#[cfg(feature = "h264-decoder")]
pub struct H264 {
    format: Format,
    sets: params::Sets,
    planes: Vec<[Vec<u8>; 3]>,
    /// Per field; a frame picture keeps its own in the first.
    mbs: Vec<[Vec<Mb>; 2]>,
    target: [Vec<u8>; 3],
    target_mbs: Vec<Mb>,
    mvd: Vec<[[[i16; 2]; 16]; 2]>,
    dpb: Dpb<usize, Held>,
    current: Option<dpb::Current<Coding>>,
    stride: [usize; 3],
    frame_height: [usize; 3],
    ready: VecDeque<Frame>,
}

#[cfg(feature = "h264-decoder")]
impl Default for H264 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "h264-decoder")]
impl H264 {
    pub fn new() -> Self {
        Self {
            format: Format::default(),
            sets: params::Sets::default(),
            planes: Vec::new(),
            mbs: Vec::new(),
            target: [Vec::new(), Vec::new(), Vec::new()],
            target_mbs: Vec::new(),
            mvd: Vec::new(),
            dpb: Dpb::new(),
            current: None,
            stride: [0; 3],
            frame_height: [0; 3],
            ready: VecDeque::new(),
        }
    }

    fn reconfigure(&mut self, sps: &Sps, format: Format) {
        self.format = format;
        let mb_width = sps.mb_width;
        let mb_height = sps.mb_height();
        let stride = [mb_width * 16, mb_width * 8, mb_width * 8];
        let height = [mb_height * 16, mb_height * 8, mb_height * 8];
        let frames = dpb::dpb_frames(sps);
        self.dpb.reorder = sps
            .max_reorder
            .map_or(frames, |count| count as usize)
            .min(frames);
        self.dpb.capacity = frames;
        self.stride = stride;
        self.frame_height = height;
        self.planes = (0..frames + 1)
            .map(|_| std::array::from_fn(|plane| vec![128u8; stride[plane] * height[plane]]))
            .collect();
        self.mbs = (0..frames + 1).map(|_| [Vec::new(), Vec::new()]).collect();
        self.target = std::array::from_fn(|plane| vec![128u8; stride[plane] * height[plane]]);
        self.dpb.stores.clear();
        self.dpb.pending = None;
        self.dpb.serial = 0;
        self.current = None;
        self.ready.clear();
    }
}

#[cfg(feature = "h264-decoder")]
impl H264 {
    fn with_slots<R>(&mut self, act: impl FnOnce(&mut Dpb<usize, Held>, &mut Slots<'_>) -> R) -> R {
        let Self {
            dpb,
            planes,
            format,
            stride,
            ready,
            ..
        } = self;
        let mut slots = Slots {
            planes,
            format: *format,
            stride: *stride,
            ready,
        };
        act(dpb, &mut slots)
    }

    fn free_buffer(&mut self) -> Option<usize> {
        self.with_slots(|dpb, slots| dpb.free_slot(slots))
    }

    fn bump(&mut self) -> bool {
        self.with_slots(|dpb, slots| dpb.bump(slots))
    }

    fn drain(&mut self) {
        self.with_slots(|dpb, slots| dpb.drain(slots));
    }

    fn refresh(&mut self, no_output_of_prior_pics: bool) {
        self.with_slots(|dpb, slots| dpb.refresh(slots, no_output_of_prior_pics));
    }

    fn mark(&mut self, serial: i32, parity: usize, marking: &Marking) {
        self.with_slots(|dpb, slots| dpb.mark(slots, serial, parity, marking));
    }
}

#[cfg(feature = "h264-decoder")]
impl H264 {
    fn reference(&self, found: &dpb::Reference) -> Reference {
        let store = &self.dpb.stores[found.at];
        let current = self
            .current
            .as_ref()
            .and_then(|current| self.dpb.index(current.serial));
        Reference {
            // Only a second field predicting from the first points at the current picture.
            buffer: if Some(found.at) == current {
                None
            } else {
                Some(store.slot)
            },
            parity: found.parity,
            id: found.id,
            poc: match found.parity {
                None => store.order(),
                Some(parity) => store.poc[parity],
            },
            long_term: found.long_term,
        }
    }
}

#[cfg(feature = "h264-decoder")]
impl H264 {
    fn start(
        &mut self,
        tag: u64,
        picture: &Picture,
        header: &Header,
        sps: &Sps,
    ) -> Result<(), Error> {
        let poc = self.dpb.order_count(sps, header);
        let parity = usize::from(header.bottom_field);
        let field = header.field_pic;

        if header.idr {
            self.refresh(header.no_output_of_prior_pics);
        }

        let waiting = self
            .dpb
            .pending
            .and_then(|(serial, first)| Some((self.dpb.index(serial)?, first)));
        let second = match waiting {
            Some((at, first)) if field && first != parity => {
                self.dpb.stores[at].frame_num == header.frame_num
            }
            _ => false,
        };

        let store = if second {
            self.dpb.pending = None;
            waiting.expect("a field was waiting").0
        } else {
            if waiting.is_some() {
                self.dpb.pending = None;
                if let Some(store) = self.dpb.stores.last_mut() {
                    store.output = false;
                }
            }
            self.finish_target();
            let Some(buffer) = self.free_buffer() else {
                return Err(Error::Damaged(
                    "a picture buffer that cannot be made room in",
                ));
            };
            self.target = std::mem::take(&mut self.planes[buffer]);
            if self.target[0].is_empty() {
                self.target = std::array::from_fn(|plane| {
                    vec![128u8; self.stride[plane] * self.frame_height[plane]]
                });
            }
            let serial = self.dpb.next_serial();
            self.dpb.stores.push(Store {
                slot: buffer,
                serial,
                frame_num: header.frame_num,
                wrap: header.frame_num as i32,
                long_index: 0,
                present: [false; 2],
                has_poc: [false; 2],
                poc: [0; 2],
                marked: [Marked::No; 2],
                output: true,
                held: Held {
                    tag,
                    structure: picture.structure,
                    top_field_first: match picture.structure {
                        Structure::Frame => picture.top_field_first,
                        Structure::Top => true,
                        Structure::Bottom => false,
                    },
                    repeat_first_field: picture.repeat_first_field,
                    progressive: picture.progressive,
                },
            });
            self.dpb.stores.len() - 1
        };

        let mb_width = sps.mb_width;
        let mb_height = sps.mb_height() / if field { 2 } else { 1 };
        let count = mb_width * mb_height;
        if second {
            self.target = std::mem::take(&mut self.planes[self.dpb.stores[store].slot]);
        }
        self.target_mbs.clear();
        self.target_mbs.resize(count, Mb::default());
        self.mvd.clear();
        self.mvd.resize(count, [[[0; 2]; 16]; 2]);

        self.dpb.stores[store].poc[parity] = poc[parity];
        self.dpb.stores[store].has_poc[parity] = true;
        if !field {
            self.dpb.stores[store].poc = poc;
            self.dpb.stores[store].has_poc = [true; 2];
        }
        self.current = Some(dpb::Current {
            serial: self.dpb.stores[store].serial,
            parity,
            field,
            marking: Marking::new(header, sps),
            extra: Coding {
                poc: poc[parity],
                slices: 0,
                geometry: Geometry {
                    stride: self.stride,
                    at: std::array::from_fn(|plane| {
                        if field {
                            parity * self.stride[plane]
                        } else {
                            0
                        }
                    }),
                    row: std::array::from_fn(|plane| {
                        self.stride[plane] * if field { 2 } else { 1 }
                    }),
                    width: [mb_width * 16, mb_width * 8, mb_width * 8],
                    height: [mb_height * 16, mb_height * 8, mb_height * 8],
                    mb_width,
                    mb_height,
                    field,
                    bottom: parity == 1,
                },
            },
        });
        Ok(())
    }

    fn finish_target(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        let Some(at) = self.dpb.index(current.serial) else {
            return;
        };
        let buffer = self.dpb.stores[at].slot;
        let geometry = current.extra.geometry;
        {
            let (luma, chroma) = self.target.split_at_mut(1);
            let (cb, cr) = chroma.split_at_mut(1);
            let mut planes = [
                deblock::Plane {
                    data: &mut luma[0],
                    at: geometry.at[0],
                    stride: geometry.row[0],
                },
                deblock::Plane {
                    data: &mut cb[0],
                    at: geometry.at[1],
                    stride: geometry.row[1],
                },
                deblock::Plane {
                    data: &mut cr[0],
                    at: geometry.at[2],
                    stride: geometry.row[2],
                },
            ];
            deblock::filter(
                &mut planes,
                &deblock::Picture {
                    mbs: &self.target_mbs,
                    mb_width: geometry.mb_width,
                    mb_height: geometry.mb_height,
                    field: geometry.field,
                },
            );
        }
        self.planes[buffer] = std::mem::take(&mut self.target);
        self.mbs[buffer][if current.field { current.parity } else { 0 }] =
            std::mem::take(&mut self.target_mbs);
        let store = &mut self.dpb.stores[at];
        store.present[current.parity] = true;
        if !current.field {
            store.present = [true; 2];
        }
        self.mark(current.serial, current.parity, &current.marking);
        let complete = self
            .dpb
            .index(current.serial)
            .is_some_and(|at| self.dpb.stores[at].complete());
        self.dpb.pending = if complete {
            None
        } else {
            Some((current.serial, current.parity))
        };
        self.drain();
    }
}

#[cfg(feature = "h264-decoder")]
impl H264 {
    #[allow(clippy::too_many_arguments)]
    fn read_slice(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        idr: bool,
        idc: u32,
        bytes: &[u8],
        started: &mut bool,
    ) -> Result<(), Error> {
        let data = unescape(bytes, bytes.len());
        let found = params::header(&data, idr, idc, |id| self.sets.find(id));
        let Some((header, pps, sps)) = found else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        if header.redundant != 0 {
            return Ok(());
        }

        if !*started {
            if sps.mb_adaptive {
                return Err(Error::Unsupported(
                    "macroblock adaptive frame and field coding",
                ));
            }
            // 符号化したサイズを超えるものは渡せない（サーフェスの幅が行の長さを超える）。
            if format.width as usize > sps.mb_width * 16
                || format.height as usize > sps.mb_height() * 16
            {
                return Err(Error::Damaged(
                    "a picture larger than its sequence parameter set",
                ));
            }
            if self.format != format
                || self.planes.is_empty()
                || self.stride[0] != sps.mb_width * 16
                || self.frame_height[0] != sps.mb_height() * 16
            {
                self.reconfigure(&sps, format);
            }
            self.start(tag, picture, &header, &sps)?;
            self.dpb.number_frames(&sps, header.frame_num);
            *started = true;
        }
        let Some(current) = self.current.clone() else {
            return Err(Error::Damaged("a slice with no picture"));
        };
        if current.extra.geometry.field != header.field_pic {
            return Err(Error::Damaged("a picture whose slices disagree"));
        }

        let full = self
            .dpb
            .initial_lists(&header, current.extra.poc, current.parity);
        let mut chosen = full.clone();
        for (list, into) in chosen.iter_mut().enumerate() {
            into.truncate(header.num_ref_idx[list]);
        }
        self.dpb.reorder_lists(&header, &sps, &mut chosen, &full);
        for (list, into) in chosen.iter_mut().enumerate() {
            // A short list means a lost reference; repeating entries keeps the damage in the picture.
            while into.len() < header.num_ref_idx[list] {
                let Some(first) = into.first().copied() else {
                    break;
                };
                into.push(first);
            }
        }
        let lists: [Vec<Reference>; 2] = std::array::from_fn(|list| {
            chosen[list]
                .iter()
                .map(|found| self.reference(found))
                .collect()
        });

        let colocated = if header.kind == SliceKind::B {
            lists[1]
                .first()
                .and_then(|found| {
                    let buffer = found.buffer?;
                    let side = found.parity.unwrap_or(0);
                    let mbs = self.mbs[buffer][side].as_slice();
                    if mbs.len() != self.target_mbs.len() {
                        return None;
                    }
                    Some(Colocated {
                        mbs: Some(mbs),
                        poc: found.poc,
                    })
                })
                .unwrap_or_default()
        } else {
            Colocated::default()
        };

        let weights = params::weights(&sps, &pps);
        let geometry = current.extra.geometry;
        let number = current.extra.slices;
        if let Some(current) = self.current.as_mut() {
            current.extra.slices = current.extra.slices.saturating_add(1);
        }
        let mut coder = slice::Coder::new(
            &header,
            &sps,
            &pps,
            &weights,
            geometry,
            &mut self.target,
            &self.planes,
            &mut self.target_mbs,
            &mut self.mvd,
            &lists,
            colocated,
            current.extra.poc,
            number,
            &data,
        );
        coder.run()
    }
}

#[cfg(feature = "h264-decoder")]
impl Decoder for H264 {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format above 4:2:0"));
        }
        if format.depth != 8 {
            return Err(Error::Unsupported("a sample deeper than eight bits"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Damaged("a parameter set with no size"));
        }

        let mut started = false;
        let mut damage = None;
        for nal in Nals::new(data, Framing::StartCodes) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5 & 3);
            let bytes = &data[nal.body.clone()];
            match kind {
                SEQUENCE | PICTURE => self.sets.absorb(kind, bytes),
                SLICE | IDR => {
                    let error = self.read_slice(
                        tag,
                        picture,
                        format,
                        kind == IDR,
                        idc,
                        bytes,
                        &mut started,
                    );
                    if let Err(error) = error {
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }
        if !started {
            return Err(damage.unwrap_or(Error::Damaged("an access unit with no slice")));
        }
        self.finish_target();
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take(&mut self) -> Option<Frame> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        self.finish_target();
        if let Some((serial, _)) = self.dpb.pending.take()
            && let Some(at) = self.dpb.index(serial)
        {
            self.dpb.stores[at].output = false;
        }
        while self.bump() {}
    }

    fn reset(&mut self) {
        self.dpb.stores.clear();
        self.dpb.pending = None;
        self.dpb.forget();
        self.current = None;
        self.ready.clear();
    }
}
