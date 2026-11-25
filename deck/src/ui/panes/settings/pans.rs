// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! パン&スキャンのプリセット（`[[video.pan]]`）。比率のメニューに並ぶ。

use egui::{FontId, Rect, Stroke, pos2, vec2};

use super::form::Form;
use super::{File, skin};
use crate::config::file::{PAN_PREFIX, Pan};
use crate::ui::icons::Icon;

const NUMBER: f32 = 44.0;
const BUTTON: f32 = 26.0;
const GAP: f32 = 10.0;
/// 比率の二つの欄の間の「:」の幅。
const COLON: f32 = 12.0;

/// プリセット一つを一行にした表。ソースのプロファイルの表と同じ形。
pub(super) fn pans(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    form.section("パン&スキャン");
    let mut remove = None;
    let mut swap = None;
    let count = edited.video.pans.len();
    // 名前を変えたプリセット（前、後）。比率で選んでいれば追う。
    let mut renamed = None;
    let top = *form.at;
    let head = form.line(26.0);
    let pair = NUMBER * 2.0 + 4.0;
    let delete_left = head.right() - 10.0 - BUTTON;
    let order_left = delete_left - 6.0 - (BUTTON * 2.0 + 4.0);
    let aspect_left = order_left - GAP - (NUMBER * 2.0 + COLON);
    let size_left = aspect_left - GAP - pair;
    let place_left = size_left - GAP - pair;
    let name_left = head.left() + 10.0;
    let name_width = (place_left - GAP - name_left).max(60.0);
    for (x, text) in [
        (name_left, "名前"),
        (place_left, "位置 横・縦 %"),
        (size_left, "大きさ 横・縦 %"),
        (aspect_left, "比率 横:縦"),
        (order_left, "順番"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    if count == 0 {
        let row = form.line(30.0);
        hline(form, row);
        form.ui.painter().text(
            pos2(name_left, row.center().y),
            egui::Align2::LEFT_CENTER,
            "プリセットはありません",
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
    }
    for (index, pan) in edited.video.pans.iter_mut().enumerate() {
        let row = form.line(30.0);
        hline(form, row);
        if loaded.video.pans.get(index) != Some(&*pan) {
            let colour = form.look.changed;
            form.ui.painter().circle_filled(
                pos2(row.left() - skin::DOT + 3.0, row.center().y),
                3.0,
                colour,
            );
        }
        let cell = |x: f32, width: f32| {
            Rect::from_min_size(pos2(x, row.center().y - 13.0), vec2(width, 26.0))
        };
        let before = pan.name.clone();
        form.field(
            cell(name_left, name_width),
            ("pan.name", index),
            &mut pan.name,
            "名前",
        );
        if pan.name != before {
            renamed = Some((before, pan.name.clone()));
        }
        for (x, [first, second]) in [
            (place_left, [&mut pan.x, &mut pan.y]),
            (size_left, [&mut pan.width, &mut pan.height]),
        ] {
            form.number_box(cell(x, NUMBER), first, 0.0..=100.0);
            form.number_box(cell(x + NUMBER + 4.0, NUMBER), second, 0.0..=100.0);
        }
        let [across, down] = &mut pan.aspect;
        form.number_box(cell(aspect_left, NUMBER), across, 1..=10_000);
        form.ui.painter().text(
            pos2(aspect_left + NUMBER + COLON / 2.0, row.center().y),
            egui::Align2::CENTER_CENTER,
            ":",
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
        form.number_box(cell(aspect_left + NUMBER + COLON, NUMBER), down, 1..=10_000);
        let up = cell(order_left, BUTTON);
        if form.button_enabled(up, ("pan.up", index), Icon::ArrowUp, index > 0) {
            swap = Some((index - 1, index));
        }
        let down = cell(up.right() + 4.0, BUTTON);
        if form.button_enabled(
            down,
            ("pan.down", index),
            Icon::ArrowDown,
            index + 1 < count,
        ) {
            swap = Some((index, index + 1));
        }
        if form.button(
            cell(delete_left, BUTTON),
            ("pan.remove", index),
            Icon::Cross,
        ) {
            remove = Some(index);
        }
    }
    let frame = Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), *form.at - 2.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, form.look.button_edge),
        egui::StrokeKind::Inside,
    );
    *form.at += 4.0;
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
    let add = Rect::from_min_size(row.min, vec2(80.0, row.height()));
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

/// 行の上の区切り線。1 行目では見出しとの境になる。
fn hline(form: &Form<'_, '_>, row: Rect) {
    form.ui.painter().hline(
        row.x_range(),
        row.top() - 3.0,
        Stroke::new(1.0, form.look.edge),
    );
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
