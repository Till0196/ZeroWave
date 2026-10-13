// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The prediction unit: merge, motion vectors and inter prediction.

use super::*;

impl Coder<'_> {
    /// Clause 7.3.8.6, and the motion it works out.
    pub(super) fn prediction_unit(
        &mut self,
        at: (usize, usize),
        size: (usize, usize),
        part: usize,
    ) -> Cand {
        let merge = self.cabac.decision(ctx::MERGE_FLAG) == 1;
        if part == 0 && self.cu.partition == Partition::Whole {
            self.cu.merged = merge;
        }
        if merge {
            let index = self.read_merge_index();
            return self.merge_candidate(at, size, index, part);
        }

        let flags = if self.header.kind != Kind::B {
            1u8
        } else if size.0 + size.1 == 12 {
            // 8x4 and 4x8 are never bi-predicted.
            1 << self.cabac.decision(ctx::INTER_PRED_IDC + 4)
        } else {
            let depth = usize::from(self.depth_at(at.0, at.1));
            if self.cabac.decision(ctx::INTER_PRED_IDC + depth.min(4)) == 1 {
                3
            } else {
                1 << self.cabac.decision(ctx::INTER_PRED_IDC + 4)
            }
        };

        let mut cand = Cand {
            flags,
            ref_idx: [-1, -1],
            mv: [[0; 2]; 2],
        };
        let mut mvd = [[0i32; 2]; 2];
        let mut mvp_flag = [0usize; 2];
        for list in 0..2 {
            if flags >> list & 1 == 0 {
                continue;
            }
            let mut index = 0usize;
            let count = self.header.num_ref_idx[list];
            if count > 1 {
                if self.cabac.decision(ctx::REF_IDX) == 1 {
                    index = 1;
                    if count > 2 && self.cabac.decision(ctx::REF_IDX + 1) == 1 {
                        index = 2;
                        while index + 1 < count && self.cabac.bypass() == 1 {
                            index += 1;
                        }
                    }
                }
            }
            cand.ref_idx[list] = index as i8;
            if list == 1 && self.header.mvd_l1_zero && flags == 3 {
                mvd[1] = [0, 0];
            } else {
                mvd[list] = self.read_mvd();
            }
            mvp_flag[list] = self.cabac.decision(ctx::MVP_FLAG) as usize;
        }
        for list in 0..2 {
            if flags >> list & 1 == 0 {
                continue;
            }
            let predictor = self.predictor().predictor(
                self.cu.at,
                self.cu.size,
                at,
                size,
                part,
                list,
                cand.ref_idx[list].max(0) as usize,
                mvp_flag[list],
            );
            // Equation 8-105: wraps at sixteen bits.
            cand.mv[list] = [
                wrap16(i32::from(predictor[0]) + mvd[list][0]),
                wrap16(i32::from(predictor[1]) + mvd[list][1]),
            ];
        }
        cand
    }

    pub(super) fn read_merge_index(&mut self) -> usize {
        if self.header.max_merge_cand <= 1 {
            return 0;
        }
        if self.cabac.decision(ctx::MERGE_IDX) == 0 {
            return 0;
        }
        let mut index = 1usize;
        while index + 1 < self.header.max_merge_cand && self.cabac.bypass() == 1 {
            index += 1;
        }
        index
    }

    /// Clause 7.3.8.9: a motion vector difference.
    pub(super) fn read_mvd(&mut self) -> [i32; 2] {
        let greater0 = [
            self.cabac.decision(ctx::MVD) == 1,
            self.cabac.decision(ctx::MVD) == 1,
        ];
        let mut greater1 = [false; 2];
        for i in 0..2 {
            if greater0[i] {
                greater1[i] = self.cabac.decision(ctx::MVD + 1) == 1;
            }
        }
        let mut out = [0i32; 2];
        for i in 0..2 {
            if !greater0[i] {
                continue;
            }
            let mut value = 1i32;
            if greater1[i] {
                value = 2 + self.cabac.golomb(1) as i32;
            }
            if self.cabac.bypass() == 1 {
                value = -value;
            }
            out[i] = value;
        }
        out
    }

    pub(super) fn merge_candidate(
        &mut self,
        at: (usize, usize),
        size: (usize, usize),
        index: usize,
        part: usize,
    ) -> Cand {
        self.predictor().merge(
            self.cu.at,
            self.cu.size,
            at,
            size,
            part,
            self.cu.partition,
            index,
        )
    }

    pub(super) fn predict_inter(&mut self, at: (usize, usize), size: (usize, usize), cand: Cand) {
        self.record_motion(at, size, cand);
        let weighted = match self.header.kind {
            Kind::P => self.pps.weighted_pred,
            Kind::B => self.pps.weighted_bipred,
            Kind::I => false,
        };
        let used = [cand.flags & 1 != 0, cand.flags & 2 != 0];
        let mut buffer = [None; 2];
        for list in 0..2 {
            if used[list] {
                buffer[list] = self.lists[list]
                    .get(cand.ref_idx[list].max(0) as usize)
                    .and_then(|entry| entry.id);
            }
        }
        for plane in 0..3 {
            let shift = usize::from(plane > 0);
            let (w, h) = (size.0 >> shift, size.1 >> shift);
            let (x, y) = (at.0 >> shift, at.1 >> shift);
            if w == 0 || h == 0 {
                continue;
            }
            let depth = self.sps.depth[shift];
            let stride = self.target.stride[plane];
            let pool = self.target.pool;
            let (width, height) = (self.sps.width >> shift, self.sps.height >> shift);
            let Buffers {
                prediction,
                scratch,
                gathered,
                ..
            } = &mut *self.buffers;
            for list in 0..2 {
                let Some(buffer) = buffer[list] else {
                    continue;
                };
                let into = &mut prediction[list];
                let reference = inter::Reference {
                    samples: &pool[buffer][plane],
                    stride,
                    width,
                    height,
                };
                if plane == 0 {
                    inter::luma(
                        &reference,
                        x as i32,
                        y as i32,
                        cand.mv[list],
                        w,
                        h,
                        depth,
                        into,
                        w,
                        scratch,
                        gathered,
                    );
                } else {
                    inter::chroma(
                        &reference,
                        x as i32,
                        y as i32,
                        cand.mv[list],
                        w,
                        h,
                        depth,
                        into,
                        w,
                        scratch,
                        gathered,
                    );
                }
            }
            let weights = [
                self.header.weights[0][cand.ref_idx[0].max(0) as usize][plane],
                self.header.weights[1][cand.ref_idx[1].max(0) as usize][plane],
            ];
            let denom = if plane == 0 {
                self.header.luma_log2_denom
            } else {
                self.header.chroma_log2_denom
            };
            let (first, second) = (&self.buffers.prediction[0], &self.buffers.prediction[1]);
            let target = &mut self.target.planes[plane][x + y * stride..];
            if weighted {
                inter::weighted(
                    first, second, used, weights, denom, w, h, depth, target, w, stride,
                );
            } else {
                inter::combine(first, second, used, w, h, depth, target, w, stride);
            }
        }
    }

    pub(super) fn record_motion(&mut self, at: (usize, usize), size: (usize, usize), cand: Cand) {
        let mut poc = [0i32; 2];
        let mut long = [false; 2];
        for list in 0..2 {
            if cand.flags >> list & 1 == 1
                && let Some(entry) = self.lists[list].get(cand.ref_idx[list].max(0) as usize)
            {
                poc[list] = entry.poc;
                long[list] = entry.long;
            }
        }
        for y in (at.1..(at.1 + size.1).min(self.sps.height)).step_by(4) {
            for x in (at.0..(at.0 + size.0).min(self.sps.width)).step_by(4) {
                let unit = &mut self.target.units[(y >> 2) * self.unit_width + (x >> 2)];
                unit.intra = false;
                unit.mv = cand.mv;
                unit.ref_idx = cand.ref_idx;
                unit.flags = cand.flags;
                unit.poc = poc;
                unit.long = long;
                if x == at.0 && at.0 != self.cu.at.0 {
                    unit.edge |= EDGE_PU_LEFT;
                }
                if y == at.1 && at.1 != self.cu.at.1 {
                    unit.edge |= EDGE_PU_TOP;
                }
            }
        }
    }
}

/// Equation 8-105: motion vectors are sixteen bits and wrap.
pub(super) fn wrap16(value: i32) -> i16 {
    (value & 0xffff) as u16 as i16
}
