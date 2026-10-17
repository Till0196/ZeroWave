// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのページ（`ui` hook、[`zwsdk::ui::Page`]）を、設定ウィンドウのウィジェットで表示する別ウィンドウ。

use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use zwsdk::ui::{Cell, Event, Inline, Node, Tone};

use super::form::Label;
use super::plugin_bound::{self, Bound};
use super::{Form, Look, Pressed, State, scroll_bar, scrolled, skin};
use crate::ui::icons::{self, Icon};

pub const WIDTH: f32 = 760.0;
pub const HEIGHT: f32 = 640.0;

const TABS: f32 = 44.0;

fn tone(look: &Look, tone: Tone) -> Color32 {
    match tone {
        Tone::Plain => look.text,
        Tone::Muted => look.label,
        Tone::Accent => look.primary,
        Tone::Good => look.ok,
        Tone::Warn => look.typed,
        Tone::Bad => look.error,
    }
}

fn text_width(form: &Form<'_, '_>, text: &str, size: f32) -> f32 {
    form.ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(size), form.look.text)
        .size()
        .x
}

/// `scope` は egui の識別子をページごとに分ける。キーに対応付けた欄は `bound` が読み書きする。
fn render(
    form: &mut Form<'_, '_>,
    nodes: &[Node],
    scope: &str,
    bound: &mut Bound<'_>,
) -> Option<Event> {
    let mut said = None;
    for node in nodes {
        let event = match node {
            Node::Section { label } => {
                form.section(label);
                None
            }
            Node::Note { text, tone: t } => {
                note(form, text, tone(form.look, *t));
                None
            }
            Node::Toggle {
                id,
                label,
                on,
                indent,
                enabled,
            } => toggle(form, scope, id, label, *on, *indent, *enabled),
            Node::Line { label, items } => line(form, scope, label, items),
            Node::Buttons { buttons, right } => {
                let row = form.line(skin::SELECT_HEIGHT);
                buttons_at(form, scope, row, buttons, *right, false)
            }
            Node::Table {
                id,
                columns,
                rows,
                empty,
            } => table(form, scope, id, columns, rows, empty),
            Node::Bar {
                label,
                parts,
                left,
                right,
            } => {
                bar(form, label, parts, left, right);
                None
            }
            Node::Columns { width, left, right } => {
                columns(form, scope, *width, left, right, bound)
            }
            Node::Space { height } => {
                *form.at += height;
                None
            }
            Node::Segments { id, options, value } => {
                plugin_bound::segments(form, scope, id, options, value)
            }
            Node::Program {
                key,
                label,
                badge,
                hint,
            } => {
                plugin_bound::program(form, scope, bound, key, label, badge.as_ref(), hint);
                None
            }
            Node::List { key, label, hint } => {
                plugin_bound::list(form, scope, bound, key, label, hint);
                None
            }
            Node::Map { key, label, empty } => {
                plugin_bound::map(form, scope, bound, key, label, empty);
                None
            }
            Node::Command {
                program,
                args,
                env,
                label,
            } => {
                plugin_bound::command(form, scope, bound, program, args, env, label);
                None
            }
        };
        if said.is_none() {
            said = event;
        }
    }
    said
}

fn note(form: &mut Form<'_, '_>, text: &str, colour: Color32) {
    let left = form.body.left() + skin::INSET + skin::DOT;
    let width = form.body.right() - skin::PAD - left;
    let galley = form.ui.painter().layout(
        text.to_owned(),
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        colour,
        width.max(40.0),
    );
    let height = galley.size().y;
    form.ui
        .painter()
        .galley(pos2(left, *form.at + 2.0), galley, colour);
    *form.at += height + 4.0 + skin::ROW_GAP;
}

