// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのページのうち、設定のキーに対応付けた欄と切り替え。キーの値は本体が読み書きする
//! （起動する物と環境変数は guest が書けないため）。`confirm` のページは下書きに置き、「適用」で書く。

use std::collections::BTreeMap;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use zwsdk::ui::{Badge, Event, Tone};

use super::{Form, Look, skin};
use crate::ui::icons::{self, Icon};

pub type Draft = BTreeMap<String, Option<String>>;

const CARD: f32 = 66.0;
const CHIP: f32 = 24.0;
const ROW: f32 = 32.0;
const NAME: f32 = 150.0;

pub(super) struct Bound<'s> {
    stem: &'s str,
    settings: Vec<zwsdk::host::Setting>,
    saved: BTreeMap<String, String>,
    draft: Option<&'s mut Draft>,
    read_only: bool,
    pub(super) programs: Vec<(String, Rect)>,
    pub(super) browse: Option<String>,
    pub(super) wrote: bool,
}

impl<'s> Bound<'s> {
    pub(super) fn new(stem: &'s str, draft: Option<&'s mut Draft>) -> Self {
        let settings = crate::plugins::named(stem)
            .map(|plugin| plugin.manifest().config.clone())
            .unwrap_or_default();
        Self {
            stem,
            settings,
            saved: crate::config::plugins::values(stem),
            draft,
            read_only: false,
            programs: Vec::new(),
            browse: None,
            wrote: false,
        }
    }

    pub(super) fn read_only(stem: &'s str) -> Self {
        Self {
            read_only: true,
            ..Self::new(stem, None)
        }
    }

    pub(super) fn setting(&self, key: &str) -> Option<&zwsdk::host::Setting> {
        self.settings.iter().find(|setting| setting.key == key)
    }

    fn is(&self, key: &str, kind: &str) -> bool {
        self.setting(key)
            .is_some_and(|setting| setting.kind == kind)
    }

    /// 起動する物と環境変数はマニフェストの既定を持たない（[`zwcore::plugins`] と同じ）。
    fn fallback(&self, key: &str) -> String {
        self.setting(key)
            .filter(|setting| setting.kind != "program" && !setting.is_env())
            .map(|setting| setting.default.clone())
            .unwrap_or_default()
    }

    fn saved_value(&self, key: &str) -> String {
        self.saved
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.fallback(key))
    }

    pub(super) fn value(&self, key: &str) -> String {
        match self.draft.as_ref().and_then(|draft| draft.get(key)) {
            Some(Some(value)) => value.clone(),
            Some(None) => self.fallback(key),
            None => self.saved_value(key),
        }
    }

    pub(super) fn put(&mut self, key: &str, value: Option<String>) {
        if self.read_only {
            return;
        }
        let same = match &value {
            Some(value) => *value == self.saved_value(key),
            None => !self.saved.contains_key(key),
        };
        match self.draft.as_deref_mut() {
            Some(draft) if same => {
                draft.remove(key);
            }
            Some(draft) => {
                draft.insert(key.to_owned(), value);
            }
            None => {
                crate::config::plugins::put(self.stem, key, value.as_deref());
                match value {
                    Some(value) => self.saved.insert(key.to_owned(), value),
                    None => self.saved.remove(key),
                };
                self.wrote = true;
            }
        }
    }
}

fn text_width(form: &Form<'_, '_>, text: &str, font: FontId) -> f32 {
    form.ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, form.look.text)
        .size()
        .x
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn tone_colour(look: &Look, tone: Tone) -> Color32 {
    match tone {
        Tone::Plain => look.text,
        Tone::Muted => look.group,
        Tone::Accent => look.primary,
        Tone::Good => look.ok,
        Tone::Warn => look.typed,
        Tone::Bad => look.error,
    }
}

fn clipped(
    form: &Form<'_, '_>,
    at: egui::Pos2,
    text: &str,
    font: FontId,
    colour: Color32,
    width: f32,
) {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, colour));
    job.wrap = TextWrapping::truncate_at_width(width.max(10.0));
    let galley = form.ui.painter().layout_job(job);
    let top = at.y - galley.size().y / 2.0;
    form.ui.painter().galley(pos2(at.x, top), galley, colour);
}

