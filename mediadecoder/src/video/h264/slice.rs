// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The macroblock layer: clause 7.3.5 read and clause 8 carried out, for both entropy coders.
//! Deblocking runs over the whole picture afterwards: intra prediction needs unfiltered samples.

use super::cabac::Cabac;
use super::inter::{self, Scratch, View, Weighting};
use super::intra::{self, Edge};
use super::mb::{Kind, Mb};
use super::params::{Header, Kind as SliceKind, Pps, Sps, Weights};
use super::tables::{self, BLOCK_TO_RASTER, RASTER_TO_BLOCK};
use super::transform::{self, Scale};
use super::vlc;
use crate::video::Error;
use crate::video::bits::Reader;

mod macroblock;
mod motion;
mod read;
mod residual;
mod samples;

#[derive(Debug, Clone, Copy)]
pub struct Reference {
    /// Nothing where it is the picture being decoded (second field predicting from the first).
    pub buffer: Option<usize>,
    pub parity: Option<usize>,
    pub id: i32,
    pub poc: i32,
    pub long_term: bool,
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            buffer: None,
            parity: None,
            id: -1,
            poc: 0,
            long_term: false,
        }
    }
}

/// A field writes every other row (twice the stride, one row down for bottom).
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub stride: [usize; 3],
    pub at: [usize; 3],
    pub row: [usize; 3],
    pub width: [usize; 3],
    pub height: [usize; 3],
    pub mb_width: usize,
    pub mb_height: usize,
    pub field: bool,
    pub bottom: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Colocated<'a> {
    pub mbs: Option<&'a [Mb]>,
    pub poc: i32,
}

