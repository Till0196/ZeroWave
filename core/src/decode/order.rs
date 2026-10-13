// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 表示順を決める。VideoToolbox はここで並べ替え（[`Queue`]）、Media Foundation はキーだけ使う。
//! 規則はソフトウェアデコーダー・VA-API と合わせる。
//! TODO: フィールド対と H.264 の `memory_management_control_operation = 5` は確かめていない。

use mediadecoder::bits::{Reader, unescape};
use mediadecoder::es::{Coding, Framing, Nals, Picture};
use mediadecoder::video::h264::dpb::dpb_frames;
use mediadecoder::video::h264::params as h264;
use mediadecoder::video::hevc::params as hevc;

const PARAMETER_BYTES: usize = 4096;

/// スライスヘッダーをどれだけ復元してから読むか（POC までは先頭の数十バイト）。
const HEADER_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Default)]
pub struct Key {
    pub order: i32,
    /// 入れる前にバッファにあるものを全部出す（シーケンスの切れ目、POC が 0 に戻る）。
    pub drain_before: bool,
    pub bidirectional: bool,
    pub depth: usize,
}

#[derive(Debug)]
pub enum Keys {
    Mpeg2,
    H264(Box<H264>),
    Hevc(Box<Hevc>),
}

impl Keys {
    /// アクセスユニットを読んで表示順のキーを返す。読めなければ既定のキー（捨てるより遅れて出す）。
    pub fn key(&mut self, picture: &Picture, framing: Framing, data: &[u8]) -> Key {
        match self {
            Self::Mpeg2 => Key {
                bidirectional: picture.coding == Coding::B,
                ..Key::default()
            },
            Self::H264(state) => state.key(framing, data),
            Self::Hevc(state) => state.key(framing, data),
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::Mpeg2 => {}
            Self::H264(state) => **state = H264::default(),
            Self::Hevc(state) => **state = Hevc::default(),
        }
    }
}

/// H.264 の POC（節 8.2.1）。`mediadecoder::video::h264::dpb` と同じ式で数える。
#[derive(Debug, Default)]
pub struct H264 {
    sps: std::collections::BTreeMap<u32, h264::Sps>,
    pps: std::collections::BTreeMap<u32, (u32, bool)>,
    previous_msb: i32,
    previous_lsb: i32,
    previous_offset: i32,
    previous_frame_num: u32,
}

impl H264 {
    fn key(&mut self, framing: Framing, data: &[u8]) -> Key {
        let mut key = Key::default();
        let mut found = false;
        for nal in Nals::new(data, framing) {
            let kind = nal.header & 0x1f;
            let idc = u32::from(nal.header >> 5) & 0x03;
            let body = data.get(nal.body.clone()).unwrap_or(&[]);
            match kind {
                7 => {
                    if let Some(sps) = h264::sequence(&unescape(body, PARAMETER_BYTES)) {
                        self.sps.insert(sps.id, sps);
                    }
                }
                8 => {
                    let bytes = unescape(body, PARAMETER_BYTES);
                    let mut reader = Reader::new(&bytes);
                    let id = reader.ue();
                    let sequence = reader.ue();
                    if let Some(pps) = h264::picture(&bytes, self.sps.get(&sequence)) {
                        self.pps
                            .insert(id, (sequence, pps.bottom_field_order_present));
                    }
                }
                // 最初の VCL NAL ユニットだけを読む（二つ目以降を読むと状態を二度進める）。
                1..=5 if !found => {
                    found = true;
                    key = self.slice(&unescape(body, HEADER_BYTES), kind == 5, idc);
                }
                _ => {}
            }
        }
        key
    }

