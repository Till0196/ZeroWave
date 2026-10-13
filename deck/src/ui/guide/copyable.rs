// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組説明の本文。見出しと値を一つの文章にして、読み取り専用の入力欄で出す（選択・コピー・すべて選択）。

use egui::text::{CCursorRange, LayoutJob};
use egui::{Color32, FontId, Id, TextFormat, Ui};

#[derive(Debug, Clone)]
pub struct Look {
    pub heading_font: FontId,
    pub heading: Color32,
    pub text_font: FontId,
    pub text: Color32,
    pub leading: Option<f32>,
}

/// 見出しが空なら値だけ。
pub fn show(ui: &mut Ui, id: Id, entries: &[(String, String)], look: &Look, width: f32) {
    let mut text = String::new();
    let mut headings: Vec<std::ops::Range<usize>> = Vec::new();
    for (heading, value) in entries {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        if !heading.is_empty() {
            let from = text.len();
            text.push_str(heading);
            headings.push(from..text.len());
            text.push('\n');
        }
        text.push_str(value);
    }
    let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, width: f32| {
        let whole = buffer.as_str();
        let mut job = LayoutJob::default();
        let body = TextFormat {
            font_id: look.text_font.clone(),
            color: look.text,
            line_height: look.leading,
            ..Default::default()
        };
        let heading = TextFormat {
            font_id: look.heading_font.clone(),
            color: look.heading,
            ..Default::default()
        };
        let mut at = 0;
        for range in &headings {
            if range.end > whole.len() || range.start < at {
                continue;
            }
            job.append(&whole[at..range.start], 0.0, body.clone());
            job.append(&whole[range.clone()], 0.0, heading.clone());
            at = range.end;
        }
        job.append(&whole[at..], 0.0, body);
        job.wrap.max_width = width;
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let mut shown: &str = &text;
    let output = egui::TextEdit::multiline(&mut shown)
        .id(id)
        .frame(egui::Frame::NONE)
        .desired_width(width)
        .layouter(&mut layouter)
        .show(ui);
    // 右クリックで入力欄の選択範囲が消えるので、こちらで記録しておき右クリックされたらそれに戻す。
    let kept = id.with("選んでいた範囲");
    let secondary = output.response.secondary_clicked()
        || ui.input(|input| input.pointer.secondary_pressed()) && output.response.hovered();
    let range = output.cursor_range.filter(|range| !range.is_empty());
    if secondary {
        if let Some(range) = ui.data(|data| data.get_temp::<CCursorRange>(kept)) {
            let mut state = output.state.clone();
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), id);
        }
    } else if !output.response.context_menu_opened() {
        match range {
            Some(range) => {
                ui.data_mut(|data| data.insert_temp(kept, range));
            }
            None if output.response.clicked() => {
                ui.data_mut(|data| {
                    data.remove::<CCursorRange>(kept);
                });
            }
            None => {}
        }
    }
    let chosen = ui
        .data(|data| data.get_temp::<CCursorRange>(kept))
        .filter(|range| !range.is_empty())
        .map(|range| range.slice_str(&text).to_owned());
    output.response.context_menu(|ui| {
        if ui.button("コピー").clicked() {
            ui.ctx()
                .copy_text(chosen.clone().unwrap_or_else(|| text.clone()));
            ui.close();
        }
        if ui.button("すべて選択").clicked() {
            let all = CCursorRange::select_all(&output.galley);
            let mut state = output.state.clone();
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
            ui.data_mut(|data| data.insert_temp(kept, all));
            ui.memory_mut(|memory| memory.request_focus(id));
            ui.close();
        }
    });
}