fn rule(form: &mut Form<'_, '_>, label: &str, button: Option<(&str, egui::Id)>) -> bool {
    let look = form.look;
    *form.at += 8.0;
    let row = form.line(24.0);
    let font = FontId::proportional(skin::SECTION_SIZE - 0.5);
    let width = text_width(form, label, font.clone());
    form.ui.painter().text(
        pos2(row.left(), row.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        look.group,
    );
    let mut line_end = row.right();
    let mut pressed = false;
    if let Some((text, id)) = button {
        let font = FontId::proportional(skin::TEXT_SIZE - 0.5);
        let w = text_width(form, text, font.clone()) + 20.0;
        let rect = Rect::from_min_size(pos2(row.right() - w, row.center().y - 11.0), vec2(w, 22.0));
        let response = form.ui.interact(rect, id, Sense::click());
        let painter = form.ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, skin::BUTTON_ROUNDING, look.hover);
        }
        painter.rect_stroke(
            rect,
            skin::BUTTON_ROUNDING,
            Stroke::new(1.0, look.edge),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            if response.hovered() {
                look.text
            } else {
                look.label
            },
        );
        line_end = rect.left() - 8.0;
        pressed = response.clicked();
    }
    let start = row.left() + width + 8.0;
    if line_end > start {
        form.ui.painter().hline(
            start..=line_end,
            row.center().y,
            Stroke::new(1.0, look.edge),
        );
    }
    pressed
}

fn small_button(form: &mut Form<'_, '_>, rect: Rect, id: egui::Id, label: &str) -> bool {
    let look = form.look;
    let response = form.ui.interact(rect, id, Sense::click());
    let painter = form.ui.painter();
    painter.rect_filled(
        rect,
        skin::BUTTON_ROUNDING,
        if response.hovered() {
            look.hover
        } else {
            look.button_fill
        },
    );
    painter.rect_stroke(
        rect,
        skin::BUTTON_ROUNDING,
        Stroke::new(1.0, look.button_edge),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(skin::TEXT_SIZE - 0.5),
        look.text,
    );
    response.clicked()
}

fn glyph(form: &mut Form<'_, '_>, rect: Rect, id: egui::Id, icon: Icon) -> egui::Response {
    let look = form.look;
    let response = form.ui.interact(rect, id, Sense::click());
    let painter = form.ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4.0, look.hover);
    }
    icons::paint_at(
        painter,
        rect.center(),
        skin::ICON,
        icon,
        if response.hovered() {
            look.text
        } else {
            look.group
        },
    );
    response
}

pub(super) fn segments(
    form: &mut Form<'_, '_>,
    scope: &str,
    id: &str,
    options: &[(String, String)],
    value: &str,
) -> Option<Event> {
    let look = form.look;
    let row = form.line(32.0);
    let font = FontId::proportional(skin::TEXT_SIZE);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| text_width(form, label, font.clone()) + 28.0)
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * options.len().saturating_sub(1) as f32 + 6.0;
    let frame = Rect::from_min_size(row.min, vec2(total, row.height()));
    form.ui.painter().rect_filled(frame, 7.0, look.select_fill);
    form.ui.painter().rect_stroke(
        frame,
        7.0,
        Stroke::new(1.0, look.edge),
        egui::StrokeKind::Inside,
    );
    let mut x = frame.left() + 3.0;
    let mut said = None;
    for ((option, label), width) in options.iter().zip(widths) {
        let rect = Rect::from_min_size(
            pos2(x, frame.top() + 3.0),
            vec2(width, frame.height() - 6.0),
        );
        x = rect.right() + 2.0;
        let response = form.ui.interact(
            rect,
            form.ui
                .id()
                .with(("plugin.segment", scope, id, option.as_str())),
            Sense::click(),
        );
        let chosen = option == value;
        let painter = form.ui.painter();
        if chosen {
            painter.rect_filled(rect, 5.0, look.hover);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left() + 4.0, rect.bottom() - 2.0),
                    rect.max - vec2(4.0, 0.0),
                ),
                1.0,
                look.primary,
            );
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            if chosen || response.hovered() {
                look.text
            } else {
                look.label
            },
        );
        if response.clicked() && !chosen {
            said = Some(Event {
                id: id.to_owned(),
                value: option.clone(),
            });
        }
    }
    said
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// 前後の `"` を一組外す（Windows の「パスのコピー」）。
fn unquoted(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
        .trim()
}

