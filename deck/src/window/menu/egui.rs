// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メニューバーをウィンドウの中に egui でレンダリングする（Windows と Linux）。Windows の OS のメニューはダークモードに従わないため。

use super::{self as menu, EMPTY, Item};
use crate::ui::icons::{self, Icon};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shown {
    Picked(menu::Picked),
    Open(usize, ::egui::Pos2),
}

pub fn show(ui: &mut ::egui::Ui, filling: &menu::Filling, native: bool) -> Option<Shown> {
    let mut pressed = None;
    spaced(ui);
    ::egui::MenuBar::new().ui(ui, |bar| {
        for (index, (label, items)) in menu::BAR.iter().enumerate() {
            if native {
                let response = bar.button(*label);
                if response.clicked() {
                    pressed = Some(Shown::Open(index, response.rect.left_bottom()));
                }
                continue;
            }
            bar.menu_button(*label, |menu| {
                if let Some(picked) = items_of(menu, items, filling) {
                    pressed = Some(Shown::Picked(picked));
                }
            });
        }
    });
    pressed
}

pub fn context(ui: &mut ::egui::Ui, filling: &menu::Filling) -> Option<menu::Picked> {
    spaced(ui);
    items_of(ui, menu::CONTEXT, filling)
}

fn spaced(ui: &mut ::egui::Ui) {
    ui.style_mut().spacing.item_spacing = ::egui::vec2(6.0, 3.0);
    ui.style_mut().spacing.button_padding = ::egui::vec2(8.0, 4.0);
}

