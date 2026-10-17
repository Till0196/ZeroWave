// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 下のパネル。パネルの番組表にタブの列とつまみ、⧉・▾・✕ を付けたもの。
//! 別ウィンドウに切り離しても同じ物を表示する（つまみの代わりにウィンドウの大きさで行が増える）。

use std::collections::BTreeMap;

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::ui::guide::mini;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::mini as look;

/// プラグインがパネルを出していないときもタブの列を出すか。
pub const ALWAYS_TABS: bool = true;

pub const WINDOW_TITLE: &str = "パネルウィンドウ - ZeroWave";

pub const WINDOW_WIDTH: f32 = 760.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    Guide,
    Info,
    /// プラグインのパネル（マニフェストの `ui.panels`）。
    Plugin {
        stem: String,
        id: String,
        title: String,
    },
}

impl Tab {
    /// `state.toml` に保存する名前。
    pub fn key(&self) -> String {
        match self {
            Self::Guide => String::from("guide"),
            Self::Info => String::from("info"),
            Self::Plugin { stem, id, .. } => format!("plugin:{stem}:{id}"),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Guide => "番組表",
            Self::Info => "番組情報",
            Self::Plugin { title, .. } => title,
        }
    }
}

/// パネルの番組表・番組情報・プラグインのパネル（ファイル名、識別子、名前）の順。
pub fn tabs_of(panels: &[(String, String, String)]) -> Vec<Tab> {
    tabs_with(panels, ALWAYS_TABS)
}

fn tabs_with(panels: &[(String, String, String)], always: bool) -> Vec<Tab> {
    if !always && panels.is_empty() {
        return vec![Tab::Guide];
    }
    let mut tabs = vec![Tab::Guide, Tab::Info];
    tabs.extend(panels.iter().map(|(stem, id, title)| Tab::Plugin {
        stem: stem.clone(),
        id: id.clone(),
        title: title.clone(),
    }));
    tabs
}

pub fn chosen(tabs: &[Tab], key: &str) -> usize {
    tabs.iter().position(|tab| tab.key() == key).unwrap_or(0)
}

/// 入りきらないものは「⋯」へ。選んでいるタブは必ず見せる（入る範囲の最後のタブと入れ替える）。
pub fn fit(widths: &[f32], room: f32, chosen: usize) -> Vec<usize> {
    let mut shown = Vec::new();
    let mut used = 0.0;
    for (index, width) in widths.iter().enumerate() {
        if used + width > room && !shown.is_empty() {
            break;
        }
        used += width;
        shown.push(index);
    }
    if chosen < widths.len() && !shown.contains(&chosen) {
        let want = widths[chosen];
        while shown.len() > 1 && shown.iter().map(|i| widths[*i]).sum::<f32>() + want > room {
            shown.pop();
        }
        if shown.len() == 1 && widths[shown[0]] + want > room {
            shown.clear();
        }
        shown.push(chosen);
    }
    shown
}

pub fn height(rows: u32, folded: bool) -> f32 {
    look::GRIP + look::BAR_HEIGHT + if folded { 0.0 } else { mini::body_height(rows) }
}

pub fn rows_docked(height: f32) -> u32 {
    mini::rows_in(height - look::GRIP - look::BAR_HEIGHT)
}

pub fn rows_in_window(height: f32) -> u32 {
    mini::rows_in(height - look::BAR_HEIGHT)
}

pub fn window_height(rows: u32) -> f32 {
    look::BAR_HEIGHT + mini::body_height(rows)
}

pub struct Panel<'a> {
    pub strip: mini::Strip<'a>,
    pub tabs: &'a [Tab],
    pub tab: usize,
    pub folded: bool,
    /// 別ウィンドウに切り離している。
    pub detached: bool,
    pub detail: Option<&'a crate::draw::scene::Idle>,
    /// つまみで広げられる行の数の上限（映像の場所を残す）。
    pub most_rows: u32,
}

/// プラグインのインスタンスはレンダリングのスレッドが持つ。
pub struct Plugins<'a> {
    pub host: &'a mut crate::plugin_ui::Host,
    pub scrolls: &'a mut BTreeMap<String, (f32, f32)>,
    pub guide: &'a zwcore::guide::Guide,
    pub look: &'a crate::ui::theme::settings::Skin,
}

#[derive(Debug, Clone)]
pub enum Pressed {
    Strip(mini::Pressed),
    Tab(String),
    Detach,
    Dock,
    Fold,
    Close,
    Rows(u32),
    OpenPage(String, String),
}