#[allow(clippy::too_many_arguments)]
fn toggle(
    form: &mut Form<'_, '_>,
    scope: &str,
    id: &str,
    label: &str,
    on: bool,
    indent: u8,
    enabled: bool,
) -> Option<Event> {
    let look = form.look;
    let left = form.body.left() + skin::INSET + skin::DOT + f32::from(indent) * 22.0;
    let row = Rect::from_min_max(
        pos2(left, *form.at),
        pos2(form.body.right() - skin::PAD, *form.at + 28.0),
    );
    *form.at = row.bottom() + 2.0;
    let colour = if enabled { look.text } else { look.label };
    let galley = form.ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(skin::TEXT_SIZE),
        colour,
    );
    let hit = Rect::from_min_size(
        row.min,
        vec2(skin::CHECK + 8.0 + galley.size().x, row.height()),
    );
    let clicked = enabled
        && form
            .ui
            .interact(
                hit,
                form.ui.scope_id().with(("plugin.toggle", scope, id)),
                Sense::click(),
            )
            .clicked();
    let tick = Rect::from_min_size(
        pos2(row.left(), row.center().y - skin::CHECK / 2.0),
        vec2(skin::CHECK, skin::CHECK),
    );
    tick_box(form, tick, on);
    form.ui.painter().galley(
        pos2(tick.right() + 8.0, row.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
    clicked.then(|| Event {
        id: id.to_owned(),
        value: (!on).to_string(),
    })
}

fn tick_box(form: &Form<'_, '_>, rect: Rect, on: bool) {
    let look = form.look;
    let painter = form.ui.painter();
    painter.rect_filled(rect, 2.0, if on { look.primary } else { look.select_fill });
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, if on { look.primary } else { look.select_edge }),
        egui::StrokeKind::Inside,
    );
    if on {
        icons::paint(painter, rect.shrink(1.5), Icon::Check, Color32::WHITE);
    }
}

