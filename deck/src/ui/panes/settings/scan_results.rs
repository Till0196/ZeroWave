// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「スキャン」のページから開く別ウィンドウ：スキャン結果の削除の確認と、プロファイルからプロファイルへのコピー。

use std::collections::BTreeMap;

use egui::{FontId, Rect, pos2, vec2};

use super::{Form, Look, Pressed, State, skin};

pub const DELETE_WIDTH: f32 = 440.0;
pub const DELETE_HEIGHT: f32 = 230.0;
pub const COPY_WIDTH: f32 = 520.0;
pub const COPY_HEIGHT: f32 = 400.0;

#[derive(Debug, Clone)]
pub struct Deleting {
    pub profile: String,
    pub plan: String,
    pub gone: String,
    pub after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Copying {
    pub to: String,
    pub sources: Vec<String>,
    pub from: String,
    pub chosen: Vec<String>,
    pub results: BTreeMap<String, BTreeMap<String, String>>,
}

impl Copying {
    pub fn offered(&self) -> Vec<&'static str> {
        let had = self.results.get(&self.from);
        super::PLANS
            .iter()
            .copied()
            .filter(|plan| had.is_some_and(|had| had.contains_key(*plan)))
            .collect()
    }
}

pub fn results_of(profile: &str) -> BTreeMap<String, String> {
    let channels = crate::config::load_channels_of(profile);
    let reach = super::reach_in(&channels);
    channels
        .lines
        .iter()
        .map(|(plan, table)| {
            (
                plan.clone(),
                super::scan::result_text(table, reach.get(plan)),
            )
        })
        .collect()
}

pub fn show_delete(root: &mut egui::Ui, state: &State, look: &Look) -> Option<Pressed> {
    let Some(deleting) = state.scan_delete.clone() else {
        return Some(Pressed::Close);
    };
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let body = full.shrink2(vec2(22.0, 18.0));
    let painter = root.painter();
    let (system, _) = super::offered(&deleting.plan);
    let mut at = body.top();
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        format!(
            "{} の {system} のスキャン結果を削除しますか",
            deleting.profile
        ),
        FontId::proportional(skin::TEXT_SIZE + 2.0),
        look.text,
    );
    at += 28.0;
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        if deleting.after.is_some() {
            format!("削除すると、このプロファイルの {system} は all の結果を使います。")
        } else {
            format!("削除すると、{system} の局は一覧から無くなります。")
        },
        FontId::proportional(skin::TEXT_SIZE),
        look.label,
    );
    at += 28.0;
    let rows = [
        Some(("削除する", deleting.gone.as_str())),
        deleting.after.as_deref().map(|after| ("削除後", after)),
    ];
    let rows: Vec<(&str, &str)> = rows.into_iter().flatten().collect();
    let table = Rect::from_min_size(
        pos2(body.left(), at),
        vec2(body.width(), 10.0 + rows.len() as f32 * 26.0),
    );
    painter.rect_filled(table, skin::SELECT_ROUNDING, look.select_fill);
    painter.rect_stroke(
        table,
        skin::SELECT_ROUNDING,
        egui::Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    for (index, (label, value)) in rows.into_iter().enumerate() {
        let y = table.top() + 18.0 + index as f32 * 26.0;
        painter.text(
            pos2(table.left() + 12.0, y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(skin::TEXT_SIZE),
            look.label,
        );
        painter.text(
            pos2(table.left() + 76.0, y),
            egui::Align2::LEFT_CENTER,
            value,
            FontId::proportional(skin::TEXT_SIZE),
            look.text,
        );
    }
    let mut row_at = body.bottom() - skin::SELECT_HEIGHT;
    let mut form = Form::new(root, look, full, &mut row_at);
    let row = Rect::from_min_size(
        pos2(body.left(), body.bottom() - skin::SELECT_HEIGHT),
        vec2(body.width(), skin::SELECT_HEIGHT),
    );
    let mut pressed = None;
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 196.0, row.top()),
            vec2(92.0, row.height()),
        ),
        ("scan.delete.cancel", 0),
        "キャンセル",
    ) {
        pressed = Some(Pressed::DeleteScan(false));
    }
    if form.primary_button(
        Rect::from_min_size(
            pos2(row.right() - 96.0, row.top()),
            vec2(96.0, row.height()),
        ),
        ("scan.delete.go", 0),
        "削除する",
        true,
    ) {
        pressed = Some(Pressed::DeleteScan(true));
    }
    pressed
}