/// 切り離したウィンドウから本体へ返すこと（`Request::Panel`）。
#[derive(Debug, Clone)]
pub enum Said {
    Pressed(Pressed),
    Rows(u32),
    /// ウィンドウの大きさが変わった。終わるときには伝えられないので、そのつど保存する。
    Size((u32, u32)),
    /// ウィンドウが閉じた。大きさと、人が閉じたか（なら下へ戻す）。
    Closed {
        size: (u32, u32),
        by_hand: bool,
    },
}

/// パネルの中身と選んでいるタブ（[`Tab::key`]、空ならパネルの番組表）。
#[derive(Debug, Clone)]
pub struct Feed {
    pub banded: crate::draw::scene::Banded,
    pub tab: String,
}

pub fn show(ui: &mut Ui, panel: &Panel<'_>, plugins: &mut Plugins<'_>) -> Option<Pressed> {
    let skin = &panel.strip.theme.mini;
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let mut top = full.top();
    let mut pressed = None;
    if !panel.detached {
        let rect = Rect::from_min_size(full.min, vec2(full.width(), look::GRIP));
        if let Some(rows) = grip(ui, rect, full.bottom(), panel, skin) {
            pressed = Some(Pressed::Rows(rows));
        }
        top = rect.bottom();
    }
    let bar_rect =
        Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), look::BAR_HEIGHT));
    if let Some(said) = bar(ui, bar_rect, panel, skin) {
        pressed = Some(said);
    }
    if panel.folded {
        return pressed;
    }
    let body = Rect::from_min_max(pos2(full.left(), bar_rect.bottom()), full.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    let said = match panel.tabs.get(panel.tab).unwrap_or(&Tab::Guide) {
        Tab::Guide => mini::show(&mut child, &panel.strip).map(Pressed::Strip),
        Tab::Info => {
            let detail_skin = panel.strip.theme.detail;
            match panel.detail {
                Some(info) => crate::ui::guide::info::show(&mut child, info, &detail_skin),
                None => {
                    child
                        .painter()
                        .rect_filled(body, 0.0, detail_skin.background);
                    child.painter().text(
                        body.center(),
                        egui::Align2::CENTER_CENTER,
                        "データの取得中です",
                        crate::ui::text::font::guide(look::BAR_TITLE_SIZE),
                        detail_skin.dim,
                    );
                }
            }
            None
        }
        Tab::Plugin { stem, id, .. } => crate::ui::panes::settings::plugin_page::show_panel(
            &mut child,
            body,
            stem,
            id,
            plugins.host,
            plugins.scrolls,
            plugins.guide,
            plugins.look,
        )
        .and_then(|reply| reply.open.map(|page| Pressed::OpenPage(stem.clone(), page))),
    };
    pressed.or(said)
}

fn grip(ui: &mut Ui, rect: Rect, bottom: f32, panel: &Panel<'_>, skin: &look::Skin) -> Option<u32> {
    let response = ui
        .interact(rect, ui.scope_id().with("パネルのつまみ"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("上下にドラッグして高さを変更");
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, skin.grip);
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, skin.top_edge));
    painter.rect_filled(
        Rect::from_center_size(rect.center(), vec2(44.0, 3.0)),
        1.5,
        if response.hovered() || response.dragged() {
            skin.tab_icon
        } else {
            skin.grip_mark
        },
    );
    if panel.folded || !response.dragged() {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    let rows = rows_docked(bottom - pointer.y).min(panel.most_rows.max(1));
    (rows != panel.strip.rows).then_some(rows)
}