fn line(form: &mut Form<'_, '_>, scope: &str, label: &str, items: &[Inline]) -> Option<Event> {
    let top = *form.at;
    let left = if label.is_empty() {
        form.body.left() + skin::INSET + skin::DOT
    } else {
        form.value_left()
    };
    let right = form.body.right() - skin::PAD;
    if !label.is_empty() {
        form.ui.painter().text(
            pos2(form.label_left(), top + skin::ROW / 2.0),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
    }
    let mut x = left;
    let mut bottom = top + skin::ROW;
    let middle = top + skin::ROW / 2.0;
    let mut said = None;
    for (nth, item) in items.iter().enumerate() {
        let event = match item {
            Inline::Text { text, tone: t } => {
                let colour = tone(form.look, *t);
                let width = text_width(form, text, skin::TEXT_SIZE);
                form.ui.painter().text(
                    pos2(x, middle),
                    egui::Align2::LEFT_CENTER,
                    text,
                    FontId::proportional(skin::TEXT_SIZE),
                    colour,
                );
                x += width + 8.0;
                None
            }
            Inline::Choice {
                id,
                options,
                value,
                width,
            } => {
                let width = if *width > 0.0 {
                    *width
                } else {
                    skin::SELECT_WIDTH
                };
                let rect = Rect::from_min_size(
                    pos2(x, middle - skin::SELECT_HEIGHT / 2.0),
                    vec2(width.min(right - x), skin::SELECT_HEIGHT),
                );
                let mut now = value.clone();
                let choices: Vec<(&str, &str)> = options
                    .iter()
                    .map(|(v, l)| (v.as_str(), l.as_str()))
                    .collect();
                form.select_at(
                    rect,
                    ("plugin.choice", scope, id.as_str()),
                    &mut now,
                    &choices,
                );
                x = rect.right() + 8.0;
                (now != *value).then(|| Event {
                    id: id.clone(),
                    value: now,
                })
            }
            Inline::Input {
                id,
                value,
                width,
                hint,
            } => {
                let width = if *width > 0.0 {
                    *width
                } else {
                    skin::SELECT_WIDTH
                };
                let rect = Rect::from_min_size(
                    pos2(x, middle - skin::SELECT_HEIGHT / 2.0),
                    vec2(width.min(right - x), skin::SELECT_HEIGHT),
                );
                x = rect.right() + 8.0;
                input(form, scope, id, value, hint, rect)
            }
            Inline::Chips {
                id,
                chips,
                removable,
            } => {
                // 後ろのウィジェットの幅を先に空け、ラベルはその手前で折り返す（空けないと後ろが端ではみ出す）。
                let after = trailing_width(form, &items[nth + 1..]);
                let wrap = (right - after).max(x + 40.0);
                let (event, end) = chips_at(form, scope, id, chips, *removable, x, wrap, top + 4.0);
                bottom = bottom.max(end + 4.0);
                x = if after > 0.0 { wrap + 8.0 } else { right };
                event
            }
            Inline::Button(button) => {
                let width = text_width(form, &button.label, skin::TEXT_SIZE) + 24.0;
                let rect = Rect::from_min_size(pos2(x, middle - 12.0), vec2(width.max(28.0), 24.0));
                x = rect.right() + 6.0;
                button_at(form, scope, rect, button).then(|| Event {
                    id: button.id.clone(),
                    value: String::new(),
                })
            }
        };
        if said.is_none() {
            said = event;
        }
    }
    *form.at = bottom + skin::ROW_GAP;
    said
}

fn trailing_width(form: &Form<'_, '_>, items: &[Inline]) -> f32 {
    items
        .iter()
        .map(|item| match item {
            Inline::Text { text, .. } => text_width(form, text, skin::TEXT_SIZE) + 8.0,
            Inline::Choice { width, .. } | Inline::Input { width, .. } => {
                (if *width > 0.0 {
                    *width
                } else {
                    skin::SELECT_WIDTH
                }) + 8.0
            }
            Inline::Chips { .. } => 0.0,
            Inline::Button(button) => {
                (text_width(form, &button.label, skin::TEXT_SIZE) + 24.0).max(28.0) + 6.0
            }
        })
        .sum()
}

/// 書き込む欄。一文字ごとに返すと入力の途中で中身が入れ替わるので、書き終えたらイベントにする。
fn input(
    form: &mut Form<'_, '_>,
    scope: &str,
    id: &str,
    value: &str,
    hint: &str,
    frame: Rect,
) -> Option<Event> {
    let look = form.look;
    let painter = form.ui.painter();
    painter.rect_filled(frame, skin::SELECT_ROUNDING, look.select_fill);
    painter.rect_stroke(
        frame,
        skin::SELECT_ROUNDING,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let edit_id = form.ui.scope_id().with(("plugin.input", scope, id));
    let buffer_id = edit_id.with("buffer");
    let ctx = form.ui.ctx().clone();
    let focused = ctx.memory(|memory| memory.has_focus(edit_id));
    let mut buffer = if focused {
        ctx.data_mut(|data| data.get_temp::<String>(buffer_id))
            .unwrap_or_else(|| value.to_owned())
    } else {
        value.to_owned()
    };
    let mut inner = form
        .ui
        .new_child(egui::UiBuilder::new().max_rect(frame.shrink2(vec2(8.0, 5.0))));
    let response = inner.add(
        egui::TextEdit::singleline(&mut buffer)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(skin::TEXT_SIZE))
            .text_color(look.text),
    );
    if response.has_focus() {
        ctx.data_mut(|data| data.insert_temp(buffer_id, buffer.clone()));
    }
    if response.lost_focus() {
        ctx.data_mut(|data| data.remove::<String>(buffer_id));
        if buffer != value {
            return Some(Event {
                id: id.to_owned(),
                value: buffer,
            });
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn chips_at(
    form: &mut Form<'_, '_>,
    scope: &str,
    id: &str,
    chips: &[zwsdk::ui::Chip],
    removable: bool,
    left: f32,
    right: f32,
    top: f32,
) -> (Option<Event>, f32) {
    let look = form.look;
    let height = 24.0;
    let mut at = pos2(left, top);
    let mut said = None;
    for chip in chips {
        let text = text_width(form, &chip.label, skin::TEXT_SIZE - 1.0);
        let width = text + 20.0 + if removable { 16.0 } else { 0.0 };
        if at.x > left && at.x + width > right {
            at = pos2(left, at.y + height + 6.0);
        }
        let rect = Rect::from_min_size(at, vec2(width, height));
        let response = form.ui.interact(
            rect,
            form.ui
                .scope_id()
                .with(("plugin.chip", scope, id, chip.id.as_str())),
            Sense::click(),
        );
        let painter = form.ui.painter();
        if chip.on {
            painter.rect_filled(rect, height / 2.0, look.chosen_fill);
        } else if response.hovered() {
            painter.rect_filled(rect, height / 2.0, look.select_fill);
        }
        painter.rect_stroke(
            rect,
            height / 2.0,
            Stroke::new(
                1.0,
                if chip.on {
                    look.chosen_mark.gamma_multiply(0.7)
                } else {
                    look.button_edge
                },
            ),
            egui::StrokeKind::Inside,
        );
        painter.text(
            pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &chip.label,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            if chip.on { look.chosen_text } else { look.text },
        );
        if response.clicked() {
            let on_cross = removable
                && response
                    .interact_pointer_pos()
                    .is_some_and(|p| p.x > rect.right() - 18.0);
            said = Some(Event {
                id: id.to_owned(),
                value: if on_cross {
                    format!("remove:{}", chip.id)
                } else {
                    chip.id.clone()
                },
            });
        }
        if removable {
            icons::paint_at(
                form.ui.painter(),
                pos2(rect.right() - 11.0, rect.center().y),
                skin::ICON - 2.0,
                Icon::Cross,
                look.label,
            );
        }
        at.x = rect.right() + 6.0;
    }
    (said, at.y + height)
}

fn button_at(form: &mut Form<'_, '_>, scope: &str, rect: Rect, button: &zwsdk::ui::Button) -> bool {
    let id = ("plugin.button", scope, button.id.as_str());
    let label: Label<'_> = match button.icon.as_str() {
        "up" => Icon::ArrowUp.into(),
        "down" => Icon::ArrowDown.into(),
        "remove" => Icon::Cross.into(),
        _ => button.label.as_str().into(),
    };
    if button.primary {
        form.primary_button(rect, id, label, button.enabled)
    } else {
        form.button_enabled(rect, id, label, button.enabled)
    }
}

fn buttons_at(
    form: &mut Form<'_, '_>,
    scope: &str,
    row: Rect,
    buttons: &[zwsdk::ui::Button],
    right: bool,
    small: bool,
) -> Option<Event> {
    let mut picked = None;
    let width_of = |form: &Form<'_, '_>, button: &zwsdk::ui::Button, least: f32| {
        if !button.icon.is_empty() {
            return row.height();
        }
        (text_width(form, &button.label, skin::TEXT_SIZE) + 24.0).max(least)
    };
    let gaps = 6.0 * buttons.len().saturating_sub(1) as f32;
    let mut widths: Vec<f32> = buttons
        .iter()
        .map(|button| width_of(form, button, if small { 28.0 } else { 88.0 }))
        .collect();
    if widths.iter().sum::<f32>() + gaps > row.width() {
        widths = buttons
            .iter()
            .map(|button| width_of(form, button, 28.0))
            .collect();
    }
    let total: f32 = widths.iter().sum::<f32>() + gaps;
    let mut x = if right {
        row.right() - total
    } else {
        row.left()
    };
    for (button, width) in buttons.iter().zip(widths) {
        let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
        if button_at(form, scope, rect, button) {
            picked = Some(Event {
                id: button.id.clone(),
                value: String::new(),
            });
        }
        x = rect.right() + 6.0;
    }
    picked
}

fn bar(form: &mut Form<'_, '_>, label: &str, parts: &[(f32, Tone)], left: &str, right: &str) {
    let look = form.look;
    if !label.is_empty() {
        form.section(label);
    }
    let row = form.line(10.0);
    let painter = form.ui.painter();
    painter.rect_filled(row, 5.0, look.select_fill);
    let mut x = row.left();
    for (share, t) in parts {
        let width = row.width() * share.clamp(0.0, 1.0);
        let width = width.min(row.right() - x);
        if width <= 0.0 {
            break;
        }
        let colour = match t {
            Tone::Plain => look.primary,
            Tone::Muted => look.label,
            _ => tone(look, *t),
        };
        painter.rect_filled(
            Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height())),
            0.0,
            colour,
        );
        x += width + 1.0;
    }
    let words = form.line(16.0);
    let painter = form.ui.painter();
    painter.text(
        pos2(words.left(), words.center().y),
        egui::Align2::LEFT_CENTER,
        left,
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        look.label,
    );
    painter.text(
        pos2(words.right(), words.center().y),
        egui::Align2::RIGHT_CENTER,
        right,
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        look.label,
    );
}

fn table(
    form: &mut Form<'_, '_>,
    scope: &str,
    id: &str,
    columns: &[zwsdk::ui::Column],
    rows: &[zwsdk::ui::Row],
    empty: &str,
) -> Option<Event> {
    let look = form.look;
    let whole = form.line(18.0);
    let fixed: f32 = columns.iter().map(|c| c.width).sum();
    let flexible = columns.iter().filter(|c| c.width <= 0.0).count().max(1) as f32;
    let share = ((whole.width() - fixed) / flexible).max(40.0);
    let mut lefts = Vec::with_capacity(columns.len());
    let mut x = whole.left();
    for column in columns {
        let width = if column.width > 0.0 {
            column.width
        } else {
            share
        };
        lefts.push((x, width));
        x += width;
    }
    for (column, (left, width)) in columns.iter().zip(&lefts) {
        let (pos, align) = if column.right {
            (
                pos2(left + width - 8.0, whole.center().y),
                egui::Align2::RIGHT_CENTER,
            )
        } else {
            (
                pos2(left + 8.0, whole.center().y),
                egui::Align2::LEFT_CENTER,
            )
        };
        form.ui.painter().text(
            pos,
            align,
            &column.label,
            FontId::proportional(skin::TEXT_SIZE - 1.5),
            look.label,
        );
    }
    let frame_top = *form.at;
    if rows.is_empty() {
        let rect = Rect::from_min_size(pos2(whole.left(), *form.at), vec2(whole.width(), 44.0));
        form.ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            empty,
            FontId::proportional(skin::TEXT_SIZE),
            look.label,
        );
        *form.at = rect.bottom();
    }
    let mut picked = None;
    for row in rows {
        let tall = row
            .cells
            .iter()
            .any(|cell| matches!(cell, Cell::Text { sub, .. } if !sub.is_empty()))
            || row
                .cells
                .iter()
                .any(|cell| matches!(cell, Cell::Chips { .. }));
        let height = if tall { 42.0 } else { 32.0 };
        let line = Rect::from_min_size(pos2(whole.left(), *form.at), vec2(whole.width(), height));
        *form.at = line.bottom();
        if line.bottom() < form.body.top() || line.top() > form.body.bottom() {
            continue;
        }
        let response = form.ui.interact(
            line,
            form.ui.scope_id().with(("plugin.row", scope, id, &row.id)),
            Sense::click(),
        );
        {
            let painter = form.ui.painter();
            if row.selected {
                painter.rect_filled(line, 0.0, look.chosen_fill);
            } else if response.hovered() {
                painter.rect_filled(line, 0.0, look.select_fill);
            }
            painter.line_segment(
                [
                    pos2(line.left(), line.top()),
                    pos2(line.right(), line.top()),
                ],
                Stroke::new(0.5, look.edge),
            );
        }
        if response.clicked() {
            picked = Some(Event {
                id: id.to_owned(),
                value: row.id.clone(),
            });
        }
        let dim = |colour: Color32| {
            if row.dim {
                colour.gamma_multiply(0.45)
            } else {
                colour
            }
        };
        for (index, (cell, (left, width))) in row.cells.iter().zip(&lefts).enumerate() {
            let rect = Rect::from_min_size(pos2(*left, line.top()), vec2(*width, line.height()));
            let right_aligned = columns.get(index).is_some_and(|c| c.right);
            let event = match cell {
                Cell::Text { text, tone: t, sub } => {
                    let colour = if row.selected && *t == Tone::Plain {
                        look.chosen_text
                    } else {
                        tone(look, *t)
                    };
                    let painter = form
                        .ui
                        .painter()
                        .with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
                    let (x, align) = if right_aligned {
                        (rect.right() - 8.0, egui::Align2::RIGHT_CENTER)
                    } else {
                        (rect.left() + 8.0, egui::Align2::LEFT_CENTER)
                    };
                    if sub.is_empty() {
                        painter.text(
                            pos2(x, rect.center().y),
                            align,
                            text,
                            FontId::proportional(skin::TEXT_SIZE),
                            dim(colour),
                        );
                    } else {
                        painter.text(
                            pos2(x, rect.center().y - 8.0),
                            align,
                            text,
                            FontId::proportional(skin::TEXT_SIZE),
                            dim(colour),
                        );
                        painter.text(
                            pos2(x, rect.center().y + 8.0),
                            align,
                            sub,
                            FontId::monospace(skin::TEXT_SIZE - 2.0),
                            dim(look.label),
                        );
                    }
                    None
                }
                Cell::Tag { text, tone: t } => {
                    let colour = match t {
                        Tone::Plain => look.label,
                        _ => tone(look, *t),
                    };
                    let w = text_width(form, text, skin::TEXT_SIZE - 2.0) + 14.0;
                    let tag = Rect::from_min_size(
                        pos2(rect.left() + 6.0, rect.center().y - 9.0),
                        vec2(w, 18.0),
                    );
                    let painter = form.ui.painter();
                    painter.rect_stroke(
                        tag,
                        9.0,
                        Stroke::new(1.0, dim(colour.gamma_multiply(0.7))),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        tag.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        FontId::proportional(skin::TEXT_SIZE - 2.0),
                        dim(colour),
                    );
                    None
                }
                Cell::Chips { chips, joint } => {
                    let painter = form
                        .ui
                        .painter()
                        .with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
                    let mut x = rect.left() + 6.0;
                    for (i, chip) in chips.iter().enumerate() {
                        if i > 0 && !joint.is_empty() {
                            let galley = painter.layout_no_wrap(
                                joint.clone(),
                                FontId::proportional(skin::TEXT_SIZE - 2.0),
                                look.label,
                            );
                            let w = galley.size().x;
                            painter.galley(
                                pos2(x, rect.center().y - galley.size().y / 2.0),
                                galley,
                                look.label,
                            );
                            x += w + 6.0;
                        }
                        let galley = painter.layout_no_wrap(
                            chip.clone(),
                            FontId::proportional(skin::TEXT_SIZE - 1.0),
                            dim(look.text),
                        );
                        let w = galley.size().x + 18.0;
                        let pill =
                            Rect::from_min_size(pos2(x, rect.center().y - 11.0), vec2(w, 22.0));
                        painter.rect_filled(pill, 11.0, look.select_fill);
                        painter.galley(
                            pos2(pill.left() + 9.0, pill.center().y - galley.size().y / 2.0),
                            galley,
                            dim(look.text),
                        );
                        x = pill.right() + 6.0;
                    }
                    None
                }
                Cell::Toggle { id, on } => {
                    let tick = Rect::from_center_size(
                        pos2(rect.left() + 14.0, rect.center().y),
                        vec2(skin::CHECK, skin::CHECK),
                    );
                    let hit = Rect::from_center_size(tick.center(), vec2(28.0, rect.height()));
                    let clicked = form
                        .ui
                        .interact(
                            hit,
                            form.ui.scope_id().with(("plugin.cell", scope, id.as_str())),
                            Sense::click(),
                        )
                        .clicked();
                    tick_box(form, tick, *on);
                    clicked.then(|| Event {
                        id: id.clone(),
                        value: (!*on).to_string(),
                    })
                }
                Cell::Choice { id, options, value } => {
                    let select = Rect::from_min_size(
                        pos2(
                            rect.left() + 4.0,
                            rect.center().y - skin::SELECT_HEIGHT / 2.0,
                        ),
                        vec2(rect.width() - 8.0, skin::SELECT_HEIGHT),
                    );
                    let mut now = value.clone();
                    let choices: Vec<(&str, &str)> = options
                        .iter()
                        .map(|(v, l)| (v.as_str(), l.as_str()))
                        .collect();
                    form.select_at(
                        select,
                        ("plugin.cellchoice", scope, id.as_str()),
                        &mut now,
                        &choices,
                    );
                    (now != *value).then(|| Event {
                        id: id.clone(),
                        value: now,
                    })
                }
                Cell::Buttons { buttons } => {
                    let row_rect = Rect::from_min_max(
                        pos2(rect.left(), rect.center().y - 12.0),
                        pos2(rect.right() - 4.0, rect.center().y + 12.0),
                    );
                    buttons_at(form, scope, row_rect, buttons, true, true)
                }
            };
            if event.is_some() {
                picked = event;
            }
        }
    }
    let frame = Rect::from_min_max(
        pos2(whole.left(), whole.top() - 2.0),
        pos2(whole.right(), *form.at),
    );
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, look.edge),
        egui::StrokeKind::Inside,
    );
    let _ = frame_top;
    *form.at += skin::ROW_GAP + 4.0;
    picked
}

