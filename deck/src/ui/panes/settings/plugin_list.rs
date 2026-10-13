// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインの設定の一覧（`kind = "list"`）とテーブル（`kind = "map"`）の欄と小ウィンドウ。
//! 値は `zwsdk::config` の表記（JSON の文字列）で受け、変えたら新しい文字列を返す。

use egui::{FontId, Key, Rect, Sense, Stroke, pos2, vec2};

use super::form::Form;
use super::{Look, Pressed, skin};

const LINE: f32 = 26.0;
const SIDE_BUTTON: f32 = 132.0;
const BUTTON: f32 = 26.0;

/// プラグインのウィンドウどうしで一つを共有する。
#[derive(Debug, Clone, Default)]
pub struct Lists {
    marks: std::collections::BTreeMap<(String, String), usize>,
    inline: Option<Inline>,
    pub(super) dialog: Option<Dialog>,
    browsing: Option<(String, String, Option<usize>)>,
}

#[derive(Debug, Clone)]
struct Inline {
    plugin: String,
    key: String,
    row: usize,
    text: String,
    new: bool,
    focus: bool,
}

#[derive(Debug, Clone)]
pub(super) enum Dialog {
    Text {
        plugin: String,
        key: String,
        label: String,
        text: String,
    },
    Pair {
        plugin: String,
        key: String,
        label: String,
        env: bool,
        was: Option<String>,
        name: String,
        value: String,
        pairs: Vec<(String, String)>,
        focus: bool,
    },
}