enum Entropy<'a> {
    Bits(Reader<'a>),
    Arithmetic(Box<Cabac<'a>>),
}

#[derive(Debug, Clone, Copy)]
struct Current {
    kind: Kind,
    intra16_mode: usize,
    chroma_mode: usize,
    transform8x8: bool,
    cbp_luma: u8,
    cbp_chroma: u8,
    pieces: [Piece; 16],
    pieces_at: usize,
    parts: usize,
    across: bool,
    direct: bool,
    ref0: bool,
    qp: i32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Piece {
    x: usize,
    y: usize,
    wide: usize,
    tall: usize,
    lists: u8,
    direct: bool,
    part: usize,
}

impl Default for Current {
    fn default() -> Self {
        Self {
            kind: Kind::Missing,
            intra16_mode: 0,
            chroma_mode: 0,
            transform8x8: false,
            cbp_luma: 0,
            cbp_chroma: 0,
            pieces: [Piece::default(); 16],
            pieces_at: 0,
            parts: 1,
            across: false,
            direct: false,
            ref0: false,
            qp: 0,
        }
    }
}

impl Current {
    fn pieces(&self) -> &[Piece] {
        &self.pieces[..self.pieces_at]
    }

    fn push(&mut self, piece: Piece) {
        if self.pieces_at < 16 {
            self.pieces[self.pieces_at] = piece;
            self.pieces_at += 1;
        }
    }
}

/// Table 7-14: `(partitions, side by side, lists of first, lists of second)`.
/// Types 0 (direct) and 22 (quarters) are not here.
static B_PARTS: [(u8, bool, u8, u8); 21] = [
    (1, false, 1, 0),
    (1, false, 2, 0),
    (1, false, 3, 0),
    (2, false, 1, 1),
    (2, true, 1, 1),
    (2, false, 2, 2),
    (2, true, 2, 2),
    (2, false, 1, 2),
    (2, true, 1, 2),
    (2, false, 2, 1),
    (2, true, 2, 1),
    (2, false, 1, 3),
    (2, true, 1, 3),
    (2, false, 2, 3),
    (2, true, 2, 3),
    (2, false, 3, 1),
    (2, true, 3, 1),
    (2, false, 3, 2),
    (2, true, 3, 2),
    (2, false, 3, 3),
    (2, true, 3, 3),
];

/// Table 7-18: `(pieces across, pieces down, lists)`.
static B_SUB: [(usize, usize, u8); 13] = [
    (2, 2, 0),
    (1, 1, 1),
    (1, 1, 2),
    (1, 1, 3),
    (1, 2, 1),
    (2, 1, 1),
    (1, 2, 2),
    (2, 1, 2),
    (1, 2, 3),
    (2, 1, 3),
    (2, 2, 1),
    (2, 2, 2),
    (2, 2, 3),
];

static P_SUB: [(usize, usize); 4] = [(1, 1), (1, 2), (2, 1), (2, 2)];

const CHROMA_BLOCK: usize = 16;

struct Work {
    scratch: Scratch,
    luma: [[u8; 256]; 2],
    chroma: [[[u8; 64]; 2]; 2],
}

impl Default for Work {
    fn default() -> Self {
        Self {
            scratch: Scratch::default(),
            luma: [[0; 256]; 2],
            chroma: [[[0; 64]; 2]; 2],
        }
    }
}

struct Residual {
    luma: [[i32; 16]; 16],
    luma8: [[i32; 64]; 4],
    dc: [i32; 16],
    chroma_dc: [[i32; 4]; 2],
    chroma: [[[i32; 16]; 4]; 2],
}

impl Default for Residual {
    fn default() -> Self {
        Self {
            luma: [[0; 16]; 16],
            luma8: [[0; 64]; 4],
            dc: [0; 16],
            chroma_dc: [[0; 4]; 2],
            chroma: [[[0; 16]; 4]; 2],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cat {
    LumaDc = 0,
    LumaAc = 1,
    Luma = 2,
    ChromaDc = 3,
    ChromaAc = 4,
    Luma8x8 = 5,
}

/// Table 9-40.
static CODED_BLOCK_OFFSET: [usize; 5] = [0, 4, 8, 12, 16];
static SIGNIFICANT_OFFSET: [usize; 5] = [0, 15, 29, 44, 47];
static LEVEL_OFFSET: [usize; 5] = [0, 10, 20, 30, 39];

pub struct Coder<'a> {
    header: &'a Header,
    sps: &'a Sps,
    pps: &'a Pps,
    geometry: Geometry,
    planes: &'a mut [Vec<u8>; 3],
    others: &'a [[Vec<u8>; 3]],
    mbs: &'a mut [Mb],
    mvd: &'a mut [[[[i16; 2]; 16]; 2]],
    refs: &'a [Vec<Reference>; 2],
    colocated: Colocated<'a>,
    poc: i32,

    entropy: Entropy<'a>,
    levels: [Scale; 6],
    qp: i32,
    /// The one CABAC context carried between macroblocks.
    last_delta: bool,
    slice: u16,
    implicit: Vec<Vec<i32>>,
    /// Boxed (thirty kilobytes); taken out while used so the pictures can be borrowed too.
    work: Option<Box<Work>>,
}

impl<'a> Coder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        header: &'a Header,
        sps: &'a Sps,
        pps: &'a Pps,
        weights: &Weights,
        geometry: Geometry,
        planes: &'a mut [Vec<u8>; 3],
        others: &'a [[Vec<u8>; 3]],
        mbs: &'a mut [Mb],
        mvd: &'a mut [[[[i16; 2]; 16]; 2]],
        refs: &'a [Vec<Reference>; 2],
        colocated: Colocated<'a>,
        poc: i32,
        slice: u16,
        data: &'a [u8],
    ) -> Self {
        let qp = header.qp.clamp(0, 51);
        let entropy = if pps.cabac {
            let mut reader = Reader::new(data);
            reader.skip(header.bits as u32);
            reader.align();
            let model = if header.kind.intra() {
                0
            } else {
                header.cabac_init_idc as usize + 1
            };
            Entropy::Arithmetic(Box::new(Cabac::new(reader.rest(), model, qp)))
        } else {
            let mut reader = Reader::new(data);
            reader.skip(header.bits as u32);
            Entropy::Bits(reader)
        };
        let implicit = implicit_weights(header, refs, poc);
        Self {
            header,
            sps,
            pps,
            geometry,
            planes,
            others,
            mbs,
            mvd,
            refs,
            colocated,
            poc,
            entropy,
            // The 4x4 lists run Y, Cb, Cr intra then inter; the 8x8 lists are luma only, one each way.
            levels: std::array::from_fn(|list| Scale::new(weights, list, usize::from(list >= 3))),
            qp,
            last_delta: false,
            slice,
            implicit,
            work: Some(Box::default()),
        }
    }

    fn ue(&mut self) -> u32 {
        match &mut self.entropy {
            Entropy::Bits(reader) => reader.ue(),
            Entropy::Arithmetic(_) => 0,
        }
    }

    fn se(&mut self) -> i32 {
        match &mut self.entropy {
            Entropy::Bits(reader) => reader.se(),
            Entropy::Arithmetic(_) => 0,
        }
    }

    fn bit(&mut self) -> u32 {
        match &mut self.entropy {
            Entropy::Bits(reader) => reader.bit(),
            Entropy::Arithmetic(cabac) => cabac.bypass(),
        }
    }

    fn decision(&mut self, context: usize) -> u32 {
        match &mut self.entropy {
            Entropy::Bits(_) => 0,
            Entropy::Arithmetic(cabac) => cabac.decision(context),
        }
    }

    fn overrun(&self) -> bool {
        match &self.entropy {
            Entropy::Bits(reader) => reader.overrun(),
            Entropy::Arithmetic(cabac) => cabac.overrun(),
        }
    }

    fn cabac(&self) -> bool {
        matches!(self.entropy, Entropy::Arithmetic(_))
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let total = self.geometry.mb_width * self.geometry.mb_height;
        let mut at = self.header.first_mb;
        if at >= total {
            return Err(Error::Damaged("a slice that begins past the picture"));
        }
        let mut more = true;
        while more && at < total {
            let mut coded = true;
            if !self.header.kind.intra() {
                if self.cabac() {
                    let base = if self.header.kind == SliceKind::B {
                        24
                    } else {
                        11
                    };
                    let context = base + self.skip_context(at);
                    coded = self.decision(context) == 0;
                } else {
                    let run = self.ue();
                    for _ in 0..run {
                        if at >= total {
                            break;
                        }
                        self.skipped_macroblock(at);
                        at += 1;
                    }
                    if at >= total {
                        break;
                    }
                    if run > 0 {
                        more = self.more_data();
                        if !more {
                            break;
                        }
                    }
                }
            }
            if coded {
                self.macroblock(at);
            } else {
                self.skipped_macroblock(at);
            }
            if self.overrun() {
                return Err(Error::Damaged("a slice that ran out of bits"));
            }
            at += 1;
            more = self.more_data();
        }
        Ok(())
    }

    fn more_data(&mut self) -> bool {
        match &mut self.entropy {
            Entropy::Bits(reader) => !reader.overrun() && reader.more_data(),
            Entropy::Arithmetic(cabac) => !cabac.terminate(),
        }
    }

    /// Clause 9.3.3.1.1.1.
    fn skip_context(&self, at: usize) -> usize {
        let mut context = 0;
        for (dx, dy) in [(-1i32, 0i32), (0, -1)] {
            if let Some(other) = self.neighbour_mb(at, dx, dy) {
                let mb = &self.mbs[other];
                if mb.coded() && !mb.skipped {
                    context += 1;
                }
            }
        }
        context
    }

    fn neighbour_mb(&self, at: usize, dx: i32, dy: i32) -> Option<usize> {
        let width = self.geometry.mb_width;
        let x = (at % width) as i32 + dx;
        let y = (at / width) as i32 + dy;
        if x < 0 || y < 0 || x >= width as i32 || y >= self.geometry.mb_height as i32 {
            return None;
        }
        let other = y as usize * width + x as usize;
        let mb = &self.mbs[other];
        if !mb.coded() || mb.slice != self.slice {
            return None;
        }
        Some(other)
    }

    /// `(macroblock, raster block)`; `x`, `y` may be -1, or 4 for above-right.
    fn neighbour_block(&self, at: usize, x: i32, y: i32) -> Option<(usize, usize)> {
        if y < 0 {
            let (dx, bx) = if x < 0 {
                (-1, 3)
            } else if x < 4 {
                (0, x)
            } else {
                (1, x - 4)
            };
            let other = self.neighbour_mb(at, dx, -1)?;
            return Some((other, 12 + bx as usize));
        }
        if x < 0 {
            let other = self.neighbour_mb(at, -1, 0)?;
            return Some((other, y as usize * 4 + 3));
        }
        if x >= 4 {
            return None;
        }
        Some((at, y as usize * 4 + x as usize))
    }

    pub(super) fn decoded_block(
        &self,
        at: usize,
        x: i32,
        y: i32,
        current: usize,
    ) -> Option<(usize, usize)> {
        let (other, block) = self.neighbour_block(at, x, y)?;
        if other == at && RASTER_TO_BLOCK[block] >= RASTER_TO_BLOCK[current] {
            return None;
        }
        Some((other, block))
    }
}

/// Clause 8.4.2.3.1: implicit weights.
pub(super) fn implicit_weights(
    header: &Header,
    refs: &[Vec<Reference>; 2],
    poc: i32,
) -> Vec<Vec<i32>> {
    if header.kind != SliceKind::B {
        return Vec::new();
    }
    refs[0]
        .iter()
        .map(|zero| {
            refs[1]
                .iter()
                .map(|one| {
                    if zero.long_term || one.long_term || one.poc == zero.poc {
                        return 32;
                    }
                    let td = (one.poc - zero.poc).clamp(-128, 127);
                    let tb = (poc - zero.poc).clamp(-128, 127);
                    if td == 0 {
                        return 32;
                    }
                    let tx = (16384 + (td / 2).abs()) / td;
                    let scale = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                    let weight = scale >> 2;
                    if !(-64..=128).contains(&weight) {
                        32
                    } else {
                        weight
                    }
                })
                .collect()
        })
        .collect()
}