fn columns(
    form: &mut Form<'_, '_>,
    scope: &str,
    width: f32,
    left: &[Node],
    right: &[Node],
    bound: &mut Bound<'_>,
) -> Option<Event> {
    let top = *form.at;
    let split = form.body.left() + skin::INSET + width;
    let left_rect = Rect::from_min_max(
        pos2(form.body.left(), form.body.top()),
        pos2(split + skin::PAD, form.body.bottom()),
    );
    let right_rect = Rect::from_min_max(pos2(split, form.body.top()), form.body.max);
    let look = form.look;
    let mut left_at = top;
    let mut right_at = top;
    let said_left = {
        let mut sub = Form::new(form.ui, look, left_rect, &mut left_at);
        render(&mut sub, left, scope, bound)
    };
    let said_right = {
        let mut sub = Form::new(form.ui, look, right_rect, &mut right_at);
        render(&mut sub, right, scope, bound)
    };
    *form.at = left_at.max(right_at);
    said_left.or(said_right)
}

pub fn show_window(
    root: &mut egui::Ui,
    stem: &str,
    page_id: &str,
    state: &mut State,
    guide: &zwcore::guide::Guide,
    look: &Look,
) -> Option<Pressed> {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let host = std::sync::Arc::clone(&state.plugin_ui);
    let mut host = host.lock().unwrap_or_else(|e| e.into_inner());
    let page = match host.page(stem, page_id, guide) {
        Ok(page) => page,
        Err(why) => zwsdk::ui::Page {
            nodes: vec![Node::Note {
                text: why,
                tone: Tone::Bad,
            }],
            ..Default::default()
        },
    };
    let key = format!("{stem}:{page_id}");
    let stream = crate::plugins::named(stem).is_some_and(|plugin| plugin.has("stream"));
    let mut draft = state.plugin_drafts.remove(stem).unwrap_or_default();
    let mut pressed = None;
    let mut said = None;
    let mut top = full.top();
    if !page.tabs.is_empty() {
        let band = Rect::from_min_size(full.min, vec2(full.width(), TABS));
        root.painter().rect_filled(band, 0.0, look.dark);
        let mut x = band.left() + 18.0;
        for (tab, label) in &page.tabs {
            let galley = root.painter().layout_no_wrap(
                label.clone(),
                FontId::proportional(skin::TEXT_SIZE + 1.0),
                Color32::PLACEHOLDER,
            );
            let rect = Rect::from_min_size(
                pos2(x, band.bottom() - 34.0),
                vec2(galley.size().x + 28.0, 34.0),
            );
            let response = root.interact(
                rect,
                root.scope_id().with(("plugin.tab", &key, tab)),
                Sense::click(),
            );
            let chosen = *tab == page.tab;
            if chosen {
                root.painter().rect_filled(
                    Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max),
                    0.0,
                    look.chosen_mark,
                );
            }
            root.painter().galley(
                pos2(
                    rect.center().x - galley.size().x / 2.0,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                if chosen {
                    look.text
                } else if response.hovered() {
                    look.item
                } else {
                    look.label
                },
            );
            if response.clicked() && !chosen {
                said = Some(Event {
                    id: String::from("tab"),
                    value: tab.clone(),
                });
            }
            x = rect.right() + 4.0;
        }
        top = band.bottom();
    }
    let mut bottom = full.bottom();
    if page.confirm {
        bottom = full.bottom() - skin::FOOT_HEIGHT;
        match super::foot(root, look, full, draft.len()) {
            Some(Pressed::Apply) => {
                settle(stem, &mut draft);
                pressed = Some(Pressed::Replugged(stream));
            }
            Some(Pressed::Ok) => {
                let wrote = settle(stem, &mut draft);
                pressed = Some(if wrote {
                    Pressed::Settled(stream)
                } else {
                    Pressed::Close
                });
            }
            Some(Pressed::Cancel) => {
                draft.clear();
                pressed = Some(Pressed::Close);
            }
            _ => {}
        }
    } else if !page.footer.is_empty() {
        let foot = Rect::from_min_max(
            pos2(full.left(), full.bottom() - skin::FOOT_HEIGHT),
            full.max,
        );
        bottom = foot.top();
        let mut at = 0.0;
        let mut form = Form::foot(root, look, foot, &mut at);
        let row = form.line(skin::SELECT_HEIGHT);
        form.ui.painter().text(
            pos2(foot.left() + 18.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            &page.status,
            FontId::proportional(skin::TEXT_SIZE - 0.5),
            look.label,
        );
        if let Some(event) = buttons_at(&mut form, &key, row, &page.footer, true, false) {
            said = said.or(Some(event));
        }
    }
    let body = Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), bottom));
    let (scroll, content) = state.plugin_scroll.get(&key).copied().unwrap_or_default();
    let scroll = scrolled(root, body, scroll, content);
    let mut at = body.top() + 10.0 - scroll;
    let (browse, wrote) = {
        let mut bound = Bound::new(stem, page.confirm.then_some(&mut draft));
        take_browsed(stem, state, &mut bound);
        {
            let mut inner = root.new_child(egui::UiBuilder::new().max_rect(body));
            inner.set_clip_rect(body);
            let mut form = Form::new(&mut inner, look, body, &mut at);
            if let Some(event) = render(&mut form, &page.nodes, &key, &mut bound) {
                said = said.or(Some(event));
            }
        }
        take_drops(stem, state, &mut bound);
        (bound.browse.take(), bound.wrote)
    };
    if !draft.is_empty() {
        state.plugin_drafts.insert(stem.to_owned(), draft);
    }
    let content = at + scroll - body.top() + 12.0;
    scroll_bar(root, look, body, scroll, content);
    state.plugin_scroll.insert(key, (scroll, content));
    if pressed.is_none() && wrote {
        pressed = Some(Pressed::Replugged(stream));
    }
    if pressed.is_none()
        && let Some(program) = browse
    {
        pressed = Some(Pressed::Browse {
            plugin: stem.to_owned(),
            key: program,
            label: String::from("実行ファイルを選ぶ"),
            folder: false,
        });
    }
    let Some(event) = said else {
        return pressed;
    };
    let reply = host.event(stem, page_id, &event, guide);
    root.ctx().request_repaint();
    if reply.close {
        return Some(pressed.unwrap_or(Pressed::Close));
    }
    reply
        .open
        .map(|open| Pressed::OpenPluginPage(stem.to_owned(), open))
        .or(pressed)
}