    fn slice(&mut self, data: &[u8], idr: bool, idc: u32) -> Key {
        let mut reader = Reader::new(data);
        reader.ue(); // first_mb_in_slice
        reader.ue(); // slice_type
        let pps_id = reader.ue();
        let Some(&(sps_id, bottom_present)) = self.pps.get(&pps_id) else {
            return Key::default();
        };
        let Some(sps) = self.sps.get(&sps_id).copied() else {
            return Key::default();
        };
        if sps.separate_planes {
            reader.bits(2); // colour_plane_id
        }
        let frame_num = reader.bits(sps.log2_max_frame_num as usize);
        let mut field = false;
        if !sps.frame_mbs_only {
            field = reader.bit() == 1;
            if field {
                reader.bit(); // bottom_field_flag
            }
        }
        if idr {
            reader.ue(); // idr_pic_id
        }
        let order = self.order_count(
            &sps,
            &mut reader,
            idr,
            idc,
            frame_num,
            field,
            bottom_present,
        );
        Key {
            order,
            // IDR で POC が 0 に戻るので、前のシーケンスのピクチャーを先に全部出す。
            drain_before: idr,
            bidirectional: false,
            depth: depth(&sps),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn order_count(
        &mut self,
        sps: &h264::Sps,
        reader: &mut Reader<'_>,
        idr: bool,
        idc: u32,
        frame_num: u32,
        field: bool,
        bottom_present: bool,
    ) -> i32 {
        let max_frame = 1i32 << sps.log2_max_frame_num;
        match sps.poc_type {
            0 => {
                let max = 1i32 << sps.log2_max_poc_lsb;
                let lsb = reader.bits(sps.log2_max_poc_lsb as usize) as i32;
                let bottom = if bottom_present && !field {
                    reader.se()
                } else {
                    0
                };
                let (previous_msb, previous_lsb) = if idr {
                    (0, 0)
                } else {
                    (self.previous_msb, self.previous_lsb)
                };
                let msb = if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
                    previous_msb + max
                } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
                    previous_msb - max
                } else {
                    previous_msb
                };
                if idc != 0 {
                    self.previous_msb = msb;
                    self.previous_lsb = lsb;
                }
                let top = msb + lsb;
                if field { top } else { top.min(top + bottom) }
            }
            1 => {
                let offset = if idr {
                    0
                } else if self.previous_frame_num > frame_num {
                    self.previous_offset + max_frame
                } else {
                    self.previous_offset
                };
                self.previous_offset = offset;
                self.previous_frame_num = frame_num;
                let mut delta = [0i32; 2];
                if !sps.poc_always_zero {
                    delta[0] = reader.se();
                    if bottom_present && !field {
                        delta[1] = reader.se();
                    }
                }
                let length = sps.cycle_length as i32;
                let mut absolute = if length != 0 {
                    offset + frame_num as i32
                } else {
                    0
                };
                if idc == 0 && absolute > 0 {
                    absolute -= 1;
                }
                let mut expected = 0;
                if absolute > 0 && length > 0 {
                    let cycle = (absolute - 1) / length;
                    let index = (absolute - 1) % length;
                    let sum: i32 = sps.cycle[..length as usize].iter().sum();
                    expected = cycle * sum;
                    for step in 0..=index as usize {
                        expected += sps.cycle[step];
                    }
                }
                if idc == 0 {
                    expected += sps.offset_non_ref;
                }
                expected + delta[0]
            }
            _ => {
                let offset = if idr {
                    0
                } else if self.previous_frame_num > frame_num {
                    self.previous_offset + max_frame
                } else {
                    self.previous_offset
                };
                self.previous_offset = offset;
                self.previous_frame_num = frame_num;
                if idr {
                    0
                } else if idc == 0 {
                    2 * (offset + frame_num as i32) - 1
                } else {
                    2 * (offset + frame_num as i32)
                }
            }
        }
    }
}

/// 何枚までバッファするか：[`dpb_frames`] を VUI の `max_num_reorder_frames` で上書きしたもの。
fn depth(sps: &h264::Sps) -> usize {
    let capacity = dpb_frames(sps);
    sps.max_reorder
        .map_or(capacity, |count| count as usize)
        .min(capacity)
}

