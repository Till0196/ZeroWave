// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The HEVC software decoder: Main and Main 10, 4:2:0, progressive, as Japanese 4K/8K over MMT use.
//! Decoding starts at the first IRAP (broadcast uses CRA, not IDR) and drops its leading
//! pictures. Range/SCC extensions are refused at the PPS; layers above the base are skipped.

#[cfg(feature = "hevc-decoder")]
mod cabac;
#[cfg(feature = "hevc-decoder")]
mod contexts;
#[cfg(feature = "hevc-decoder")]
mod deblock;
pub mod dpb;
#[cfg(feature = "hevc-decoder")]
mod inter;
#[cfg(feature = "hevc-decoder")]
mod intra;
#[cfg(feature = "hevc-decoder")]
mod mv;
pub mod params;
#[cfg(feature = "hevc-decoder")]
mod sao;
pub mod scaling;
#[cfg(feature = "hevc-decoder")]
mod slice;
#[cfg_attr(not(feature = "hevc-decoder"), allow(dead_code))]
mod tables;
#[cfg(test)]
#[cfg(feature = "hevc-decoder")]
mod tests;
#[cfg(feature = "hevc-decoder")]
mod transform;

#[cfg(feature = "hevc-decoder")]
use std::collections::VecDeque;

#[cfg(feature = "hevc-decoder")]
use crate::bits::unescape;
#[cfg(feature = "hevc-decoder")]
use crate::es::{Format, Framing, Nals, Picture};
#[cfg(feature = "hevc-decoder")]
use crate::video::{Decoder, Error, Frame, Plane};

#[cfg(feature = "hevc-decoder")]
use dpb::{Dpb, Frames};
#[cfg(feature = "hevc-decoder")]
use mv::{Col, Unit};
#[cfg(feature = "hevc-decoder")]
use params::{Header, Pps, Sps};

/// What the DPB keeps of a picture until it is shown; the samples stay in the pool.
#[derive(Debug, Clone, Copy)]
#[cfg(feature = "hevc-decoder")]
struct Held {
    tag: u64,
    picture: Picture,
    format: Format,
    /// Where the shown picture begins inside the coded one (conformance window).
    origin: (usize, usize),
}

/// The frame buffers for [`dpb::Dpb`], borrowed field by field since the DPB lives in [`Hevc`].
#[cfg(feature = "hevc-decoder")]
struct Pool<'a> {
    planes: &'a [[Vec<u16>; 3]],
    stride: [usize; 3],
    ready: &'a mut VecDeque<Frame>,
}

#[cfg(feature = "hevc-decoder")]
macro_rules! pool {
    ($decoder:ident) => {
        Pool {
            planes: $decoder.planes.as_slice(),
            stride: $decoder.stride,
            ready: &mut $decoder.ready,
        }
    };
}

#[cfg(feature = "hevc-decoder")]
impl Frames for Pool<'_> {
    type Id = usize;
    type Held = Held;

    fn take(&mut self, live: &[usize]) -> Option<usize> {
        (0..self.planes.len()).find(|at| !live.contains(at))
    }

    fn output(&mut self, id: usize, held: &Held) {
        let Held {
            tag,
            picture,
            format,
            origin,
        } = *held;
        let buffer = &self.planes[id];
        if buffer[0].is_empty() {
            return;
        }
        let wide = format.depth > 8;
        let planes = std::array::from_fn(|plane| {
            let divisor = if plane == 0 { 1 } else { 2 };
            let width = (format.width as usize).div_ceil(divisor);
            let height = (format.height as usize).div_ceil(divisor);
            let stride = if wide { width * 2 } else { width };
            let mut data = vec![0u8; stride * height];
            let (left, top) = (origin.0 / divisor, origin.1 / divisor);
            for y in 0..height {
                // A window that does not fit: show black rather than read past it.
                let at = (top + y) * self.stride[plane] + left;
                let Some(from) = buffer[plane].get(at..at + width) else {
                    break;
                };
                let into = &mut data[y * stride..y * stride + stride];
                if wide {
                    for (x, pair) in into.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                        *pair = from[x].to_le_bytes();
                    }
                } else {
                    for (x, byte) in into.iter_mut().enumerate() {
                        *byte = from[x] as u8;
                    }
                }
            }
            Plane {
                data,
                stride,
                width,
                height,
            }
        });
        self.ready.push_back(Frame {
            tag,
            format,
            planes,
            structure: picture.structure,
            top_field_first: picture.top_field_first,
            repeat_first_field: picture.repeat_first_field,
            progressive: picture.progressive,
        });
    }
}

