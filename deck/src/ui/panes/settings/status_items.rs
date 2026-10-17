// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「表示する項目」のテーブル。見本の右端をドラッグすると幅を変え、
//! ダブルクリックで自動の幅に戻す。

use egui::{FontId, Rect, Sense, Stroke, pos2, vec2};

use super::{File, Form, skin};
use crate::config::file::StatusItem;
use crate::ui::icons::{self, Icon};
use crate::ui::overlay::status::{self, Item, Status};

const ROW: f32 = 30.0;
const GRIP: f32 = 18.0;
const TICK: f32 = 22.0;
const NAME: f32 = 150.0;
const PAD: f32 = 8.0;
const HANDLE: f32 = 6.0;
const MIN_WIDTH: f32 = 16.0;
const MAX_WIDTH: f32 = 600.0;

#[derive(Debug, Clone, Default)]
struct Held {
    selected: Option<String>,
    moving: Option<String>,
}

/// 名前は架空の物（実在の局やチューナーの名前は出さない）。
fn sample() -> Status {
    Status {
        profile: Some(String::from("地デジ")),
        channel: Some(String::from("ゼロウェーブテレビ")),
        picture: Some((1440, 1080, 1.0)),
        volume: 70,
        sound: Some(String::from("ステレオ")),
        cnr: Some(28.5),
        counts: zwcore::filters::Counts::default(),
        bitrate: 16.85,
        clock: String::from("21:24"),
        ..Status::default()
    }
}

