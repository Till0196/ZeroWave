// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Clause 7.3.8.8: the transform tree and its units.

use super::*;

impl Coder<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn transform_tree(
        &mut self,
        x0: usize,
        y0: usize,
        xbase: usize,
        ybase: usize,
        log2: usize,
        depth: usize,
        blk: usize,
        parent_cb: bool,
        parent_cr: bool,
    ) {
        let intra_split = self.cu.intra && self.cu.partition == Partition::Quarters;
        let split = if log2 <= self.sps.max_tb_log2
            && log2 > self.sps.min_tb_log2
            && depth < self.cu.max_transform_depth
            && !(intra_split && depth == 0)
        {
            self.cabac.decision(ctx::SPLIT_TRANSFORM + 5 - log2) == 1
        } else {
            // Inferred where not sent: too large, NxN intra, or inter split with depth 0.
            let inter_split = self.sps.max_transform_depth_inter == 0
                && !self.cu.intra
                && self.cu.partition != Partition::Whole
                && depth == 0;
            log2 > self.sps.max_tb_log2 || (intra_split && depth == 0) || inter_split
        };

        let (mut cbf_cb, mut cbf_cr) = (parent_cb, parent_cr);
        if log2 > 2 {
            cbf_cb = false;
            cbf_cr = false;
            if depth == 0 || parent_cb {
                cbf_cb = self.cabac.decision(ctx::CBF_CHROMA + depth.min(3)) == 1;
            }
            if depth == 0 || parent_cr {
                cbf_cr = self.cabac.decision(ctx::CBF_CHROMA + depth.min(3)) == 1;
            }
        }

        if split {
            let half = 1usize << (log2 - 1);
            self.transform_tree(x0, y0, x0, y0, log2 - 1, depth + 1, 0, cbf_cb, cbf_cr);
            self.transform_tree(
                x0 + half,
                y0,
                x0,
                y0,
                log2 - 1,
                depth + 1,
                1,
                cbf_cb,
                cbf_cr,
            );
            self.transform_tree(
                x0,
                y0 + half,
                x0,
                y0,
                log2 - 1,
                depth + 1,
                2,
                cbf_cb,
                cbf_cr,
            );
            self.transform_tree(
                x0 + half,
                y0 + half,
                x0,
                y0,
                log2 - 1,
                depth + 1,
                3,
                cbf_cb,
                cbf_cr,
            );
            return;
        }

        let mut cbf_luma = true;
        if self.cu.intra || depth != 0 || cbf_cb || cbf_cr {
            cbf_luma = self.cabac.decision(ctx::CBF_LUMA + usize::from(depth == 0)) == 1;
        }
        self.transform_unit(x0, y0, xbase, ybase, log2, blk, cbf_luma, cbf_cb, cbf_cr);
    }

    /// Clause 7.3.8.10, and the reconstruction of the block it covers.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn transform_unit(
        &mut self,
        x0: usize,
        y0: usize,
        xbase: usize,
        ybase: usize,
        log2: usize,
        blk: usize,
        cbf_luma: bool,
        cbf_cb: bool,
        cbf_cr: bool,
    ) {
        if (cbf_luma || cbf_cb || cbf_cr) && self.pps.cu_qp_delta_enabled && !self.qp_delta_coded {
            self.read_qp_delta();
        }

        if self.cu.intra {
            let mode = self.intra_mode_at(x0, y0);
            self.predict_intra(0, x0, y0, log2, mode);
        }
        if cbf_luma {
            self.residual_coding(x0, y0, log2, 0);
            self.add_residual(0, x0, y0, log2);
        }

        if log2 > 2 {
            let (cx, cy) = (x0 / 2, y0 / 2);
            if self.cu.intra {
                self.predict_intra(1, cx, cy, log2 - 1, self.cu.chroma_mode);
                self.predict_intra(2, cx, cy, log2 - 1, self.cu.chroma_mode);
            }
            if cbf_cb {
                self.residual_coding(x0, y0, log2 - 1, 1);
                self.add_residual(1, cx, cy, log2 - 1);
            }
            if cbf_cr {
                self.residual_coding(x0, y0, log2 - 1, 2);
                self.add_residual(2, cx, cy, log2 - 1);
            }
        } else if blk == 3 {
            // At the smallest size four luma blocks share one chroma block, done after the last.
            let (cx, cy) = (xbase / 2, ybase / 2);
            if self.cu.intra {
                self.predict_intra(1, cx, cy, 2, self.cu.chroma_mode);
                self.predict_intra(2, cx, cy, 2, self.cu.chroma_mode);
            }
            if cbf_cb {
                self.residual_coding(xbase, ybase, 2, 1);
                self.add_residual(1, cx, cy, 2);
            }
            if cbf_cr {
                self.residual_coding(xbase, ybase, 2, 2);
                self.add_residual(2, cx, cy, 2);
            }
        }
        self.mark_tu_edges(x0, y0, 1 << log2, cbf_luma);
    }

    /// Reads `cu_qp_delta`, sent at most once per quantisation group.
    pub(super) fn read_qp_delta(&mut self) {
        self.qp_delta_coded = true;
        let mut magnitude = 0u32;
        if self.cabac.decision(ctx::CU_QP_DELTA) == 1 {
            magnitude = 1;
            while magnitude < 5 && self.cabac.decision(ctx::CU_QP_DELTA + 1) == 1 {
                magnitude += 1;
            }
            if magnitude == 5 {
                magnitude += self.cabac.golomb(0);
            }
        }
        let mut value = magnitude as i32;
        if value != 0 && self.cabac.bypass() == 1 {
            value = -value;
        }
        self.cu_qp_delta = value;
        let (at, size) = (self.cu.at, self.cu.size);
        self.write_qp(at.0, at.1, size);
    }

    pub(super) fn intra_mode_at(&self, x: usize, y: usize) -> u8 {
        self.unit_at(x, y).intra_mode
    }

    pub(super) fn mark_tu_edges(&mut self, x0: usize, y0: usize, size: usize, cbf: bool) {
        let (filter_left, filter_top) = self.cu_edges();
        for y in (y0..(y0 + size).min(self.sps.height)).step_by(4) {
            for x in (x0..(x0 + size).min(self.sps.width)).step_by(4) {
                let unit = &mut self.target.units[(y >> 2) * self.unit_width + (x >> 2)];
                unit.cbf = cbf;
                if x == x0 && (x0 != self.cu.at.0 || filter_left) {
                    unit.edge |= EDGE_TU_LEFT;
                }
                if y == y0 && (y0 != self.cu.at.1 || filter_top) {
                    unit.edge |= EDGE_TU_TOP;
                }
            }
        }
    }

    /// Whether the CU's left and top edges are filtered (picture, tile and slice boundaries).
    pub(super) fn cu_edges(&self) -> (bool, bool) {
        let (x0, y0) = self.cu.at;
        let ctb = |x: usize, y: usize| {
            (y >> self.sps.ctb_log2) * self.ctb_width + (x >> self.sps.ctb_log2)
        };
        let here = ctb(x0, y0);
        let allowed = |there: usize| {
            (self.layout.tile[there] == self.layout.tile[here] || self.pps.loop_filter_across_tiles)
                && (self.layout.slice_addr[there] == self.slice_addr
                    || self.header.loop_filter_across_slices)
        };
        let left = x0 > 0 && allowed(ctb(x0 - 1, y0));
        let top = y0 > 0 && allowed(ctb(x0, y0 - 1));
        (left, top)
    }

    /// Clause 7.3.8.7: PCM samples, read outside the arithmetic decoder.
    pub(super) fn pcm_sample(&mut self, x0: usize, y0: usize, log2: usize) {
        let at = self.cabac.byte() + self.header.data_at;
        let mut reader = crate::bits::Reader::new(&self.data[at.min(self.data.len())..]);
        for plane in 0..3 {
            let shift = usize::from(plane > 0);
            let bits = usize::from(self.sps.pcm_depth[shift]);
            let size = 1usize << (log2 - shift);
            let depth = self.sps.depth[shift];
            let up = u32::from(depth) - bits as u32;
            let stride = self.target.stride[plane];
            for y in 0..size {
                for x in 0..size {
                    let value = reader.bits(bits) << up;
                    let (px, py) = ((x0 >> shift) + x, (y0 >> shift) + y);
                    self.target.planes[plane][px + py * stride] = value as u16;
                }
            }
        }
        // Clause 9.3.2.5: restart over the contexts as they stood.
        let state = self.cabac.contexts();
        let resume = at + reader.at().div_ceil(8);
        let start = resume.min(self.data.len());
        self.cabac = Cabac::new(&self.data[start..], self.init_type(), self.header.qp);
        self.cabac.restart(0, state);
    }
}