#[derive(Debug, Clone)]
#[cfg(feature = "hevc-decoder")]
struct Current {
    buffer: usize,
    poc: i32,
    tag: u64,
    picture: Picture,
    format: Format,
    /// The first slice segment's header, which later ones and dependent segments inherit.
    header: Box<Header>,
    output: bool,
    origin: (usize, usize),
    sps_id: u32,
    pps_id: u32,
}

#[derive(Debug)]
#[cfg(feature = "hevc-decoder")]
pub struct Hevc {
    /// Length-prefixed (MMT) or start codes (TS); not sniffed, a length can look like a start code.
    framing: Framing,
    format: Format,
    sets: params::Sets,
    /// Samples as u16 whatever the depth; converted once on output.
    planes: Vec<[Vec<u16>; 3]>,
    cols: Vec<Vec<Col>>,
    target: [Vec<u16>; 3],
    /// SAO reads from here while writing the picture, so a changed sample is not compared against.
    scratch: [Vec<u16>; 3],
    target_col: Vec<Col>,
    units: Vec<Unit>,
    depths: Vec<u8>,
    skips: Vec<bool>,
    offsets: Vec<[sao::Offsets; 3]>,
    ctb_slice_addr: Vec<u32>,
    ctb_slice_index: Vec<u16>,
    rs_to_ts: Vec<u32>,
    ts_to_rs: Vec<u32>,
    tile: Vec<u16>,
    slices: Vec<deblock::Slice>,
    across_slices: Vec<bool>,
    buffers: slice::Buffers,
    wavefront: Option<[u8; contexts::CONTEXTS]>,
    /// QP of the last CU; the next quantisation group and dependent segments predict from it.
    qp_carry: i32,

    dpb: Dpb<usize, Held>,
    current: Option<Current>,
    skipping: bool,
    stride: [usize; 3],
    height: [usize; 3],
    ready: VecDeque<Frame>,
}

#[cfg(feature = "hevc-decoder")]
impl Default for Hevc {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "hevc-decoder")]
impl Hevc {
    pub fn new() -> Self {
        Self::with_framing(Framing::Lengths)
    }

    pub fn with_framing(framing: Framing) -> Self {
        Self {
            framing,
            format: Format::default(),
            sets: params::Sets::default(),
            planes: Vec::new(),
            cols: Vec::new(),
            target: [Vec::new(), Vec::new(), Vec::new()],
            scratch: [Vec::new(), Vec::new(), Vec::new()],
            target_col: Vec::new(),
            units: Vec::new(),
            depths: Vec::new(),
            skips: Vec::new(),
            offsets: Vec::new(),
            ctb_slice_addr: Vec::new(),
            ctb_slice_index: Vec::new(),
            rs_to_ts: Vec::new(),
            ts_to_rs: Vec::new(),
            tile: Vec::new(),
            slices: Vec::new(),
            across_slices: Vec::new(),
            buffers: slice::Buffers::default(),
            wavefront: None,
            qp_carry: 0,
            dpb: Dpb::new(0, 0),
            current: None,
            skipping: false,
            stride: [0; 3],
            height: [0; 3],
            ready: VecDeque::new(),
        }
    }