fn terminal_icon(painter: &egui::Painter, centre: egui::Pos2, colour: Color32) {
    let stroke = Stroke::new(1.5, colour);
    let frame = Rect::from_center_size(centre, vec2(17.0, 13.0));
    painter.rect_stroke(frame, 1.5, stroke, egui::StrokeKind::Middle);
    let (l, t) = (frame.left(), frame.top());
    painter.line_segment([pos2(l + 3.5, t + 4.0), pos2(l + 6.5, t + 6.5)], stroke);
    painter.line_segment([pos2(l + 6.5, t + 6.5), pos2(l + 3.5, t + 9.0)], stroke);
    painter.line_segment([pos2(l + 8.5, t + 9.5), pos2(l + 13.5, t + 9.5)], stroke);
}

fn badge_at(form: &Form<'_, '_>, left: f32, middle: f32, badge: &Badge) -> f32 {
    let colour = tone_colour(form.look, badge.tone);
    let font = FontId::proportional(skin::TEXT_SIZE - 1.5);
    let width = text_width(form, &badge.text, font.clone()) + 26.0;
    let rect = Rect::from_min_size(pos2(left, middle - 9.0), vec2(width, 18.0));
    let painter = form.ui.painter();
    painter.rect_filled(rect, 9.0, colour.gamma_multiply(0.14));
    painter.circle_filled(pos2(rect.left() + 10.0, middle), 3.5, colour);
    painter.text(
        pos2(rect.left() + 18.0, middle),
        egui::Align2::LEFT_CENTER,
        &badge.text,
        font,
        colour,
    );
    rect.right()
}

pub(super) fn program(
    form: &mut Form<'_, '_>,
    scope: &str,
    bound: &mut Bound<'_>,
    key: &str,
    label: &str,
    badge: Option<&Badge>,
    hint: &str,
) {
    if !bound.is(key, "program") {
        form.note(format!("{key} はプログラムのパスの設定ではありません"));
        return;
    }
    if !label.is_empty() {
        rule(form, label, None);
    }
    let look = form.look;
    let value = bound.value(key);
    let path = unquoted(&value).to_owned();
    let rect = form.line(CARD);
    bound.programs.push((key.to_owned(), rect));
    let hovering = form
        .ui
        .ctx()
        .input(|input| !input.raw.hovered_files.is_empty());
    {
        let painter = form.ui.painter();
        painter.rect_filled(rect, 8.0, look.select_fill);
        painter.rect_stroke(
            rect,
            8.0,
            Stroke::new(
                if hovering { 1.5 } else { 1.0 },
                if hovering { look.primary } else { look.edge },
            ),
            egui::StrokeKind::Inside,
        );
        let icon = Rect::from_min_size(
            pos2(rect.left() + 14.0, rect.center().y - 20.0),
            vec2(40.0, 40.0),
        );
        painter.rect_filled(icon, 8.0, look.button_fill);
        terminal_icon(painter, icon.center(), look.label);
    }
    let empty = path.is_empty();
    let cross =
        Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), vec2(24.0, 24.0));
    let change_label = if empty { "選ぶ…" } else { "変更…" };
    let change_width = text_width(
        form,
        change_label,
        FontId::proportional(skin::TEXT_SIZE - 0.5),
    ) + 22.0;
    let change_right = if empty {
        rect.right() - 14.0
    } else {
        cross.left() - 6.0
    };
    let change = Rect::from_min_size(
        pos2(change_right - change_width, rect.center().y - 12.0),
        vec2(change_width, 24.0),
    );
    let left = rect.left() + 14.0 + 40.0 + 14.0;
    let room = change.left() - 12.0 - left;
    let top = rect.center().y - 10.0;
    let bottom = rect.center().y + 11.0;
    if empty {
        form.ui.painter().text(
            pos2(left, top),
            egui::Align2::LEFT_CENTER,
            "選ばれていません",
            FontId::proportional(skin::TEXT_SIZE + 2.0),
            look.label,
        );
        let hint = if hint.is_empty() {
            "「選ぶ…」で選ぶか、ファイルをここへドロップします"
        } else {
            hint
        };
        clipped(
            form,
            pos2(left, bottom),
            hint,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.group,
            room,
        );
    } else {
        let name = file_name(&path);
        let font = FontId::proportional(skin::TEXT_SIZE + 2.0);
        let name_width = text_width(form, name, font.clone()).min(room);
        clipped(form, pos2(left, top), name, font, look.text, room);
        if let Some(badge) = badge
            && !badge.text.is_empty()
        {
            badge_at(form, left + name_width + 10.0, top, badge);
        }
        clipped(
            form,
            pos2(left, bottom),
            &path,
            mono(skin::TEXT_SIZE - 1.0),
            look.group,
            room,
        );
    }
    if !bound.read_only {
        if small_button(
            form,
            change,
            form.ui.id().with(("plugin.program.change", scope, key)),
            change_label,
        ) {
            bound.browse = Some(key.to_owned());
        }
        if !empty
            && glyph(
                form,
                cross,
                form.ui.id().with(("plugin.program.clear", scope, key)),
                Icon::Cross,
            )
            .on_hover_text("外す")
            .clicked()
        {
            bound.put(key, None);
        }
    }
}

