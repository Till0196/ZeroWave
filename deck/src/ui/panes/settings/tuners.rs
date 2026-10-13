// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「チューナーの状態」の別ウィンドウ。チューナーごとにテーブルを一つ。「更新」で問い合わせ直す。

use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};

use super::scan::{boards_answer, system_name, systems_of};
use super::{File, Form, Look, Pressed, State, skin};

pub const WIDTH: f32 = 680.0;
pub const HEIGHT: f32 = 420.0;

const FOOT: f32 = 46.0;

/// 状態の列は中身を真ん中に置く。
const COLUMNS: [(f32, &str); 5] = [
    (0.0, "チューナー"),
    (110.0, "状態"),
    (188.0, "受信中"),
    (0.0, "信号"),
    (0.0, "受信方式"),
];
const STATE_WIDTH: f32 = 70.0;
const SIGNAL_WIDTH: f32 = 96.0;
const SYSTEMS_WIDTH: f32 = 96.0;

pub fn show(root: &mut egui::Ui, state: &mut State, edited: &File, look: &Look) -> Option<Pressed> {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let body =
        Rect::from_min_max(full.min, pos2(full.right(), foot.top())).shrink2(vec2(16.0, 12.0));
    let answer = boards_answer(&state.boards);
    {
        let mut inside = root.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .id_salt("チューナーの状態"),
        );
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut inside, |ui| match answer {
                None => said(
                    ui,
                    look,
                    "coaxd にチューナーを問い合わせています…",
                    look.label,
                ),
                Some(Err(why)) => said(
                    ui,
                    look,
                    &format!("coaxd に接続できないか、チューナーがありません（{why}）"),
                    look.typed,
                ),
                Some(Ok(boards)) if boards.is_empty() => said(
                    ui,
                    look,
                    "coaxd に接続できないか、チューナーがありません",
                    look.typed,
                ),
                Some(Ok(boards)) => {
                    for board in &boards {
                        board_table(ui, look, board, edited);
                        ui.add_space(10.0);
                    }
                }
            });
    }
    let mut at = 0.0;
    let mut form = Form::foot(root, look, foot, &mut at);
    let row = form.line(skin::SELECT_HEIGHT);
    let mut pressed = None;
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 184.0, row.top()),
            vec2(88.0, row.height()),
        ),
        ("tuners.refresh", 0),
        "更新",
    ) {
        pressed = Some(Pressed::Boards);
    }
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 88.0, row.top()),
            vec2(88.0, row.height()),
        ),
        ("tuners.close", 0),
        "閉じる",
    ) {
        pressed = Some(Pressed::Close);
    }
    pressed
}

fn said(ui: &mut egui::Ui, look: &Look, text: &str, colour: Color32) {
    let _ = look;
    ui.label(
        egui::RichText::new(text)
            .font(FontId::proportional(skin::TEXT_SIZE))
            .color(colour),
    );
}