pub fn show_copy(root: &mut egui::Ui, state: &mut State, look: &Look) -> Option<Pressed> {
    let Some(copying) = state.scan_copy.as_mut() else {
        return Some(Pressed::Close);
    };
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let body = full.shrink2(vec2(22.0, 18.0));
    let mut at = body.top();
    let mut form = Form::new(root, look, full, &mut at);

    const LABEL: f32 = 80.0;
    let from_rect = Rect::from_min_size(
        pos2(body.left() + LABEL, *form.at),
        vec2(200.0, skin::SELECT_HEIGHT),
    );
    form.ui.painter().text(
        pos2(body.left(), from_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "複製元",
        FontId::proportional(skin::TEXT_SIZE),
        look.label,
    );
    let choices: Vec<(&str, &str)> = copying
        .sources
        .iter()
        .map(|name| (name.as_str(), name.as_str()))
        .collect();
    let before = copying.from.clone();
    form.select_at(
        from_rect,
        ("scan.copy.from", 0),
        &mut copying.from,
        &choices,
    );
    if copying.from != before {
        copying.chosen.clear();
    }
    *form.at = from_rect.bottom() + 16.0;
    form.ui.painter().text(
        pos2(body.left(), *form.at),
        egui::Align2::LEFT_CENTER,
        "複製先",
        FontId::proportional(skin::TEXT_SIZE),
        look.label,
    );
    form.ui.painter().text(
        pos2(body.left() + LABEL, *form.at),
        egui::Align2::LEFT_CENTER,
        copying.to.as_str(),
        FontId::proportional(skin::TEXT_SIZE),
        look.text,
    );
    *form.at += 18.0;

    const CHECK: f32 = 24.0;
    const SYSTEM: f32 = 136.0;
    let offered = copying.offered();
    let top = *form.at;
    let column = (body.width() - 20.0 - CHECK - SYSTEM) / 2.0;
    let left = body.left() + 10.0;
    let head_y = top + 13.0;
    for (x, text) in [
        (left + CHECK, "方式"),
        (left + CHECK + SYSTEM, "複製元"),
        (left + CHECK + SYSTEM + column, "複製先"),
    ] {
        form.ui.painter().text(
            pos2(x, head_y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
    }
    let mut y = top + 26.0;
    let none = BTreeMap::new();
    let from = copying.results.get(&copying.from).unwrap_or(&none).clone();
    let to = copying.results.get(&copying.to).unwrap_or(&none).clone();
    for plan in super::PLANS {
        let row = Rect::from_min_size(pos2(left, y), vec2(body.width() - 20.0, 30.0));
        form.ui
            .painter()
            .hline(row.x_range(), row.top(), egui::Stroke::new(1.0, look.edge));
        let on = copying.chosen.iter().any(|one| one == plan);
        let can = offered.contains(plan);
        let colour = if can { look.text } else { look.label };
        if can
            && form.tick(
                Rect::from_min_size(row.min, vec2(CHECK - 4.0, row.height())),
                ("scan.copy.plan", *plan),
                on,
            )
        {
            if on {
                copying.chosen.retain(|one| one != plan);
            } else {
                copying.chosen.push((*plan).to_owned());
            }
        }
        let (system, _) = super::offered(plan);
        let had = from.get(*plan).map_or("なし", String::as_str);
        let replaced = to.get(*plan).map(|was| format!("{was}（置き換え）"));
        for (x, text, tint) in [
            (left + CHECK, system, colour),
            (left + CHECK + SYSTEM, had, look.label),
            (
                left + CHECK + SYSTEM + column,
                replaced.as_deref().unwrap_or("なし"),
                if replaced.is_some() && on {
                    look.changed
                } else {
                    look.label
                },
            ),
        ] {
            form.fitted(
                pos2(x, row.center().y),
                text,
                column - 8.0,
                FontId::proportional(skin::TEXT_SIZE),
                tint,
            );
        }
        y += 30.0;
    }
    let frame = Rect::from_min_max(pos2(body.left(), top), pos2(body.right(), y + 4.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        egui::Stroke::new(1.0, look.button_edge),
        egui::StrokeKind::Inside,
    );
    form.ui.painter().text(
        pos2(body.left(), y + 20.0),
        egui::Align2::LEFT_CENTER,
        "複製先にある結果は置き換えます。",
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        look.label,
    );

    let row = Rect::from_min_size(
        pos2(body.left(), body.bottom() - skin::SELECT_HEIGHT),
        vec2(body.width(), skin::SELECT_HEIGHT),
    );
    let mut pressed = None;
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 196.0, row.top()),
            vec2(92.0, row.height()),
        ),
        ("scan.copy.cancel", 0),
        "キャンセル",
    ) {
        pressed = Some(Pressed::CopyScan(false));
    }
    let ready = copying
        .chosen
        .iter()
        .any(|plan| offered.contains(&plan.as_str()));
    if form.primary_button(
        Rect::from_min_size(
            pos2(row.right() - 96.0, row.top()),
            vec2(96.0, row.height()),
        ),
        ("scan.copy.go", 0),
        "複製する",
        ready,
    ) {
        pressed = Some(Pressed::CopyScan(true));
    }
    pressed
}
