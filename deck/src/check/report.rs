// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 突き合わせの結果を数え、どこが違うかを言葉にする。

use super::*;

fn shot_luma(shot: &Shot, shift: u32) -> zwcore::decode::Luma<'_> {
    zwcore::decode::Luma {
        bytes: &shot.luma,
        pitch: shot.pitch,
        shift,
    }
}

#[derive(Debug, Clone)]
pub(super) struct Verdict {
    pub(super) worst: f64,
    pub(super) mean: f64,
    pub(super) counted: usize,
    pub(super) ordered: bool,
    pub(super) held_back: usize,
    pub(super) every: Vec<(u64, f64)>,
    pub(super) poorest: Vec<(u64, f64)>,
}

pub(super) fn compare(
    hardware: &Pass,
    software: &Pass,
    units: &[Cutout],
    format: Format,
) -> Verdict {
    // Order must match too; see [`Verdict::held_back`].
    let held_back = hardware
        .shots
        .first()
        .and_then(|first| software.shots.iter().position(|shot| shot.tag == first.tag))
        .unwrap_or(0);
    let common = hardware.shots.len().min(software.shots.len() - held_back);
    let ordered = common > 0
        && (0..common).all(|at| hardware.shots[at].tag == software.shots[held_back + at].tag);
    let entry: std::collections::BTreeSet<u64> = units
        .iter()
        .filter(|unit| unit.picture.random_access)
        .map(|unit| unit.tag)
        .collect();
    let leading = hardware
        .shots
        .iter()
        .position(|shot| entry.contains(&shot.tag))
        .unwrap_or(0);

    let by_tag: std::collections::BTreeMap<u64, &Shot> =
        software.shots.iter().map(|shot| (shot.tag, shot)).collect();
    let mut worst = f64::INFINITY;
    let mut total = 0f64;
    let mut counted = 0usize;
    let mut finite = 0usize;
    let mut each: Vec<(u64, f64)> = Vec::new();
    for shot in hardware.shots[leading..]
        .iter()
        .take(common.saturating_sub(leading))
    {
        let Some(mine) = by_tag.get(&shot.tag) else {
            continue;
        };
        let found = zwcore::decode::psnr(
            shot_luma(mine, 0),
            shot_luma(
                shot,
                if format.depth > 8 {
                    16 - u32::from(format.depth)
                } else {
                    0
                },
            ),
            format,
        );
        each.push((shot.tag, found));
        worst = worst.min(found);
        if found.is_finite() {
            total += found;
            finite += 1;
        }
        counted += 1;
    }
    Verdict {
        worst,
        mean: if finite > 0 {
            total / finite as f64
        } else {
            worst
        },
        counted,
        ordered,
        held_back,
        every: each.clone(),
        poorest: {
            each.sort_by(|left, right| left.1.total_cmp(&right.1));
            each.truncate(5);
            each
        },
    }
}

/// Where the two decoders disagree: a band points at slices, scattered blocks at prediction.
pub(super) fn where_wrong(mine: &Shot, theirs: &Shot, format: Format) -> String {
    let wide = format.depth > 8;
    let shift = if wide {
        16 - u32::from(format.depth)
    } else {
        0
    };
    let sample = |shot: &Shot, x: usize, y: usize, shifted: u32| -> i64 {
        let at = y * shot.pitch + x * if wide { 2 } else { 1 };
        if wide {
            let low = u32::from(shot.luma.get(at).copied().unwrap_or(0));
            let high = u32::from(shot.luma.get(at + 1).copied().unwrap_or(0));
            i64::from((low | (high << 8)) >> shifted)
        } else {
            i64::from(shot.luma.get(at).copied().unwrap_or(0))
        }
    };
    let across = (format.width as usize).div_ceil(16);
    let down = (format.height as usize).div_ceil(16);
    let mut bad = 0usize;
    let mut gross = 0usize;
    let mut rows: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut worst = 0i64;
    // Error on one line parity only means fields were predicted unalike.
    let mut parity = [0f64, 0f64];
    for row in 0..down {
        for column in 0..across {
            let mut most = 0i64;
            for y in row * 16..((row + 1) * 16).min(format.height as usize) {
                for x in column * 16..((column + 1) * 16).min(format.width as usize) {
                    let difference = (sample(mine, x, y, 0) - sample(theirs, x, y, shift)).abs();
                    most = most.max(difference);
                    parity[y & 1] += (difference * difference) as f64;
                }
            }
            if most > 1 {
                bad += 1;
                rows.insert(row);
            }
            if most > 16 {
                gross += 1;
            }
            worst = worst.max(most);
        }
    }
    let lines = (format.height as f64) / 2.0 * (format.width as f64);
    format!(
        "違うMB {bad}/{} （大きく違う {gross}）、MB行 {}/{down} 、最大の差 {worst}、\
         偶数行 {:.2} / 奇数行 {:.2}",
        across * down,
        rows.len(),
        (parity[0] / lines).sqrt(),
        (parity[1] / lines).sqrt(),
    )
}