fn shown_arg(arg: &str) -> String {
    if arg.is_empty() || arg.contains(char::is_whitespace) {
        format!("\"{arg}\"")
    } else {
        arg.to_owned()
    }
}

pub(super) fn list(
    form: &mut Form<'_, '_>,
    scope: &str,
    bound: &mut Bound<'_>,
    key: &str,
    label: &str,
    hint: &str,
) {
    if !bound.is(key, "list") {
        form.note(format!("{key} はリストの設定ではありません"));
        return;
    }
    let look = form.look;
    let mut items = zwsdk::config::list_of(&bound.value(key));
    let input_id = form.ui.id().with(("plugin.list.input", scope, key));
    let drag_id = form.ui.id().with(("plugin.list.drag", scope, key));
    let add = (!bound.read_only).then(|| {
        (
            "＋ 追加",
            form.ui.id().with(("plugin.list.add", scope, key)),
        )
    });
    if rule(form, if label.is_empty() { key } else { label }, add) {
        form.ui.memory_mut(|memory| memory.request_focus(input_id));
    }
    let row = form.line(CHIP);
    let (left, right) = (row.left(), row.right());
    let font = mono(skin::TEXT_SIZE - 0.5);
    // ラベルの場所を先に決める（ドラッグ先を求めるのに全部の場所が要る）。
    let mut rects = Vec::with_capacity(items.len());
    let mut at = row.min;
    for item in &items {
        let width = text_width(form, &shown_arg(item), font.clone()) + 16.0 + 18.0;
        if at.x > left && at.x + width > right {
            at = pos2(left, at.y + CHIP + 6.0);
        }
        rects.push(Rect::from_min_size(at, vec2(width.min(right - left), CHIP)));
        at.x += width + 6.0;
    }
    let input_width = 190.0_f32.min(right - left);
    if at.x > left && at.x + input_width > right {
        at = pos2(left, at.y + CHIP + 6.0);
    }
    let input_rect = Rect::from_min_size(at, vec2(input_width, CHIP));
    let dragging: Option<usize> = form.ui.ctx().data(|data| data.get_temp(drag_id));
    let pointer = form.ui.ctx().pointer_interact_pos();
    let mut changed = false;
    let mut remove = None;
    let mut dropped = None;
    for (index, (item, rect)) in items.iter().zip(&rects).enumerate() {
        let response = form.ui.interact(
            *rect,
            form.ui.id().with(("plugin.list.chip", scope, key, index)),
            if bound.read_only {
                Sense::hover()
            } else {
                Sense::click_and_drag()
            },
        );
        let cross = Rect::from_min_max(pos2(rect.right() - 20.0, rect.top()), rect.max);
        let held = dragging == Some(index);
        let painter = form.ui.painter();
        painter.rect_filled(*rect, 4.0, if held { look.hover } else { look.button_fill });
        if held {
            painter.rect_stroke(
                *rect,
                4.0,
                Stroke::new(1.0, look.primary),
                egui::StrokeKind::Inside,
            );
        }
        clipped(
            form,
            pos2(rect.left() + 8.0, rect.center().y),
            &shown_arg(item),
            font.clone(),
            look.text,
            rect.width() - 30.0,
        );
        let over_cross = response.hover_pos().is_some_and(|p| cross.contains(p));
        icons::paint_at(
            form.ui.painter(),
            cross.center(),
            skin::ICON - 2.0,
            Icon::Cross,
            if over_cross { look.text } else { look.group },
        );
        if response.drag_started() {
            form.ui
                .ctx()
                .data_mut(|data| data.insert_temp(drag_id, index));
        }
        if response.drag_stopped() {
            dropped = Some(index);
        }
        if response.clicked()
            && response
                .interact_pointer_pos()
                .is_some_and(|p| cross.contains(p))
        {
            remove = Some(index);
        }
    }
    if let (Some(from), Some(p)) = (dragging, pointer) {
        let before = rects
            .iter()
            .enumerate()
            .filter(|(index, rect)| {
                *index != from
                    && (rect.bottom() < p.y
                        || (rect.top() <= p.y + 3.0
                            && rect.center().x < p.x
                            && rect.bottom() >= p.y - 3.0))
            })
            .count();
        let mark = rects
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != from)
            .nth(before)
            .map_or_else(
                || {
                    rects
                        .iter()
                        .enumerate()
                        .rfind(|(i, _)| *i != from)
                        .map(|(_, r)| pos2(r.right() + 3.0, r.top()))
                },
                |(_, r)| Some(pos2(r.left() - 3.0, r.top())),
            );
        if let Some(mark) = mark {
            form.ui.painter().vline(
                mark.x,
                mark.y..=mark.y + CHIP,
                Stroke::new(2.0, look.primary),
            );
        }
        if dropped.is_some() {
            let item = items.remove(from);
            items.insert(before.min(items.len()), item);
            changed = true;
            form.ui.ctx().data_mut(|data| data.remove::<usize>(drag_id));
        }
    } else if dropped.is_some() {
        form.ui.ctx().data_mut(|data| data.remove::<usize>(drag_id));
    }
    if let Some(index) = remove {
        items.remove(index);
        changed = true;
    }
    if !bound.read_only {
        let focused = form.ui.ctx().memory(|memory| memory.has_focus(input_id));
        let edge = if focused { look.primary } else { look.edge };
        let r = input_rect;
        for (a, b) in [
            (r.left_top(), r.right_top()),
            (r.right_top(), r.right_bottom()),
            (r.right_bottom(), r.left_bottom()),
            (r.left_bottom(), r.left_top()),
        ] {
            form.ui.painter().extend(egui::Shape::dashed_line(
                &[a, b],
                Stroke::new(1.0, edge),
                4.0,
                3.0,
            ));
        }
        let buffer_id = input_id.with("buffer");
        let mut buffer: String = form
            .ui
            .ctx()
            .data(|data| data.get_temp(buffer_id))
            .unwrap_or_default();
        let mut inner = form
            .ui
            .new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(8.0, 3.0))));
        let response = inner.add(
            egui::TextEdit::singleline(&mut buffer)
                .id(input_id)
                .frame(egui::Frame::NONE)
                .hint_text(if hint.is_empty() {
                    "入力して Enter"
                } else {
                    hint
                })
                .desired_width(f32::INFINITY)
                .font(font.clone())
                .text_color(look.text),
        );
        let entered =
            response.lost_focus() && inner.input(|input| input.key_pressed(egui::Key::Enter));
        if entered && !buffer.is_empty() {
            items.push(std::mem::take(&mut buffer));
            changed = true;
            form.ui.memory_mut(|memory| memory.request_focus(input_id));
        }
        form.ui
            .ctx()
            .data_mut(|data| data.insert_temp(buffer_id, buffer));
    }
    *form.at = input_rect
        .bottom()
        .max(rects.last().map_or(0.0, |r| r.bottom()))
        + 8.0;
    if changed {
        bound.put(key, Some(zwsdk::config::list_text(&items)));
    }
}

