// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Everything below the slice segment header (clause 7.3.8) with its reconstruction (8.4, 8.5).
//! Parsing and reconstruction run together because intra prediction reads the previous TU.

use crate::video::Error;

use super::cabac::Cabac;
use super::contexts as ctx;
use super::inter;
use super::intra::{self, Neighbours};
use super::mv::{
    Cand, Col, EDGE_PU_LEFT, EDGE_PU_TOP, EDGE_TU_LEFT, EDGE_TU_TOP, Entry, Partition, Predictor,
    Unit,
};
use super::params::{Header, Kind, Pps, Scaling, Sps};
use super::sao;
use super::tables::{SCAN_4, chroma_qp, scan_order};
use super::transform;

mod motion;
mod residual;
mod tree;
mod unit;

const PRED: usize = inter::MOST * inter::MOST;

#[derive(Debug)]
pub struct Buffers {
    prediction: [Vec<i32>; 2],
    scratch: Vec<i32>,
    gathered: Vec<i32>,
    coefficients: Vec<i32>,
    neighbours: Neighbours,
}

impl Default for Buffers {
    fn default() -> Self {
        Self {
            prediction: [vec![0; PRED], vec![0; PRED]],
            scratch: vec![0; inter::SCRATCH],
            gathered: vec![0; inter::SCRATCH],
            coefficients: vec![0; 32 * 32],
            neighbours: Neighbours::default(),
        }
    }
}

#[derive(Debug)]
pub struct Target<'a> {
    pub planes: &'a mut [Vec<u16>; 3],
    pub stride: [usize; 3],
    /// Every frame held, except the one being decoded (taken out so it can be written).
    pub pool: &'a [[Vec<u16>; 3]],
    pub units: &'a mut [Unit],
    pub sao: &'a mut [[sao::Offsets; 3]],
}

#[derive(Debug)]
pub struct Layout<'a> {
    pub rs_to_ts: &'a [u32],
    pub ts_to_rs: &'a [u32],
    pub tile: &'a [u16],
    /// Filled as the picture is decoded; a CTB not yet decoded is never read.
    pub slice_addr: &'a mut [u32],
    pub slice_index: &'a mut [u16],
}

#[derive(Debug)]
pub struct Coder<'a> {
    cabac: Cabac<'a>,
    sps: &'a Sps,
    pps: &'a Pps,
    header: &'a Header,
    scaling: &'a Scaling,
    target: Target<'a>,
    layout: Layout<'a>,
    lists: [Vec<Entry>; 2],
    colocated: Option<&'a [Col]>,
    col_poc: i32,
    poc: i32,
    no_backward: bool,
    buffers: &'a mut Buffers,
    data: &'a [u8],

    slice_index: u16,
    slice_addr: u32,

    unit_width: usize,
    ctb_width: usize,
    qp: i32,
    qp_prev: i32,
    qp_pred: i32,
    cu_qp_delta: i32,
    qp_delta_coded: bool,
    cu: CodingUnit,
    depth: Vec<u8>,
    skip: Vec<bool>,
    /// Contexts saved at a row's second CTB for wavefront parallel processing.
    wavefront: Option<[u8; ctx::CONTEXTS]>,
}

#[derive(Debug, Clone, Copy, Default)]
struct CodingUnit {
    at: (usize, usize),
    size: usize,
    intra: bool,
    skip: bool,
    bypass: bool,
    pcm: bool,
    partition: Partition,
    /// A merged whole-unit block with no residual would have been a skip, so no root flag.
    merged: bool,
    luma_mode: [u8; 4],
    chroma_mode: u8,
    max_transform_depth: usize,
}