pub(super) fn copied(
    bad: &Shot,
    before: Option<&Shot>,
    after: Option<&Shot>,
    format: Format,
) -> String {
    let wide = format.depth > 8;
    let shift = if wide {
        16 - u32::from(format.depth)
    } else {
        0
    };
    let sample = |shot: &Shot, x: usize, y: usize, shifted: u32| -> i64 {
        let at = y * shot.pitch + x * if wide { 2 } else { 1 };
        if wide {
            let low = u32::from(shot.luma.get(at).copied().unwrap_or(0));
            let high = u32::from(shot.luma.get(at + 1).copied().unwrap_or(0));
            i64::from((low | (high << 8)) >> shifted)
        } else {
            i64::from(shot.luma.get(at).copied().unwrap_or(0))
        }
    };
    let across = (format.width as usize).div_ceil(16);
    let down = (format.height as usize).div_ceil(16);
    let mut like = [0usize; 2];
    let mut counted = 0usize;
    for row in 0..down {
        for column in 0..across {
            let mut most = [0i64; 2];
            for y in row * 16..((row + 1) * 16).min(format.height as usize) {
                for x in column * 16..((column + 1) * 16).min(format.width as usize) {
                    let ours = sample(bad, x, y, shift);
                    for (at, other) in [before, after].into_iter().enumerate() {
                        if let Some(other) = other {
                            most[at] = most[at].max((ours - sample(other, x, y, 0)).abs());
                        } else {
                            most[at] = i64::MAX;
                        }
                    }
                }
            }
            if most[0] <= 1 || most[1] <= 1 {
                counted += 1;
                if most[0] <= most[1] {
                    like[0] += 1;
                } else {
                    like[1] += 1;
                }
            }
        }
    }
    format!(
        "参照そのままの MB {counted}（前方 {} / 後方 {}）",
        like[0], like[1]
    )
}

pub(super) fn spread(best: &Verdict, units: &[Cutout]) -> String {
    let by_tag: std::collections::BTreeMap<u64, &Picture> =
        units.iter().map(|unit| (unit.tag, &unit.picture)).collect();
    let mut kinds: std::collections::BTreeMap<String, (usize, f64, f64, usize)> =
        std::collections::BTreeMap::new();
    for (tag, found) in &best.every {
        if !found.is_finite() {
            continue;
        }
        let name = by_tag.get(tag).map_or_else(
            || String::from("?"),
            |picture| format!("{:?}", picture.coding),
        );
        let entry = kinds
            .entry(name)
            .or_insert((0, f64::INFINITY, f64::NEG_INFINITY, 0));
        entry.0 += 1;
        entry.1 = entry.1.min(*found);
        entry.2 = entry.2.max(*found);
        if *found < MPEG2_FLOOR {
            entry.3 += 1;
        }
    }
    kinds
        .into_iter()
        .map(|(name, (count, worst, best, poor))| {
            format!(
                "{name} {count}枚 {worst:.1}〜{best:.1}{}",
                match poor {
                    0 => String::new(),
                    poor => format!("（{MPEG2_FLOOR:.0} dB 未満 {poor}枚）"),
                }
            )
        })
        .collect::<Vec<_>>()
        .join("、")
}

pub(super) fn poorest(best: &Verdict, units: &[Cutout]) -> Option<String> {
    let by_tag: std::collections::BTreeMap<u64, &Picture> =
        units.iter().map(|unit| (unit.tag, &unit.picture)).collect();
    let listed: Vec<String> = best
        .poorest
        .iter()
        .filter(|(_, found)| found.is_finite())
        .map(|(tag, found)| match by_tag.get(tag) {
            Some(picture) => format!(
                "tag {tag} {found:.1} dB（{:?} {:?}{}）",
                picture.coding,
                picture.structure,
                if picture.repeat_first_field {
                    " RFF"
                } else {
                    ""
                }
            ),
            None => format!("tag {tag} {found:.1} dB"),
        })
        .collect();
    (!listed.is_empty()).then(|| listed.join("、"))
}

/// Says how two orderings differ (one extra picture shifts everything after it).
pub(super) fn divergence(hardware: &Pass, software: &Pass) {
    let tags = |pass: &Pass| pass.shots.iter().map(|shot| shot.tag).collect::<Vec<_>>();
    let (left, right) = (tags(hardware), tags(software));
    let at = (0..left.len().min(right.len()))
        .find(|at| left[*at] != right[*at])
        .unwrap_or(left.len().min(right.len()));
    let window = |tags: &[u64]| {
        tags.iter()
            .skip(at)
            .take(4)
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "    位置 {at} で分かれます: ハード [{}] / ソフト [{}]",
        window(&left),
        window(&right)
    );
    let only = |these: &[u64], those: &[u64]| {
        these
            .iter()
            .filter(|tag| !those.contains(tag))
            .map(|tag| tag.to_string())
            .collect::<Vec<_>>()
    };
    let extra = only(&left, &right);
    let missing = only(&right, &left);
    if extra.is_empty() && missing.is_empty() {
        println!("    出した枚数は同じで、並べ替えだけが違います");
        return;
    }
    if !extra.is_empty() {
        println!(
            "    ハードだけが出した {} 枚: [{}]",
            extra.len(),
            extra.join(", ")
        );
    }
    if !missing.is_empty() {
        println!(
            "    ソフトだけが出した {} 枚: [{}]",
            missing.len(),
            missing.join(", ")
        );
    }
}