pub(super) fn items(form: &mut Form<'_, '_>, edited: &mut File, loaded: &File) {
    let before = edited.ui.status_list();
    let mut list = before.clone();
    let changed = list != loaded.ui.status_list();
    form.key("ui.status_items");
    let _ = form.label_row("表示する項目", changed);
    *form.at -= skin::ROW_GAP + 4.0;
    let held_id = form.ui.scope_id().with("ステータスバーの項目");
    let mut held: Held = form
        .ui
        .ctx()
        .data(|data| data.get_temp(held_id))
        .unwrap_or_default();
    let size = status::text_size(edited.ui.status_font_size);
    let mode = if form.ui.visuals().dark_mode {
        crate::ui::theme::Mode::Dark
    } else {
        crate::ui::theme::Mode::Light
    };
    let bar_skin = crate::ui::theme::status::skin(mode);
    let shown = sample();
    let look = form.look;
    let left = form.label_left() - skin::GAP;
    let right = form.body.right() - skin::PAD;
    let whole = Rect::from_min_max(
        pos2(left, *form.at),
        pos2(right, *form.at + ROW * list.len() as f32),
    );
    let pointer = form.ui.input(|input| input.pointer.interact_pos());
    let released = form.ui.input(|input| input.pointer.any_released());
    let mut moved: Option<(usize, usize)> = None;
    for nth in 0..list.len() {
        let line = Rect::from_min_size(
            pos2(whole.left(), whole.top() + ROW * nth as f32),
            vec2(whole.width(), ROW),
        );
        let key = list[nth].id.clone();
        let Some(item) = Item::of(&key) else {
            continue;
        };
        let selected = held.selected.as_deref() == Some(key.as_str());
        let grip = Rect::from_min_size(pos2(line.left() + PAD, line.top()), vec2(GRIP, ROW));
        let tick = Rect::from_min_size(pos2(grip.right(), line.top()), vec2(TICK, ROW));
        let name = Rect::from_min_size(pos2(tick.right(), line.top()), vec2(NAME, ROW));
        // チェックボックスの当たり判定と重ならないよう、⋮⋮ と名前は別々に受けて合わせる。
        let by_grip = form.ui.interact(
            grip,
            form.ui.scope_id().with(("項目を並べ替える", &key)),
            Sense::click_and_drag(),
        );
        let by_name = form.ui.interact(
            name,
            form.ui.scope_id().with(("項目の名前", &key)),
            Sense::click_and_drag(),
        );
        let grab = by_grip.union(by_name);
        if grab.clicked() || grab.drag_started() {
            held.selected = Some(key.clone());
        }
        if grab.drag_started() {
            held.moving = Some(key.clone());
        }
        if grab.hovered() || grab.dragged() {
            form.ui.ctx().set_cursor_icon(if grab.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        if held.moving.as_deref() == Some(key.as_str())
            && let Some(at) = pointer
        {
            let to = ((at.y - whole.top()) / ROW)
                .floor()
                .clamp(0.0, (list.len() - 1) as f32) as usize;
            if to != nth {
                moved = Some((nth, to));
            }
        }
        let painter = form.ui.painter();
        if selected {
            painter.rect_filled(line, 0.0, look.chosen_fill);
        }
        if nth > 0 {
            painter.hline(
                line.left() + 1.0..=line.right() - 1.0,
                line.top(),
                Stroke::new(1.0, look.rule),
            );
        }
        icons::paint_at(
            painter,
            grip.left_center() + vec2(skin::ICON / 2.0, 0.0),
            skin::ICON,
            Icon::Grip,
            look.label,
        );
        painter.text(
            name.left_center(),
            egui::Align2::LEFT_CENTER,
            item.name(),
            FontId::proportional(skin::TEXT_SIZE),
            if selected {
                look.chosen_text
            } else {
                look.text
            },
        );
        if form.tick(tick, ("項目を表示する", &key), list[nth].shown) {
            list[nth].shown = !list[nth].shown;
        }
        let natural = status::natural_width(form.ui.ctx(), item, &shown, size);
        let width = if list[nth].width > 0 {
            list[nth].width as f32
        } else {
            natural
        };
        let room = (line.right() - PAD - (name.right() + 4.0)).max(MIN_WIDTH);
        let height = (size + 10.0).clamp(22.0, ROW - 4.0);
        let preview = Rect::from_min_size(
            pos2(name.right() + 4.0, line.center().y - height / 2.0),
            vec2(width.min(room), height),
        );
        form.ui
            .painter()
            .rect_filled(preview, 0.0, bar_skin.background);
        let mut inside = form.ui.new_child(egui::UiBuilder::new().max_rect(preview));
        inside.set_clip_rect(preview.intersect(form.ui.clip_rect()));
        status::preview(&mut inside, item, preview, &shown, &bar_skin, size);
        form.ui.painter().rect_stroke(
            preview,
            0.0,
            Stroke::new(1.0, bar_skin.divider),
            egui::StrokeKind::Inside,
        );
        let handle = Rect::from_center_size(
            pos2(preview.right(), preview.center().y),
            vec2(HANDLE, preview.height()),
        );
        let pull = form.ui.interact(
            handle,
            form.ui.scope_id().with(("項目の幅", &key)),
            Sense::click_and_drag(),
        );
        if pull.hovered() || pull.dragged() {
            form.ui
                .ctx()
                .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if pull.drag_started() || pull.clicked() {
            held.selected = Some(key.clone());
        }
        if pull.double_clicked() {
            list[nth].width = 0;
        } else if pull.dragged()
            && let Some(at) = pointer
        {
            list[nth].width = (at.x - preview.left()).clamp(MIN_WIDTH, MAX_WIDTH).round() as u32;
        }
        if selected || pull.dragged() {
            form.ui.painter().rect_filled(handle, 2.0, look.primary);
            let said = if list[nth].width > 0 {
                format!("{} px（ドラッグで変更）", list[nth].width)
            } else {
                format!("自動 {} px（ドラッグで変更）", natural.round())
            };
            form.ui.painter().text(
                pos2(handle.right() + 6.0, line.center().y),
                egui::Align2::LEFT_CENTER,
                said,
                FontId::proportional(skin::TEXT_SIZE - 1.5),
                look.chosen_text,
            );
        }
    }
    form.ui.painter().rect_stroke(
        whole,
        4.0,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    if let Some((from, to)) = moved {
        let item = list.remove(from);
        list.insert(to, item);
    }
    if released {
        held.moving = None;
    }
    form.ui
        .ctx()
        .data_mut(|data| data.insert_temp(held_id, held));
    *form.at = whole.bottom() + skin::ROW_GAP + 4.0;
    let button = Rect::from_min_size(pos2(left, *form.at), vec2(120.0, 26.0));
    if form.button(button, "項目を初期状態に戻す", "初期状態に戻す") {
        list = StatusItem::defaults();
    }
    let said = form.ui.painter().layout(
        String::from(
            "左端の点をドラッグして並べ替え、見本の右端をドラッグして幅を変更（ダブルクリックで自動）",
        ),
        FontId::proportional(skin::TEXT_SIZE - 1.5),
        look.label,
        (form.body.right() - skin::PAD - button.right() - 10.0).max(80.0),
    );
    let said_top = button.center().y - said.size().y / 2.0;
    let said_bottom = said_top + said.size().y;
    form.ui
        .painter()
        .galley(pos2(button.right() + 10.0, said_top), said, look.label);
    *form.at = button.bottom().max(said_bottom) + skin::ROW_GAP + 4.0;
    if list != before {
        edited.ui.set_status_list(list);
    }
    form.key("ui.status_multi_row");
    form.check(
        "",
        "入りきらない項目を二行目に表示する",
        &mut edited.ui.status_multi_row,
        loaded.ui.status_multi_row,
    );
    form.key("ui.status_max_rows");
    form.slider(
        "最大の行数",
        &mut edited.ui.status_max_rows,
        loaded.ui.status_max_rows,
        2..=4,
        "行",
    );
}
