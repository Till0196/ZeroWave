// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HEVC で VideoToolbox に渡すアクセスユニットを選ぶ。VideoToolbox は DPB を自分で持つので、手元に無い
//! 参照を使うピクチャー（途中から入ったときに前の GOP を参照するもの、受信で欠けた参照を使うもの）を渡すと
//! `kVTVideoDecoderBadDataErr` を返す。参照の有無だけを共通の DPB（[`Dpb`]）で追い、そういうピクチャーと、
//! 渡さなかったピクチャーを参照するピクチャーは渡さない。

use mediadecoder::bits::{Reader, unescape};
use mediadecoder::es::{Framing, Nals};
use mediadecoder::video::hevc::dpb::{Dpb, Frames};
use mediadecoder::video::hevc::params;

#[derive(Debug)]
pub struct Gate {
    sets: params::Sets,
    dpb: Dpb<u64, ()>,
    next: u64,
}

/// 参照の有無を数えるだけなので、名前は番号、出力はしない。
struct Names<'a> {
    next: &'a mut u64,
}

impl Frames for Names<'_> {
    type Id = u64;
    type Held = ();

    fn take(&mut self, _live: &[u64]) -> Option<u64> {
        *self.next += 1;
        Some(*self.next)
    }

    fn output(&mut self, _id: u64, _held: &()) {}
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            sets: params::Sets::default(),
            dpb: Dpb::new(0, 16),
            next: 0,
        }
    }
}

impl Gate {
    /// このアクセスユニットを渡してよいか。読めないアクセスユニットは渡す（判断は VideoToolbox に任せる）。
    pub fn admits(&mut self, framing: Framing, data: &[u8]) -> bool {
        for nal in Nals::new(data, framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let second = data.get(nal.body.start).copied().unwrap_or(0);
            let temporal = (second & 7).saturating_sub(1);
            if (nal.header & 1) != 0 || (second >> 3) != 0 {
                continue;
            }
            let body = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            match kind {
                params::SPS_NUT | params::PPS_NUT => {
                    let _ = self.sets.absorb(kind, body);
                }
                params::EOS_NUT | params::EOB_NUT => self.dpb.end_of_sequence(),
                kind if params::is_slice(kind) => {
                    return self.picture(kind, temporal, body);
                }
                _ => {}
            }
        }
        true
    }

    fn picture(&mut self, kind: u8, temporal: u8, body: &[u8]) -> bool {
        let payload = unescape(body, body.len());
        let mut reader = Reader::new(&payload);
        if reader.bit() != 1 {
            // 先頭のスライスセグメントの無いピクチャーは読めない。
            return true;
        }
        if params::is_irap(kind) {
            reader.bit();
        }
        let Some((pps, sps)) = self.sets.find(reader.ue()) else {
            return true;
        };
        let Ok(header) = params::header(&payload, kind, &sps, &pps, None) else {
            return true;
        };
        self.dpb
            .limits(sps.max_num_reorder, sps.max_dec_pic_buffering);
        let mut names = Names {
            next: &mut self.next,
        };
        let Some(poc) = self.dpb.begin(&mut names, &sps, &header, kind, temporal) else {
            // 捨てる RASL ピクチャー。
            return false;
        };
        if self
            .dpb
            .sets
            .iter()
            .flatten()
            .any(|entry| entry.id.is_none())
        {
            return false;
        }
        let id = {
            *names.next += 1;
            *names.next
        };
        self.dpb.push(&mut names, id, poc, false, ());
        true
    }

    pub fn reset(&mut self) {
        let mut names = Names {
            next: &mut self.next,
        };
        self.dpb.reset(&mut names);
    }
}