    /// Takes up an SPS's sizes and drops everything held (a channel change).
    fn reconfigure(&mut self, sps: &Sps, format: Format) {
        self.format = format;
        let size = 1usize << sps.ctb_log2;
        let width = sps.ctb_width() * size;
        let height = sps.ctb_height() * size;
        self.stride = [width, width / 2, width / 2];
        self.height = [height, height / 2, height / 2];
        let frames = sps.max_dec_pic_buffering + 1;
        self.dpb
            .limits(sps.max_num_reorder, sps.max_dec_pic_buffering);
        let make = |stride: [usize; 3], height: [usize; 3]| -> [Vec<u16>; 3] {
            std::array::from_fn(|plane| vec![0u16; stride[plane] * height[plane]])
        };
        self.planes = (0..frames)
            .map(|_| make(self.stride, self.height))
            .collect();
        let col = sps.width.div_ceil(16) * sps.height.div_ceil(16);
        self.cols = (0..frames).map(|_| vec![Col::default(); col]).collect();
        self.target = make(self.stride, self.height);
        self.scratch = make(self.stride, self.height);
        self.target_col = vec![Col::default(); col];
        let units = (sps.width / 4) * (sps.height / 4);
        self.units = vec![Unit::default(); units];
        self.depths = vec![0; units];
        self.skips = vec![false; units];
        self.offsets = vec![[sao::Offsets::default(); 3]; sps.ctbs()];
        self.ctb_slice_addr = vec![u32::MAX; sps.ctbs()];
        self.ctb_slice_index = vec![0; sps.ctbs()];
        self.dpb.stores.clear();
        self.current = None;
        self.ready.clear();
    }

    /// Clause 6.5.1: CTB raster/tile scan conversion.
    fn tiling(&mut self, sps: &Sps, pps: &Pps) {
        let ctb_width = sps.ctb_width();
        let ctb_height = sps.ctb_height();
        let count = ctb_width * ctb_height;
        self.rs_to_ts = vec![0; count];
        self.ts_to_rs = vec![0; count];
        self.tile = vec![0; count];
        let columns: Vec<usize> = if pps.column_width.is_empty() {
            vec![ctb_width]
        } else {
            pps.column_width.clone()
        };
        let rows: Vec<usize> = if pps.row_height.is_empty() {
            vec![ctb_height]
        } else {
            pps.row_height.clone()
        };
        let mut left = Vec::with_capacity(columns.len());
        let mut at = 0;
        for &width in &columns {
            left.push(at);
            at += width;
        }
        let mut top = Vec::with_capacity(rows.len());
        let mut at = 0;
        for &height in &rows {
            top.push(at);
            at += height;
        }
        let mut ts = 0usize;
        for (row, &y0) in top.iter().enumerate() {
            for (column, &x0) in left.iter().enumerate() {
                let id = (row * columns.len() + column) as u16;
                for y in y0..(y0 + rows[row]).min(ctb_height) {
                    for x in x0..(x0 + columns[column]).min(ctb_width) {
                        let rs = y * ctb_width + x;
                        self.rs_to_ts[rs] = ts as u32;
                        self.ts_to_rs[ts] = rs as u32;
                        self.tile[rs] = id;
                        ts += 1;
                    }
                }
            }
        }
    }
}

/// Emulation prevention byte positions; entry point offsets count them (7.4.7.1).
#[cfg(feature = "hevc-decoder")]
fn escapes(bytes: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut zeroes = 0;
    for (at, &byte) in bytes.iter().enumerate() {
        if zeroes == 2 && byte == 0x03 {
            out.push(at);
            zeroes = 0;
            continue;
        }
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
    }
    out
}

/// Emulation prevention bytes before the slice data (VA-API and V4L2 want the count).
pub fn escapes_before(bytes: &[u8], data_at: usize) -> usize {
    let mut zeroes = 0;
    let mut escapes = 0;
    let mut kept = 0;
    for &byte in bytes {
        if kept >= data_at {
            break;
        }
        if zeroes >= 2 && byte == 3 {
            escapes += 1;
            zeroes = 0;
            continue;
        }
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
        kept += 1;
    }
    escapes
}