fn settle(stem: &str, draft: &mut plugin_bound::Draft) -> bool {
    let wrote = !draft.is_empty();
    for (key, value) in std::mem::take(draft) {
        crate::config::plugins::put(stem, &key, value.as_deref());
    }
    wrote
}

fn take_browsed(stem: &str, state: &mut State, bound: &mut Bound<'_>) {
    let Ok(mut browsed) = state.browsed.lock() else {
        return;
    };
    browsed.retain(|(plugin, key, path)| {
        let mine = plugin == stem
            && bound
                .setting(key)
                .is_some_and(|setting| setting.kind == "program");
        if mine {
            bound.put(key, Some(path.to_string_lossy().into_owned()));
        }
        !mine
    });
}

fn take_drops(stem: &str, state: &mut State, bound: &mut Bound<'_>) {
    let mine: Vec<super::Dropped> = state
        .drops
        .iter()
        .filter(|dropped| dropped.plugin == stem)
        .cloned()
        .collect();
    if mine.is_empty() {
        return;
    }
    state.drops.retain(|dropped| dropped.plugin != stem);
    for dropped in mine {
        let under = dropped.at.and_then(|(x, y)| {
            bound
                .programs
                .iter()
                .find(|(_, rect)| rect.expand(12.0).contains(pos2(x, y)))
        });
        let target = match (under, bound.programs.as_slice()) {
            (Some((key, _)), _) | (None, [(key, _)]) => Some(key.clone()),
            _ => None,
        };
        if let Some(key) = target {
            bound.put(&key, Some(dropped.path.to_string_lossy().into_owned()));
        }
    }
}

