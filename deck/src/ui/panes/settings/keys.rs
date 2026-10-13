// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「キー割り当て」のページ。

use egui::{FontId, Rect, Stroke, pos2, vec2};

use super::{File, Form, skin};

pub(super) fn keys(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    capturing: &mut Option<crate::keys::Action>,
) {
    use crate::keys::{Action, Bindings, Chord};
    let mut bindings = Bindings::of(&edited.keys);
    let mut changed = false;
    if let Some(action) = *capturing {
        let pressed: Option<Option<Chord>> = form.ui.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(if *key == egui::Key::Escape {
                    None
                } else {
                    Some(Chord::of(*key, *modifiers))
                }),
                _ => None,
            })
        });
        if let Some(pressed) = pressed {
            if let Some(chord) = pressed {
                bindings.bind(action, chord);
                changed = true;
            }
            *capturing = None;
        }
    }
    *form.at += 10.0;
    form.note("変えたい操作のキーを押してから、割り当てるキーを押してください（Esc で中止）。");
    let top = *form.at;
    let head = form.line(26.0);
    let key_left = head.right() - 10.0 - 60.0 - 10.0 - 170.0;
    for (x, text) in [(head.left() + 10.0, "操作"), (key_left, "キー")] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    for (index, action) in Action::all().into_iter().enumerate() {
        let row = form.line(32.0);
        form.ui.painter().hline(
            row.x_range(),
            row.top() - 3.0,
            Stroke::new(1.0, form.look.edge),
        );
        let text = form.look.text;
        form.fitted(
            pos2(row.left() + 10.0, row.center().y),
            &action.label(),
            key_left - 10.0 - row.left() - 10.0,
            FontId::proportional(skin::TEXT_SIZE),
            text,
        );
        let chord = bindings.chord_of(action);
        let waiting = *capturing == Some(action);
        let key_rect =
            Rect::from_min_size(pos2(key_left, row.center().y - 13.0), vec2(170.0, 26.0));
        let response = form.ui.interact(
            key_rect,
            form.ui.id().with(("keys.bind", index)),
            egui::Sense::click(),
        );
        let painter = form.ui.painter();
        painter.rect_filled(key_rect, skin::SELECT_ROUNDING, form.look.select_fill);
        painter.rect_stroke(
            key_rect,
            skin::SELECT_ROUNDING,
            Stroke::new(
                1.0,
                if waiting {
                    form.look.primary
                } else {
                    form.look.select_edge
                },
            ),
            egui::StrokeKind::Inside,
        );
        painter.text(
            pos2(key_rect.left() + 10.0, key_rect.center().y),
            egui::Align2::LEFT_CENTER,
            if waiting {
                String::from("キーを押してください")
            } else {
                chord.map_or(String::from("（なし）"), Chord::shown)
            },
            FontId::proportional(skin::TEXT_SIZE),
            if waiting {
                form.look.chosen_text
            } else if chord.is_some() {
                form.look.text
            } else {
                form.look.label
            },
        );
        if response.clicked() {
            *capturing = if waiting { None } else { Some(action) };
        }
        if chord.is_some()
            && !waiting
            && form.button(
                Rect::from_min_size(
                    pos2(key_rect.right() + 10.0, row.center().y - 13.0),
                    vec2(60.0, 26.0),
                ),
                ("keys.unbind", index),
                "解除",
            )
        {
            bindings.unbind(action);
            changed = true;
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
    let row = form.line(skin::SELECT_HEIGHT);
    if form.button(
        Rect::from_min_size(row.min, vec2(100.0, row.height())),
        ("keys.default", 0),
        "既定に戻す",
    ) {
        bindings = Bindings::of(&std::collections::BTreeMap::new());
        changed = true;
    }
    if changed {
        edited.keys = bindings.changes_from_default();
    }
}