/// Turns entry point offsets, which count the escapes, into offsets into the unescaped payload.
#[cfg(feature = "hevc-decoder")]
fn entry_points(escapes: &[usize], data_at: usize, offsets: &[usize]) -> Vec<usize> {
    let mut begin = data_at;
    for &at in escapes {
        if at <= begin {
            begin += 1;
        } else {
            break;
        }
    }
    offsets
        .iter()
        .map(|&offset| {
            let at = begin + offset;
            at - escapes.partition_point(|&escape| escape < at)
        })
        .collect()
}

#[cfg(feature = "hevc-decoder")]
impl Hevc {
    #[allow(clippy::too_many_arguments)]
    fn start_picture(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        header: &Header,
        sps: &Sps,
        pps: &Pps,
        kind: u8,
        poc: i32,
    ) -> Result<(), Error> {
        let Some(buffer) = self.dpb.free(&mut pool!(self)) else {
            return Err(Error::Damaged(
                "a picture buffer that cannot be made room in",
            ));
        };
        self.target = std::mem::take(&mut self.planes[buffer]);
        self.target_col = std::mem::take(&mut self.cols[buffer]);
        if self.target[0].is_empty() {
            self.target =
                std::array::from_fn(|plane| vec![0u16; self.stride[plane] * self.height[plane]]);
        }
        let col = sps.width.div_ceil(16) * sps.height.div_ceil(16);
        self.target_col.clear();
        self.target_col.resize(col, Col::default());
        self.units
            .iter_mut()
            .for_each(|unit| *unit = Unit::default());
        self.depths.fill(0);
        self.skips.fill(false);
        self.offsets.fill([sao::Offsets::default(); 3]);
        self.ctb_slice_addr.fill(u32::MAX);
        self.ctb_slice_index.fill(0);
        self.slices.clear();
        self.across_slices.clear();
        self.wavefront = None;
        self.tiling(sps, pps);
        self.current = Some(Current {
            buffer,
            poc,
            tag,
            picture: *picture,
            format,
            header: Box::new(header.clone()),
            sps_id: pps.sps_id,
            pps_id: pps.id,
            output: header.pic_output_flag && !(params::is_rasl(kind) && self.dpb.drop_rasl()),
            origin: (2 * sps.crop[0] as usize, 2 * sps.crop[2] as usize),
        });
        Ok(())
    }

    fn finish_picture(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        let sps = self
            .sets
            .sps
            .get(current.sps_id as usize)
            .cloned()
            .flatten();
        let pps = self
            .sets
            .pps
            .get(current.pps_id as usize)
            .cloned()
            .flatten();
        if let (Some(sps), Some(pps)) = (&sps, pps) {
            self.deblock(sps, &pps);
            self.sample_offsets(sps, &pps);
        }
        if let Some(sps) = sps {
            self.store_motion(&sps);
        }
        self.planes[current.buffer] = std::mem::take(&mut self.target);
        self.cols[current.buffer] = std::mem::take(&mut self.target_col);
        self.dpb.push(
            &mut pool!(self),
            current.buffer,
            current.poc,
            current.output,
            Held {
                tag: current.tag,
                picture: current.picture,
                format: current.format,
                origin: current.origin,
            },
        );
    }

    fn store_motion(&mut self, sps: &Sps) {
        let columns = sps.width.div_ceil(16);
        let unit_width = sps.width / 4;
        for (at, col) in self.target_col.iter_mut().enumerate() {
            let (bx, by) = (at % columns, at / columns);
            let unit = self.units[(by * 4) * unit_width + bx * 4];
            *col = Col::default();
            if unit.intra {
                continue;
            }
            col.flags = unit.flags;
            col.mv = unit.mv;
            col.poc = unit.poc;
            col.long = unit.long;
        }
    }