impl Dialog {
    fn plugin(&self) -> &str {
        match self {
            Self::Text { plugin, .. } | Self::Pair { plugin, .. } => plugin,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub changed: Option<String>,
    pub pressed: Option<Pressed>,
}

fn caption(form: &mut Form<'_, '_>, label: &str) {
    let left = form.label_left();
    let top = *form.at;
    form.ui.painter().text(
        pos2(left, top + 10.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(skin::TEXT_SIZE),
        form.look.label,
    );
    *form.at = top + 24.0;
}

fn frame(form: &Form<'_, '_>, table: Rect) {
    let painter = form.ui.painter();
    painter.rect_filled(table, skin::SELECT_ROUNDING, form.look.select_fill);
    painter.rect_stroke(
        table,
        skin::SELECT_ROUNDING,
        Stroke::new(1.0, form.look.select_edge),
        egui::StrokeKind::Inside,
    );
}

fn none(form: &Form<'_, '_>, table: Rect, top: f32) {
    form.ui.painter().text(
        pos2(table.left() + 10.0, top + LINE / 2.0),
        egui::Align2::LEFT_CENTER,
        "（ありません）",
        FontId::proportional(skin::TEXT_SIZE),
        form.look.label,
    );
}

fn cell(form: &Form<'_, '_>, rect: Rect, text: &str, chosen: bool) {
    let look = form.look;
    form.ui.painter().with_clip_rect(rect).text(
        pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(skin::TEXT_SIZE),
        if chosen { look.chosen_text } else { look.text },
    );
}

impl Lists {
    fn mark(&self, plugin: &str, key: &str) -> Option<usize> {
        self.marks
            .get(&(plugin.to_owned(), key.to_owned()))
            .copied()
    }

    fn set_mark(&mut self, plugin: &str, key: &str, row: Option<usize>) {
        let id = (plugin.to_owned(), key.to_owned());
        match row {
            Some(row) => {
                self.marks.insert(id, row);
            }
            None => {
                self.marks.remove(&id);
            }
        }
    }

    fn editing(&self, plugin: &str, key: &str) -> Option<&Inline> {
        self.inline
            .as_ref()
            .filter(|inline| inline.plugin == plugin && inline.key == key)
    }

    fn commit(&mut self, plugin: &str, key: &str, items: &mut Vec<String>) -> bool {
        if self.editing(plugin, key).is_none() {
            return false;
        }
        let Some(inline) = self.inline.take() else {
            return false;
        };
        if inline.new {
            if inline.text.is_empty() {
                let mark = self.mark(plugin, key).filter(|_| !items.is_empty());
                self.set_mark(plugin, key, mark.map(|m| m.min(items.len() - 1)));
                return false;
            }
            let at = inline.row.min(items.len());
            items.insert(at, inline.text);
            self.set_mark(plugin, key, Some(at));
            return true;
        }
        match items.get_mut(inline.row) {
            Some(item) if *item != inline.text => {
                *item = inline.text;
                true
            }
            _ => false,
        }
    }

    pub(super) fn browsed_into_list(
        &mut self,
        plugin: &str,
        key: &str,
        items: &mut Vec<String>,
        path: String,
    ) {
        let row = match self.browsing.take() {
            Some((p, k, row)) if p == plugin && k == key => row,
            other => {
                self.browsing = other;
                None
            }
        };
        match row.filter(|row| *row < items.len()) {
            Some(row) => {
                items[row] = path;
                self.set_mark(plugin, key, Some(row));
            }
            None => {
                items.push(path);
                self.set_mark(plugin, key, Some(items.len() - 1));
            }
        }
    }

    pub(super) fn browsed_into_dialog(&mut self, plugin: &str, key: &str, path: String) -> bool {
        match &mut self.dialog {
            Some(Dialog::Pair {
                plugin: p,
                key: k,
                value,
                ..
            }) if p == plugin && k == key => {
                *value = path;
                true
            }
            _ => false,
        }
    }
}

pub(super) fn list(
    form: &mut Form<'_, '_>,
    lists: &mut Lists,
    plugin: &str,
    setting: &zwsdk::host::Setting,
    label: &str,
    now: &str,
) -> Outcome {
    let key = setting.key.as_str();
    let mut items = zwsdk::config::list_of(now);
    let mut changed = false;
    let mut pressed = None;
    let browse = matches!(setting.item.as_str(), "program" | "folder");
    caption(form, label);
    let mut shown = items.clone();
    if let Some(inline) = lists.editing(plugin, key).filter(|inline| inline.new) {
        shown.insert(inline.row.min(shown.len()), String::new());
    }
    let buttons = if browse { 7.0 } else { 6.0 };
    let height = (LINE * shown.len().max(4) as f32 + 2.0).max(buttons * (BUTTON + 6.0) - 6.0);
    let right = form.body.right() - skin::PAD;
    let table = Rect::from_min_max(
        pos2(form.label_left(), *form.at),
        pos2(right - SIDE_BUTTON - 8.0, *form.at + height),
    );
    *form.at = table.bottom() + skin::ROW_GAP + 6.0;
    frame(form, table);
    if shown.is_empty() {
        none(form, table, table.top() + 1.0);
    }
    let mark = lists.mark(plugin, key).filter(|m| *m < shown.len());
    let mut clicked = None;
    let mut edit = None;
    for (row, item) in shown.iter().enumerate() {
        let line = Rect::from_min_size(
            pos2(table.left() + 1.0, table.top() + 1.0 + LINE * row as f32),
            vec2(table.width() - 2.0, LINE),
        );
        let editing = lists
            .editing(plugin, key)
            .is_some_and(|inline| inline.row == row);
        if editing {
            let look = form.look;
            let Some(inline) = lists.inline.as_mut() else {
                continue;
            };
            form.ui
                .painter()
                .rect_filled(line.shrink(1.0), 3.0, look.chosen_fill);
            let id = form.ui.id().with(("plugin.list.edit", key));
            let response = form.ui.put(
                line.shrink2(vec2(4.0, 2.0)),
                egui::TextEdit::singleline(&mut inline.text)
                    .id(id)
                    .font(FontId::proportional(skin::TEXT_SIZE))
                    .text_color(look.text),
            );
            if inline.focus {
                response.request_focus();
                inline.focus = false;
            }
            let (enter, escape) = form.ui.input(|input| {
                (
                    input.key_pressed(Key::Enter),
                    input.key_pressed(Key::Escape),
                )
            });
            if escape {
                let new = inline.new;
                lists.inline = None;
                if new {
                    let mark = mark.filter(|_| !items.is_empty());
                    lists.set_mark(plugin, key, mark.map(|m| m.min(items.len() - 1)));
                }
            } else if enter || response.lost_focus() {
                changed |= lists.commit(plugin, key, &mut items);
            }
            continue;
        }
        let response = form.ui.interact(
            line,
            form.ui.id().with(("plugin.list.row", key, row)),
            Sense::click(),
        );
        let chosen = mark == Some(row);
        if chosen {
            form.ui
                .painter()
                .rect_filled(line.shrink(1.0), 3.0, form.look.chosen_fill);
        }
        cell(form, line, item, chosen);
        if response.double_clicked() {
            edit = Some(row);
        } else if response.clicked() {
            clicked = Some(row);
        }
    }
    // 書き換えている行を決めてから選ぶ（空の新しい行が消えると、後ろの行が一つ前へ来る）。
    for (target, start) in [(clicked, false), (edit, true)] {
        let Some(mut row) = target else {
            continue;
        };
        if let Some(inline) = lists.editing(plugin, key).cloned() {
            changed |= lists.commit(plugin, key, &mut items);
            if inline.new && inline.text.is_empty() && row > inline.row {
                row -= 1;
            }
        }
        if row < items.len() {
            lists.set_mark(plugin, key, Some(row));
            if start {
                lists.inline = Some(Inline {
                    plugin: plugin.to_owned(),
                    key: key.to_owned(),
                    row,
                    text: items[row].clone(),
                    new: false,
                    focus: true,
                });
            }
        }
    }

    let x = right - SIDE_BUTTON;
    let mut y = table.top();
    let mut button = |form: &mut Form<'_, '_>, label: &str, enabled: bool| {
        let rect = Rect::from_min_size(pos2(x, y), vec2(SIDE_BUTTON, BUTTON));
        y += BUTTON + 6.0;
        form.button_enabled(rect, ("plugin.list", key, label), label, enabled)
    };
    let mark = lists.mark(plugin, key).filter(|m| *m < shown.len());
    let count = shown.len();
    if button(form, "新規(N)", true) {
        changed |= lists.commit(plugin, key, &mut items);
        let at = lists
            .mark(plugin, key)
            .filter(|m| *m < items.len())
            .map_or(items.len(), |m| m + 1);
        lists.set_mark(plugin, key, Some(at));
        lists.inline = Some(Inline {
            plugin: plugin.to_owned(),
            key: key.to_owned(),
            row: at,
            text: String::new(),
            new: true,
            focus: true,
        });
    }
    if button(form, "編集(E)", mark.is_some()) {
        changed |= lists.commit(plugin, key, &mut items);
        if let Some(row) = lists.mark(plugin, key).filter(|m| *m < items.len()) {
            lists.inline = Some(Inline {
                plugin: plugin.to_owned(),
                key: key.to_owned(),
                row,
                text: items[row].clone(),
                new: false,
                focus: true,
            });
        }
    }
    if browse && button(form, "参照(B)…", true) {
        changed |= lists.commit(plugin, key, &mut items);
        let row = lists.mark(plugin, key).filter(|m| *m < items.len());
        lists.browsing = Some((plugin.to_owned(), key.to_owned(), row));
        pressed = Some(Pressed::Browse {
            plugin: plugin.to_owned(),
            key: key.to_owned(),
            label: label.to_owned(),
            folder: setting.item == "folder",
        });
    }
    if button(form, "削除(D)", mark.is_some()) {
        changed |= lists.commit(plugin, key, &mut items);
        if let Some(row) = lists.mark(plugin, key).filter(|m| *m < items.len()) {
            items.remove(row);
            changed = true;
            let next = (!items.is_empty()).then(|| row.min(items.len() - 1));
            lists.set_mark(plugin, key, next);
        }
    }
    if button(form, "上へ(U)", mark.is_some_and(|m| m > 0)) {
        changed |= lists.commit(plugin, key, &mut items);
        if let Some(row) = lists
            .mark(plugin, key)
            .filter(|m| *m > 0 && *m < items.len())
        {
            items.swap(row - 1, row);
            changed = true;
            lists.set_mark(plugin, key, Some(row - 1));
        }
    }
    if button(form, "下へ(O)", mark.is_some_and(|m| m + 1 < count)) {
        changed |= lists.commit(plugin, key, &mut items);
        if let Some(row) = lists.mark(plugin, key).filter(|m| *m + 1 < items.len()) {
            items.swap(row, row + 1);
            changed = true;
            lists.set_mark(plugin, key, Some(row + 1));
        }
    }
    if button(form, "テキストで編集(T)…", true) {
        changed |= lists.commit(plugin, key, &mut items);
        lists.dialog = Some(Dialog::Text {
            plugin: plugin.to_owned(),
            key: key.to_owned(),
            label: label.to_owned(),
            text: items.join("\n"),
        });
    }
    if !setting.hint.is_empty() {
        form.note(&setting.hint);
    }
    Outcome {
        changed: changed.then(|| zwsdk::config::list_text(&items)),
        pressed,
    }
}

pub(super) fn map(
    form: &mut Form<'_, '_>,
    lists: &mut Lists,
    plugin: &str,
    setting: &zwsdk::host::Setting,
    label: &str,
    now: &str,
) -> Outcome {
    let key = setting.key.as_str();
    let env = setting.is_env();
    let mut pairs = zwsdk::config::map_of(now);
    let mut changed = false;
    caption(form, label);
    let right = form.body.right() - skin::PAD;
    let rows = pairs.len().max(3) as f32;
    let table = Rect::from_min_max(
        pos2(form.label_left(), *form.at),
        pos2(right, *form.at + LINE * (rows + 1.0) + 2.0),
    );
    *form.at = table.bottom() + 6.0;
    frame(form, table);
    let split = table.left() + (table.width() * 0.36).round();
    let head = Rect::from_min_size(
        table.left_top() + vec2(1.0, 1.0),
        vec2(table.width() - 2.0, LINE),
    );
    let (name_head, value_head) = if env {
        ("変数", "値")
    } else {
        ("名前", "値")
    };
    {
        let painter = form.ui.painter();
        painter.rect_filled(head, 3.0, form.look.button_fill);
        painter.hline(
            head.x_range(),
            head.bottom(),
            Stroke::new(1.0, form.look.rule),
        );
        painter.vline(split, table.y_range(), Stroke::new(1.0, form.look.rule));
        for (x, text) in [(head.left(), name_head), (split, value_head)] {
            painter.text(
                pos2(x + 8.0, head.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(skin::TEXT_SIZE - 1.0),
                form.look.label,
            );
        }
    }
    if pairs.is_empty() {
        none(form, table, head.bottom());
    }
    let mark = lists.mark(plugin, key).filter(|m| *m < pairs.len());
    let mut open = None;
    for (row, (name, value)) in pairs.iter().enumerate() {
        let line = Rect::from_min_size(
            pos2(head.left(), head.bottom() + LINE * row as f32),
            vec2(head.width(), LINE),
        );
        let response = form.ui.interact(
            line,
            form.ui.id().with(("plugin.map.row", key, row)),
            Sense::click(),
        );
        let chosen = mark == Some(row);
        if chosen {
            form.ui
                .painter()
                .rect_filled(line.shrink(1.0), 3.0, form.look.chosen_fill);
        }
        cell(
            form,
            Rect::from_min_max(line.min, pos2(split, line.bottom())),
            name,
            chosen,
        );
        cell(
            form,
            Rect::from_min_max(pos2(split, line.top()), line.max),
            value,
            chosen,
        );
        if response.clicked() || response.double_clicked() {
            lists.set_mark(plugin, key, Some(row));
        }
        if response.double_clicked() {
            open = Some(row);
        }
    }
    let row = form.line(BUTTON);
    let remove = Rect::from_min_size(pos2(row.right() - 80.0, row.top()), vec2(80.0, BUTTON));
    let edit = Rect::from_min_size(pos2(remove.left() - 104.0, row.top()), vec2(96.0, BUTTON));
    let add = Rect::from_min_size(pos2(edit.left() - 104.0, row.top()), vec2(96.0, BUTTON));
    let mark = lists.mark(plugin, key).filter(|m| *m < pairs.len());
    let dialog = |was: Option<usize>| Dialog::Pair {
        plugin: plugin.to_owned(),
        key: key.to_owned(),
        label: label.to_owned(),
        env,
        was: was.map(|row| pairs[row].0.clone()),
        name: was.map_or_else(String::new, |row| pairs[row].0.clone()),
        value: was.map_or_else(String::new, |row| pairs[row].1.clone()),
        pairs: pairs.clone(),
        focus: true,
    };
    if form.button(add, ("plugin.map.add", key), "新規(W)…") {
        lists.dialog = Some(dialog(None));
    }
    if form.button_enabled(edit, ("plugin.map.edit", key), "編集(I)…", mark.is_some())
        && let Some(row) = mark
    {
        open = Some(row);
    }
    if let Some(row) = open {
        lists.dialog = Some(dialog(Some(row)));
    }
    if form.button_enabled(
        remove,
        ("plugin.map.remove", key),
        "削除(L)",
        mark.is_some(),
    ) && let Some(row) = mark
    {
        pairs.remove(row);
        changed = true;
        let next = (!pairs.is_empty()).then(|| row.min(pairs.len() - 1));
        lists.set_mark(plugin, key, next);
    }
    *form.at += skin::ROW_GAP;
    if !setting.hint.is_empty() {
        form.note(&setting.hint);
    }
    Outcome {
        changed: changed.then(|| zwsdk::config::map_text(&pairs)),
        pressed: None,
    }
}

fn pair_problem(
    pairs: &[(String, String)],
    was: Option<&str>,
    name: &str,
    env: bool,
) -> Option<&'static str> {
    if env && !name.is_empty() && !zwsdk::config::env_name(name) {
        return Some("変数名に使えるのは半角英数字と _ だけです。");
    }
    if was != Some(name) && pairs.iter().any(|(had, _)| had == name) {
        return Some(if env {
            "同じ名前の変数があります。"
        } else {
            "同じ名前の項目があります。"
        });
    }
    None
}

fn put_pair(pairs: &mut Vec<(String, String)>, was: Option<&str>, name: &str, value: &str) {
    pairs.retain(|(had, _)| Some(had.as_str()) != was && had != name);
    pairs.push((name.to_owned(), value.to_owned()));
    pairs.sort();
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug)]
pub(super) enum Done {
    Put(String, String),
    Browse(Pressed),
}

fn alt(ui: &egui::Ui, key: Key) -> bool {
    ui.input(|input| input.modifiers.alt && input.key_pressed(key))
}

pub(super) fn dialog(
    root: &mut egui::Ui,
    look: &Look,
    plugin: &str,
    lists: &mut Lists,
) -> Option<Done> {
    if lists.dialog.as_ref().is_none_or(|it| it.plugin() != plugin) {
        return None;
    }
    let frame = egui::Frame::new()
        .fill(look.body)
        .stroke(Stroke::new(1.0, look.button_edge))
        .corner_radius(8);
    let shown = egui::Modal::new(egui::Id::new(("plugin.dialog", plugin)))
        .frame(frame)
        .show(root.ctx(), |ui| {
            let Some(dialog) = lists.dialog.as_mut() else {
                return (None, false);
            };
            match dialog {
                Dialog::Text {
                    key, label, text, ..
                } => text_dialog(ui, look, key, label, text),
                Dialog::Pair {
                    key,
                    label,
                    env,
                    was,
                    name,
                    value,
                    pairs,
                    focus,
                    ..
                } => pair_dialog(
                    ui,
                    look,
                    PairDraft {
                        plugin,
                        key,
                        label,
                        env: *env,
                        was: was.as_deref(),
                        name,
                        value,
                        pairs,
                        focus,
                    },
                ),
            }
        });
    let dismissed = shown.should_close();
    let (done, close) = shown.inner;
    if close || dismissed {
        lists.dialog = None;
    }
    done
}

fn title(ui: &egui::Ui, look: &Look, area: Rect, text: &str) {
    ui.painter().text(
        pos2(area.left() + 20.0, area.top() + 22.0),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(skin::HEAD_SIZE),
        look.text,
    );
}

fn ok_cancel(form: &mut Form<'_, '_>, area: Rect, id: &str, ok: bool) -> Option<bool> {
    let top = area.bottom() - 16.0 - skin::SELECT_HEIGHT;
    let cancel = Rect::from_min_size(
        pos2(area.right() - 20.0 - 96.0, top),
        vec2(96.0, skin::SELECT_HEIGHT),
    );
    let accept = Rect::from_min_size(
        pos2(cancel.left() - 8.0 - 96.0, top),
        vec2(96.0, skin::SELECT_HEIGHT),
    );
    if form.primary_button(accept, (id, "ok"), "OK", ok) {
        return Some(true);
    }
    if form.button(cancel, (id, "cancel"), "キャンセル") {
        return Some(false);
    }
    None
}

fn text_dialog(
    ui: &mut egui::Ui,
    look: &Look,
    key: &str,
    label: &str,
    text: &mut String,
) -> (Option<Done>, bool) {
    let (area, _) = ui.allocate_exact_size(vec2(460.0, 340.0), Sense::hover());
    title(ui, look, area, "テキストで編集");
    ui.painter().text(
        pos2(area.left() + 20.0, area.top() + 50.0),
        egui::Align2::LEFT_CENTER,
        format!("{label}：一行に一つ。空行は無視します。"),
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        look.label,
    );
    let field = Rect::from_min_max(
        pos2(area.left() + 20.0, area.top() + 66.0),
        pos2(area.right() - 20.0, area.bottom() - 60.0),
    );
    ui.painter()
        .rect_filled(field, skin::SELECT_ROUNDING, look.select_fill);
    ui.painter().rect_stroke(
        field,
        skin::SELECT_ROUNDING,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink(4.0)));
    egui::ScrollArea::vertical()
        .id_salt(("plugin.text", key))
        .auto_shrink([false, false])
        .show(&mut inner, |ui| {
            ui.add_sized(
                vec2(field.width() - 12.0, field.height() - 10.0),
                egui::TextEdit::multiline(text)
                    .frame(egui::Frame::NONE)
                    .font(FontId::monospace(skin::TEXT_SIZE))
                    .text_color(look.text),
            );
        });
    let mut at = 0.0;
    let mut form = Form::new(ui, look, area, &mut at);
    match ok_cancel(&mut form, area, "plugin.text", true) {
        Some(true) => (
            Some(Done::Put(
                key.to_owned(),
                zwsdk::config::list_text(&lines_of(text)),
            )),
            true,
        ),
        Some(false) => (None, true),
        None => (None, false),
    }
}

struct PairDraft<'a> {
    plugin: &'a str,
    key: &'a str,
    label: &'a str,
    env: bool,
    was: Option<&'a str>,
    name: &'a mut String,
    value: &'a mut String,
    pairs: &'a [(String, String)],
    focus: &'a mut bool,
}

