// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Clause 7.3.8.5: the coding unit.

use super::*;

impl Coder<'_> {
    pub(super) fn coding_unit(&mut self, x0: usize, y0: usize, log2: usize) {
        let size = 1usize << log2;
        let mut cu = CodingUnit {
            at: (x0, y0),
            size,
            intra: true,
            partition: Partition::Whole,
            max_transform_depth: self.sps.max_transform_depth_intra,
            ..CodingUnit::default()
        };
        if self.pps.transquant_bypass_enabled {
            cu.bypass = self.cabac.decision(ctx::BYPASS_FLAG) == 1;
        }
        if !self.header.kind.intra() {
            let mut context = 0usize;
            if self.available(x0, y0, x0 as i32 - 1, y0 as i32) && self.skip_at(x0 - 1, y0) {
                context += 1;
            }
            if self.available(x0, y0, x0 as i32, y0 as i32 - 1) && self.skip_at(x0, y0 - 1) {
                context += 1;
            }
            cu.skip = self.cabac.decision(ctx::CU_SKIP + context) == 1;
        }
        self.set_skip(x0, y0, size, cu.skip);

        if cu.skip {
            cu.intra = false;
            cu.merged = true;
            self.cu = cu;
            self.write_qp(x0, y0, size);
            let index = self.read_merge_index();
            let candidate = self.merge_candidate((x0, y0), (size, size), index, 0);
            self.predict_inter((x0, y0), (size, size), candidate);
            self.mark_tu_edges(x0, y0, size, false);
            return;
        }

        if !self.header.kind.intra() {
            cu.intra = self.cabac.decision(ctx::PRED_MODE) == 1;
        }
        if !cu.intra {
            cu.max_transform_depth = self.sps.max_transform_depth_inter;
        }
        if !cu.intra || log2 == self.sps.min_cb_log2 {
            cu.partition = self.read_partition(cu.intra, log2);
        }
        if cu.intra && cu.partition == Partition::Quarters {
            // An intra NxN unit splits its transform tree once by definition.
            cu.max_transform_depth += 1;
        }
        self.cu = cu;

        if cu.intra {
            if cu.partition == Partition::Whole
                && self.sps.pcm_enabled
                && log2 >= self.sps.pcm_min_log2
                && log2 <= self.sps.pcm_max_log2
                && self.cabac.terminate()
            {
                self.cu.pcm = true;
                for y in (y0..(y0 + size).min(self.sps.height)).step_by(4) {
                    for x in (x0..(x0 + size).min(self.sps.width)).step_by(4) {
                        let unit = &mut self.target.units[(y >> 2) * self.unit_width + (x >> 2)];
                        unit.intra = true;
                        unit.intra_mode = 1;
                        unit.flags = 0;
                    }
                }
                self.write_qp(x0, y0, size);
                self.pcm_sample(x0, y0, log2);
                self.mark_tu_edges(x0, y0, size, false);
                return;
            }
            self.read_intra_modes(x0, y0, log2);
            self.write_qp(x0, y0, size);
        } else {
            self.write_qp(x0, y0, size);
            let parts = self.cu.partition.parts();
            for part in 0..parts {
                let (offset, block) = self.cu.partition.block(size, part);
                let at = (x0 + offset.0, y0 + offset.1);
                let motion = self.prediction_unit(at, block, part);
                self.predict_inter(at, block, motion);
            }
        }

        let mut root = true;
        if !self.cu.intra && !(self.cu.partition == Partition::Whole && self.cu.merged) {
            root = self.cabac.decision(ctx::RQT_ROOT_CBF) == 1;
        }
        if root {
            self.transform_tree(x0, y0, x0, y0, log2, 0, 0, true, true);
        } else {
            self.mark_tu_edges(x0, y0, size, false);
        }
    }

    /// Table 9-45: how a coding unit is cut into prediction units.
    pub(super) fn read_partition(&mut self, intra: bool, log2: usize) -> Partition {
        if intra {
            return if self.cabac.decision(ctx::PART_MODE) == 1 {
                Partition::Whole
            } else {
                Partition::Quarters
            };
        }
        if self.cabac.decision(ctx::PART_MODE) == 1 {
            return Partition::Whole;
        }
        let smallest = log2 == self.sps.min_cb_log2;
        if self.cabac.decision(ctx::PART_MODE + 1) == 1 {
            if smallest || !self.sps.amp_enabled {
                return Partition::Horizontal;
            }
            // The third bin tells a half from a quarter, the fourth which side the quarter is on.
            if self.cabac.decision(ctx::PART_MODE + 3) == 1 {
                return Partition::Horizontal;
            }
            return if self.cabac.bypass() == 1 {
                Partition::HorizontalDown
            } else {
                Partition::HorizontalUp
            };
        }
        if smallest {
            if log2 == 3 {
                return Partition::Vertical;
            }
            return if self.cabac.decision(ctx::PART_MODE + 2) == 1 {
                Partition::Vertical
            } else {
                Partition::Quarters
            };
        }
        if !self.sps.amp_enabled {
            return Partition::Vertical;
        }
        if self.cabac.decision(ctx::PART_MODE + 3) == 1 {
            return Partition::Vertical;
        }
        if self.cabac.bypass() == 1 {
            Partition::VerticalRight
        } else {
            Partition::VerticalLeft
        }
    }

    /// Clause 8.4.2: the luma prediction modes of a coding unit.
    pub(super) fn read_intra_modes(&mut self, x0: usize, y0: usize, log2: usize) {
        let size = 1usize << log2;
        let parts = if self.cu.partition == Partition::Quarters {
            4
        } else {
            1
        };
        let step = if parts == 4 { size / 2 } else { size };
        let mut previous = [false; 4];
        for flag in previous.iter_mut().take(parts) {
            *flag = self.cabac.decision(ctx::PREV_INTRA_LUMA) == 1;
        }
        let mut chosen = [0u8; 4];
        for part in 0..parts {
            let at = (x0 + (part % 2) * step, y0 + (part / 2) * step);
            let candidates = self.mode_candidates(at);
            if previous[part] {
                let index = if self.cabac.bypass() == 0 {
                    0
                } else if self.cabac.bypass() == 0 {
                    1
                } else {
                    2
                };
                chosen[part] = candidates[index];
            } else {
                let mut sorted = candidates;
                sorted.sort_unstable();
                let mut mode = self.cabac.bypass_bits(5) as u8;
                for &candidate in &sorted {
                    if mode >= candidate {
                        mode += 1;
                    }
                }
                chosen[part] = mode;
            }
            // Recorded now: a quarter may be the next one's neighbour.
            self.cu.luma_mode[part] = chosen[part];
            let (bx, by) = at;
            for y in (by..(by + step).min(self.sps.height)).step_by(4) {
                for x in (bx..(bx + step).min(self.sps.width)).step_by(4) {
                    let unit = &mut self.target.units[(y >> 2) * self.unit_width + (x >> 2)];
                    unit.intra = true;
                    unit.intra_mode = chosen[part];
                    unit.flags = 0;
                }
            }
        }
        // Table 8-2: a collision with luma becomes mode 34.
        let mut chroma = self.cu.luma_mode[0];
        if self.cabac.decision(ctx::INTRA_CHROMA) == 1 {
            let index = self.cabac.bypass_bits(2) as usize;
            let wanted = [0u8, 26, 10, 1][index];
            chroma = if wanted == self.cu.luma_mode[0] {
                34
            } else {
                wanted
            };
        }
        self.cu.chroma_mode = chroma;
    }

    pub(super) fn mode_candidates(&self, at: (usize, usize)) -> [u8; 3] {
        let (x, y) = at;
        let side = |nx: i32, ny: i32, above: bool| -> u8 {
            if !self.available(x, y, nx, ny) {
                return 1;
            }
            // Not across the CTB row above; the standard avoids a picture-wide row of modes.
            if above && (ny as usize) < (y >> self.sps.ctb_log2) << self.sps.ctb_log2 {
                return 1;
            }
            let unit = self.unit_at(nx as usize, ny as usize);
            if !unit.intra || unit.pcm {
                return 1;
            }
            unit.intra_mode
        };
        let a = side(x as i32 - 1, y as i32, false);
        let b = side(x as i32, y as i32 - 1, true);
        if a == b {
            if a < 2 {
                return [0, 1, 26];
            }
            return [
                a,
                2 + ((u32::from(a) + 29) % 32) as u8,
                2 + ((u32::from(a) - 2 + 1) % 32) as u8,
            ];
        }
        let third = if a != 0 && b != 0 {
            0
        } else if a != 1 && b != 1 {
            1
        } else {
            26
        };
        [a, b, third]
    }

    pub(super) fn skip_at(&self, x: usize, y: usize) -> bool {
        self.skip[(y >> 2) * self.unit_width + (x >> 2)]
    }

    pub(super) fn set_skip(&mut self, x0: usize, y0: usize, size: usize, value: bool) {
        for y in (y0..(y0 + size).min(self.sps.height)).step_by(4) {
            for x in (x0..(x0 + size).min(self.sps.width)).step_by(4) {
                self.skip[(y >> 2) * self.unit_width + (x >> 2)] = value;
            }
        }
    }

    pub(super) fn write_qp(&mut self, x0: usize, y0: usize, size: usize) {
        self.qp = self.derived_qp();
        let qp = self.qp as i8;
        let bypass = self.cu.bypass;
        let pcm = self.cu.pcm;
        for y in (y0..(y0 + size).min(self.sps.height)).step_by(4) {
            for x in (x0..(x0 + size).min(self.sps.width)).step_by(4) {
                let unit = &mut self.target.units[(y >> 2) * self.unit_width + (x >> 2)];
                unit.qp = qp;
                unit.bypass = bypass;
                unit.pcm = pcm;
            }
        }
    }

    /// Equation 8-283.
    pub(super) fn derived_qp(&self) -> i32 {
        if !self.pps.cu_qp_delta_enabled {
            return self.header.qp;
        }
        let offset = self.sps.qp_offset(false);
        let range = 52 + offset;
        ((self.qp_pred + self.cu_qp_delta + 52 + 2 * offset).rem_euclid(range)) - offset
    }
}