    fn deblock(&mut self, sps: &Sps, pps: &Pps) {
        if self.slices.iter().all(|slice| slice.disabled) {
            return;
        }
        let picture = deblock::Picture {
            units: &self.units,
            unit_width: sps.width / 4,
            width: sps.width,
            height: sps.height,
            depth: sps.depth,
            chroma_offset: pps.qp_offset,
            ctb_log2: sps.ctb_log2,
            ctb_width: sps.ctb_width(),
            ctb_slice: &self.ctb_slice_index,
            slices: &self.slices,
            pcm_loop_filter_disabled: sps.pcm_loop_filter_disabled,
        };
        let [luma, cb, cr] = &mut self.target;
        let mut planes = [
            deblock::Plane {
                samples: luma.as_mut_slice(),
                stride: self.stride[0],
            },
            deblock::Plane {
                samples: cb.as_mut_slice(),
                stride: self.stride[1],
            },
            deblock::Plane {
                samples: cr.as_mut_slice(),
                stride: self.stride[2],
            },
        ];
        deblock::filter(&picture, &mut planes);
    }

    fn sample_offsets(&mut self, sps: &Sps, pps: &Pps) {
        if !sps.sao_enabled
            || self
                .offsets
                .iter()
                .all(|ctb| ctb.iter().all(|plane| plane.kind == 0))
        {
            return;
        }
        for plane in 0..3 {
            self.scratch[plane].copy_from_slice(&self.target[plane]);
        }
        let ctb_width = sps.ctb_width();
        let ctb_height = sps.ctb_height();
        let size = 1usize << sps.ctb_log2;
        for cy in 0..ctb_height {
            for cx in 0..ctb_width {
                let rs = cy * ctb_width + cx;
                let reach = self.sao_reach(sps, pps, cx, cy);
                let untouchable = self.has_untouchable(sps, cx * size, cy * size, size);
                for plane in 0..3 {
                    let shift = usize::from(plane > 0);
                    let offsets = self.offsets[rs][plane];
                    if offsets.kind == 0 {
                        continue;
                    }
                    let unit_width = sps.width / 4;
                    let units = &self.units;
                    let pcm_off = sps.pcm_loop_filter_disabled;
                    let (source, into) = (&self.scratch[plane], &mut self.target[plane]);
                    let mut view = sao::Plane {
                        source,
                        into,
                        stride: self.stride[plane],
                        width: sps.width >> shift,
                        height: sps.height >> shift,
                        depth: sps.depth[shift],
                    };
                    let at = ((cx * size) >> shift, (cy * size) >> shift);
                    let span = (size >> shift, size >> shift);
                    // Ask once per CTB, since almost none has a unit the filters must leave alone.
                    if untouchable {
                        sao::apply(
                            &mut view,
                            offsets,
                            at,
                            span,
                            |x: usize, y: usize| {
                                let unit =
                                    units[((y << shift) >> 2) * unit_width + ((x << shift) >> 2)];
                                unit.bypass || (pcm_off && unit.pcm)
                            },
                            reach,
                        );
                    } else {
                        sao::apply(&mut view, offsets, at, span, |_, _| false, reach);
                    }
                }
            }
        }
    }

    fn has_untouchable(&self, sps: &Sps, x0: usize, y0: usize, size: usize) -> bool {
        let unit_width = sps.width / 4;
        for y in (y0..(y0 + size).min(sps.height)).step_by(4) {
            for x in (x0..(x0 + size).min(sps.width)).step_by(4) {
                let unit = self.units[(y >> 2) * unit_width + (x >> 2)];
                if unit.bypass || (sps.pcm_loop_filter_disabled && unit.pcm) {
                    return true;
                }
            }
        }
        false
    }

