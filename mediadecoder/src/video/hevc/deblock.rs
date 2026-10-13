// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The deblocking filter, clause 8.7.2. It is in the loop, so any difference drifts.
//! All vertical edges of the picture are filtered before any horizontal one.

use super::mv::{EDGE_PU_LEFT, EDGE_PU_TOP, EDGE_TU_LEFT, EDGE_TU_TOP, Unit};
use super::tables::{BETA, TC, chroma_qp};

#[derive(Debug, Clone, Copy, Default)]
pub struct Slice {
    pub disabled: bool,
    /// `slice_beta_offset_div2` and `slice_tc_offset_div2`, already doubled.
    pub beta_offset: i32,
    pub tc_offset: i32,
}

#[derive(Debug)]
pub struct Plane<'a> {
    pub samples: &'a mut [u16],
    pub stride: usize,
}

#[derive(Debug)]
pub struct Picture<'a> {
    pub units: &'a [Unit],
    pub unit_width: usize,
    pub width: usize,
    pub height: usize,
    pub depth: [u8; 2],
    /// `pps_cb_qp_offset` and `pps_cr_qp_offset`; slice-level offsets deliberately unused.
    pub chroma_offset: [i32; 2],
    pub ctb_log2: usize,
    pub ctb_width: usize,
    pub ctb_slice: &'a [u16],
    pub slices: &'a [Slice],
    pub pcm_loop_filter_disabled: bool,
}

impl Picture<'_> {
    fn unit(&self, x: usize, y: usize) -> Unit {
        self.units[(y >> 2) * self.unit_width + (x >> 2)]
    }

    fn slice_of(&self, x: usize, y: usize) -> Slice {
        let ctb = (y >> self.ctb_log2) * self.ctb_width + (x >> self.ctb_log2);
        let at = self.ctb_slice.get(ctb).copied().unwrap_or(0) as usize;
        self.slices.get(at).copied().unwrap_or_default()
    }

    /// Clause 8.7.2.4: boundary strength (2 intra, 1 coefficients or motion, 0 otherwise).
    fn strength(&self, p: Unit, q: Unit, transform_edge: bool) -> u8 {
        if p.intra || q.intra {
            return 2;
        }
        if transform_edge && (p.cbf || q.cbf) {
            return 1;
        }
        let count = |unit: Unit| (unit.flags & 1) + (unit.flags >> 1 & 1);
        if count(p) != count(q) {
            return 1;
        }
        let refs = |unit: Unit| -> [i32; 2] {
            let mut out = [i32::MIN; 2];
            let mut at = 0;
            for list in 0..2 {
                if unit.flags >> list & 1 == 1 {
                    out[at] = unit.poc[list];
                    at += 1;
                }
            }
            out.sort_unstable();
            out
        };
        let mvs = |unit: Unit| -> [[i16; 2]; 2] {
            let mut out = [[0i16; 2]; 2];
            let mut at = 0;
            for list in 0..2 {
                if unit.flags >> list & 1 == 1 {
                    out[at] = unit.mv[list];
                    at += 1;
                }
            }
            out
        };
        let p_refs = refs(p);
        let q_refs = refs(q);
        if p_refs != q_refs {
            return 1;
        }
        let far = |a: [i16; 2], b: [i16; 2]| (a[0] - b[0]).abs() >= 4 || (a[1] - b[1]).abs() >= 4;
        let p_mv = mvs(p);
        let q_mv = mvs(q);
        if count(p) == 1 {
            return u8::from(far(p_mv[0], q_mv[0]));
        }
        // Both bi-predicted from the same two pictures: filtered only if both pairings differ.
        let p_ordered = order_by_reference(p);
        let q_ordered = order_by_reference(q);
        if p_refs[0] != p_refs[1] {
            return u8::from(far(p_ordered[0], q_ordered[0]) || far(p_ordered[1], q_ordered[1]));
        }
        let straight = far(p_ordered[0], q_ordered[0]) || far(p_ordered[1], q_ordered[1]);
        let crossed = far(p_ordered[0], q_ordered[1]) || far(p_ordered[1], q_ordered[0]);
        u8::from(straight && crossed)
    }
}

fn order_by_reference(unit: Unit) -> [[i16; 2]; 2] {
    if unit.poc[0] <= unit.poc[1] {
        [unit.mv[0], unit.mv[1]]
    } else {
        [unit.mv[1], unit.mv[0]]
    }
}

pub fn filter(picture: &Picture, planes: &mut [Plane; 3]) {
    for vertical in [true, false] {
        luma(picture, &mut planes[0], vertical);
        for plane in 1..3 {
            let (_, right) = planes.split_at_mut(plane);
            chroma(picture, &mut right[0], plane - 1, vertical);
        }
    }
}

