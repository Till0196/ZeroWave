// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「お気に入りの管理」。いまのプロファイルのお気に入りをツリーで並べ替え、OK で `favourites.toml` に書く。
//! ドロップ先：フォルダの行は上・中・下の三つ、局の行は前か後の二つに分ける。

use egui::{FontId, Rect, Sense, Stroke, pos2, vec2};

use super::form::Form;
use super::{Look, Pressed, skin};
use crate::config::favourites::{self, List, Node};
use crate::ui::icons::{self, Icon};

pub const WIDTH: f32 = 560.0;
pub const HEIGHT: f32 = 600.0;
const ROW: f32 = 30.0;
const INDENT: f32 = 20.0;
const FOOT: f32 = 52.0;

pub type Path = Vec<usize>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drop {
    Before(Path),
    After(Path),
    Inside(Path),
}

#[derive(Debug, Clone, Default)]
pub struct Draft {
    profile: String,
    items: Vec<Node>,
    loaded: Vec<Node>,
    selected: Option<Path>,
    renaming: Option<(Path, String)>,
    dragging: Option<Path>,
    wrong: Option<String>,
}

impl Draft {
    pub fn load() -> Self {
        let profile = crate::config::load_state().profile;
        let items = favourites::of(&profile).items;
        Self {
            profile,
            loaded: items.clone(),
            items,
            ..Self::default()
        }
    }
}

pub fn flatten(items: &[Node]) -> Vec<(Path, usize)> {
    fn walk(items: &[Node], prefix: &mut Path, out: &mut Vec<(Path, usize)>) {
        for (at, node) in items.iter().enumerate() {
            prefix.push(at);
            out.push((prefix.clone(), prefix.len() - 1));
            if node.key().is_none() {
                walk(&node.items, prefix, out);
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(items, &mut Vec::new(), &mut out);
    out
}

pub fn get<'a>(items: &'a [Node], path: &[usize]) -> Option<&'a Node> {
    let (first, rest) = path.split_first()?;
    let node = items.get(*first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        get(&node.items, rest)
    }
}

fn siblings_mut<'a>(items: &'a mut Vec<Node>, parent: &[usize]) -> Option<&'a mut Vec<Node>> {
    match parent.split_first() {
        None => Some(items),
        Some((first, rest)) => siblings_mut(&mut items.get_mut(*first)?.items, rest),
    }
}

pub fn remove(items: &mut Vec<Node>, path: &[usize]) -> Option<Node> {
    let (last, parent) = path.split_last()?;
    let siblings = siblings_mut(items, parent)?;
    (*last < siblings.len()).then(|| siblings.remove(*last))
}

pub fn move_to(items: &mut Vec<Node>, path: &[usize], drop: &Drop) -> Option<Path> {
    let target = match drop {
        Drop::Before(at) | Drop::After(at) | Drop::Inside(at) => at,
    };
    if target.starts_with(path) {
        return None;
    }
    let node = remove(items, path)?;
    // 取り出した分、後ろの添字が一つ前へずれる。
    let shift = |at: &[usize]| -> Path {
        let mut at = at.to_vec();
        let depth = path.len() - 1;
        if at.len() > depth && at[..depth] == path[..depth] && at[depth] > path[depth] {
            at[depth] -= 1;
        }
        at
    };
    let (parent, index) = match drop {
        Drop::Before(at) => {
            let at = shift(at);
            let (last, parent) = at.split_last()?;
            (parent.to_vec(), *last)
        }
        Drop::After(at) => {
            let at = shift(at);
            let (last, parent) = at.split_last()?;
            (parent.to_vec(), *last + 1)
        }
        Drop::Inside(at) => {
            let at = shift(at);
            let length = get(items, &at)?.items.len();
            (at, length)
        }
    };
    let siblings = siblings_mut(items, &parent)?;
    let index = index.min(siblings.len());
    siblings.insert(index, node);
    let mut placed = parent;
    placed.push(index);
    Some(placed)
}