    /// Which of a CTB's eight neighbours SAO may read, given tile and slice boundaries.
    fn sao_reach(&self, sps: &Sps, pps: &Pps, cx: usize, cy: usize) -> sao::Reach {
        let ctb_width = sps.ctb_width();
        let ctb_height = sps.ctb_height();
        let here = cy * ctb_width + cx;
        let allows = |dx: i32, dy: i32| -> bool {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx as usize >= ctb_width || ny as usize >= ctb_height {
                return false;
            }
            let there = ny as usize * ctb_width + nx as usize;
            if self.tile[here] != self.tile[there] && !pps.loop_filter_across_tiles {
                return false;
            }
            if self.ctb_slice_addr[here] == self.ctb_slice_addr[there] {
                return true;
            }
            // The later slice in coding order decides, because it knew the other was there.
            let later = if self.rs_to_ts[here] > self.rs_to_ts[there] {
                self.ctb_slice_index[here]
            } else {
                self.ctb_slice_index[there]
            };
            self.across_slices
                .get(later as usize)
                .copied()
                .unwrap_or(true)
        };
        sao::Reach {
            usable: [
                allows(-1, 0),
                allows(1, 0),
                allows(0, -1),
                allows(0, 1),
                allows(-1, -1),
                allows(1, -1),
                allows(-1, 1),
                allows(1, 1),
            ],
        }
    }
}

#[cfg(feature = "hevc-decoder")]
impl Hevc {
    #[allow(clippy::too_many_arguments)]
    fn read_slice(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        kind: u8,
        temporal: u8,
        bytes: &[u8],
        started: &mut bool,
    ) -> Result<(), Error> {
        let data = unescape(bytes, bytes.len());
        let mut reader = crate::bits::Reader::new(&data);
        reader.bit();
        if params::is_irap(kind) {
            reader.bit();
        }
        let pps_id = reader.ue();
        let Some((pps, sps)) = self.sets.find(pps_id) else {
            return Err(Error::Damaged(
                "a slice with no parameter set to read it by",
            ));
        };
        let previous = self
            .current
            .as_ref()
            .map(|current| (*current.header).clone());
        let header = params::header(&data, kind, &sps, &pps, previous.as_ref())?;

        if header.first_segment {
            self.finish_picture();
            self.skipping = false;
            // ピクチャーサイズは別に読んだ SPS から来る。符号化したサイズを超えるものは渡せない。
            if format.width as usize > sps.width || format.height as usize > sps.height {
                return Err(Error::Damaged(
                    "a picture larger than its sequence parameter set",
                ));
            }
            if self.format != format
                || self.planes.is_empty()
                || self.stride[0] != sps.ctb_width() * (1 << sps.ctb_log2)
                || self.units.len() != (sps.width / 4) * (sps.height / 4)
            {
                self.reconfigure(&sps, format);
            }
            let Some(poc) = self
                .dpb
                .begin(&mut pool!(self), &sps, &header, kind, temporal)
            else {
                // Leading picture of the starting point: its references came before we started.
                self.skipping = true;
                return Ok(());
            };
            self.start_picture(tag, picture, format, &header, &sps, &pps, kind, poc)?;
            *started = true;
        } else if self.skipping {
            return Ok(());
        } else if self.current.is_none() {
            return Err(Error::Damaged("a slice segment with no picture"));
        }

        let Some(poc) = self.current.as_ref().map(|current| current.poc) else {
            return Ok(());
        };
        if !header.dependent
            && !header.first_segment
            && let Some(current) = self.current.as_mut()
        {
            *current.header = header.clone();
        }
        let lists = self.dpb.lists(&header);

        let collocated = if header.temporal_mvp_enabled {
            let list = usize::from(!header.collocated_from_l0);
            lists[list]
                .get(header.collocated_ref_idx)
                .and_then(|entry| entry.id.map(|buffer| (buffer, entry.poc)))
        } else {
            None
        };

        let slice_index = if header.dependent && !self.slices.is_empty() {
            (self.slices.len() - 1) as u16
        } else {
            self.slices.push(deblock::Slice {
                disabled: header.deblocking_disabled,
                beta_offset: header.beta_offset,
                tc_offset: header.tc_offset,
            });
            self.across_slices.push(header.loop_filter_across_slices);
            (self.slices.len() - 1) as u16
        };
        let slice_addr = if header.dependent {
            self.ctb_slice_addr
                .get(header.segment_address)
                .copied()
                .filter(|&addr| addr != u32::MAX)
                .unwrap_or(header.segment_address as u32)
        } else {
            header.segment_address as u32
        };

        let scaling = match &pps.scaling {
            Some(scaling) => scaling.clone(),
            None => sps.scaling.clone(),
        };
        let mut header = header;
        if !header.entry_points.is_empty() {
            let escapes = escapes(bytes);
            if !escapes.is_empty() {
                header.entry_points = entry_points(&escapes, header.data_at, &header.entry_points);
            } else {
                for point in &mut header.entry_points {
                    *point += header.data_at;
                }
            }
        }
        if !header.dependent {
            self.qp_carry = header.qp;
        }
        let depths = std::mem::take(&mut self.depths);
        let skips = std::mem::take(&mut self.skips);
        let wavefront = self.wavefront;
        let qp = self.qp_carry;
        let col_poc = collocated.map_or(0, |(_, poc)| poc);
        let result = {
            let colocated = collocated.map(|(buffer, _)| self.cols[buffer].as_slice());
            let target = slice::Target {
                planes: &mut self.target,
                stride: self.stride,
                pool: &self.planes,
                units: &mut self.units,
                sao: &mut self.offsets,
            };
            let layout = slice::Layout {
                rs_to_ts: &self.rs_to_ts,
                ts_to_rs: &self.ts_to_rs,
                tile: &self.tile,
                slice_addr: &mut self.ctb_slice_addr,
                slice_index: &mut self.ctb_slice_index,
            };
            let mut coder = slice::Coder::new(
                &sps,
                &pps,
                &header,
                &scaling,
                target,
                layout,
                lists,
                colocated,
                col_poc,
                poc,
                slice_index,
                slice_addr,
                &mut self.buffers,
                &data,
                depths,
                skips,
                wavefront,
                qp,
            );
            let outcome = coder.run();
            let (depths, skips, wavefront, qp) = coder.finish();
            (outcome, depths, skips, wavefront, qp)
        };
        self.depths = result.1;
        self.skips = result.2;
        self.wavefront = result.3;
        self.qp_carry = result.4;
        result.0
    }
}