fn bar(ui: &mut Ui, rect: Rect, panel: &Panel<'_>, skin: &look::Skin) -> Option<Pressed> {
    ui.painter().rect_filled(rect, 0.0, skin.bar);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.bar_edge),
    );
    let mut pressed = None;
    let mut right = rect.right();
    if panel.detached {
        let label = "本体に戻す";
        let font = crate::ui::text::font::guide(look::BAR_TITLE_SIZE);
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_owned(), font, egui::Color32::PLACEHOLDER);
        let width = galley.size().x + 14.0 + 6.0 + look::TAB_PAD * 2.0;
        let button =
            Rect::from_min_max(pos2(right - width, rect.top()), pos2(right, rect.bottom()));
        let response = ui.interact(
            button,
            ui.scope_id().with("パネルを本体に戻す"),
            Sense::click(),
        );
        let ink = if response.hovered() {
            skin.tab_text
        } else {
            skin.tab_icon
        };
        let icon = Rect::from_center_size(
            pos2(button.left() + look::TAB_PAD + 7.0, button.center().y),
            vec2(12.0, 12.0),
        );
        windows_icon(ui.painter(), icon, ink);
        ui.painter().galley(
            pos2(
                icon.right() + 6.0,
                button.center().y - galley.size().y / 2.0,
            ),
            galley,
            ink,
        );
        if response.clicked() {
            pressed = Some(Pressed::Dock);
        }
        right = button.left();
    } else {
        let icons: [(&str, Pressed); 3] = [
            ("閉じる", Pressed::Close),
            (
                if panel.folded {
                    "広げる"
                } else {
                    "折りたたむ"
                },
                Pressed::Fold,
            ),
            ("別ウィンドウに切り離す", Pressed::Detach),
        ];
        for (tip, said) in icons {
            let button = Rect::from_min_max(
                pos2(right - look::ICON_WIDTH, rect.top()),
                pos2(right, rect.bottom()),
            );
            let response = ui
                .interact(
                    button,
                    ui.scope_id().with(("パネルの釦", tip)),
                    Sense::click(),
                )
                .on_hover_text(tip);
            let ink = match (&said, response.hovered()) {
                (Pressed::Close, true) => skin.bar_clock,
                (Pressed::Close, false) => skin.close,
                (_, true) => skin.tab_text,
                (_, false) => skin.tab_icon,
            };
            let painter = ui.painter();
            match said {
                Pressed::Close => {
                    icons::paint_at(painter, button.center(), 16.0, Icon::Cross, ink);
                }
                Pressed::Fold => {
                    let fold = if panel.folded {
                        Icon::TriUp
                    } else {
                        Icon::TriDown
                    };
                    icons::paint_at(painter, button.center(), 12.0, fold, ink);
                }
                _ => windows_icon(
                    painter,
                    Rect::from_center_size(button.center(), vec2(12.0, 12.0)),
                    ink,
                ),
            }
            if response.clicked() {
                pressed = Some(said);
            }
            right = button.left();
        }
    }
    if let Some(said) = tab_row(ui, rect, right, panel, skin) {
        pressed = Some(said);
    }
    pressed
}

fn windows_icon(painter: &egui::Painter, rect: Rect, ink: egui::Color32) {
    let back = Rect::from_min_size(rect.min, rect.size() * 0.7);
    let front = Rect::from_min_max(rect.min + rect.size() * 0.3, rect.max);
    let stroke = Stroke::new(1.2, ink);
    painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
    painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
}