fn board_table(
    ui: &mut egui::Ui,
    look: &Look,
    board: &zwcore::input::source::coaxd::Board,
    edited: &File,
) {
    let width = ui.available_width();
    let (head, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(
        board.model.clone(),
        FontId::proportional(skin::TEXT_SIZE + 0.5),
        look.text,
    );
    let model_width = galley.size().x;
    painter.galley(
        pos2(head.left(), head.center().y - galley.size().y / 2.0),
        galley,
        look.text,
    );
    if !board.location.is_empty() {
        painter.text(
            pos2(head.left() + model_width + 8.0, head.center().y),
            egui::Align2::LEFT_CENTER,
            &board.location,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
    }
    ui.add_space(4.0);
    let top = ui.cursor().top();
    let inner = |rect: Rect| rect.shrink2(vec2(10.0, 0.0));
    let right_columns = |row: Rect| {
        let row = inner(row);
        let systems = row.right() - SYSTEMS_WIDTH;
        let signal = systems - 8.0 - SIGNAL_WIDTH;
        (signal, systems)
    };
    let (row, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let (signal_left, systems_left) = right_columns(row);
    for (index, (x, text)) in COLUMNS.iter().enumerate() {
        let left = match index {
            3 => signal_left,
            4 => systems_left,
            _ => inner(row).left() + x,
        };
        ui.painter().text(
            pos2(left, row.center().y),
            egui::Align2::LEFT_CENTER,
            *text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
    }
    for held in &board.tuners {
        let tuner = &held.tuner;
        let busy_with = held
            .priority
            .filter(|priority| tuner.leased && *priority != 0);
        let height = if busy_with.is_some() { 40.0 } else { 30.0 };
        let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let painter = ui.painter();
        painter.hline(row.x_range(), row.top(), Stroke::new(1.0, look.edge));
        let cells = inner(row);
        let (signal_left, systems_left) = right_columns(row);
        let text = |x: f32, right: f32, value: &str, colour: Color32| {
            let cell = Rect::from_min_max(pos2(x, row.top()), pos2(right, row.bottom()));
            painter.with_clip_rect(cell).text(
                pos2(cell.left(), cell.center().y),
                egui::Align2::LEFT_CENTER,
                value,
                FontId::proportional(skin::TEXT_SIZE),
                colour,
            );
        };
        let name = if tuner.name.is_empty() {
            tuner.id.clone()
        } else {
            tuner.name.clone()
        };
        text(cells.left(), cells.left() + 104.0, &name, look.text);
        let state_centre = cells.left() + 110.0 + STATE_WIDTH / 2.0;
        let (shown, colour) = if tuner.leased {
            (
                match busy_with {
                    Some(priority) => format!("使用中\n（優先度 {priority}）"),
                    None => String::from("使用中"),
                },
                look.typed,
            )
        } else {
            (String::from("空き"), look.label)
        };
        let galley = painter.layout(
            shown,
            FontId::proportional(skin::TEXT_SIZE),
            colour,
            STATE_WIDTH + 20.0,
        );
        let mut job = (*galley.job).clone();
        job.halign = egui::Align::Center;
        let galley = painter.layout_job(job);
        painter.galley(
            pos2(state_centre, row.center().y - galley.size().y / 2.0),
            galley,
            colour,
        );
        let told = |name: &str| {
            held.tuned
                .iter()
                .find(|(had, _)| had == name)
                .map(|(_, value)| value.clone())
        };
        let mut tuned = told("CHANNEL")
            .or_else(|| told("FREQUENCY"))
            .unwrap_or_default();
        if let Some(stream) = told("STREAM_ID") {
            let stream = stream
                .parse::<u32>()
                .map_or(stream.clone(), |id| zwcore::si::hex_id(id, 4));
            tuned.push_str(&format!(" / StreamID {stream}"));
        }
        if tuned.is_empty() {
            text(cells.left() + 188.0, signal_left - 8.0, "—", look.label);
        } else {
            text(cells.left() + 188.0, signal_left - 8.0, &tuned, look.text);
        }
        let (signal, colour) = match (held.locked, held.cnr, held.strength) {
            (Some(false), _, _) => (String::from("未同期"), look.typed),
            (_, Some(cnr), _) => (format!("CNR {cnr:.1} dB"), look.ok),
            (_, None, Some(strength)) => (format!("{strength:.1} dBm"), look.ok),
            _ => (String::from("—"), look.label),
        };
        text(signal_left, systems_left - 8.0, &signal, colour);
        let systems: Vec<String> = systems_of(edited, tuner)
            .iter()
            .map(|one| system_name(one))
            .collect();
        let systems = if systems.is_empty() {
            String::from("未設定")
        } else {
            systems.join("・")
        };
        text(systems_left, cells.right(), &systems, look.label);
    }
    let bottom = ui.cursor().top();
    ui.painter().rect_stroke(
        Rect::from_min_max(
            pos2(ui.min_rect().left(), top),
            pos2(ui.min_rect().left() + width, bottom),
        ),
        6.0,
        Stroke::new(1.0, look.button_edge),
        egui::StrokeKind::Inside,
    );
}