pub fn show(root: &mut egui::Ui, draft: &mut Draft, look: &Look) -> Option<Pressed> {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let foot_rect = egui::Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let body = egui::Rect::from_min_max(full.min, pos2(full.right(), foot_rect.top()));
    {
        let mut inner = root.new_child(egui::UiBuilder::new().max_rect(body));
        inner.set_clip_rect(body);
        let mut at = body.top() + 6.0;
        let mut form = Form::new(&mut inner, look, body, &mut at);
        let profile = if draft.profile.is_empty() {
            super::ALL
        } else {
            draft.profile.as_str()
        };
        form.section(&format!("プロファイル {profile} のお気に入り"));
        toolbar(&mut form, draft);
        let top = *form.at;
        let list = Rect::from_min_max(
            pos2(body.left() + skin::PAD, top),
            pos2(body.right() - skin::PAD, body.bottom() - 30.0),
        );
        rows(form.ui, look, list, draft);
        let note_at = list.bottom() + 6.0;
        let note = draft.wrong.clone().unwrap_or_else(|| {
            String::from("ドラッグで並べ替えます。フォルダの行の中ほどにドロップすると、フォルダの中へ入ります。名前はダブルクリックでも変えられます")
        });
        let colour = if draft.wrong.is_some() {
            look.error
        } else {
            look.label
        };
        let galley = form.ui.painter().layout(
            note,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            colour,
            list.width(),
        );
        form.ui
            .painter()
            .galley(pos2(list.left(), note_at), galley, colour);
    }
    let mut at = 0.0;
    let mut form = Form::foot(root, look, foot_rect, &mut at);
    let row = form.line(skin::SELECT_HEIGHT);
    let cancel = Rect::from_min_size(
        pos2(row.right() - 88.0, row.top()),
        vec2(88.0, row.height()),
    );
    let ok = Rect::from_min_size(
        pos2(cancel.left() - 96.0, row.top()),
        vec2(88.0, row.height()),
    );
    if form.primary_button(ok, ("favourites", "ok"), "OK", true) {
        commit_rename(draft);
        if draft.items == draft.loaded {
            return Some(Pressed::Close);
        }
        let list = List {
            items: draft.items.clone(),
        };
        return match favourites::save(&draft.profile, list) {
            Ok(()) => Some(Pressed::Close),
            Err(why) => {
                draft.wrong = Some(why);
                None
            }
        };
    }
    if form.button(cancel, ("favourites", "cancel"), "キャンセル") {
        return Some(Pressed::Close);
    }
    None
}

fn toolbar(form: &mut Form<'_, '_>, draft: &mut Draft) {
    let row = form.line(skin::SELECT_HEIGHT);
    let mut x = row.left();
    let mut button = |form: &mut Form<'_, '_>, label: &str, width: f32, enabled: bool| {
        let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
        x += width + 8.0;
        form.button_enabled(rect, ("favourites.tool", label), label, enabled)
    };
    let selected = draft.selected.clone();
    let has = selected
        .as_ref()
        .is_some_and(|path| get(&draft.items, path).is_some());
    if button(form, "新規フォルダ", 104.0, true) {
        commit_rename(draft);
        let placed = match &selected {
            Some(path) if has => move_after_insert(&mut draft.items, path),
            _ => {
                draft.items.push(Node::folder(""));
                vec![draft.items.len() - 1]
            }
        };
        draft.selected = Some(placed.clone());
        draft.renaming = Some((placed, String::from("新しいフォルダ")));
    }
    if button(form, "名前の変更", 96.0, has)
        && let Some(path) = &selected
        && let Some(node) = get(&draft.items, path)
    {
        draft.renaming = Some((path.clone(), node.name.clone()));
    }
    if button(form, "削除", 64.0, has)
        && let Some(path) = &selected
    {
        draft.renaming = None;
        remove(&mut draft.items, path);
        draft.selected = None;
    }
    let up = selected
        .as_ref()
        .filter(|_| has)
        .and_then(|path| path.last().filter(|last| **last > 0).map(|_| path.clone()));
    if button(form, "上へ", 56.0, up.is_some())
        && let Some(path) = up
    {
        let mut before = path.clone();
        if let Some(last) = before.last_mut() {
            *last -= 1;
        }
        draft.selected = move_to(&mut draft.items, &path, &Drop::Before(before));
    }
    let down = selected.as_ref().filter(|_| has).and_then(|path| {
        let (last, parent) = path.split_last()?;
        let count = if parent.is_empty() {
            draft.items.len()
        } else {
            get(&draft.items, parent)?.items.len()
        };
        (*last + 1 < count).then(|| path.clone())
    });
    if button(form, "下へ", 56.0, down.is_some())
        && let Some(path) = down
    {
        let mut after = path.clone();
        if let Some(last) = after.last_mut() {
            *last += 1;
        }
        draft.selected = move_to(&mut draft.items, &path, &Drop::After(after));
    }
}