fn tab_row(
    ui: &mut Ui,
    rect: Rect,
    right: f32,
    panel: &Panel<'_>,
    skin: &look::Skin,
) -> Option<Pressed> {
    let font = crate::ui::text::font::guide(look::BAR_TITLE_SIZE);
    let tag_font = crate::ui::text::font::guide(look::TAG_SIZE);
    if panel.tabs.len() <= 1 {
        ui.painter().text(
            pos2(rect.left() + look::TAB_PAD, rect.center().y),
            egui::Align2::LEFT_CENTER,
            Tab::Guide.title(),
            font,
            skin.bar_title,
        );
        return None;
    }
    let galleys: Vec<(
        std::sync::Arc<egui::Galley>,
        Option<std::sync::Arc<egui::Galley>>,
    )> = panel
        .tabs
        .iter()
        .map(|tab| {
            let text = ui.painter().layout_no_wrap(
                tab.title().to_owned(),
                font.clone(),
                egui::Color32::PLACEHOLDER,
            );
            let tag = matches!(tab, Tab::Plugin { .. }).then(|| {
                ui.painter()
                    .layout_no_wrap(String::from("P"), tag_font.clone(), skin.tab_tag)
            });
            (text, tag)
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|(text, tag)| {
            text.size().x + look::TAB_PAD * 2.0 + tag.as_ref().map_or(0.0, |tag| tag.size().x + 5.0)
        })
        .collect();
    let room = right - rect.left();
    let mut shown = fit(&widths, room, panel.tab);
    let overflow = shown.len() < panel.tabs.len();
    if overflow {
        shown = fit(&widths, room - look::ICON_WIDTH, panel.tab);
    }
    let mut pressed = None;
    let mut x = rect.left();
    for index in &shown {
        let (text, tag) = &galleys[*index];
        let tab_rect =
            Rect::from_min_size(pos2(x, rect.top()), vec2(widths[*index], rect.height()));
        let tab = &panel.tabs[*index];
        let response = ui.interact(
            tab_rect,
            ui.scope_id().with(("パネルのタブ", tab.key())),
            Sense::click(),
        );
        let on = *index == panel.tab;
        let ink = if on || response.hovered() {
            skin.tab_text
        } else {
            skin.tab_dim
        };
        let painter = ui.painter();
        painter.galley(
            pos2(
                tab_rect.left() + look::TAB_PAD,
                tab_rect.center().y - text.size().y / 2.0,
            ),
            text.clone(),
            ink,
        );
        if let Some(tag) = tag {
            painter.galley(
                pos2(
                    tab_rect.left() + look::TAB_PAD + text.size().x + 5.0,
                    tab_rect.center().y - tag.size().y / 2.0 + 1.0,
                ),
                tag.clone(),
                skin.tab_tag,
            );
        }
        if on {
            painter.rect_filled(
                Rect::from_min_max(pos2(tab_rect.left(), tab_rect.bottom() - 2.0), tab_rect.max),
                0.0,
                skin.tab_mark,
            );
        }
        if response.clicked() && (!on || panel.folded) {
            pressed = Some(Pressed::Tab(tab.key()));
        }
        x = tab_rect.right();
    }
    if overflow {
        let more = Rect::from_min_size(pos2(x, rect.top()), vec2(look::ICON_WIDTH, rect.height()));
        let response = ui
            .interact(
                more,
                ui.scope_id().with("パネルのほかのタブ"),
                Sense::click(),
            )
            .on_hover_text("ほかのタブ");
        let ink = if response.hovered() {
            skin.tab_text
        } else {
            skin.tab_dim
        };
        for dx in [-5.0, 0.0, 5.0] {
            ui.painter()
                .circle_filled(pos2(more.center().x + dx, more.center().y), 1.4, ink);
        }
        egui::Popup::menu(&response).show(|menu| {
            menu.set_min_width(160.0);
            for (index, tab) in panel.tabs.iter().enumerate() {
                if shown.contains(&index) {
                    continue;
                }
                let label = if matches!(tab, Tab::Plugin { .. }) {
                    format!("{}  P", tab.title())
                } else {
                    tab.title().to_owned()
                };
                if menu
                    .button(egui::RichText::new(label).font(font.clone()))
                    .clicked()
                {
                    pressed = Some(Pressed::Tab(tab.key()));
                    menu.close();
                }
            }
        });
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> Vec<(String, String, String)> {
        vec![(
            String::from("video_filter"),
            String::from("filter"),
            String::from("映像フィルター"),
        )]
    }

    #[test]
    fn tabs_come_in_order() {
        let tabs = tabs_with(&filter(), true);
        let keys: Vec<String> = tabs.iter().map(Tab::key).collect();
        assert_eq!(keys, ["guide", "info", "plugin:video_filter:filter"]);
        assert_eq!(tabs[2].title(), "映像フィルター");
        assert_eq!(
            tabs_with(&[], true).len(),
            2,
            "パネルが無くてもタブの列は出す"
        );
        assert_eq!(tabs_with(&[], false), [Tab::Guide]);
        assert_eq!(tabs_with(&filter(), false).len(), 3);
        assert_eq!(chosen(&tabs, "plugin:video_filter:filter"), 2);
        assert_eq!(
            chosen(&tabs, "plugin:gone:x"),
            0,
            "無くなったタブは番組表へ"
        );
    }

    #[test]
    fn tabs_that_do_not_fit_go_to_more() {
        let widths = [80.0, 60.0, 100.0, 90.0];
        assert_eq!(fit(&widths, 1000.0, 0), [0, 1, 2, 3]);
        assert_eq!(fit(&widths, 250.0, 0), [0, 1, 2]);
        assert_eq!(fit(&widths, 250.0, 3), [0, 1, 3]);
        assert_eq!(fit(&widths, 50.0, 0), [0], "一つは出す");
        assert_eq!(fit(&widths, 50.0, 2), [2]);
    }

    #[test]
    fn heights_and_rows() {
        for rows in [1, 2, 4, 7] {
            assert_eq!(rows_docked(height(rows, false)), rows);
            assert_eq!(rows_in_window(window_height(rows)), rows);
        }
        assert_eq!(height(4, true), look::GRIP + look::BAR_HEIGHT);
        assert!(height(4, false) > height(2, false));
    }
}
