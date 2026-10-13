// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The deblocking filter of clause 8.7.
//! Runs after the whole picture: intra prediction must see unfiltered samples.

use super::mb::Mb;
use super::tables::{ALPHA, BETA, TC0};

pub struct Plane<'a> {
    pub data: &'a mut [u8],
    pub at: usize,
    /// Twice the buffer's stride for a field.
    pub stride: usize,
}

pub struct Picture<'a> {
    pub mbs: &'a [Mb],
    pub mb_width: usize,
    pub mb_height: usize,
    /// A field picture: weakens horizontal macroblock edges and halves the vertical MV limit.
    pub field: bool,
}

pub fn filter(planes: &mut [Plane<'_>; 3], picture: &Picture<'_>) {
    for mb_y in 0..picture.mb_height {
        for mb_x in 0..picture.mb_width {
            macroblock(planes, picture, mb_x, mb_y);
        }
    }
}

fn neighbour(picture: &Picture<'_>, at: usize, other: usize, inside: bool) -> Option<usize> {
    if !inside {
        return None;
    }
    let mb = &picture.mbs[at];
    let mbn = &picture.mbs[other];
    if !mbn.coded() {
        return None;
    }
    if mb.deblock == 2 && mb.slice != mbn.slice {
        return None;
    }
    Some(other)
}

fn macroblock(planes: &mut [Plane<'_>; 3], picture: &Picture<'_>, mb_x: usize, mb_y: usize) {
    let at = mb_y * picture.mb_width + mb_x;
    let mb = picture.mbs[at];
    if !mb.coded() || mb.deblock == 1 {
        return;
    }
    let left = neighbour(picture, at, at.wrapping_sub(1), mb_x > 0);
    let above = neighbour(picture, at, at.wrapping_sub(picture.mb_width), mb_y > 0);

    // Strengths are worked out before any sample is touched, so their inputs are unfiltered.
    let mut vertical = [[0u8; 4]; 4];
    let mut horizontal = [[0u8; 4]; 4];
    for edge in 0..4 {
        for group in 0..4 {
            vertical[edge][group] = strength(picture, at, left, edge, group, true);
            horizontal[edge][group] = strength(picture, at, above, edge, group, false);
        }
    }

    for (vertical_edge, edges) in [(true, &vertical), (false, &horizontal)] {
        for (edge, strengths) in edges.iter().enumerate() {
            if strengths.iter().all(|&value| value == 0) {
                continue;
            }
            if mb.transform8x8 && edge % 2 == 1 {
                continue;
            }
            let other = if edge == 0 {
                match if vertical_edge { left } else { above } {
                    Some(other) => picture.mbs[other],
                    None => continue,
                }
            } else {
                mb
            };
            luma_edge(
                &mut planes[0],
                mb_x,
                mb_y,
                edge,
                vertical_edge,
                strengths,
                &mb,
                &other,
            );
            if edge % 2 == 0 {
                for component in 0..2 {
                    chroma_edge(
                        &mut planes[component + 1],
                        mb_x,
                        mb_y,
                        edge / 2,
                        vertical_edge,
                        strengths,
                        &mb,
                        &other,
                        component,
                    );
                }
            }
        }
    }
}

/// Clause 8.7.2.1: boundary strength for four samples.
fn strength(
    picture: &Picture<'_>,
    at: usize,
    other: Option<usize>,
    edge: usize,
    group: usize,
    vertical: bool,
) -> u8 {
    let mb = &picture.mbs[at];
    let mb_edge = edge == 0;
    let (q_mb, p_mb) = match (mb_edge, other) {
        (true, None) => return 0,
        (true, Some(other)) => (mb, &picture.mbs[other]),
        (false, _) => (mb, mb),
    };
    let (q_block, p_block) = if vertical {
        let q = group * 4 + edge;
        (q, if mb_edge { group * 4 + 3 } else { q - 1 })
    } else {
        let q = edge * 4 + group;
        (q, if mb_edge { 12 + group } else { q - 4 })
    };

    if mb_edge && (p_mb.kind.intra() || q_mb.kind.intra()) {
        return if picture.field && !vertical { 3 } else { 4 };
    }
    if p_mb.kind.intra() || q_mb.kind.intra() {
        return 3;
    }
    if p_mb.coded_block(p_block) || q_mb.coded_block(q_block) {
        return 2;
    }
    if moved(p_mb, p_block, q_mb, q_block, picture.field) {
        return 1;
    }
    0
}

/// Compares referenced pictures, not indices: the sides may be in slices with different lists.
fn moved(p_mb: &Mb, p_block: usize, q_mb: &Mb, q_block: usize, field: bool) -> bool {
    let limit = [4i32, if field { 2 } else { 4 }];
    let p_quarter = (p_block / 8) * 2 + (p_block % 4) / 2;
    let q_quarter = (q_block / 8) * 2 + (q_block % 4) / 2;
    let pics = |mb: &Mb, quarter: usize| [mb.pics[0][quarter], mb.pics[1][quarter]];
    let (p_pics, q_pics) = (pics(p_mb, p_quarter), pics(q_mb, q_quarter));
    let count = |pics: [i32; 2]| pics.iter().filter(|&&pic| pic >= 0).count();
    if count(p_pics) != count(q_pics) {
        return true;
    }
    let differs = |a: [i16; 2], b: [i16; 2]| {
        (i32::from(a[0]) - i32::from(b[0])).abs() >= limit[0]
            || (i32::from(a[1]) - i32::from(b[1])).abs() >= limit[1]
    };
    let mv = |mb: &Mb, list: usize, block: usize| mb.mv[list][block];

    match count(p_pics) {
        0 => false,
        1 => {
            let p_list = usize::from(p_pics[1] >= 0);
            let q_list = usize::from(q_pics[1] >= 0);
            p_pics[p_list] != q_pics[q_list]
                || differs(mv(p_mb, p_list, p_block), mv(q_mb, q_list, q_block))
        }
        _ => {
            if p_pics[0] == p_pics[1] {
                // Both lists name the same picture, so both pairings must be ruled out.
                if q_pics[0] != p_pics[0] || q_pics[1] != p_pics[0] {
                    return true;
                }
                let straight = differs(mv(p_mb, 0, p_block), mv(q_mb, 0, q_block))
                    || differs(mv(p_mb, 1, p_block), mv(q_mb, 1, q_block));
                let crossed = differs(mv(p_mb, 0, p_block), mv(q_mb, 1, q_block))
                    || differs(mv(p_mb, 1, p_block), mv(q_mb, 0, q_block));
                straight && crossed
            } else if p_pics == q_pics {
                differs(mv(p_mb, 0, p_block), mv(q_mb, 0, q_block))
                    || differs(mv(p_mb, 1, p_block), mv(q_mb, 1, q_block))
            } else if p_pics[0] == q_pics[1] && p_pics[1] == q_pics[0] {
                differs(mv(p_mb, 0, p_block), mv(q_mb, 1, q_block))
                    || differs(mv(p_mb, 1, p_block), mv(q_mb, 0, q_block))
            } else {
                true
            }
        }
    }
}

/// Clause 8.7.2.2.
fn thresholds(qp: i32, alpha_offset: i32, beta_offset: i32) -> (i32, i32, usize) {
    let index_a = (qp + alpha_offset).clamp(0, 51) as usize;
    let index_b = (qp + beta_offset).clamp(0, 51) as usize;
    (i32::from(ALPHA[index_a]), i32::from(BETA[index_b]), index_a)
}

#[allow(clippy::too_many_arguments)]
fn luma_edge(
    plane: &mut Plane<'_>,
    mb_x: usize,
    mb_y: usize,
    edge: usize,
    vertical: bool,
    strengths: &[u8; 4],
    mb: &Mb,
    other: &Mb,
) {
    let corner = plane.at + mb_y * 16 * plane.stride + mb_x * 16;
    let (across, along) = if vertical {
        (1usize, plane.stride)
    } else {
        (plane.stride, 1usize)
    };
    let q0 = corner + edge * 4 * across;
    let qp = (i32::from(mb.qpy) + i32::from(other.qpy) + 1) >> 1;
    let (alpha, beta, index_a) = thresholds(qp, i32::from(mb.alpha), i32::from(mb.beta));
    for (group, &strength) in strengths.iter().enumerate() {
        if strength == 0 {
            continue;
        }
        for sample in 0..4 {
            let at = q0 + (group * 4 + sample) * along;
            edge_samples(
                plane.data, at, across, strength, alpha, beta, index_a, false,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chroma_edge(
    plane: &mut Plane<'_>,
    mb_x: usize,
    mb_y: usize,
    edge: usize,
    vertical: bool,
    strengths: &[u8; 4],
    mb: &Mb,
    other: &Mb,
    component: usize,
) {
    let corner = plane.at + mb_y * 8 * plane.stride + mb_x * 8;
    let (across, along) = if vertical {
        (1usize, plane.stride)
    } else {
        (plane.stride, 1usize)
    };
    let q0 = corner + edge * 4 * across;
    let qp = (i32::from(mb.qpc[component]) + i32::from(other.qpc[component]) + 1) >> 1;
    let (alpha, beta, index_a) = thresholds(qp, i32::from(mb.alpha), i32::from(mb.beta));
    for sample in 0..8 {
        let strength = strengths[sample / 2];
        if strength == 0 {
            continue;
        }
        let at = q0 + sample * along;
        edge_samples(plane.data, at, across, strength, alpha, beta, index_a, true);
    }
}

/// Clauses 8.7.2.3 and 8.7.2.4. `at` is q0, `step` the distance across the edge.
#[allow(clippy::too_many_arguments)]
fn edge_samples(
    data: &mut [u8],
    at: usize,
    step: usize,
    strength: u8,
    alpha: i32,
    beta: i32,
    index_a: usize,
    chroma: bool,
) {
    let get = |data: &[u8], side: i32, index: i32| -> i32 {
        let offset = if side < 0 { -(index + 1) } else { index };
        i32::from(data[(at as i64 + i64::from(offset) * step as i64) as usize])
    };
    let p = |data: &[u8], index: i32| get(data, -1, index);
    let q = |data: &[u8], index: i32| get(data, 1, index);

    let (p0, p1, p2, p3) = (p(data, 0), p(data, 1), p(data, 2), p(data, 3));
    let (q0, q1, q2, q3) = (q(data, 0), q(data, 1), q(data, 2), q(data, 3));
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return;
    }
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0).abs();
    let mut put = |side: i32, index: i32, value: i32| {
        let offset = if side < 0 { -(index + 1) } else { index };
        data[(at as i64 + i64::from(offset) * step as i64) as usize] = value.clamp(0, 255) as u8;
    };

    if strength < 4 {
        let tc0 = i32::from(TC0[usize::from(strength) - 1][index_a]);
        let tc = if chroma {
            tc0 + 1
        } else {
            tc0 + i32::from(ap < beta) + i32::from(aq < beta)
        };
        let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        put(-1, 0, p0 + delta);
        put(1, 0, q0 - delta);
        if !chroma && ap < beta {
            put(
                -1,
                1,
                p1 + ((p2 + ((p0 + q0 + 1) >> 1) - (p1 << 1)) >> 1).clamp(-tc0, tc0),
            );
        }
        if !chroma && aq < beta {
            put(
                1,
                1,
                q1 + ((q2 + ((p0 + q0 + 1) >> 1) - (q1 << 1)) >> 1).clamp(-tc0, tc0),
            );
        }
        return;
    }

    let strong = (p0 - q0).abs() < ((alpha >> 2) + 2);
    if !chroma && ap < beta && strong {
        put(-1, 0, (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3);
        put(-1, 1, (p2 + p1 + p0 + q0 + 2) >> 2);
        put(-1, 2, (2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3);
    } else {
        put(-1, 0, (2 * p1 + p0 + q1 + 2) >> 2);
    }
    if !chroma && aq < beta && strong {
        put(1, 0, (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3);
        put(1, 1, (p0 + q0 + q1 + q2 + 2) >> 2);
        put(1, 2, (2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3);
    } else {
        put(1, 0, (2 * q1 + q0 + p1 + 2) >> 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::h264::mb::Kind;

    fn line(samples: [i32; 8], strength: u8, alpha: i32, beta: i32, index_a: usize) -> Vec<u8> {
        // p counts away from the edge, the buffer towards it.
        let mut data: Vec<u8> = samples.iter().map(|&value| value as u8).collect();
        edge_samples(&mut data, 4, 1, strength, alpha, beta, index_a, false);
        data
    }

    #[test]
    fn an_edge_that_is_already_smooth_is_left_alone() {
        let samples = [40, 41, 42, 43, 44, 45, 46, 47];
        let got = line(samples, 2, 15, 6, 30);
        assert_eq!(got, samples.map(|value| value as u8));
    }

    #[test]
    fn a_step_larger_than_the_threshold_is_left_alone() {
        let samples = [40, 40, 40, 40, 200, 200, 200, 200];
        let got = line(samples, 4, 15, 6, 30);
        assert_eq!(got, samples.map(|value| value as u8));
    }

    #[test]
    fn a_small_step_at_a_block_edge_is_smoothed_by_at_most_the_limit() {
        let samples = [40, 40, 40, 40, 48, 48, 48, 48];
        let got = line(samples, 1, 15, 6, 30);
        let tc0 = i32::from(TC0[0][30]);
        assert!(i32::from(got[3]) > 40 && i32::from(got[3]) <= 40 + tc0 + 2);
        assert!(i32::from(got[4]) < 48 && i32::from(got[4]) >= 48 - tc0 - 2);
        assert_eq!(got[0], 40);
        assert_eq!(got[7], 48);
    }

    #[test]
    fn the_strongest_filter_is_the_five_tap_of_equation_8_477() {
        let samples = [40, 41, 42, 43, 45, 46, 47, 48];
        let got = line(samples, 4, 15, 6, 30);
        let (p3, p2, p1, p0) = (40, 41, 42, 43);
        let (q0, q1, q2, q3) = (45, 46, 47, 48);
        assert_eq!(
            i32::from(got[3]),
            (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3
        );
        assert_eq!(i32::from(got[2]), (p2 + p1 + p0 + q0 + 2) >> 2);
        assert_eq!(i32::from(got[1]), (2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3);
        assert_eq!(
            i32::from(got[4]),
            (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3
        );
        assert_eq!(i32::from(got[5]), (p0 + q0 + q1 + q2 + 2) >> 2);
        assert_eq!(i32::from(got[6]), (2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3);
    }

    fn picture(mbs: &[Mb], field: bool) -> Picture<'_> {
        Picture {
            mbs,
            mb_width: 2,
            mb_height: 1,
            field,
        }
    }

    #[test]
    fn the_boundary_strength_table_is_the_one_of_clause_8_7_2_1() {
        let mut mbs = [Mb::default(), Mb::default()];
        for mb in &mut mbs {
            mb.kind = Kind::Inter;
            mb.slice = 0;
            mb.pics[0] = [0; 4];
            mb.refs[0] = [0; 4];
        }

        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 0);

        mbs[1].mv[0][0] = [4, 0];
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 1);
        mbs[1].mv[0][0] = [3, 0];
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 0);
        mbs[1].mv[0][0] = [0, 2];
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 0);
        assert_eq!(strength(&picture(&mbs, true), 1, Some(0), 0, 0, true), 1);
        mbs[1].mv[0][0] = [0, 0];

        mbs[1].pics[0][0] = 7;
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 1);
        mbs[1].pics[0][0] = -1;

        mbs[0].nnz[3] = 1;
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 2);
        mbs[0].nnz[3] = 0;

        mbs[0].kind = Kind::Intra16x16;
        assert_eq!(strength(&picture(&mbs, false), 1, Some(0), 0, 0, true), 4);
        assert_eq!(strength(&picture(&mbs, true), 1, Some(0), 0, 0, true), 4);
        assert_eq!(strength(&picture(&mbs, true), 1, Some(0), 0, 0, false), 3);
        mbs[1].kind = Kind::Intra16x16;
        assert_eq!(strength(&picture(&mbs, false), 1, None, 1, 0, true), 3);
    }
}