#[derive(Debug, Clone, Default)]
struct Editing {
    at: Option<usize>,
    name: String,
    value: String,
}

fn switch(
    form: &mut Form<'_, '_>,
    centre: egui::Pos2,
    id: egui::Id,
    on: bool,
    enabled: bool,
) -> bool {
    let look = form.look;
    let rect = Rect::from_center_size(centre, vec2(28.0, 16.0));
    let response = form.ui.interact(
        rect.expand(4.0),
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let painter = form.ui.painter();
    painter.rect_filled(rect, 8.0, if on { look.primary } else { look.hover });
    let knob = if on {
        rect.right() - 8.0
    } else {
        rect.left() + 8.0
    };
    painter.circle_filled(pos2(knob, rect.center().y), 5.5, Color32::WHITE);
    enabled && response.clicked()
}

pub(super) fn map(
    form: &mut Form<'_, '_>,
    scope: &str,
    bound: &mut Bound<'_>,
    key: &str,
    label: &str,
    empty: &str,
) {
    if !bound.is(key, "map") {
        form.note(format!("{key} はテーブルの設定ではありません"));
        return;
    }
    let env = bound.setting(key).is_some_and(zwsdk::host::Setting::is_env);
    let look = form.look;
    let mut entries = zwsdk::config::entries_of(&bound.value(key));
    let edit_id = form.ui.id().with(("plugin.map.edit", scope, key));
    let name_id = edit_id.with("name");
    let value_id = edit_id.with("value");
    let mut editing: Option<Editing> = form.ui.ctx().data(|data| data.get_temp(edit_id));
    // 編集を始めたフレームは名前の欄へ（欄が出来る前に要求しても効かない）。
    let mut focus_name = form
        .ui
        .ctx()
        .data(|data| data.get_temp::<bool>(name_id))
        .unwrap_or(false);
    let add =
        (!bound.read_only).then(|| ("＋ 追加", form.ui.id().with(("plugin.map.add", scope, key))));
    if rule(form, if label.is_empty() { key } else { label }, add) {
        editing = Some(Editing::default());
        focus_name = true;
    }
    let whole = form.line(0.0);
    let (left, right) = (whole.left(), whole.right());
    let top = whole.top();
    let mut y = top;
    let mut changed = false;
    let mut remove = None;
    let mut finish: Option<Option<Editing>> = None;
    let mono_font = mono(skin::TEXT_SIZE - 0.5);
    let shown_rows = entries.len() + usize::from(editing.as_ref().is_some_and(|e| e.at.is_none()));
    let frame = Rect::from_min_max(
        pos2(left, top),
        pos2(right, top + ROW * shown_rows.max(1) as f32),
    );
    form.ui.painter().rect_filled(frame, 6.0, look.select_fill);
    if shown_rows == 0 {
        form.ui.painter().text(
            frame.center(),
            egui::Align2::CENTER_CENTER,
            if empty.is_empty() {
                "ありません"
            } else {
                empty
            },
            FontId::proportional(skin::TEXT_SIZE - 0.5),
            look.group,
        );
    }
    let name_left = left + 52.0;
    let value_left = name_left + NAME + 10.0;
    let more = Rect::from_min_size(pos2(right - 32.0, 0.0), vec2(24.0, 22.0));
    for index in 0..shown_rows {
        let line = Rect::from_min_size(pos2(left, y), vec2(right - left, ROW));
        y = line.bottom();
        if index > 0 {
            form.ui
                .painter()
                .hline(line.x_range(), line.top(), Stroke::new(1.0, look.dark));
        }
        let editing_here = editing
            .as_ref()
            .is_some_and(|e| e.at == Some(index) || (e.at.is_none() && index == entries.len()));
        if editing_here {
            let Some(edit) = editing.as_mut() else {
                continue;
            };
            let name_rect = Rect::from_min_size(
                pos2(name_left - 6.0, line.top() + 3.0),
                vec2(NAME + 4.0, ROW - 6.0),
            );
            let value_rect = Rect::from_min_max(
                pos2(value_left - 6.0, line.top() + 3.0),
                pos2(right - 64.0, line.bottom() - 3.0),
            );
            let bad = env && !edit.name.is_empty() && !zwsdk::config::env_name(&edit.name);
            let mut left_field = false;
            for (rect, id, text, hint, wrong) in [
                (name_rect, name_id, &mut edit.name, "名前", bad),
                (value_rect, value_id, &mut edit.value, "値", false),
            ] {
                let painter = form.ui.painter();
                painter.rect_filled(rect, skin::SELECT_ROUNDING, look.body);
                painter.rect_stroke(
                    rect,
                    skin::SELECT_ROUNDING,
                    Stroke::new(1.0, if wrong { look.error } else { look.select_edge }),
                    egui::StrokeKind::Inside,
                );
                let mut inner = form
                    .ui
                    .new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 3.0))));
                let response = inner.add(
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .font(mono_font.clone())
                        .text_color(look.text),
                );
                if id == name_id && focus_name {
                    response.request_focus();
                    focus_name = false;
                }
                left_field |= response.lost_focus();
            }
            let ok =
                Rect::from_min_size(pos2(right - 60.0, line.center().y - 11.0), vec2(24.0, 22.0));
            let cancel =
                Rect::from_min_size(pos2(right - 32.0, line.center().y - 11.0), vec2(24.0, 22.0));
            let entered = left_field && form.ui.input(|input| input.key_pressed(egui::Key::Enter));
            let escaped = left_field && form.ui.input(|input| input.key_pressed(egui::Key::Escape));
            let ok_clicked = glyph(form, ok, edit_id.with("ok"), Icon::Check).clicked();
            let cancel_clicked = glyph(form, cancel, edit_id.with("cancel"), Icon::Cross).clicked();
            if escaped || cancel_clicked {
                finish = Some(None);
            } else if (entered || ok_clicked) && !bad {
                finish = Some(Some(edit.clone()));
            }
            continue;
        }
        let Some((name, value, on)) = entries.get(index).cloned() else {
            continue;
        };
        if switch(
            form,
            pos2(left + 24.0, line.center().y),
            form.ui.id().with(("plugin.map.on", scope, key, index)),
            on,
            !bound.read_only,
        ) {
            entries[index].2 = !on;
            changed = true;
        }
        clipped(
            form,
            pos2(name_left, line.center().y),
            &name,
            mono_font.clone(),
            if on { look.text } else { look.group },
            NAME,
        );
        clipped(
            form,
            pos2(value_left, line.center().y),
            &value,
            mono_font.clone(),
            if on { look.label } else { look.group },
            more.left() - 8.0 - value_left,
        );
        if bound.read_only {
            continue;
        }
        let menu = more.translate(vec2(0.0, line.center().y - 11.0));
        let response = glyph(
            form,
            menu,
            form.ui.id().with(("plugin.map.more", scope, key, index)),
            Icon::More,
        );
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(120.0);
            for (word, edit) in [("編集", true), ("削除", false)] {
                let pressed = ui
                    .button(
                        egui::RichText::new(word)
                            .font(FontId::proportional(skin::TEXT_SIZE))
                            .color(look.text),
                    )
                    .clicked();
                if pressed {
                    if edit {
                        editing = Some(Editing {
                            at: Some(index),
                            name: name.clone(),
                            value: value.clone(),
                        });
                        focus_name = true;
                    } else {
                        remove = Some(index);
                    }
                    ui.close();
                }
            }
        });
    }
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, look.edge),
        egui::StrokeKind::Inside,
    );
    *form.at = frame.bottom() + 8.0;
    if let Some(index) = remove {
        entries.remove(index);
        changed = true;
    }
    match finish {
        Some(Some(edit)) => {
            let name = edit.name.trim().to_owned();
            let on = edit.at.and_then(|at| entries.get(at)).is_none_or(|e| e.2);
            if let Some(at) = edit.at
                && at < entries.len()
            {
                entries.remove(at);
            }
            if !name.is_empty() {
                entries.retain(|(other, _, _)| *other != name);
                entries.push((name, edit.value, on));
            }
            changed = true;
            editing = None;
        }
        Some(None) => editing = None,
        None => {}
    }
    form.ui
        .ctx()
        .data_mut(|data| data.insert_temp(name_id, focus_name && editing.is_some()));
    form.ui.ctx().data_mut(|data| match editing {
        Some(edit) => {
            data.insert_temp(edit_id, edit);
        }
        None => {
            data.remove::<Editing>(edit_id);
        }
    });
    if changed {
        bound.put(key, Some(zwsdk::config::entries_text(&entries)));
    }
}

