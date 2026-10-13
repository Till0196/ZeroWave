// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Intra prediction, residual coding and dequantisation.

use super::*;

impl Coder<'_> {
    /// Clause 8.4.4.2: reference samples, substitution, filtering, and prediction.
    pub(super) fn predict_intra(
        &mut self,
        plane: usize,
        x: usize,
        y: usize,
        log2: usize,
        mode: u8,
    ) {
        let size = 1usize << log2;
        let shift = usize::from(plane > 0);
        let depth = self.sps.depth[shift];
        let stride = self.target.stride[plane];
        let width = self.sps.width >> shift;
        let height = self.sps.height >> shift;
        {
            let neighbours = &mut self.buffers.neighbours;
            neighbours.begin(size);
        }
        let usable = |coder: &Self, nx: usize, ny: usize| -> bool {
            let (lx, ly) = ((nx << shift) as i32, (ny << shift) as i32);
            if !coder.predictor().available(x << shift, y << shift, lx, ly) {
                return false;
            }
            !coder.pps.constrained_intra_pred || coder.unit_at(lx as usize, ly as usize).intra
        };
        for i in 0..2 * size {
            if x > 0 && y + i < height && usable(self, x - 1, y + i) {
                let value = self.target.planes[plane][(x - 1) + (y + i) * stride];
                self.buffers.neighbours.set_left(i, value);
            }
        }
        if x > 0 && y > 0 && usable(self, x - 1, y - 1) {
            let value = self.target.planes[plane][(x - 1) + (y - 1) * stride];
            self.buffers.neighbours.set_corner(value);
        }
        for i in 0..2 * size {
            if y > 0 && x + i < width && usable(self, x + i, y - 1) {
                let value = self.target.planes[plane][(x + i) + (y - 1) * stride];
                self.buffers.neighbours.set_top(i, value);
            }
        }
        self.buffers.neighbours.substitute(depth);
        let filtered = self.buffers.neighbours.filter(
            usize::from(mode),
            plane > 0,
            depth,
            self.sps.strong_intra_smoothing,
        );
        let at = x + y * stride;
        intra::predict(
            &self.buffers.neighbours,
            usize::from(mode),
            plane > 0,
            depth,
            filtered,
            &mut self.target.planes[plane][at..],
            stride,
        );
    }

    pub(super) fn add_residual(&mut self, plane: usize, x: usize, y: usize, log2: usize) {
        let size = 1usize << log2;
        let shift = usize::from(plane > 0);
        let depth = self.sps.depth[shift];
        let most = (1i32 << depth) - 1;
        let stride = self.target.stride[plane];
        for row in 0..size {
            for column in 0..size {
                let at = (x + column) + (y + row) * stride;
                let value = i32::from(self.target.planes[plane][at])
                    + self.buffers.coefficients[column + row * size];
                self.target.planes[plane][at] = value.clamp(0, most) as u16;
            }
        }
    }

    /// Clauses 7.3.8.11 and 8.6: one transform block's coefficients and residual.
    pub(super) fn residual_coding(&mut self, x0: usize, y0: usize, log2: usize, cidx: usize) {
        let size = 1usize << log2;
        let count = size * size;
        self.buffers.coefficients[..count].fill(0);

        let mut skip = false;
        if self.pps.transform_skip_enabled && !self.cu.bypass && log2 <= 2 {
            skip = self
                .cabac
                .decision(ctx::TRANSFORM_SKIP + usize::from(cidx > 0))
                == 1;
        }

        // Clause 7.4.9.11: small intra blocks scan along the prediction direction.
        let mut scan_idx = 0usize;
        if self.cu.intra && (log2 == 2 || (log2 == 3 && cidx == 0)) {
            let mode = if cidx == 0 {
                self.intra_mode_at(x0, y0)
            } else {
                self.cu.chroma_mode
            };
            scan_idx = match mode {
                6..=14 => 2,
                22..=30 => 1,
                _ => 0,
            };
        }

        // Last significant position: both prefixes come before either suffix.
        let mut prefix = [0usize; 2];
        let mut last = [0usize; 2];
        for (which, value) in prefix.iter_mut().enumerate() {
            let base = if which == 0 { ctx::LAST_X } else { ctx::LAST_Y };
            let (offset, shift) = if cidx == 0 {
                (3 * (log2 - 2) + ((log2 - 1) >> 2), (log2 + 1) >> 2)
            } else {
                (15, log2 - 2)
            };
            let most = (log2 << 1) - 1;
            let mut count = 0usize;
            while count < most && self.cabac.decision(base + offset + (count >> shift)) == 1 {
                count += 1;
            }
            *value = count;
        }
        for (which, value) in last.iter_mut().enumerate() {
            let count = prefix[which];
            *value = if count > 3 {
                let bits = (count >> 1) - 1;
                let suffix = self.cabac.bypass_bits(bits as u32) as usize;
                (1 << bits) * (2 + (count & 1)) + suffix
            } else {
                count
            };
        }
        if scan_idx == 2 {
            last.swap(0, 1);
        }

        let sub_log2 = log2 - 2;
        let sub_scan = scan_order(sub_log2, scan_idx);
        let coefficient_scan = &SCAN_4[scan_idx];
        let mut last_sub = (1usize << (2 * sub_log2)) - 1;
        let mut last_pos = 16usize;
        loop {
            if last_pos == 0 {
                last_pos = 16;
                if last_sub == 0 {
                    break;
                }
                last_sub -= 1;
            }
            last_pos -= 1;
            let xs = sub_scan[last_sub][0] as usize;
            let ys = sub_scan[last_sub][1] as usize;
            let xc = (xs << 2) + coefficient_scan[last_pos][0] as usize;
            let yc = (ys << 2) + coefficient_scan[last_pos][1] as usize;
            if xc == last[0] && yc == last[1] {
                break;
            }
        }

        let sub_side = 1usize << sub_log2;
        let mut coded = [false; 64];
        coded[sub_scan[last_sub][1] as usize * 8 + sub_scan[last_sub][0] as usize] = true;
        coded[0] = true;
        let mut greater1_ctx = 1u32;
        let mut ctx_set;
        let mut first_sub = true;

        for i in (0..=last_sub).rev() {
            let xs = sub_scan[i][0] as usize;
            let ys = sub_scan[i][1] as usize;
            let mut infer_dc = false;
            if i < last_sub && i > 0 {
                let mut context = 0usize;
                if xs + 1 < sub_side && coded[ys * 8 + xs + 1] {
                    context += 1;
                }
                if ys + 1 < sub_side && coded[(ys + 1) * 8 + xs] {
                    context += 1;
                }
                let context = context.min(1) + if cidx > 0 { 2 } else { 0 };
                coded[ys * 8 + xs] = self.cabac.decision(ctx::CODED_SUB_BLOCK + context) == 1;
                infer_dc = true;
            }
            if !coded[ys * 8 + xs] {
                continue;
            }

            let mut positions = [0usize; 16];
            let mut found = 0usize;
            let first = if i == last_sub { last_pos } else { 15 };
            if i == last_sub {
                positions[found] = last_pos;
                found += 1;
            }
            let start = if i == last_sub {
                if last_pos == 0 {
                    None
                } else {
                    Some(last_pos - 1)
                }
            } else {
                Some(first)
            };
            if let Some(start) = start {
                for n in (0..=start).rev() {
                    if n == 0 && infer_dc && found == 0 {
                        positions[found] = 0;
                        found += 1;
                        break;
                    }
                    let xc = (xs << 2) + coefficient_scan[n][0] as usize;
                    let yc = (ys << 2) + coefficient_scan[n][1] as usize;
                    let context =
                        self.sig_context(cidx, log2, scan_idx, xc, yc, xs, ys, sub_side, &coded);
                    if self.cabac.decision(ctx::SIG_COEFF + context) == 1 {
                        positions[found] = n;
                        found += 1;
                        infer_dc = false;
                    }
                }
            }
            if found == 0 {
                continue;
            }

            // Clause 9.3.4.2.6.
            ctx_set = if i > 0 && cidx == 0 { 2 } else { 0 };
            if !first_sub && greater1_ctx == 0 {
                ctx_set += 1;
            }
            first_sub = false;
            greater1_ctx = 1;
            let base = if cidx == 0 {
                ctx::GREATER1
            } else {
                ctx::GREATER1 + 16
            };
            let mut greater1 = [false; 16];
            let mut last_greater1 = usize::MAX;
            for (k, flag) in greater1.iter_mut().enumerate().take(found.min(8)) {
                let bin = self
                    .cabac
                    .decision(base + ctx_set * 4 + greater1_ctx.min(3) as usize)
                    == 1;
                *flag = bin;
                if bin {
                    if last_greater1 == usize::MAX {
                        last_greater1 = k;
                    }
                    greater1_ctx = 0;
                } else if greater1_ctx > 0 && greater1_ctx < 3 {
                    greater1_ctx += 1;
                }
            }

            let mut greater2 = false;
            if last_greater1 != usize::MAX {
                let base = if cidx == 0 {
                    ctx::GREATER2
                } else {
                    ctx::GREATER2 + 4
                };
                greater2 = self.cabac.decision(base + ctx_set) == 1;
            }

            // Sign data hiding.
            let hidden = self.pps.sign_data_hiding
                && !self.cu.bypass
                && positions[0] - positions[found - 1] > 3;
            let mut signs = [false; 16];
            for (k, sign) in signs.iter_mut().take(found).enumerate() {
                if hidden && k == found - 1 {
                    continue;
                }
                *sign = self.cabac.bypass() == 1;
            }

            let mut rice = 0u32;
            let mut sum = 0u32;
            for (k, &n) in positions[..found].iter().enumerate() {
                let written = k;
                let mut level = 1u32
                    + u32::from(k < 8 && greater1[k])
                    + u32::from(k == last_greater1 && greater2);
                let threshold = if written < 8 {
                    if k == last_greater1 { 3 } else { 2 }
                } else {
                    1
                };
                if level == threshold {
                    let remaining = self.read_remaining(rice);
                    level += remaining;
                    // Clause 9.3.3.11: Rice parameter adaptation.
                    if level > (3 << rice) {
                        rice = (rice + 1).min(4);
                    }
                }
                sum += level;
                let negative = signs[k] ^ (hidden && k == found - 1 && sum % 2 == 1);
                let xc = (xs << 2) + coefficient_scan[n][0] as usize;
                let yc = (ys << 2) + coefficient_scan[n][1] as usize;
                let value = level as i32;
                self.buffers.coefficients[xc + yc * size] = if negative { -value } else { value };
            }
        }

        self.dequantise(log2, cidx, skip);
    }

    /// Clause 9.3.3.11: `coeff_abs_level_remaining`.
    pub(super) fn read_remaining(&mut self, rice: u32) -> u32 {
        let mut prefix = 0u32;
        while prefix < 4 && self.cabac.bypass() == 1 {
            prefix += 1;
        }
        if prefix < 4 {
            return (prefix << rice) + self.cabac.bypass_bits(rice);
        }
        (4 << rice) + self.cabac.golomb(rice + 1)
    }

    /// Clause 9.3.4.2.5: the significance flag's context.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sig_context(
        &self,
        cidx: usize,
        log2: usize,
        scan_idx: usize,
        xc: usize,
        yc: usize,
        xs: usize,
        ys: usize,
        sub_side: usize,
        coded: &[bool; 64],
    ) -> usize {
        if log2 == 2 {
            const MAP: [usize; 16] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8];
            let base = MAP[(yc << 2) + xc];
            return if cidx == 0 { base } else { 27 + base };
        }
        if xc + yc == 0 {
            return if cidx == 0 { 0 } else { 27 };
        }
        let mut previous = 0usize;
        if xs + 1 < sub_side && coded[ys * 8 + xs + 1] {
            previous += 1;
        }
        if ys + 1 < sub_side && coded[(ys + 1) * 8 + xs] {
            previous += 2;
        }
        let xp = xc & 3;
        let yp = yc & 3;
        let mut sig = match previous {
            0 => {
                if xp + yp == 0 {
                    2
                } else if xp + yp < 3 {
                    1
                } else {
                    0
                }
            }
            1 => {
                if yp == 0 {
                    2
                } else if yp == 1 {
                    1
                } else {
                    0
                }
            }
            2 => {
                if xp == 0 {
                    2
                } else if xp == 1 {
                    1
                } else {
                    0
                }
            }
            _ => 2,
        };
        if cidx == 0 {
            if xs + ys > 0 {
                sig += 3;
            }
            sig += if log2 == 3 {
                if scan_idx == 0 { 9 } else { 15 }
            } else {
                21
            };
            sig
        } else {
            sig += if log2 == 3 { 9 } else { 12 };
            27 + sig
        }
    }

    pub(super) fn dequantise(&mut self, log2: usize, cidx: usize, skip: bool) {
        let count = 1usize << (2 * log2);
        if self.cu.bypass {
            return;
        }
        let shift = usize::from(cidx > 0);
        let depth = self.sps.depth[shift];
        let qp = self.transform_qp(cidx);
        let matrix = if self.sps.scaling_list_enabled && !(skip && log2 > 2) {
            let matrix_id = if self.cu.intra { 0 } else { 3 } + cidx;
            Some(self.scaling.matrix(log2 - 2, matrix_id))
        } else {
            None
        };
        transform::scale(
            &mut self.buffers.coefficients[..count],
            log2,
            qp,
            depth,
            matrix,
        );
        if skip {
            transform::skipped(&mut self.buffers.coefficients, log2, depth);
        } else {
            let sine = self.cu.intra && log2 == 2 && cidx == 0;
            transform::inverse(&mut self.buffers.coefficients, log2, sine, depth);
        }
    }

    /// Clause 8.6.1: the QP a transform block is scaled by (chroma through Table 8-10).
    pub(super) fn transform_qp(&self, cidx: usize) -> i32 {
        if cidx == 0 {
            return (self.qp + self.sps.qp_offset(false)).clamp(0, 51 + self.sps.qp_offset(false));
        }
        let offset = self.pps.qp_offset[cidx - 1] + self.header.slice_qp_offset[cidx - 1];
        let chroma_offset = self.sps.qp_offset(true);
        let qpi = (self.qp + offset).clamp(-chroma_offset, 57);
        chroma_qp(qpi) + chroma_offset
    }
}