impl<'a> Coder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sps: &'a Sps,
        pps: &'a Pps,
        header: &'a Header,
        scaling: &'a Scaling,
        target: Target<'a>,
        layout: Layout<'a>,
        lists: [Vec<Entry>; 2],
        colocated: Option<&'a [Col]>,
        col_poc: i32,
        poc: i32,
        slice_index: u16,
        slice_addr: u32,
        buffers: &'a mut Buffers,
        data: &'a [u8],
        depth: Vec<u8>,
        skip: Vec<bool>,
        wavefront: Option<[u8; ctx::CONTEXTS]>,
        qp: i32,
    ) -> Self {
        // Equation 9-7: initType.
        let init_type = match header.kind {
            Kind::I => 0,
            Kind::P => {
                if header.cabac_init_flag {
                    2
                } else {
                    1
                }
            }
            Kind::B => {
                if header.cabac_init_flag {
                    1
                } else {
                    2
                }
            }
        };
        let no_backward = lists
            .iter()
            .all(|list| list.iter().all(|entry| entry.poc <= poc));
        // The whole payload, so every reported position is measured from one place.
        let mut cabac = Cabac::new(data, init_type, header.qp);
        let fresh = cabac.contexts();
        cabac.restart(header.data_at, fresh);
        let ctb_width = sps.ctb_width();
        Self {
            cabac,
            sps,
            pps,
            header,
            scaling,
            target,
            layout,
            lists,
            colocated,
            col_poc,
            poc,
            no_backward,
            buffers,
            data,
            slice_index,
            slice_addr,
            unit_width: sps.width / 4,
            ctb_width,
            qp,
            qp_prev: qp,
            qp_pred: qp,
            cu_qp_delta: 0,
            qp_delta_coded: false,
            cu: CodingUnit::default(),
            depth,
            skip,
            wavefront,
        }
    }

    pub fn finish(self) -> (Vec<u8>, Vec<bool>, Option<[u8; ctx::CONTEXTS]>, i32) {
        (self.depth, self.skip, self.wavefront, self.qp)
    }

    fn predictor(&self) -> Predictor<'_> {
        Predictor {
            units: self.target.units,
            unit_width: self.unit_width,
            width: self.sps.width,
            height: self.sps.height,
            min_tb_log2: self.sps.min_tb_log2,
            ctb_log2: self.sps.ctb_log2,
            ctb_width: self.ctb_width,
            ctb_order: self.layout.rs_to_ts,
            ctb_slice: self.layout.slice_addr,
            ctb_tile: self.layout.tile,
            lists: &self.lists,
            poc: self.poc,
            kind: self.header.kind,
            max_merge: self.header.max_merge_cand,
            par_merge_log2: self.pps.log2_parallel_merge_level,
            temporal_enabled: self.header.temporal_mvp_enabled,
            col: self.colocated,
            col_width: self.sps.width.div_ceil(16),
            col_poc: self.col_poc,
            no_backward: self.no_backward,
            collocated_from_l0: self.header.collocated_from_l0,
        }
    }

    fn available(&self, x: usize, y: usize, nx: i32, ny: i32) -> bool {
        self.predictor().available(x, y, nx, ny)
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let mut ts = self.layout.rs_to_ts[self.header.segment_address] as usize;
        let first = ts;
        let mut substream = 0usize;
        loop {
            let rs = self.layout.ts_to_rs[ts] as usize;
            let (cx, cy) = (rs % self.ctb_width, rs / self.ctb_width);
            if ts != first {
                let new_tile =
                    self.layout.tile[rs] != self.layout.tile[self.layout.ts_to_rs[ts - 1] as usize];
                let new_row = self.pps.entropy_coding_sync_enabled && cx == 0;
                if new_tile || new_row {
                    substream += 1;
                    let Some(&at) = self.header.entry_points.get(substream - 1) else {
                        return Err(Error::Damaged("a substream with no entry point"));
                    };
                    if at >= self.data.len() {
                        return Err(Error::Damaged("an entry point past the end of a slice"));
                    }
                    // Clause 9.3.1: from the second CTB above, if this slice and tile decoded it.
                    let above_right = cy > 0
                        && self.ctb_width > 1
                        && self.layout.slice_addr[rs - self.ctb_width + 1] == self.slice_addr
                        && self.layout.tile[rs - self.ctb_width + 1] == self.layout.tile[rs];
                    let state = match self.wavefront {
                        Some(saved) if new_row && !new_tile && above_right => saved,
                        _ => Cabac::new(&[], self.init_type(), self.header.qp).contexts(),
                    };
                    self.cabac.restart(at, state);
                    // Clause 8.6.1: a tile or wavefront row starts from the slice QP;
                    // set the QP itself, not the prediction: the next CTB predicts from here.
                    self.qp = self.header.qp;
                    self.qp_prev = self.header.qp;
                }
            }
            self.layout.slice_addr[rs] = self.slice_addr;
            self.layout.slice_index[rs] = self.slice_index;

            self.coding_tree_unit(cx, cy);
            if self.pps.entropy_coding_sync_enabled && (cx == 1 || self.ctb_width == 1) {
                self.wavefront = Some(self.cabac.contexts());
            }
            if self.cabac.overrun() {
                return Err(Error::Damaged("a slice that ran out under the decoder"));
            }
            let end = self.cabac.terminate();
            ts += 1;
            if end || ts >= self.layout.ts_to_rs.len() {
                break;
            }
            let next = self.layout.ts_to_rs[ts] as usize;
            let new_tile = self.layout.tile[next] != self.layout.tile[rs];
            let new_row =
                self.pps.entropy_coding_sync_enabled && next.is_multiple_of(self.ctb_width);
            if new_tile || new_row {
                // `end_of_subset_one_bit`; the position then comes from the entry point.
                self.cabac.terminate();
            }
        }
        Ok(())
    }

    fn init_type(&self) -> usize {
        match self.header.kind {
            Kind::I => 0,
            Kind::P => usize::from(self.header.cabac_init_flag) + 1,
            Kind::B => 2 - usize::from(self.header.cabac_init_flag),
        }
    }

    fn coding_tree_unit(&mut self, cx: usize, cy: usize) {
        let size = 1usize << self.sps.ctb_log2;
        if self.header.sao_luma || self.header.sao_chroma {
            self.read_sao(cx, cy);
        }
        self.coding_quadtree(cx * size, cy * size, self.sps.ctb_log2, 0);
    }

    /// Clause 7.3.8.3: SAO parameters, possibly merged from left or above.
    fn read_sao(&mut self, cx: usize, cy: usize) {
        let rs = cy * self.ctb_width + cx;
        let mut merge_left = false;
        let mut merge_up = false;
        if cx > 0
            && self.same_slice_and_tile(rs, rs - 1)
            && self.cabac.decision(ctx::SAO_MERGE) == 1
        {
            merge_left = true;
        }
        if !merge_left
            && cy > 0
            && self.same_slice_and_tile(rs, rs - self.ctb_width)
            && self.cabac.decision(ctx::SAO_MERGE) == 1
        {
            merge_up = true;
        }
        if merge_left {
            self.target.sao[rs] = self.target.sao[rs - 1];
            return;
        }
        if merge_up {
            self.target.sao[rs] = self.target.sao[rs - self.ctb_width];
            return;
        }
        let depth = self.sps.depth;
        let mut offsets = [sao::Offsets::default(); 3];
        for plane in 0..3 {
            if (plane == 0 && !self.header.sao_luma) || (plane > 0 && !self.header.sao_chroma) {
                continue;
            }
            if plane < 2 {
                // Truncated unary: first bin in context, second bypass.
                let mut kind = 0u8;
                if self.cabac.decision(ctx::SAO_TYPE) == 1 {
                    kind = if self.cabac.bypass() == 1 { 2 } else { 1 };
                }
                offsets[plane].kind = kind;
            } else {
                offsets[2].kind = offsets[1].kind;
            }
            if offsets[plane].kind == 0 {
                continue;
            }
            let most = (1u32 << (u32::from(depth[usize::from(plane > 0)]).min(10) - 5)) - 1;
            let mut magnitude = [0i32; 4];
            for value in &mut magnitude {
                let mut count = 0;
                while count < most && self.cabac.bypass() == 1 {
                    count += 1;
                }
                *value = count as i32;
            }
            if offsets[plane].kind == 1 {
                for value in &mut magnitude {
                    if *value != 0 && self.cabac.bypass() == 1 {
                        *value = -*value;
                    }
                }
                offsets[plane].band = self.cabac.bypass_bits(5) as u8;
            } else {
                // Edge offset signs are fixed by category.
                magnitude[2] = -magnitude[2];
                magnitude[3] = -magnitude[3];
                if plane == 0 {
                    offsets[0].class = self.cabac.bypass_bits(2) as u8;
                } else if plane == 1 {
                    offsets[1].class = self.cabac.bypass_bits(2) as u8;
                } else {
                    offsets[2].class = offsets[1].class;
                }
            }
            for (i, &value) in magnitude.iter().enumerate() {
                offsets[plane].value[i + 1] = value;
            }
        }
        self.target.sao[rs] = offsets;
    }

    fn same_slice_and_tile(&self, here: usize, there: usize) -> bool {
        self.layout.tile[here] == self.layout.tile[there]
            && self.layout.slice_addr[there] == self.slice_addr
            && (self.layout.rs_to_ts[there] as usize) < self.layout.rs_to_ts[here] as usize
    }

    /// Clause 7.3.8.4.
    fn coding_quadtree(&mut self, x0: usize, y0: usize, log2: usize, depth: usize) {
        let size = 1usize << log2;
        let mut split = log2 > self.sps.min_cb_log2;
        if x0 + size <= self.sps.width
            && y0 + size <= self.sps.height
            && log2 > self.sps.min_cb_log2
        {
            let mut context = 0usize;
            if self.available(x0, y0, x0 as i32 - 1, y0 as i32)
                && usize::from(self.depth_at(x0 - 1, y0)) > depth
            {
                context += 1;
            }
            if self.available(x0, y0, x0 as i32, y0 as i32 - 1)
                && usize::from(self.depth_at(x0, y0 - 1)) > depth
            {
                context += 1;
            }
            split = self.cabac.decision(ctx::SPLIT_CU + context) == 1;
        } else if x0 + size <= self.sps.width && y0 + size <= self.sps.height {
            split = false;
        }

        if self.pps.cu_qp_delta_enabled
            && log2 >= self.sps.ctb_log2 - self.pps.diff_cu_qp_delta_depth
        {
            self.qp_delta_coded = false;
            self.cu_qp_delta = 0;
            self.qp_prev = self.qp;
            self.qp_pred = self.predicted_qp(x0, y0);
        }

        if split {
            let half = size / 2;
            self.coding_quadtree(x0, y0, log2 - 1, depth + 1);
            if x0 + half < self.sps.width {
                self.coding_quadtree(x0 + half, y0, log2 - 1, depth + 1);
            }
            if y0 + half < self.sps.height {
                self.coding_quadtree(x0, y0 + half, log2 - 1, depth + 1);
            }
            if x0 + half < self.sps.width && y0 + half < self.sps.height {
                self.coding_quadtree(x0 + half, y0 + half, log2 - 1, depth + 1);
            }
            return;
        }
        self.set_depth(x0, y0, size, depth as u8);
        self.coding_unit(x0, y0, log2);
    }

    /// Equation 8-282: predicted QP of a quantisation group.
    fn predicted_qp(&self, x0: usize, y0: usize) -> i32 {
        let mask = (1usize << (self.sps.ctb_log2 - self.pps.diff_cu_qp_delta_depth)) - 1;
        let xq = x0 & !mask;
        let yq = y0 & !mask;
        let ctb = |x: usize, y: usize| {
            (y >> self.sps.ctb_log2) * self.ctb_width + (x >> self.sps.ctb_log2)
        };
        let here = ctb(x0, y0);
        let side = |nx: i32, ny: i32| -> i32 {
            if nx < 0 || ny < 0 || !self.available(x0, y0, nx, ny) {
                return self.qp_prev;
            }
            if ctb(nx as usize, ny as usize) != here {
                return self.qp_prev;
            }
            i32::from(self.unit_at(nx as usize, ny as usize).qp)
        };
        let a = side(xq as i32 - 1, yq as i32);
        let b = side(xq as i32, yq as i32 - 1);
        (a + b + 1) >> 1
    }

    fn depth_at(&self, x: usize, y: usize) -> u8 {
        self.depth[(y >> 2) * self.unit_width + (x >> 2)]
    }

    fn set_depth(&mut self, x0: usize, y0: usize, size: usize, depth: u8) {
        for y in (y0..(y0 + size).min(self.sps.height)).step_by(4) {
            for x in (x0..(x0 + size).min(self.sps.width)).step_by(4) {
                self.depth[(y >> 2) * self.unit_width + (x >> 2)] = depth;
            }
        }
    }

    fn unit_at(&self, x: usize, y: usize) -> Unit {
        self.target.units[(y >> 2) * self.unit_width + (x >> 2)]
    }
}