fn quoted(word: &str) -> String {
    if !word.is_empty() && !word.contains(|c: char| c.is_whitespace() || c == '"') {
        return word.to_owned();
    }
    format!("\"{}\"", word.replace('"', "\\\""))
}

pub(super) fn command_line(
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> (String, String) {
    let mut shown = Vec::new();
    let mut full = Vec::new();
    for (name, value) in env {
        shown.push(format!("{name}=…"));
        full.push(format!("{name}={}", quoted(value)));
    }
    let program = quoted(program);
    shown.push(program.clone());
    full.push(program);
    for arg in args {
        shown.push(quoted(arg));
        full.push(quoted(arg));
    }
    (shown.join(" "), full.join(" "))
}

pub(super) fn command(
    form: &mut Form<'_, '_>,
    scope: &str,
    bound: &Bound<'_>,
    program: &str,
    args: &str,
    env: &str,
    label: &str,
) {
    let look = form.look;
    rule(
        form,
        if label.is_empty() {
            "実行するコマンド"
        } else {
            label
        },
        None,
    );
    let path = unquoted(&bound.value(program)).to_owned();
    let args = if args.is_empty() {
        Vec::new()
    } else {
        zwsdk::config::list_of(&bound.value(args))
    };
    let env = if env.is_empty() {
        Vec::new()
    } else {
        zwsdk::config::map_on(&bound.value(env))
    };
    let (shown, full) = command_line(&path, &args, &env);
    let rect = form.line(34.0);
    form.ui.painter().rect_filled(rect, 6.0, look.dark);
    form.ui.painter().rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, look.edge),
        egui::StrokeKind::Inside,
    );
    let font = mono(skin::TEXT_SIZE - 0.5);
    form.ui.painter().text(
        pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "$",
        font.clone(),
        look.group,
    );
    let copy_id = form.ui.id().with(("plugin.command.copy", scope, program));
    let copied_at: Option<f64> = form.ui.ctx().data(|data| data.get_temp(copy_id));
    let now = form.ui.input(|input| input.time);
    let copied = copied_at.is_some_and(|at| now - at < 1.5);
    let word = if copied {
        "コピーしました"
    } else {
        "コピー"
    };
    let word_font = FontId::proportional(skin::TEXT_SIZE - 1.0);
    let word_width = text_width(form, word, word_font.clone()) + 16.0;
    let copy = Rect::from_min_size(
        pos2(rect.right() - 6.0 - word_width, rect.center().y - 11.0),
        vec2(word_width, 22.0),
    );
    if path.is_empty() {
        clipped(
            form,
            pos2(rect.left() + 26.0, rect.center().y),
            "（実行ファイルが選ばれていません）",
            FontId::proportional(skin::TEXT_SIZE - 0.5),
            look.group,
            copy.left() - rect.left() - 34.0,
        );
        return;
    }
    clipped(
        form,
        pos2(rect.left() + 26.0, rect.center().y),
        &shown,
        font,
        look.label,
        copy.left() - rect.left() - 34.0,
    );
    let response = form.ui.interact(copy, copy_id, Sense::click());
    form.ui.painter().text(
        copy.center(),
        egui::Align2::CENTER_CENTER,
        word,
        word_font,
        if copied {
            look.ok
        } else if response.hovered() {
            look.text
        } else {
            look.group
        },
    );
    if response.clicked() {
        form.ui.ctx().copy_text(full);
        form.ui
            .ctx()
            .data_mut(|data| data.insert_temp(copy_id, now));
    }
    if copied {
        form.ui
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_hides_values_but_copies_them() {
        let (shown, full) = command_line(
            r"C:\Program Files\x\x.exe",
            &[String::from("-v"), String::from("a b")],
            &[(String::from("TMPDIR"), String::from("$TMPDIR\\zw"))],
        );
        assert_eq!(shown, r#"TMPDIR=… "C:\Program Files\x\x.exe" -v "a b""#);
        assert_eq!(
            full,
            r#"TMPDIR=$TMPDIR\zw "C:\Program Files\x\x.exe" -v "a b""#
        );
    }

    #[test]
    fn a_file_name_is_the_last_part_of_either_kind_of_path() {
        assert_eq!(file_name(r"C:\Tools\a\run.exe"), "run.exe");
        assert_eq!(file_name("/usr/local/bin/run"), "run");
        assert_eq!(unquoted(r#" "C:\a b\c.exe" "#), r"C:\a b\c.exe");
    }
}