/// メニューの中身をウィンドウの高さに収める。収まらなければ ▲▼ のボタンでスクロールする。
fn fitting<R>(ui: &mut ::egui::Ui, width: f32, add: impl FnOnce(&mut ::egui::Ui) -> R) -> R {
    // ポップアップの幅はここで決める（使える幅いっぱいに取ると、開いたポップアップの幅に引きずられて広がる）。
    ui.set_min_width(width);
    ui.set_max_width(width);
    let id = ui.id().with("はみ出し");
    let room = (ui.ctx().content_rect().height() - 24.0).clamp(120.0, MOST_ROWS * ROW);
    let (offset, content): (f32, f32) = ui.data(|data| data.get_temp(id)).unwrap_or_default();
    let over = content > room + 0.5;
    let view = if over { room - 2.0 * STRIP } else { room };
    let most = (content - view).max(0.0);
    let step = SCROLL_SPEED * ui.input(|input| input.stable_dt).min(0.1);
    let mut offset = offset.clamp(0.0, most);
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    if over && strip(ui, width, true, offset > 0.0) {
        offset = (offset - step).max(0.0);
    }
    let shown = ::egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(view)
        .auto_shrink([true, true])
        .scroll_bar_visibility(::egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .vertical_scroll_offset(offset)
        .show(ui, add);
    let mut offset = shown.state.offset.y;
    let content = shown.content_size.y;
    if over && strip(ui, width, false, offset < most) {
        offset = (offset + step).min(most);
    }
    ui.spacing_mut().item_spacing.y = spacing;
    ui.data_mut(|data| data.insert_temp(id, (offset, content)));
    shown.inner
}

fn strip(ui: &mut ::egui::Ui, width: f32, up: bool, more: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(::egui::vec2(width, STRIP), ::egui::Sense::hover());
    let going = response.hovered() && more;
    let visuals = ui.visuals();
    if going {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        ui.ctx().request_repaint();
    }
    let ink = if more {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let mark = if up { Icon::ChevUp } else { Icon::ChevDown };
    icons::paint_at(ui.painter(), rect.center(), MARK, mark, ink);
    going
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mark {
    None,
    Check,
    Dot,
}

/// メニューの一行。列は左からチェック・アイコン・名前・キー・▶。`icon` が `Some(None)` なら列を空けたまま。
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut ::egui::Ui,
    width: f32,
    mark: Mark,
    icon: Option<Option<&::egui::TextureHandle>>,
    label: &str,
    key: &str,
    sub: bool,
    enabled: bool,
) -> ::egui::Response {
    let sense = if enabled {
        ::egui::Sense::click()
    } else {
        ::egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(::egui::vec2(width, ROW), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals().clone();
    let painter = ui.painter();
    if enabled && (response.hovered() || response.has_focus()) || mark == Mark::Dot {
        painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let ink = if enabled {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let middle = rect.center().y;
    let gutter = rect.left() + GUTTER / 2.0;
    match mark {
        Mark::None => {}
        Mark::Check => {
            icons::paint_at(
                painter,
                ::egui::pos2(gutter, middle),
                MARK,
                Icon::Check,
                ink,
            );
        }
        Mark::Dot => {
            painter.circle_filled(::egui::pos2(gutter, middle), 3.0, ink);
        }
    }
    let mut text_left = rect.left() + GUTTER;
    if let Some(icon) = icon {
        if let Some(texture) = icon {
            let size = ::egui::vec2(LOGO.0, LOGO.1);
            let at = ::egui::Rect::from_center_size(
                ::egui::pos2(text_left + size.x / 2.0, middle),
                size,
            );
            painter.image(
                texture.id(),
                at,
                ::egui::Rect::from_min_max(::egui::pos2(0.0, 0.0), ::egui::pos2(1.0, 1.0)),
                ::egui::Color32::WHITE,
            );
        }
        text_left += ICON;
    }
    let font = ::egui::TextStyle::Button.resolve(ui.style());
    let right = rect.right() - ARROW;
    let key_width = if key.is_empty() {
        0.0
    } else {
        text_width(ui, key) + KEY_GAP
    };
    let mut job = ::egui::text::LayoutJob::simple_singleline(label.to_owned(), font.clone(), ink);
    job.wrap =
        ::egui::text::TextWrapping::truncate_at_width((right - key_width - text_left).max(8.0));
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let painter = ui.painter();
    painter.galley(
        ::egui::pos2(text_left, middle - galley.size().y / 2.0),
        galley,
        ink,
    );
    if !key.is_empty() {
        painter.text(
            ::egui::pos2(right, middle),
            ::egui::Align2::RIGHT_CENTER,
            key,
            font,
            visuals.weak_text_color(),
        );
    }
    if sub {
        let at = ::egui::pos2(rect.right() - ARROW / 2.0, middle);
        icons::paint_at(painter, at, MARK, Icon::ChevRight, ink);
    }
    response
}

fn text_width(ui: &::egui::Ui, text: &str) -> f32 {
    let font = ::egui::TextStyle::Button.resolve(ui.style());
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font, ::egui::Color32::WHITE)
            .size()
            .x
    })
}

fn menu_width<'a>(
    ui: &mut ::egui::Ui,
    labels: impl Iterator<Item = (&'a str, &'a str)>,
    icons: bool,
) -> f32 {
    let mut widest = 0.0f32;
    for (label, key) in labels {
        let key = if key.is_empty() {
            0.0
        } else {
            KEY_GAP + text_width(ui, key)
        };
        widest = widest.max(text_width(ui, label) + key);
    }
    let fixed = GUTTER + if icons { ICON } else { 0.0 } + ARROW + 8.0;
    (widest + fixed).clamp(WIDTH * 0.6, MOST_WIDTH)
}

fn sub_row<R>(
    ui: &mut ::egui::Ui,
    width: f32,
    label: &str,
    enabled: bool,
    icon: Option<Option<&::egui::TextureHandle>>,
    content: impl FnOnce(&mut ::egui::Ui) -> Option<R>,
) -> Option<R> {
    let response = row(ui, width, Mark::None, icon, label, "", true, enabled);
    if !enabled {
        return None;
    }
    ::egui::containers::menu::SubMenu::new()
        .show(ui, &response, content)
        .and_then(|shown| shown.inner)
}

pub fn items_of(
    ui: &mut ::egui::Ui,
    items: &[Item],
    filling: &menu::Filling,
) -> Option<menu::Picked> {
    let width = menu_width(
        ui,
        items.iter().filter_map(|item| match item {
            Item::Command {
                label, key, toggle, ..
            } => Some((*label, if *toggle { "" } else { key.unwrap_or_default() })),
            Item::Group { label, .. } => Some((*label, "")),
            Item::Separator => None,
        }),
        false,
    );
    fitting(ui, width, |ui| items_in(ui, width, items, filling))
}

fn items_in(
    ui: &mut ::egui::Ui,
    width: f32,
    items: &[Item],
    filling: &menu::Filling,
) -> Option<menu::Picked> {
    // 空のポップアップは壊れて見えるので、中身の無い見出しも出してその旨を表示する。
    if items.is_empty() {
        row(ui, WIDTH * 0.6, Mark::None, None, EMPTY, "", false, false);
        return None;
    }
    let mut pressed = None;
    for item in items {
        match item {
            Item::Separator => separator(ui, width),
            Item::Command {
                label,
                command,
                key,
                enabled,
                toggle,
            } => {
                let mark = if *toggle && filling.is_on(*command) {
                    Mark::Check
                } else {
                    Mark::None
                };
                let key = if *toggle { "" } else { key.unwrap_or_default() };
                if row(ui, width, mark, None, label, key, false, *enabled).clicked() {
                    pressed = Some(menu::Picked::Command(*command));
                    ui.close();
                }
            }
            Item::Group {
                label,
                group,
                enabled,
            } => {
                if let Some(picked) = sub_row(ui, width, label, *enabled, None, |ui| {
                    group_of(ui, *group, filling)
                }) {
                    pressed = Some(picked);
                    ui.close();
                }
            }
        }
    }
    pressed
}

fn separator(ui: &mut ::egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(::egui::vec2(width, 9.0), ::egui::Sense::hover());
    let y = rect.center().y;
    ui.painter().hline(
        rect.left() + 4.0..=rect.right() - 4.0,
        y,
        ::egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
}

fn group_of(
    ui: &mut ::egui::Ui,
    group: menu::Group,
    filling: &menu::Filling,
) -> Option<menu::Picked> {
    entries(ui, &menu::entries_of(group, filling))
}

fn entries(ui: &mut ::egui::Ui, entries: &[menu::Entry]) -> Option<menu::Picked> {
    use menu::Entry;
    let logos: Vec<Option<::egui::TextureHandle>> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Pick {
                logo: Some(logo), ..
            } => crate::ui::logo_texture(ui.ctx(), logo),
            _ => None,
        })
        .collect();
    let with_icons = logos.iter().any(Option::is_some);
    let width = menu_width(
        ui,
        entries.iter().filter_map(|entry| match entry {
            Entry::Pick { label, .. } | Entry::Sub { label, .. } | Entry::Off { label } => {
                Some((label.as_str(), ""))
            }
            _ => None,
        }),
        with_icons,
    );
    fitting(ui, width, |ui| {
        entries_in(ui, width, entries, &logos, with_icons)
    })
}

fn entries_in(
    ui: &mut ::egui::Ui,
    width: f32,
    entries: &[menu::Entry],
    logos: &[Option<::egui::TextureHandle>],
    with_icons: bool,
) -> Option<menu::Picked> {
    use menu::Entry;
    if entries.is_empty() {
        row(ui, WIDTH * 0.6, Mark::None, None, EMPTY, "", false, false);
        return None;
    }
    let mut picked = None;
    for (entry, logo) in entries.iter().zip(logos) {
        let icon = with_icons.then_some(logo.as_ref());
        match entry {
            Entry::Pick {
                label,
                chosen,
                picked: what,
                ..
            } => {
                let mark = if *chosen == Some(true) {
                    Mark::Dot
                } else {
                    Mark::None
                };
                if row(ui, width, mark, icon, label, "", false, true).clicked() {
                    picked = Some(*what);
                    ui.close();
                }
            }
            Entry::Sub {
                label,
                entries: below,
            } => {
                if let Some(what) =
                    sub_row(ui, width, label, true, icon, |ui| self::entries(ui, below))
                {
                    picked = Some(what);
                }
            }
            Entry::Break => {}
            Entry::Separator => separator(ui, width),
            Entry::Off { label } => {
                row(ui, width, Mark::None, icon, label, "", false, false);
            }
        }
    }
    picked
}

const ROW: f32 = 22.0;
const MOST_ROWS: f32 = 18.0;
const MOST_WIDTH: f32 = 360.0;
const KEY_GAP: f32 = 24.0;
const GUTTER: f32 = 28.0;
const ICON: f32 = 40.0;
const LOGO: (f32, f32) = (30.0, 17.0);
const ARROW: f32 = 20.0;
const MARK: f32 = 12.0;
const STRIP: f32 = 18.0;
const SCROLL_SPEED: f32 = 360.0;

const WIDTH: f32 = 240.0;