fn pair_dialog(ui: &mut egui::Ui, look: &Look, draft: PairDraft<'_>) -> (Option<Done>, bool) {
    let (area, _) = ui.allocate_exact_size(vec2(480.0, 236.0), Sense::hover());
    let heading = match (draft.env, draft.was.is_some()) {
        (true, false) => "新しい環境変数",
        (true, true) => "環境変数の編集",
        (false, false) => "新しい項目",
        (false, true) => "項目の編集",
    };
    title(ui, look, area, heading);
    let left = area.left() + 20.0;
    let field_left = area.left() + 112.0;
    let problem = pair_problem(draft.pairs, draft.was, draft.name, draft.env);
    let ok = problem.is_none() && !draft.name.is_empty();
    let mut done = None;
    let mut close = false;
    {
        let mut at = 0.0;
        let mut form = Form::new(ui, look, area, &mut at);
        for (nth, said) in ["変数名(N):", "変数値(V):"].into_iter().enumerate() {
            let top = area.top() + 48.0 + 38.0 * nth as f32;
            form.ui.painter().text(
                pos2(left, top + skin::SELECT_HEIGHT / 2.0),
                egui::Align2::LEFT_CENTER,
                said,
                FontId::proportional(skin::TEXT_SIZE),
                look.label,
            );
            let rect = Rect::from_min_max(
                pos2(field_left, top),
                pos2(area.right() - 20.0, top + skin::SELECT_HEIGHT),
            );
            let value = if nth == 0 {
                &mut *draft.name
            } else {
                &mut *draft.value
            };
            form.field(rect, ("plugin.pair", draft.key, nth), value, "");
        }
        if let Some(problem) = problem {
            form.ui.painter().text(
                pos2(field_left, area.top() + 134.0),
                egui::Align2::LEFT_CENTER,
                problem,
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                look.error,
            );
        }
        let top = area.top() + 150.0;
        let folder = Rect::from_min_size(pos2(field_left, top), vec2(156.0, BUTTON));
        let file = Rect::from_min_size(pos2(folder.right() + 8.0, top), vec2(146.0, BUTTON));
        let ask = |folder: bool| {
            Done::Browse(Pressed::Browse {
                plugin: draft.plugin.to_owned(),
                key: draft.key.to_owned(),
                label: draft.label.to_owned(),
                folder,
            })
        };
        if form.button(
            folder,
            ("plugin.pair.folder", draft.key),
            "フォルダーの参照(D)…",
        ) || alt(form.ui, Key::D)
        {
            done = Some(ask(true));
        }
        if form.button(file, ("plugin.pair.file", draft.key), "ファイルの参照(F)…")
            || alt(form.ui, Key::F)
        {
            done = Some(ask(false));
        }
        let enter = form.ui.input(|input| input.key_pressed(Key::Enter));
        match ok_cancel(&mut form, area, "plugin.pair", ok) {
            Some(true) => close = true,
            Some(false) => return (None, true),
            None if enter && ok => close = true,
            None => {}
        }
    }
    // `form.field` の欄の id は ui の id とキーから作る。それに合わせて指定する。
    let name_field = ui.id().with(("plugin.pair", draft.key, 0usize));
    let value_field = ui.id().with(("plugin.pair", draft.key, 1usize));
    if std::mem::take(draft.focus) || alt(ui, Key::N) {
        ui.memory_mut(|memory| memory.request_focus(name_field));
    }
    if alt(ui, Key::V) {
        ui.memory_mut(|memory| memory.request_focus(value_field));
    }
    if close {
        let mut pairs = draft.pairs.to_vec();
        put_pair(&mut pairs, draft.was, draft.name, draft.value);
        return (
            Some(Done::Put(
                draft.key.to_owned(),
                zwsdk::config::map_text(&pairs),
            )),
            true,
        );
    }
    (done, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(of: &[(&str, &str)]) -> Vec<(String, String)> {
        of.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_bad_or_repeated_name_is_refused() {
        let had = pairs(&[("A", "1"), ("B", "2")]);
        assert_eq!(pair_problem(&had, None, "C_1", true), None);
        assert_eq!(
            pair_problem(&had, None, "", true),
            None,
            "空はエラーにしない"
        );
        assert_eq!(
            pair_problem(&had, None, "A-B", true),
            Some("変数名に使えるのは半角英数字と _ だけです。")
        );
        assert_eq!(
            pair_problem(&had, None, "B", true),
            Some("同じ名前の変数があります。")
        );
        assert_eq!(
            pair_problem(&had, Some("B"), "B", true),
            None,
            "自分は数えない"
        );
        assert_eq!(
            pair_problem(&had, None, "a b", false),
            None,
            "env でなければ文字は問わない"
        );
        assert_eq!(
            pair_problem(&had, Some("A"), "B", false),
            Some("同じ名前の項目があります。")
        );
    }

    #[test]
    fn an_edited_pair_replaces_its_old_name() {
        let mut had = pairs(&[("A", "1"), ("B", "2")]);
        put_pair(&mut had, Some("A"), "C", "3");
        assert_eq!(had, pairs(&[("B", "2"), ("C", "3")]));
        put_pair(&mut had, None, "A", "$HOME");
        assert_eq!(had, pairs(&[("A", "$HOME"), ("B", "2"), ("C", "3")]));
    }

    #[test]
    fn text_editing_ignores_blank_lines() {
        assert_eq!(lines_of("-v\r\n\n  \n-o x\n"), ["-v", "-o x"]);
        assert_eq!(lines_of(""), Vec::<String>::new());
    }

    #[test]
    fn an_empty_new_row_is_not_added() {
        let mut lists = Lists::default();
        let mut items = vec![String::from("a"), String::from("b")];
        lists.inline = Some(Inline {
            plugin: String::from("p"),
            key: String::from("k"),
            row: 1,
            text: String::new(),
            new: true,
            focus: false,
        });
        lists.set_mark("p", "k", Some(1));
        assert!(!lists.commit("p", "k", &mut items));
        assert_eq!(items, ["a", "b"]);
        lists.inline = Some(Inline {
            plugin: String::from("p"),
            key: String::from("k"),
            row: 1,
            text: String::from("x"),
            new: true,
            focus: false,
        });
        assert!(lists.commit("p", "k", &mut items));
        assert_eq!(items, ["a", "x", "b"]);
        assert_eq!(lists.mark("p", "k"), Some(1));
    }

    #[test]
    fn a_browsed_path_fills_the_asked_row_or_is_added() {
        let mut lists = Lists::default();
        let mut items = vec![String::from("a")];
        lists.browsing = Some((String::from("p"), String::from("k"), Some(0)));
        lists.browsed_into_list("p", "k", &mut items, String::from("/x"));
        assert_eq!(items, ["/x"]);
        lists.browsed_into_list("p", "k", &mut items, String::from("/y"));
        assert_eq!(items, ["/x", "/y"]);
        assert_eq!(lists.mark("p", "k"), Some(1));
    }
}