/// 下のパネル（`ui.panels`）の中身。ページの見出しとフッターは付けずに `rect` へレンダリングする。
#[allow(clippy::too_many_arguments)]
pub fn show_panel(
    root: &mut egui::Ui,
    rect: Rect,
    stem: &str,
    panel: &str,
    host: &mut crate::plugin_ui::Host,
    scrolls: &mut std::collections::BTreeMap<String, (f32, f32)>,
    guide: &zwcore::guide::Guide,
    look: &Look,
) -> Option<zwsdk::ui::Reply> {
    root.painter().rect_filled(rect, 0.0, look.body);
    let page = match host.page(stem, panel, guide) {
        Ok(page) => page,
        Err(why) => zwsdk::ui::Page {
            nodes: vec![Node::Note {
                text: why,
                tone: Tone::Bad,
            }],
            ..Default::default()
        },
    };
    let key = format!("panel:{stem}:{panel}");
    let (scroll, content) = scrolls.get(&key).copied().unwrap_or_default();
    let scroll = scrolled(root, rect, scroll, content);
    let mut at = rect.top() + 4.0 - scroll;
    let said = {
        let mut inner = root.new_child(egui::UiBuilder::new().max_rect(rect));
        inner.set_clip_rect(rect.intersect(root.clip_rect()));
        let mut form = Form::new(&mut inner, look, rect, &mut at);
        // 下のパネルではキーに対応付けた欄は表示するだけ。
        let mut bound = Bound::read_only(stem);
        render(&mut form, &page.nodes, &key, &mut bound)
    };
    let content = at + scroll - rect.top() + 8.0;
    scroll_bar(root, look, rect, scroll, content);
    scrolls.insert(key, (scroll, content));
    let event = said?;
    let reply = host.event(stem, panel, &event, guide);
    root.ctx().request_repaint();
    Some(reply)
}