#[cfg(feature = "hevc-decoder")]
impl Decoder for Hevc {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("a chroma format other than 4:2:0"));
        }
        if format.depth != 8 && format.depth != 10 {
            return Err(Error::Unsupported("a sample that is not eight or ten bits"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Damaged("a parameter set with no size"));
        }

        let mut started = false;
        let mut damage = None;
        for nal in Nals::new(data, self.framing) {
            let kind = (nal.header >> 1) & 0x3f;
            let second = data.get(nal.body.start).copied().unwrap_or(0);
            let temporal = (second & 7).saturating_sub(1);
            // Layers above the base are skipped, not refused: the base is whole.
            if (nal.header & 1) != 0 || (second >> 3) != 0 {
                continue;
            }
            let bytes = data.get(nal.body.start + 1..nal.body.end).unwrap_or(&[]);
            match kind {
                params::SPS_NUT | params::PPS_NUT => {
                    if let Err(error) = self.sets.absorb(kind, bytes) {
                        damage.get_or_insert(error);
                    }
                }
                params::EOS_NUT | params::EOB_NUT => {
                    // End of sequence: nothing before the next picture may be predicted from.
                    self.finish_picture();
                    self.dpb.end_of_sequence();
                }
                _ if params::is_slice(kind) => {
                    let error =
                        self.read_slice(tag, picture, format, kind, temporal, bytes, &mut started);
                    if let Err(error) = error {
                        // A lost slice is a lost strip; the next intra picture puts it right.
                        damage.get_or_insert(error);
                    }
                }
                _ => {}
            }
        }
        if !started && self.current.is_none() {
            return match damage {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        match damage {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take(&mut self) -> Option<Frame> {
        self.ready.pop_front()
    }

    fn flush(&mut self) {
        self.finish_picture();
        self.dpb.bump_all(&mut pool!(self));
    }

    fn reset(&mut self) {
        self.finish_picture();
        self.dpb.reset(&mut pool!(self));
        self.current = None;
        self.ready.clear();
    }
}