/// Clauses 8.7.2.5.1 and 8.7.2.5.2 for luma: every 8x8-grid edge, four lines at a time.
fn luma(picture: &Picture, plane: &mut Plane, vertical: bool) {
    let depth = picture.depth[0];
    let most = (1i32 << depth) - 1;
    let (step_x, step_y) = if vertical { (8usize, 4usize) } else { (4, 8) };
    let mut y = 0;
    while y < picture.height {
        let mut x = 0;
        while x < picture.width {
            if x == 0 && vertical || y == 0 && !vertical {
                x += step_x;
                continue;
            }
            let q = picture.unit(x, y);
            let (px, py) = if vertical { (x - 1, y) } else { (x, y - 1) };
            let p = picture.unit(px, py);
            let (tu, pu) = if vertical {
                (EDGE_TU_LEFT, EDGE_PU_LEFT)
            } else {
                (EDGE_TU_TOP, EDGE_PU_TOP)
            };
            if q.edge & (tu | pu) == 0 {
                x += step_x;
                continue;
            }
            let slice = picture.slice_of(x, y);
            if slice.disabled {
                x += step_x;
                continue;
            }
            let strength = picture.strength(p, q, q.edge & tu != 0);
            if strength == 0 {
                x += step_x;
                continue;
            }

            let qp = (i32::from(p.qp) + i32::from(q.qp) + 1) >> 1;
            let beta_index = (qp + slice.beta_offset).clamp(0, 51) as usize;
            let beta = BETA[beta_index] * (1 << (depth - 8));
            let tc_index =
                (qp + 2 * (i32::from(strength) - 1) + slice.tc_offset).clamp(0, 53) as usize;
            let tc = TC[tc_index] * (1 << (depth - 8));

            // The four lines are decided together from the first and last.
            let stride = plane.stride;
            let place = |i: i32, k: i32| -> usize {
                if vertical {
                    (x as i32 + i) as usize + (y as i32 + k) as usize * stride
                } else {
                    (x as i32 + k) as usize + (y as i32 + i) as usize * stride
                }
            };
            // Passed in rather than captured so the read does not hold a borrow across the write.
            let at = |samples: &[u16], i: i32, k: i32| -> i32 { i32::from(samples[place(i, k)]) };
            let dp0 = (at(plane.samples, -3, 0) - 2 * at(plane.samples, -2, 0)
                + at(plane.samples, -1, 0))
            .abs();
            let dp3 = (at(plane.samples, -3, 3) - 2 * at(plane.samples, -2, 3)
                + at(plane.samples, -1, 3))
            .abs();
            let dq0 = (at(plane.samples, 2, 0) - 2 * at(plane.samples, 1, 0)
                + at(plane.samples, 0, 0))
            .abs();
            let dq3 = (at(plane.samples, 2, 3) - 2 * at(plane.samples, 1, 3)
                + at(plane.samples, 0, 3))
            .abs();
            let dpq0 = dp0 + dq0;
            let dpq3 = dp3 + dq3;
            let dp = dp0 + dp3;
            let dq = dq0 + dq3;
            if dpq0 + dpq3 >= beta {
                x += step_x;
                continue;
            }
            let strong = |k: i32, dpq: i32| {
                dpq < (beta >> 2)
                    && (at(plane.samples, -4, k) - at(plane.samples, -1, k)).abs()
                        + (at(plane.samples, 0, k) - at(plane.samples, 3, k)).abs()
                        < (beta >> 3)
                    && (at(plane.samples, -1, k) - at(plane.samples, 0, k)).abs()
                        < (5 * tc + 1) >> 1
            };
            let strong = strong(0, 2 * dpq0) && strong(3, 2 * dpq3);
            let filter_p = dp < (beta + (beta >> 1)) >> 3;
            let filter_q = dq < (beta + (beta >> 1)) >> 3;
            let keep_p = p.bypass || (picture.pcm_loop_filter_disabled && p.pcm);
            let keep_q = q.bypass || (picture.pcm_loop_filter_disabled && q.pcm);

            for k in 0..4i32 {
                let mut sample = [0i32; 8];
                for (i, value) in sample.iter_mut().enumerate() {
                    *value = at(plane.samples, i as i32 - 4, k);
                }
                let p3 = sample[0];
                let p2 = sample[1];
                let p1 = sample[2];
                let p0 = sample[3];
                let q0 = sample[4];
                let q1 = sample[5];
                let q2 = sample[6];
                let q3 = sample[7];
                let mut out = sample;
                if strong {
                    let clip =
                        |value: i32, around: i32| value.clamp(around - 2 * tc, around + 2 * tc);
                    out[3] = clip((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3, p0);
                    out[2] = clip((p2 + p1 + p0 + q0 + 2) >> 2, p1);
                    out[1] = clip((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3, p2);
                    out[4] = clip((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3, q0);
                    out[5] = clip((p0 + q0 + q1 + q2 + 2) >> 2, q1);
                    out[6] = clip((p0 + q0 + q1 + 3 * q2 + 2 * q3 + 4) >> 3, q2);
                } else {
                    let delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
                    if delta.abs() >= tc * 10 {
                        continue;
                    }
                    let delta = delta.clamp(-tc, tc);
                    out[3] = (p0 + delta).clamp(0, most);
                    out[4] = (q0 - delta).clamp(0, most);
                    if filter_p {
                        let step = (((p2 + p0 + 1) >> 1) - p1 + delta) >> 1;
                        out[2] = (p1 + step.clamp(-(tc >> 1), tc >> 1)).clamp(0, most);
                    }
                    if filter_q {
                        let step = (((q2 + q0 + 1) >> 1) - q1 - delta) >> 1;
                        out[5] = (q1 + step.clamp(-(tc >> 1), tc >> 1)).clamp(0, most);
                    }
                }
                let count = if strong { 3usize } else { 2 };
                let mut writes = [(0usize, 0u16); 6];
                let mut written = 0usize;
                for i in 0..count as i32 {
                    if !keep_p {
                        writes[written] = (place(-1 - i, k), out[3 - i as usize] as u16);
                        written += 1;
                    }
                    if !keep_q {
                        writes[written] = (place(i, k), out[4 + i as usize] as u16);
                        written += 1;
                    }
                }
                for &(at, value) in &writes[..written] {
                    plane.samples[at] = value;
                }
            }
            x += step_x;
        }
        y += step_y;
    }
}

/// Clause 8.7.2.5.5: chroma is filtered only beside an intra block, one sample each side.
fn chroma(picture: &Picture, plane: &mut Plane, which: usize, vertical: bool) {
    let depth = picture.depth[1];
    let most = (1i32 << depth) - 1;
    let (step_x, step_y) = if vertical { (16usize, 8usize) } else { (8, 16) };
    let mut y = 0;
    while y < picture.height {
        let mut x = 0;
        while x < picture.width {
            if x == 0 && vertical || y == 0 && !vertical {
                x += step_x;
                continue;
            }
            let q = picture.unit(x, y);
            let (px, py) = if vertical { (x - 1, y) } else { (x, y - 1) };
            let p = picture.unit(px, py);
            let (tu, pu) = if vertical {
                (EDGE_TU_LEFT, EDGE_PU_LEFT)
            } else {
                (EDGE_TU_TOP, EDGE_PU_TOP)
            };
            if q.edge & (tu | pu) == 0 {
                x += step_x;
                continue;
            }
            let slice = picture.slice_of(x, y);
            if slice.disabled || picture.strength(p, q, q.edge & tu != 0) != 2 {
                x += step_x;
                continue;
            }
            let qpi = ((i32::from(p.qp) + i32::from(q.qp) + 1) >> 1) + picture.chroma_offset[which];
            let index = (chroma_qp(qpi.clamp(0, 51)) + 2 + slice.tc_offset).clamp(0, 53) as usize;
            let tc = TC[index] * (1 << (depth - 8));
            let keep_p = p.bypass || (picture.pcm_loop_filter_disabled && p.pcm);
            let keep_q = q.bypass || (picture.pcm_loop_filter_disabled && q.pcm);
            let (cx, cy) = (x / 2, y / 2);
            for k in 0..4usize {
                let at = |i: i32| -> (usize, usize) {
                    if vertical {
                        ((cx as i32 + i) as usize, cy + k)
                    } else {
                        (cx + k, (cy as i32 + i) as usize)
                    }
                };
                let read = |i: i32| {
                    let (sx, sy) = at(i);
                    i32::from(plane.samples[sx + sy * plane.stride])
                };
                let p1 = read(-2);
                let p0 = read(-1);
                let q0 = read(0);
                let q1 = read(1);
                let delta = ((((q0 - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
                if !keep_p {
                    let (sx, sy) = at(-1);
                    plane.samples[sx + sy * plane.stride] = (p0 + delta).clamp(0, most) as u16;
                }
                if !keep_q {
                    let (sx, sy) = at(0);
                    plane.samples[sx + sy * plane.stride] = (q0 - delta).clamp(0, most) as u16;
                }
            }
            x += step_x;
        }
        y += step_y;
    }
}

#[cfg(test)]
mod tests {
    use super::super::mv::Unit;
    use super::{Picture, Plane, Slice, filter};

    fn picture<'a>(units: &'a [Unit], slices: &'a [Slice]) -> Picture<'a> {
        Picture {
            units,
            unit_width: 16,
            width: 64,
            height: 64,
            depth: [8, 8],
            chroma_offset: [0, 0],
            ctb_log2: 6,
            ctb_width: 1,
            ctb_slice: &[0],
            slices,
            pcm_loop_filter_disabled: false,
        }
    }

    fn inter(poc: [i32; 2], mv: [[i16; 2]; 2], flags: u8) -> Unit {
        Unit {
            mv,
            poc,
            flags,
            intra: false,
            qp: 30,
            ..Unit::default()
        }
    }

    #[test]
    fn the_boundary_strength_is_clause_8_7_2_4() {
        let slices = [Slice::default()];
        let units = [Unit::default(); 256];
        let picture = picture(&units, &slices);

        let intra = Unit {
            intra: true,
            ..Unit::default()
        };
        let still = inter([4, 0], [[0, 0], [0, 0]], 1);
        assert_eq!(picture.strength(intra, still, false), 2);
        assert_eq!(picture.strength(still, intra, false), 2);

        let coded = Unit { cbf: true, ..still };
        assert_eq!(picture.strength(coded, still, true), 1);
        assert_eq!(picture.strength(coded, still, false), 0);

        let other = inter([8, 0], [[0, 0], [0, 0]], 1);
        assert_eq!(picture.strength(still, other, false), 1);

        let moved = inter([4, 0], [[4, 0], [0, 0]], 1);
        assert_eq!(picture.strength(still, moved, false), 1);
        let nearly = inter([4, 0], [[3, 0], [0, 0]], 1);
        assert_eq!(picture.strength(still, nearly, false), 0);
        let down = inter([4, 0], [[0, 4], [0, 0]], 1);
        assert_eq!(picture.strength(still, down, false), 1);

        let both = inter([4, 8], [[0, 0], [0, 0]], 3);
        assert_eq!(picture.strength(still, both, false), 1);

        let shifted = inter([4, 8], [[3, 0], [0, 3]], 3);
        assert_eq!(picture.strength(both, shifted, false), 0);
        let apart = inter([4, 8], [[4, 0], [0, 0]], 3);
        assert_eq!(picture.strength(both, apart, false), 1);

        let twice = inter([4, 4], [[0, 0], [8, 0]], 3);
        let swapped = inter([4, 4], [[8, 0], [0, 0]], 3);
        assert_eq!(picture.strength(twice, swapped, false), 0);
        let far = inter([4, 4], [[0, 0], [16, 0]], 3);
        assert_eq!(picture.strength(twice, far, false), 1);
    }

    #[test]
    fn a_slice_that_turns_the_filter_off_has_nothing_filtered() {
        let mut units = [Unit {
            intra: true,
            qp: 40,
            edge: super::EDGE_TU_LEFT | super::EDGE_TU_TOP,
            ..Unit::default()
        }; 256];
        units[0].edge = 0;
        let slices = [Slice {
            disabled: true,
            ..Slice::default()
        }];
        let picture = picture(&units, &slices);
        let mut samples: Vec<u16> = (0..64 * 64).map(|at| ((at * 37) % 200) as u16).collect();
        let before = samples.clone();
        let mut chroma = vec![128u16; 32 * 32];
        let mut other = vec![128u16; 32 * 32];
        let luma = samples.as_mut_slice();
        let mut planes = [
            Plane {
                samples: luma,
                stride: 64,
            },
            Plane {
                samples: &mut chroma,
                stride: 32,
            },
            Plane {
                samples: &mut other,
                stride: 32,
            },
        ];
        filter(&picture, &mut planes);
        assert_eq!(samples, before);
    }

    #[test]
    fn a_small_step_is_smoothed_and_a_large_one_is_not() {
        let units = [Unit {
            intra: true,
            qp: 37,
            edge: super::EDGE_TU_LEFT | super::EDGE_TU_TOP,
            ..Unit::default()
        }; 256];
        let slices = [Slice::default()];
        let picture = picture(&units, &slices);

        let run = |left: u16, right: u16| -> Vec<u16> {
            let mut samples: Vec<u16> = (0..64 * 64)
                .map(|at| if at % 64 < 8 { left } else { right })
                .collect();
            let mut cb = vec![128u16; 32 * 32];
            let mut cr = vec![128u16; 32 * 32];
            let mut planes = [
                Plane {
                    samples: &mut samples,
                    stride: 64,
                },
                Plane {
                    samples: &mut cb,
                    stride: 32,
                },
                Plane {
                    samples: &mut cr,
                    stride: 32,
                },
            ];
            filter(&picture, &mut planes);
            samples
        };

        let smoothed = run(100, 104);
        assert_ne!(smoothed[7], 100);
        assert_ne!(smoothed[8], 104);
        let kept = run(20, 220);
        assert_eq!(kept[7], 20);
        assert_eq!(kept[8], 220);
    }
}