/// HEVC の POC（節 8.3.1）。RASL・RADL と上位の時間階層は状態を進めない。
#[derive(Debug, Default)]
pub struct Hevc {
    sps: std::collections::BTreeMap<u32, Box<hevc::Sps>>,
    pps: std::collections::BTreeMap<u32, PpsBits>,
    previous_msb: i32,
    previous_lsb: i32,
    started: bool,
}

#[derive(Debug, Clone, Copy)]
struct PpsBits {
    sps_id: u32,
    output_flag_present: bool,
    extra_header_bits: usize,
}

impl Hevc {
    fn key(&mut self, framing: Framing, data: &[u8]) -> Key {
        let mut key = Key::default();
        let mut found = false;
        for nal in Nals::new(data, framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let body = data.get(nal.body.clone()).unwrap_or(&[]);
            let payload = body.get(1..).unwrap_or(&[]);
            match kind {
                hevc::SPS_NUT => {
                    if let Ok(sps) = hevc::sequence(&unescape(payload, PARAMETER_BYTES)) {
                        self.sps.insert(sps.id, Box::new(sps));
                    }
                }
                hevc::PPS_NUT => {
                    let bytes = unescape(payload, PARAMETER_BYTES);
                    let sps = {
                        let mut reader = Reader::new(&bytes);
                        reader.ue();
                        let id = reader.ue();
                        self.sps.get(&id).map(|sps| &**sps)
                    };
                    if let Ok(pps) = hevc::picture(&bytes, sps) {
                        self.pps.insert(
                            pps.id,
                            PpsBits {
                                sps_id: pps.sps_id,
                                output_flag_present: pps.output_flag_present,
                                extra_header_bits: pps.num_extra_slice_header_bits,
                            },
                        );
                    }
                }
                kind if hevc::is_slice(kind) && !found => {
                    found = true;
                    key = self.slice(&unescape(payload, HEADER_BYTES), kind);
                }
                _ => {}
            }
        }
        key
    }

    fn slice(&mut self, data: &[u8], kind: u8) -> Key {
        let mut reader = Reader::new(data);
        let first = reader.bit() == 1;
        if !first {
            return Key::default();
        }
        if hevc::is_irap(kind) {
            reader.bit(); // no_output_of_prior_pics_flag
        }
        let pps_id = reader.ue();
        let Some(&pps) = self.pps.get(&pps_id) else {
            return Key::default();
        };
        let Some(sps) = self.sps.get(&pps.sps_id) else {
            return Key::default();
        };
        for _ in 0..pps.extra_header_bits {
            reader.bit();
        }
        reader.ue(); // slice_type
        if pps.output_flag_present {
            reader.bit(); // pic_output_flag
        }
        let no_rasl = hevc::is_idr(kind) || hevc::is_bla(kind) || !self.started;
        if hevc::is_irap(kind) {
            self.started = true;
        }
        let depth = sps.max_num_reorder.min(sps.max_dec_pic_buffering).max(1);
        if hevc::is_idr(kind) {
            self.previous_lsb = 0;
            self.previous_msb = 0;
            return Key {
                order: 0,
                drain_before: true,
                bidirectional: false,
                depth,
            };
        }
        let max = 1i32 << sps.log2_max_poc_lsb;
        let lsb = reader.bits(sps.log2_max_poc_lsb as usize) as i32;
        let msb = if hevc::is_irap(kind) && no_rasl {
            0
        } else if lsb < self.previous_lsb && self.previous_lsb - lsb >= max / 2 {
            self.previous_msb + max
        } else if lsb > self.previous_lsb && lsb - self.previous_lsb > max / 2 {
            self.previous_msb - max
        } else {
            self.previous_msb
        };
        // 時間階層の識別子は読んでいない（すべて階層 0 として扱う）。
        if !hevc::is_rasl(kind) && !hevc::is_radl(kind) && hevc::is_reference(kind) {
            self.previous_lsb = lsb;
            self.previous_msb = msb;
        }
        Key {
            order: msb + lsb,
            drain_before: hevc::is_irap(kind) && no_rasl,
            bidirectional: false,
            depth,
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    not(all(feature = "videotoolbox", target_vendor = "apple")),
    allow(dead_code)
)]
pub struct Queue<T> {
    mpeg2: bool,
    held: Vec<(i32, T)>,
    pending: Option<T>,
    ready: std::collections::VecDeque<T>,
    depth: usize,
}

#[cfg_attr(
    not(all(feature = "videotoolbox", target_vendor = "apple")),
    allow(dead_code)
)]
impl<T> Queue<T> {
    pub fn new(mpeg2: bool) -> Self {
        Self {
            mpeg2,
            held: Vec::new(),
            pending: None,
            ready: std::collections::VecDeque::new(),
            depth: 1,
        }
    }

