// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! パン&スキャンのプリセット（`[[video.pan]]`）。比率のメニューに並ぶ。

use egui::{FontId, Rect, pos2, vec2};

use super::form::Form;
use super::{File, skin};
use crate::config::file::{PAN_PREFIX, Pan};

const NUMBER: f32 = 56.0;

pub(super) fn pans(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    form.section("パン&スキャン");
    let mut remove = None;
    let mut swap = None;
    let count = edited.video.pans.len();
    // 名前を変えたプリセット（前、後）。比率で選んでいれば追う。
    let mut renamed = None;
    for index in 0..count {
        if index == 0 {
            *form.at += 8.0;
        }
        let pan: &mut Pan = &mut edited.video.pans[index];
        let changed = loaded.video.pans.get(index) != Some(&*pan);
        let row = form.line(skin::SELECT_HEIGHT);
        let left = row.left() + 12.0;
        if changed {
            let colour = form.look.changed;
            form.ui.painter().circle_filled(
                pos2(row.left() - skin::DOT + 3.0, row.center().y),
                3.0,
                colour,
            );
        }
        let before = pan.name.clone();
        form.field(
            Rect::from_min_size(pos2(left, row.top()), vec2(180.0, row.height())),
            ("pan.name", index),
            &mut pan.name,
            "名前",
        );
        if pan.name != before {
            renamed = Some((before, pan.name.clone()));
        }
        let buttons = [
            ("上へ", 56.0, index > 0),
            ("下へ", 56.0, index + 1 < count),
            ("削除", 60.0, true),
        ];
        let mut right = row.right();
        for (word, width, enabled) in buttons.into_iter().rev() {
            right -= width;
            let rect = Rect::from_min_size(pos2(right, row.top()), vec2(width, row.height()));
            right -= 6.0;
            if form.button_enabled(rect, ("pan", word, index), word, enabled) {
                match word {
                    "上へ" => swap = Some((index - 1, index)),
                    "下へ" => swap = Some((index, index + 1)),
                    _ => remove = Some(index),
                }
            }
        }
        let row = form.line(skin::SELECT_HEIGHT);
        let mut at = row.left() + 26.0;
        let groups: [(&str, [(&str, &mut f32); 2]); 2] = [
            ("位置", [("横", &mut pan.x), ("縦", &mut pan.y)]),
            ("大きさ", [("横", &mut pan.width), ("縦", &mut pan.height)]),
        ];
        for (title, pair) in groups {
            at = said(form, at, row, title) + 4.0;
            for (label, value) in pair {
                at = said(form, at, row, label);
                form.number_box(
                    Rect::from_min_size(pos2(at, row.top()), vec2(NUMBER, row.height())),
                    value,
                    0.0..=100.0,
                );
                at += NUMBER + 4.0;
            }
            at = said(form, at, row, "%") + 8.0;
        }
        at = said(form, at, row, "比率") + 4.0;
        let [across, down] = &mut pan.aspect;
        at = said(form, at, row, "横");
        form.number_box(
            Rect::from_min_size(pos2(at, row.top()), vec2(NUMBER, row.height())),
            across,
            1..=10_000,
        );
        at += NUMBER + 4.0;
        at = said(form, at, row, "縦");
        form.number_box(
            Rect::from_min_size(pos2(at, row.top()), vec2(NUMBER, row.height())),
            down,
            1..=10_000,
        );
        *form.at += 6.0;
    }
    if let Some((before, after)) = renamed {
        let frame = &mut edited.video.frame;
        if frame.aspect.strip_prefix(PAN_PREFIX) == Some(before.as_str()) {
            frame.aspect = format!("{PAN_PREFIX}{after}");
        }
    }
    if let Some((a, b)) = swap {
        edited.video.pans.swap(a, b);
    }
    if let Some(index) = remove {
        edited.video.pans.remove(index);
    }
    let row = form.line(skin::SELECT_HEIGHT);
    let add = Rect::from_min_size(pos2(row.left() + 12.0, row.top()), vec2(72.0, row.height()));
    if form.button(add, ("pan.add", 0), "＋ 追加") {
        let mut n = edited.video.pans.len() + 1;
        let mut name = format!("プリセット {n}");
        while edited.video.pans.iter().any(|pan| pan.name == name) {
            n += 1;
            name = format!("プリセット {n}");
        }
        edited.video.pans.push(Pan {
            name,
            ..Pan::default()
        });
    }
}

/// 行 `row` の `at` に短い札を書き、その右端を返す。
fn said(form: &mut Form<'_, '_>, at: f32, row: Rect, text: &str) -> f32 {
    let colour = form.look.label;
    let rect = form.ui.painter().text(
        pos2(at, row.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(skin::TEXT_SIZE),
        colour,
    );
    rect.right() + 4.0
}

/// 比率の選択肢：決まった物とプリセット。
pub(super) fn aspect_choices(
    fixed: &[(&'static str, &'static str)],
    pans: &[Pan],
) -> Vec<(String, String)> {
    fixed
        .iter()
        .map(|(spelled, name)| ((*spelled).to_owned(), (*name).to_owned()))
        .chain(
            pans.iter()
                .filter(|pan| !pan.name.is_empty())
                .map(|pan| (format!("{PAN_PREFIX}{}", pan.name), pan.name.clone())),
        )
        .collect()
}