fn move_after_insert(items: &mut Vec<Node>, path: &[usize]) -> Path {
    let (last, parent) = path.split_last().map_or((0, &[][..]), |(l, p)| (*l, p));
    match siblings_mut(items, parent) {
        Some(siblings) => {
            let at = (last + 1).min(siblings.len());
            siblings.insert(at, Node::folder(""));
            let mut placed = parent.to_vec();
            placed.push(at);
            placed
        }
        None => {
            items.push(Node::folder(""));
            vec![items.len() - 1]
        }
    }
}

/// 空なら元のまま（足したばかりのフォルダなら「新しいフォルダ」）。
fn commit_rename(draft: &mut Draft) {
    let Some((path, typed)) = draft.renaming.take() else {
        return;
    };
    let typed = typed.trim().to_owned();
    if let Some((last, parent)) = path.split_last()
        && let Some(siblings) = siblings_mut(&mut draft.items, parent)
        && let Some(node) = siblings.get_mut(*last)
    {
        if !typed.is_empty() {
            node.name = typed;
        } else if node.name.is_empty() {
            node.name = String::from("新しいフォルダ");
        }
    }
}

fn rows(ui: &mut egui::Ui, look: &Look, list: Rect, draft: &mut Draft) {
    ui.painter().rect_filled(list, 6.0, look.select_fill);
    ui.painter().rect_stroke(
        list,
        6.0,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let shown = flatten(&draft.items);
    if shown.is_empty() {
        ui.painter().text(
            list.center(),
            egui::Align2::CENTER_CENTER,
            "お気に入りはまだありません。メニューの「お気に入りに追加」で足せます",
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
        return;
    }
    let inner = list.shrink(1.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner);
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let released = ui.input(|input| input.pointer.any_released());
    let mut drop: Option<(Drop, Rect)> = None;
    egui::ScrollArea::vertical()
        .id_salt("favourites.rows")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let width = ui.available_width();
            for (path, depth) in &shown {
                let Some(node) = get(&draft.items, path) else {
                    continue;
                };
                let node = node.clone();
                let folder = node.key().is_none();
                let (line, response) =
                    ui.allocate_exact_size(vec2(width, ROW), Sense::click_and_drag());
                let painter = ui.painter_at(line);
                let selected = draft.selected.as_ref() == Some(path);
                if selected {
                    painter.rect_filled(line, 4.0, look.chosen_fill);
                }
                let left = line.left() + 10.0 + INDENT * *depth as f32;
                let colour = if selected {
                    look.chosen_text
                } else {
                    look.text
                };
                let renaming = draft.renaming.as_ref().is_some_and(|(at, _)| at == path);
                if folder {
                    icons::paint_at(
                        &painter,
                        pos2(left + 5.0, line.center().y),
                        10.0,
                        Icon::TriDown,
                        look.label,
                    );
                }
                let name_left = left + if folder { 16.0 } else { 0.0 };
                if renaming {
                    let field = Rect::from_min_max(
                        pos2(name_left, line.top() + 3.0),
                        pos2(line.right() - 10.0, line.bottom() - 3.0),
                    );
                    if let Some((_, typed)) = draft.renaming.as_mut() {
                        let edit = ui.put(
                            field,
                            egui::TextEdit::singleline(typed)
                                .id_salt(("favourites.rename", path.clone())),
                        );
                        edit.request_focus();
                        let (enter, escape) = ui.input(|input| {
                            (
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                            )
                        });
                        if escape {
                            draft.renaming = None;
                            if node.name.is_empty() {
                                remove(&mut draft.items, path);
                                draft.selected = None;
                                return;
                            }
                        } else if enter || edit.lost_focus() {
                            commit_rename(draft);
                        }
                    }
                } else {
                    painter.text(
                        pos2(name_left, line.center().y),
                        egui::Align2::LEFT_CENTER,
                        &node.name,
                        FontId::proportional(skin::TEXT_SIZE),
                        colour,
                    );
                    if let Some(key) = node.key() {
                        painter.text(
                            pos2(line.right() - 10.0, line.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!(
                                "NID {} / SID {}",
                                zwcore::si::hex_id(key.network, 4),
                                zwcore::si::hex_id(key.service, 4)
                            ),
                            FontId::proportional(skin::TEXT_SIZE - 2.0),
                            look.label,
                        );
                    }
                }
                if response.clicked() {
                    if draft.renaming.as_ref().is_some_and(|(at, _)| at != path) {
                        commit_rename(draft);
                    }
                    draft.selected = Some(path.clone());
                }
                if response.double_clicked() {
                    draft.selected = Some(path.clone());
                    draft.renaming = Some((path.clone(), node.name.clone()));
                }
                if response.drag_started() {
                    commit_rename(draft);
                    draft.dragging = Some(path.clone());
                    draft.selected = Some(path.clone());
                }
                if draft.dragging.is_some()
                    && let Some(at) = pointer
                    && line.contains(at)
                {
                    let fraction = (at.y - line.top()) / line.height();
                    let target = if folder {
                        if fraction < 1.0 / 3.0 {
                            Drop::Before(path.clone())
                        } else if fraction < 2.0 / 3.0 {
                            Drop::Inside(path.clone())
                        } else {
                            Drop::After(path.clone())
                        }
                    } else if fraction < 0.5 {
                        Drop::Before(path.clone())
                    } else {
                        Drop::After(path.clone())
                    };
                    drop = Some((target, line));
                }
            }
        });
    if draft.dragging.is_some() {
        if let Some((target, line)) = &drop {
            let stroke = Stroke::new(2.0, look.primary);
            match target {
                Drop::Before(_) => {
                    ui.painter()
                        .hline(line.left()..=line.right(), line.top(), stroke);
                }
                Drop::After(_) => {
                    ui.painter()
                        .hline(line.left()..=line.right(), line.bottom(), stroke);
                }
                Drop::Inside(_) => {
                    ui.painter()
                        .rect_stroke(*line, 4.0, stroke, egui::StrokeKind::Inside);
                }
            }
        }
        if released {
            if let (Some(from), Some((target, _))) = (draft.dragging.take(), drop)
                && let Some(placed) = move_to(&mut draft.items, &from, &target)
            {
                draft.selected = Some(placed);
            }
            draft.dragging = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwcore::guide::Key;

    fn station(service: u16) -> Node {
        Node::station(
            format!("局{service}"),
            Key {
                network: 4,
                service,
            },
        )
    }

    fn tree() -> Vec<Node> {
        let mut folder = Node::folder("映画");
        folder.items = vec![station(10), station(11)];
        vec![station(1), folder, station(2)]
    }

    fn names(items: &[Node]) -> Vec<String> {
        flatten(items)
            .into_iter()
            .map(|(path, depth)| format!("{depth}{}", get(items, &path).unwrap().name))
            .collect()
    }

    #[test]
    fn nodes_move_before_after_and_inside() {
        let mut items = tree();
        assert_eq!(names(&items), ["0局1", "0映画", "1局10", "1局11", "0局2"]);
        assert_eq!(
            move_to(&mut items, &[0], &Drop::After(vec![2])),
            Some(vec![2])
        );
        assert_eq!(names(&items), ["0映画", "1局10", "1局11", "0局2", "0局1"]);
        assert_eq!(
            move_to(&mut items, &[2], &Drop::Inside(vec![0])),
            Some(vec![0, 2])
        );
        assert_eq!(names(&items), ["0映画", "1局10", "1局11", "1局1", "0局2"]);
        assert_eq!(
            move_to(&mut items, &[0, 1], &Drop::Before(vec![0])),
            Some(vec![0])
        );
        assert_eq!(names(&items), ["0局11", "0映画", "1局10", "1局1", "0局2"]);
        assert_eq!(move_to(&mut items, &[1], &Drop::Inside(vec![1])), None);
        assert_eq!(move_to(&mut items, &[1], &Drop::Before(vec![1, 0])), None);
        assert_eq!(names(&items), ["0局11", "0映画", "1局10", "1局1", "0局2"]);
    }

    #[test]
    fn a_new_folder_goes_after_the_selection() {
        let mut draft = Draft {
            items: tree(),
            ..Draft::default()
        };
        let placed = move_after_insert(&mut draft.items, &[1, 0]);
        assert_eq!(placed, vec![1, 1]);
        draft.renaming = Some((placed, String::from("  ")));
        commit_rename(&mut draft);
        assert_eq!(get(&draft.items, &[1, 1]).unwrap().name, "新しいフォルダ");
    }
}