    pub fn push(&mut self, key: Key, item: T) {
        if self.mpeg2 {
            if key.bidirectional {
                // 双方向予測のピクチャーは、後方の参照がデコード済みなのでそのまま出せる。
                self.ready.push_back(item);
            } else {
                if let Some(previous) = self.pending.take() {
                    self.ready.push_back(previous);
                }
                self.pending = Some(item);
            }
            return;
        }
        if key.drain_before {
            self.drain();
        }
        self.depth = key.depth.max(1);
        self.held.push((key.order, item));
        while self.held.len() > self.depth {
            self.pop_smallest();
        }
    }

    pub fn drain(&mut self) {
        if let Some(previous) = self.pending.take() {
            self.ready.push_back(previous);
        }
        while !self.held.is_empty() {
            self.pop_smallest();
        }
    }

    fn pop_smallest(&mut self) {
        let Some(at) = self
            .held
            .iter()
            .enumerate()
            .min_by_key(|(_, (order, _))| *order)
            .map(|(at, _)| at)
        else {
            return;
        };
        let (_, item) = self.held.remove(at);
        self.ready.push_back(item);
    }

    pub fn take(&mut self) -> Option<T> {
        self.ready.pop_front()
    }

    pub fn clear(&mut self) {
        self.held.clear();
        self.pending = None;
        self.ready.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg2_shows_a_reference_when_the_next_one_arrives() {
        let mut queue = Queue::new(true);
        let reference = Key {
            bidirectional: false,
            ..Key::default()
        };
        let between = Key {
            bidirectional: true,
            ..Key::default()
        };
        queue.push(reference, "I");
        assert_eq!(queue.take(), None, "参照は次の参照まで待つ");
        queue.push(between, "B1");
        queue.push(between, "B2");
        assert_eq!(queue.take(), Some("B1"));
        assert_eq!(queue.take(), Some("B2"));
        queue.push(reference, "P");
        assert_eq!(queue.take(), Some("I"));
        queue.drain();
        assert_eq!(queue.take(), Some("P"));
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn a_reordering_queue_puts_them_out_by_order_count() {
        let mut queue = Queue::new(false);
        let key = |order| Key {
            order,
            depth: 2,
            ..Key::default()
        };
        for (order, name) in [(0, "a"), (4, "e"), (2, "c"), (1, "b"), (3, "d")] {
            queue.push(key(order), name);
        }
        queue.drain();
        let mut out = Vec::new();
        while let Some(item) = queue.take() {
            out.push(item);
        }
        assert_eq!(out, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn a_new_sequence_shows_what_came_before_it_first() {
        let mut queue = Queue::new(false);
        queue.push(
            Key {
                order: 8,
                depth: 4,
                ..Key::default()
            },
            "old",
        );
        queue.push(
            Key {
                order: 0,
                depth: 4,
                drain_before: true,
                ..Key::default()
            },
            "idr",
        );
        queue.drain();
        assert_eq!(queue.take(), Some("old"));
        assert_eq!(queue.take(), Some("idr"));
    }
}
