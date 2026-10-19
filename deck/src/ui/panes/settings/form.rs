// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 ページ分のグリッド（[`Form`]）。設定ウィンドウと、その別ウィンドウが同じウィジェットでレンダリングする。

use egui::{Color32, FontId, Painter, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use super::{Look, skin};
use crate::ui::icons::{self, Icon};

pub(super) struct Form<'u, 'a> {
    pub(super) ui: &'u mut Ui,
    pub(super) look: &'a Look,
    pub(super) body: Rect,
    pub(super) at: &'a mut f32,
    /// コマンドラインのオプションと `--set` で上書きされているキー。
    pub(super) overridden: &'a [String],
    pub(super) next_key: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FontField {
    pub(super) row: Rect,
    pub(super) open: bool,
    pub(super) auto: bool,
    pub(super) file: bool,
}

impl<'u, 'a> Form<'u, 'a> {
    pub(super) fn new(ui: &'u mut Ui, look: &'a Look, body: Rect, at: &'a mut f32) -> Self {
        Self {
            ui,
            look,
            body,
            at,
            overridden: &[],
            next_key: None,
        }
    }

    pub(super) fn foot(root: &'u mut Ui, look: &'a Look, foot: Rect, at: &'a mut f32) -> Self {
        root.painter().rect_filled(foot, 0.0, look.dark);
        root.painter().line_segment(
            [foot.left_top(), foot.right_top()],
            Stroke::new(1.0, look.edge),
        );
        *at = foot.top() + 8.0;
        Self::new(root, look, foot, at)
    }
}

impl Form<'_, '_> {
    /// 上書きの目印のためで、行の見出しではない。
    pub(super) fn key(&mut self, key: &'static str) {
        self.next_key = Some(key);
    }

    pub(super) fn section(&mut self, label: &str) {
        *self.at += 14.0;
        self.ui.painter().text(
            pos2(self.body.left() + skin::INSET + skin::DOT, *self.at),
            egui::Align2::LEFT_TOP,
            label,
            FontId::proportional(skin::SECTION_SIZE),
            self.look.section,
        );
        *self.at += 20.0;
    }

    fn inset(&self) -> f32 {
        self.body.left() + skin::INSET
    }

    pub(super) fn label_left(&self) -> f32 {
        self.inset() + skin::DOT + skin::GAP
    }

    pub(super) fn value_left(&self) -> f32 {
        self.label_left() + self.label_width() + skin::GAP
    }

    fn label_width(&self) -> f32 {
        skin::LABEL.min(((self.body.width() - 2.0 * skin::PAD) * 0.34).max(96.0))
    }

    fn value_right(&self) -> f32 {
        self.body.right() - skin::PAD
    }

    pub(super) fn fit(&self, width: f32) -> f32 {
        width.min((self.value_right() - self.value_left()).max(40.0))
    }

    fn dot(&self, row: Rect, changed: bool) {
        if changed {
            self.ui.painter().circle_filled(
                pos2(self.inset() + 3.0, row.center().y),
                3.0,
                self.look.changed,
            );
        }
    }

    fn overridden_at(&mut self, row: Rect) -> bool {
        let key = self.next_key.take();
        let overridden = key.is_some_and(|key| self.overridden.iter().any(|k| k == key));
        if overridden {
            self.ui.painter().text(
                pos2(row.right(), row.center().y),
                egui::Align2::RIGHT_CENTER,
                "コマンドラインで指定中",
                FontId::proportional(skin::TEXT_SIZE - 1.0),
                self.look.typed,
            );
        }
        overridden
    }

    fn row(&mut self, label: &str, changed: bool) -> Rect {
        let rect = Rect::from_min_max(
            pos2(self.inset(), *self.at),
            pos2(self.body.right() - skin::PAD, *self.at + skin::ROW),
        );
        let overridden = self.overridden_at(rect);
        self.dot(rect, changed);
        let colour = if overridden {
            self.look.group
        } else {
            self.look.label
        };
        let room = (self.value_left() - self.label_left() - skin::PAD).max(0.0);
        let mut job = egui::text::LayoutJob::simple_singleline(
            label.to_owned(),
            FontId::proportional(skin::TEXT_SIZE),
            colour,
        );
        job.wrap = egui::text::TextWrapping {
            max_width: room,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = self.ui.painter().layout_job(job);
        let at = pos2(self.label_left(), rect.center().y - galley.size().y / 2.0);
        if galley.elided {
            let hover = Rect::from_min_size(at, galley.size());
            self.ui
                .interact(
                    hover,
                    self.ui.scope_id().with(("ラベル", label)),
                    egui::Sense::hover(),
                )
                .on_hover_text(label);
        }
        self.ui.painter().galley(at, galley, colour);
        *self.at = rect.bottom() + skin::ROW_GAP;
        Rect::from_min_max(pos2(self.value_left(), rect.top()), rect.max)
    }

    pub(super) fn label_row(&mut self, label: &str, changed: bool) -> Rect {
        self.row(label, changed)
    }

    pub(super) fn told_row(&mut self, label: &str) -> Rect {
        self.row(label, false)
    }

    fn centred(value: Rect, width: f32, height: f32) -> Rect {
        Rect::from_min_size(
            pos2(value.left(), value.center().y - height / 2.0),
            vec2(width, height),
        )
    }

    pub(super) fn select(
        &mut self,
        label: &str,
        value: &mut String,
        was: &str,
        choices: &[(&str, &str)],
    ) {
        let value_rect = self.row(label, value != was);
        let rect = Self::centred(
            value_rect,
            self.fit(skin::SELECT_WIDTH),
            skin::SELECT_HEIGHT,
        );
        let id = self.ui.scope_id().with(("選ぶ", label));
        self.combo(rect, id, value, choices);
    }

    pub(super) fn select_row(
        &mut self,
        label: &str,
        id: impl std::hash::Hash + std::fmt::Debug,
        value: &mut String,
        choices: &[(&str, &str)],
        width: f32,
    ) -> Rect {
        let value_rect = self.row(label, false);
        let rect = Self::centred(value_rect, self.fit(width), skin::SELECT_HEIGHT);
        self.select_at(rect, id, value, choices);
        rect
    }

    pub(super) fn select_at(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        value: &mut String,
        choices: &[(&str, &str)],
    ) {
        let id = self.ui.scope_id().with(id);
        self.combo(rect, id, value, choices);
    }

    fn combo(&mut self, rect: Rect, id: egui::Id, value: &mut String, choices: &[(&str, &str)]) {
        let look = self.look;
        let shown = choices
            .iter()
            .find(|(spelling, _)| *spelling == value.as_str())
            .map_or(value.as_str(), |(_, name)| *name)
            .to_owned();
        let mut inner = self.ui.new_child(egui::UiBuilder::new().max_rect(rect));
        inner.spacing_mut().interact_size.y = rect.height();
        let widgets = &mut inner.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = look.combo;
        widgets.hovered.weak_bg_fill = look.combo_hover;
        widgets.open.weak_bg_fill = look.combo_hover;
        let response = egui::ComboBox::from_id_salt(id)
            .selected_text(
                egui::RichText::new(shown)
                    .font(FontId::proportional(skin::TEXT_SIZE))
                    .color(look.text),
            )
            .width(rect.width())
            .height(320.0)
            .show_ui(&mut inner, |ui| {
                for (spelling, name) in choices {
                    if ui
                        .selectable_label(*spelling == value.as_str(), *name)
                        .clicked()
                    {
                        *value = (*spelling).to_owned();
                    }
                }
            });
        self.ui.painter().rect_stroke(
            response.response.rect,
            skin::SELECT_ROUNDING,
            Stroke::new(1.0, look.select_edge),
            egui::StrokeKind::Inside,
        );
    }

    pub(super) fn check(&mut self, label: &str, said: &str, on: &mut bool, was: bool) {
        let changed = *on != was;
        let (box_left, middle, right) = if label.is_empty() {
            let rect = Rect::from_min_max(
                pos2(self.inset(), *self.at),
                pos2(self.body.right() - skin::PAD, *self.at + skin::ROW),
            );
            let _ = self.overridden_at(rect);
            self.dot(rect, changed);
            *self.at = rect.bottom() + skin::ROW_GAP;
            (
                self.inset() + skin::DOT + 6.0,
                rect.center().y,
                rect.right(),
            )
        } else {
            let value = self.row(label, changed);
            (value.left(), value.center().y, value.right())
        };
        let box_rect = Rect::from_min_size(
            pos2(box_left, middle - skin::CHECK / 2.0),
            vec2(skin::CHECK, skin::CHECK),
        );
        let look = self.look;
        let mut job = egui::text::LayoutJob::simple_singleline(
            said.to_owned(),
            FontId::proportional(skin::TEXT_SIZE),
            look.text,
        );
        job.wrap = egui::text::TextWrapping {
            max_width: (right - box_rect.right() - 8.0).max(20.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = self.ui.painter().layout_job(job);
        if galley.elided {
            let hover = Rect::from_min_size(
                pos2(box_rect.right() + 8.0, middle - galley.size().y / 2.0),
                galley.size(),
            );
            self.ui
                .interact(
                    hover,
                    self.ui.scope_id().with(("切った印", said)),
                    Sense::hover(),
                )
                .on_hover_text(said);
        }
        let hit = Rect::from_min_max(
            pos2(box_rect.left(), middle - skin::ROW / 2.0 + 2.0),
            pos2(
                (box_rect.right() + 8.0 + galley.size().x).min(right),
                middle + skin::ROW / 2.0 - 2.0,
            ),
        );
        let response = self.ui.interact(
            hit,
            self.ui.scope_id().with(("印", label, said)),
            Sense::click(),
        );
        if response.clicked() {
            *on = !*on;
        }
        let painter = self.ui.painter();
        tick_box(painter, look, box_rect, *on);
        painter.galley(
            pos2(box_rect.right() + 8.0, middle - galley.size().y / 2.0),
            galley,
            look.text,
        );
    }

    /// チェックと設定の値が一対一でない物のチップの行。`items` は（名前、現在のオン・オフ、元のオン・オフ）。
    pub(super) fn chips(&mut self, label: &str, items: &[(String, bool, bool)]) -> Option<usize> {
        let changed = items.iter().any(|(_, on, was)| on != was);
        let top = *self.at;
        let left = if label.is_empty() {
            self.inset() + skin::DOT + 6.0
        } else {
            self.value_left()
        };
        let right = self.body.right() - skin::PAD;
        let (hit, bottom) = self.chips_at(pos2(left, top + 4.0), right, label, items);
        let rect = Rect::from_min_max(pos2(self.inset(), top), pos2(right, top + skin::ROW));
        let _ = self.overridden_at(rect);
        self.dot(rect, changed);
        if !label.is_empty() {
            self.ui.painter().text(
                pos2(self.label_left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                FontId::proportional(skin::TEXT_SIZE),
                self.look.label,
            );
        }
        *self.at = bottom.max(rect.bottom()) + skin::ROW_GAP;
        hit
    }

    pub(super) fn chips_at(
        &mut self,
        from: egui::Pos2,
        right: f32,
        id: impl std::hash::Hash + std::fmt::Debug + Copy,
        items: &[(String, bool, bool)],
    ) -> (Option<usize>, f32) {
        let look = self.look;
        let height = 24.0;
        let mut at = from;
        let mut hit = None;
        for (index, (said, on, _)) in items.iter().enumerate() {
            let galley = self.ui.painter().layout_no_wrap(
                said.clone(),
                FontId::proportional(skin::TEXT_SIZE),
                look.text,
            );
            let width = 8.0 + skin::CHECK + 5.0 + galley.size().x + 8.0;
            if at.x > from.x && at.x + width > right {
                at = pos2(from.x, at.y + height + 6.0);
            }
            let chip = Rect::from_min_size(at, vec2(width, height));
            let response = self.ui.interact(
                chip,
                self.ui.scope_id().with(("チップ", id, index)),
                Sense::click(),
            );
            if response.clicked() {
                hit = Some(index);
            }
            let painter = self.ui.painter();
            if *on {
                painter.rect_filled(chip, height / 2.0, look.chosen_fill);
            } else if response.hovered() {
                painter.rect_filled(chip, height / 2.0, look.select_fill);
            }
            painter.rect_stroke(
                chip,
                height / 2.0,
                Stroke::new(
                    1.0,
                    if *on {
                        look.chosen_mark.gamma_multiply(0.7)
                    } else {
                        look.button_edge
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let box_rect = Rect::from_min_size(
                pos2(chip.left() + 8.0, chip.center().y - skin::CHECK / 2.0),
                vec2(skin::CHECK, skin::CHECK),
            );
            tick_box(painter, look, box_rect, *on);
            painter.galley(
                pos2(
                    box_rect.right() + 5.0,
                    chip.center().y - galley.size().y / 2.0,
                ),
                galley,
                look.text,
            );
            at.x = chip.right() + 6.0;
        }
        (hit, at.y + height)
    }

    pub(super) fn slider<N: egui::emath::Numeric + PartialEq + Copy + std::fmt::Display>(
        &mut self,
        label: &str,
        value: &mut N,
        was: N,
        range: std::ops::RangeInclusive<N>,
        unit: &str,
    ) {
        self.slider_leaving(label, value, was, range, unit, 0.0);
    }

    fn slider_leaving<N: egui::emath::Numeric + PartialEq + Copy + std::fmt::Display>(
        &mut self,
        label: &str,
        value: &mut N,
        was: N,
        range: std::ops::RangeInclusive<N>,
        unit: &str,
        leave: f32,
    ) -> f32 {
        let value_rect = self.row(label, *value != was);
        let look = self.look;
        let room = self.value_right() - value_rect.left() - 10.0 - skin::NUMBER - 34.0 - leave;
        let slider = skin::SLIDER.min(room).max(60.0);
        let bar = Self::centred(value_rect, slider, skin::SELECT_HEIGHT);
        let mut inner = self.ui.new_child(egui::UiBuilder::new().max_rect(bar));
        inner.spacing_mut().slider_width = slider;
        inner.visuals_mut().selection.bg_fill = look.primary;
        inner.add(
            egui::Slider::new(value, range.clone())
                .show_value(false)
                .trailing_fill(true)
                .logarithmic(
                    range.start().to_f64() >= 1.0
                        && range.end().to_f64() >= 1000.0 * range.start().to_f64(),
                ),
        );
        let field = Rect::from_min_size(
            pos2(bar.right() + 10.0, value_rect.center().y - 13.0),
            vec2(skin::NUMBER, 26.0),
        );
        self.number_box(field, value, range);
        self.ui.painter().text(
            pos2(field.right() + 6.0, value_rect.center().y),
            egui::Align2::LEFT_CENTER,
            unit.trim(),
            FontId::proportional(skin::TEXT_SIZE),
            look.label,
        );
        slider
    }

    pub(super) fn number_box<N: egui::emath::Numeric>(
        &mut self,
        field: Rect,
        value: &mut N,
        range: std::ops::RangeInclusive<N>,
    ) {
        let look = self.look;
        let painter = self.ui.painter();
        painter.rect_filled(field, skin::SELECT_ROUNDING, look.select_fill);
        painter.rect_stroke(
            field,
            skin::SELECT_ROUNDING,
            Stroke::new(1.0, look.select_edge),
            egui::StrokeKind::Inside,
        );
        let mut number = self
            .ui
            .new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(6.0, 3.0))));
        number.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        number.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
        number.visuals_mut().widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
        number.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
        number.visuals_mut().widgets.active.weak_bg_fill = Color32::TRANSPARENT;
        number.visuals_mut().override_text_color = Some(look.text);
        number.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_sized(
                vec2(field.width() - 12.0, 20.0),
                egui::DragValue::new(value).range(range).speed(0.2),
            );
        });
    }

    pub(super) fn slider_reset<N: egui::emath::Numeric + PartialEq + Copy + std::fmt::Display>(
        &mut self,
        label: &str,
        value: &mut N,
        was: N,
        range: std::ops::RangeInclusive<N>,
        unit: &str,
        default: N,
    ) {
        let top = *self.at;
        let slider = self.slider_leaving(label, value, was, range, unit, skin::FONT_BUTTON + 4.0);
        let row = Rect::from_min_max(
            pos2(self.value_left(), top),
            pos2(self.body.right(), top + skin::ROW),
        );
        let button = Rect::from_min_size(
            pos2(
                row.left() + slider + 10.0 + skin::NUMBER + 34.0,
                row.center().y - skin::SELECT_HEIGHT / 2.0,
            ),
            vec2(skin::FONT_BUTTON, skin::SELECT_HEIGHT),
        );
        if self.button_enabled(button, ("既定に戻す", label), "既定", *value != default) {
            *value = default;
        }
    }

    pub(super) fn text(&mut self, label: &str, value: &mut String, was: &str, hint: &str) {
        self.text_sized(label, value, was, hint, skin::SELECT_WIDTH);
    }

    pub(super) fn text_sized(
        &mut self,
        label: &str,
        value: &mut String,
        was: &str,
        hint: &str,
        width: f32,
    ) {
        let rect = self.row(label, value != was);
        let field = Self::centred(rect, self.fit(width), skin::SELECT_HEIGHT);
        self.edit(field, vec2(8.0, 5.0), None, value, hint);
    }

    /// 起動するプログラムのパスの欄と「参照…」。返すのは行の場所（ドロップの当たり判定）と押されたか。
    pub(super) fn program(
        &mut self,
        label: &str,
        value: &mut String,
        was: &str,
        hint: &str,
        id: impl std::hash::Hash + std::fmt::Debug,
    ) -> (Rect, bool) {
        let rect = self.row(label, value != was);
        let whole = Rect::from_min_max(
            pos2(self.body.left(), rect.top() - 3.0),
            pos2(self.body.right(), rect.bottom() + 3.0),
        );
        let width = skin::SELECT_WIDTH.min(rect.width());
        let button = skin::FILE_BUTTON;
        let field = Self::centred(rect, (width - button - 6.0).max(40.0), skin::SELECT_HEIGHT);
        self.edit(field, vec2(8.0, 5.0), None, value, hint);
        let browse = self.button(
            Rect::from_min_size(
                pos2(field.right() + 6.0, field.top()),
                vec2(button, field.height()),
            ),
            ("program.browse", id),
            "参照…",
        );
        (whole, browse)
    }

    pub(super) fn line(&mut self, height: f32) -> Rect {
        let rect = Rect::from_min_max(
            pos2(self.inset() + skin::DOT, *self.at),
            pos2(self.body.right() - skin::PAD, *self.at + height),
        );
        *self.at = rect.bottom() + 6.0;
        rect
    }

    pub(super) fn field(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        value: &mut String,
        hint: &str,
    ) {
        let id = self.ui.scope_id().with(id);
        self.edit(rect, vec2(8.0, 4.0), Some(id), value, hint);
    }

    fn edit(
        &mut self,
        frame: Rect,
        pad: Vec2,
        id: Option<egui::Id>,
        value: &mut String,
        hint: &str,
    ) {
        let look = self.look;
        let painter = self.ui.painter();
        painter.rect_filled(frame, skin::SELECT_ROUNDING, look.select_fill);
        painter.rect_stroke(
            frame,
            skin::SELECT_ROUNDING,
            Stroke::new(1.0, look.select_edge),
            egui::StrokeKind::Inside,
        );
        let mut inner = self
            .ui
            .new_child(egui::UiBuilder::new().max_rect(frame.shrink2(pad)));
        let mut edit = egui::TextEdit::singleline(value)
            .frame(egui::Frame::NONE)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(skin::TEXT_SIZE))
            .text_color(look.text);
        if let Some(id) = id {
            edit = edit.id(id);
        }
        inner.add(edit);
    }

    pub(super) fn button<'l>(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        label: impl Into<Label<'l>>,
    ) -> bool {
        let label = label.into();
        let look = self.look;
        let response = self
            .ui
            .interact(rect, self.ui.scope_id().with(id), Sense::click());
        let fill = if response.hovered() {
            look.hover
        } else {
            look.button_fill
        };
        self.paint_button(rect, label, fill, look.text);
        response.clicked()
    }

    /// フォントの欄。押すと選択のウィンドウを開く。
    pub(super) fn font_field(
        &mut self,
        label: &str,
        value: &str,
        changed: bool,
        auto: &str,
        files: bool,
        id: impl std::hash::Hash + std::fmt::Debug,
    ) -> FontField {
        let rect = self.row(label, changed);
        let row = Rect::from_min_max(
            pos2(self.body.left(), rect.top() - 3.0),
            pos2(self.body.right(), rect.bottom() + 3.0),
        );
        let look = self.look;
        let field = Self::centred(
            rect,
            skin::SELECT_WIDTH.min(rect.width()),
            skin::SELECT_HEIGHT,
        );
        let response = self.ui.interact(
            field,
            self.ui.scope_id().with(("font.field", id)),
            Sense::click(),
        );
        let painter = self.ui.painter();
        let fill = if response.hovered() {
            look.hover
        } else {
            look.select_fill
        };
        painter.rect_filled(field, skin::SELECT_ROUNDING, fill);
        painter.rect_stroke(
            field,
            skin::SELECT_ROUNDING,
            Stroke::new(1.0, look.select_edge),
            egui::StrokeKind::Inside,
        );
        painter.text(
            pos2(field.right() - 10.0, field.center().y),
            egui::Align2::RIGHT_CENTER,
            "…",
            FontId::proportional(skin::TEXT_SIZE),
            look.group,
        );
        let text = Rect::from_min_max(
            pos2(field.left() + 10.0, field.top() + 2.0),
            pos2(field.right() - 26.0, field.bottom() - 2.0),
        );
        let value = value.trim();
        let path = std::path::Path::new(value.trim_matches('"'));
        let clip = painter.with_clip_rect(text.intersect(painter.clip_rect()));
        if value.is_empty() {
            clip.text(
                pos2(text.left(), text.center().y),
                egui::Align2::LEFT_CENTER,
                auto,
                FontId::proportional(skin::TEXT_SIZE),
                look.group,
            );
        } else if let Some(sample) =
            super::font_pick::named_sample(self.ui.ctx(), value, skin::TEXT_SIZE)
        {
            super::font_pick::samples::paint_left(&clip, text, sample, look.text);
        } else {
            let shown = if path.is_absolute() {
                path.file_name().map_or_else(
                    || value.to_owned(),
                    |name| name.to_string_lossy().into_owned(),
                )
            } else {
                value.to_owned()
            };
            clip.text(
                pos2(text.left(), text.center().y),
                egui::Align2::LEFT_CENTER,
                shown,
                FontId::proportional(skin::TEXT_SIZE),
                look.text,
            );
        }
        let mut field = FontField {
            row,
            open: response.clicked(),
            auto: false,
            file: false,
        };
        response.context_menu(|menu| {
            if menu
                .add_enabled(!value.is_empty(), egui::Button::new("自動に戻す"))
                .clicked()
            {
                field.auto = true;
                menu.close();
            }
            if files && menu.button("ファイルを選ぶ…").clicked() {
                field.file = true;
                menu.close();
            }
        });
        if !value.is_empty() {
            response.on_hover_text(value);
        }
        field
    }

    pub(super) fn button_enabled<'l>(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        label: impl Into<Label<'l>>,
        enabled: bool,
    ) -> bool {
        let label = label.into();
        if enabled {
            return self.button(rect, id, label);
        }
        self.paint_button(rect, label, self.look.button_fill, self.look.label);
        false
    }

    pub(super) fn primary_button<'l>(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        label: impl Into<Label<'l>>,
        enabled: bool,
    ) -> bool {
        let label = label.into();
        if !enabled {
            self.paint_button(rect, label, self.look.button_fill, self.look.label);
            return false;
        }
        let response = self
            .ui
            .interact(rect, self.ui.scope_id().with(id), Sense::click());
        let painter = self.ui.painter();
        painter.rect_filled(rect, skin::BUTTON_ROUNDING, self.look.primary);
        label.paint(painter, rect, Color32::WHITE);
        response.clicked()
    }

    fn paint_button(&self, rect: Rect, label: Label<'_>, fill: Color32, colour: Color32) {
        let painter = self.ui.painter();
        painter.rect_filled(rect, skin::BUTTON_ROUNDING, fill);
        painter.rect_stroke(
            rect,
            skin::BUTTON_ROUNDING,
            Stroke::new(1.0, self.look.button_edge),
            egui::StrokeKind::Inside,
        );
        label.paint(painter, rect, colour);
    }

    pub(super) fn menu_button(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        label: &str,
        items: &[&str],
    ) -> Option<usize> {
        let look = self.look;
        let response = self
            .ui
            .interact(rect, self.ui.scope_id().with(id), Sense::click());
        let fill = if response.hovered() {
            look.hover
        } else {
            look.button_fill
        };
        self.paint_button(rect, label.into(), fill, look.text);
        let mut picked = None;
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(rect.width().max(140.0));
            for (index, item) in items.iter().enumerate() {
                if ui
                    .button(
                        egui::RichText::new(*item)
                            .font(FontId::proportional(skin::TEXT_SIZE))
                            .color(look.text),
                    )
                    .clicked()
                {
                    picked = Some(index);
                    ui.close();
                }
            }
        });
        picked
    }

    pub(super) fn kind_tag(&mut self, at: egui::Pos2, kind: &str) -> f32 {
        let (text, colour) = match kind {
            "mirakurun" => ("Mirakurun", self.look.mirakurun),
            _ => ("coaxd", self.look.coaxd),
        };
        let painter = self.ui.painter();
        let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(10.0), colour);
        let rect = Rect::from_min_size(pos2(at.x, at.y - 8.0), vec2(galley.size().x + 10.0, 16.0));
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, colour.gamma_multiply(0.75)),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            pos2(rect.left() + 5.0, rect.center().y - galley.size().y / 2.0),
            galley,
            colour,
        );
        rect.width()
    }

    pub(super) fn tick(
        &mut self,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        on: bool,
    ) -> bool {
        let box_rect = Rect::from_center_size(rect.center(), vec2(skin::CHECK, skin::CHECK));
        let response = self
            .ui
            .interact(rect, self.ui.scope_id().with(id), Sense::click());
        tick_box(self.ui.painter(), self.look, box_rect, on);
        response.clicked()
    }

    pub(super) fn told(&mut self, label: &str, value: &str) {
        let colour = self.look.text;
        self.told_in(label, value, colour);
    }

    pub(super) fn told_in(&mut self, label: &str, value: &str, colour: Color32) {
        let rect = self.row(label, false);
        let room = self.value_right() - rect.left();
        self.fitted(
            pos2(rect.left(), rect.center().y),
            value,
            room,
            FontId::proportional(skin::TEXT_SIZE),
            colour,
        );
    }

    pub(super) fn fitted(
        &mut self,
        at: egui::Pos2,
        text: &str,
        room: f32,
        font: FontId,
        colour: Color32,
    ) {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
        job.wrap = egui::text::TextWrapping {
            max_width: room.max(20.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = self.ui.painter().layout_job(job);
        let top_left = pos2(at.x, at.y - galley.size().y / 2.0);
        if galley.elided {
            let hover = Rect::from_min_size(top_left, galley.size());
            self.ui
                .interact(
                    hover,
                    self.ui.scope_id().with(("切った字", text)),
                    Sense::hover(),
                )
                .on_hover_text(text);
        }
        self.ui.painter().galley(top_left, galley, colour);
    }

    pub(super) fn told_with_button(
        &mut self,
        label: &str,
        value: &str,
        id: impl std::hash::Hash + std::fmt::Debug,
        button: &str,
        width: f32,
    ) -> bool {
        let rect = self.row(label, false);
        let galley = self.ui.painter().layout_no_wrap(
            value.to_owned(),
            FontId::proportional(skin::TEXT_SIZE),
            self.look.text,
        );
        let shown = galley.size().x.max(90.0);
        self.ui.painter().galley(
            pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            self.look.text,
        );
        self.button(
            Self::centred(
                Rect::from_min_max(pos2(rect.left() + shown + 10.0, rect.top()), rect.max),
                width,
                skin::SELECT_HEIGHT,
            ),
            id,
            button,
        )
    }

    pub(super) fn note(&mut self, text: impl ToString) {
        let left = self.inset() + skin::DOT;
        self.wrapped(left, text.to_string());
    }

    pub(super) fn hint(&mut self, text: impl ToString) {
        let left = self.value_left();
        *self.at -= skin::ROW_GAP;
        self.wrapped(left, text.to_string());
    }

    fn wrapped(&mut self, left: f32, text: String) {
        let colour = self.look.label;
        self.wrapped_in(left, text, colour, skin::TEXT_SIZE - 1.0);
        *self.at += skin::ROW_GAP;
    }

    pub(super) fn wrapped_in(&mut self, left: f32, text: String, colour: Color32, size: f32) {
        let width = self.value_right() - left;
        let galley =
            self.ui
                .painter()
                .layout(text, FontId::proportional(size), colour, width.max(40.0));
        let height = galley.size().y;
        self.ui
            .painter()
            .galley(pos2(left, *self.at + 2.0), galley, colour);
        *self.at += height + 4.0;
    }
}

#[derive(Debug, Clone)]
pub(super) struct Layer {
    pub key: String,
    pub name: String,
    pub plugin: bool,
    pub on: bool,
    pub was: bool,
    pub fixed: bool,
}

impl Form<'_, '_> {
    pub(super) fn layers(&mut self, rows: &mut [Layer]) {
        const HEIGHT: f32 = 32.0;
        let left = self.inset() + skin::DOT + 6.0;
        let whole = Rect::from_min_max(
            pos2(left, *self.at),
            pos2(
                self.body.right() - skin::PAD,
                *self.at + HEIGHT * rows.len() as f32,
            ),
        );
        let look = self.look;
        let count = rows.len();
        for (nth, row) in rows.iter_mut().enumerate() {
            let line = Rect::from_min_size(
                pos2(whole.left(), whole.top() + HEIGHT * nth as f32),
                vec2(whole.width(), HEIGHT),
            );
            let mark =
                Rect::from_min_max(pos2(self.inset(), line.top()), pos2(left, line.bottom()));
            self.dot(mark, row.on != row.was);
            if !row.fixed {
                let response = self.ui.interact(
                    line,
                    self.ui.scope_id().with(("レイヤー", &row.key)),
                    Sense::click(),
                );
                if response.clicked() {
                    row.on = !row.on;
                }
            }
            let painter = self.ui.painter();
            if row.plugin {
                let top = if nth == 0 { 5 } else { 0 };
                let bottom = if nth + 1 == count { 5 } else { 0 };
                let rounding = egui::CornerRadius {
                    nw: top,
                    ne: top,
                    sw: bottom,
                    se: bottom,
                };
                painter.rect_filled(line.shrink(1.0), rounding, look.plugin_row);
            }
            if nth > 0 {
                painter.hline(
                    line.left() + 1.0..=line.right() - 1.0,
                    line.top(),
                    Stroke::new(1.0, look.rule),
                );
            }
            let box_rect = Rect::from_min_size(
                pos2(line.left() + 12.0, line.center().y - skin::CHECK / 2.0),
                vec2(skin::CHECK, skin::CHECK),
            );
            tick_box(painter, look, box_rect, row.on);
            let mut x = box_rect.right() + 10.0;
            if row.plugin {
                let tag = painter.layout_no_wrap(
                    String::from("プラグイン"),
                    FontId::proportional(10.5),
                    look.plugin_tag,
                );
                let tag_rect = Rect::from_min_size(
                    pos2(x, line.center().y - tag.size().y / 2.0 - 1.0),
                    tag.size() + vec2(10.0, 2.0),
                );
                painter.rect_stroke(
                    tag_rect,
                    3.0,
                    Stroke::new(1.0, look.plugin_tag_edge),
                    egui::StrokeKind::Inside,
                );
                painter.galley(tag_rect.min + vec2(5.0, 1.0), tag, look.plugin_tag);
                x = tag_rect.right() + 6.0;
            }
            painter.text(
                pos2(x, line.center().y),
                egui::Align2::LEFT_CENTER,
                &row.name,
                FontId::proportional(skin::TEXT_SIZE),
                if row.fixed { look.label } else { look.text },
            );
        }
        self.ui.painter().rect_stroke(
            whole,
            6.0,
            Stroke::new(1.0, look.select_edge),
            egui::StrokeKind::Inside,
        );
        *self.at = whole.bottom() + skin::ROW_GAP + 4.0;
    }
}

fn tick_box(painter: &Painter, look: &Look, rect: Rect, on: bool) {
    let (fill, edge) = if on {
        (look.tick_fill, look.tick_edge)
    } else {
        (look.select_fill, look.select_edge)
    };
    painter.rect_filled(rect, 2.0, fill);
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    if on {
        icons::paint(painter, rect.shrink(1.5), Icon::Check, look.tick_mark);
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Label<'a> {
    Text(&'a str),
    Icon(Icon),
}

impl<'a> From<&'a str> for Label<'a> {
    fn from(text: &'a str) -> Self {
        Self::Text(text)
    }
}

impl<'a> From<&'a String> for Label<'a> {
    fn from(text: &'a String) -> Self {
        Self::Text(text)
    }
}

impl From<Icon> for Label<'_> {
    fn from(icon: Icon) -> Self {
        Self::Icon(icon)
    }
}

impl Label<'_> {
    fn paint(self, painter: &Painter, rect: Rect, colour: Color32) {
        match self {
            Self::Text(text) => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    FontId::proportional(skin::TEXT_SIZE),
                    colour,
                );
            }
            Self::Icon(icon) => {
                icons::paint_at(painter, rect.center(), skin::ICON, icon, colour);
            }
        }
    }
}
