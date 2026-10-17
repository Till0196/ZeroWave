// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! EPG 番組表の別ウィンドウ。局が列、時刻が行で、一度に出るのは一つの種別。
//! まだ届いていない時間帯は空白のまま（何も無い深夜と見分けが付かなくなるので書かない）。

use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
pub use zwcore::guide::listing::{Channel, Listing, badge_of};
use zwcore::guide::{Band, Genre, Key, Moment, Programme, civil, date_label, weekday};

use crate::ui::grid::{self, Grid};

use crate::ui::guide::chips;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::epg;

#[derive(Debug, Clone)]
pub struct Chosen {
    pub key: Key,
    pub programme: Programme,
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Box に入れるのは、他の変種よりずっと大きいから。
    Open(Box<Chosen>),
    Tune(Key),
    Band(Band),
    Shift(i64),
    Now,
    /// 今日が 0。
    Day(u32),
    /// 番組検索のウィンドウを開いてこのキーワードで検索する。番組表はそのまま。
    Search(String),
}

const SEARCH_WIDTH: f32 = 200.0;
const NAV_MARK: f32 = 10.0;
const NAV_MARK_GAP: f32 = 4.0;

/// EIT の予定表は最大で八日分（ARIB TR-B14）。
pub const DAYS: u32 = 8;

pub const DAY_STARTS_AT: i64 = 4 * 3600;

pub fn day_start(moment: Moment) -> Moment {
    let shifted = moment - DAY_STARTS_AT;
    shifted - shifted.rem_euclid(86_400) + DAY_STARTS_AT
}

/// egui の既定は「ポインターを重ねるまで見えない」細いスクロールバーなので、トラックごといつも出す太めのバーに替える
/// （番組表は横にもスクロールできると分かるのがそこだけ）。
pub fn scroll_bars(ui: &mut Ui, skin: &epg::Skin) {
    let style = ui.style_mut();
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: epg::SCROLL_BAR,
        bar_inner_margin: epg::SCROLL_MARGIN,
        bar_outer_margin: epg::SCROLL_MARGIN,
        handle_min_length: 24.0,
        ..egui::style::ScrollStyle::solid()
    };
    let visuals = &mut style.visuals;
    visuals.extreme_bg_color = skin.scroll_track;
    let radius = egui::CornerRadius::same((epg::SCROLL_BAR / 2.0) as u8);
    for (widget, colour) in [
        (&mut visuals.widgets.inactive, skin.scroll_handle),
        (&mut visuals.widgets.hovered, skin.scroll_hover),
        (&mut visuals.widgets.active, skin.scroll_held),
    ] {
        widget.bg_fill = colour;
        widget.corner_radius = radius;
    }
}

/// 番組検索から送った番組を枠で囲む秒数（最後の一秒で薄れる）。
const MARK_SECONDS: f64 = 6.0;

/// 番組の領域のスクロール量を同じフレームで見出しと時刻に伝えるので一フレーム遅れない。`jump_to` は一度だけ。
pub fn show(
    ui: &mut Ui,
    listing: &Listing,
    jump_to: Option<Key>,
    skin: &epg::Skin,
) -> Option<Action> {
    // `run_ui` は `CentralPanel` を置かないので、背景を自分で塗る（別ウィンドウでは黒が残る）。
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let (mut action, bar_height) = bar(ui, full, listing, skin);

    let below = Rect::from_min_max(pos2(full.left(), full.top() + bar_height), full.max);
    let head_row = Rect::from_min_size(below.min, vec2(below.width(), epg::HEAD_HEIGHT));
    let body = Rect::from_min_max(pos2(below.left(), head_row.bottom()), below.max);
    let body_right = Rect::from_min_max(pos2(body.left() + epg::CLOCK_WIDTH, body.top()), body.max);

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body_right));
    scroll_bars(&mut child, skin);
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    if let Some(jump) = listing.jump {
        let seconds = (jump - listing.from).max(0) as f32;
        area = area.vertical_scroll_offset((seconds / 3600.0 * epg::HOUR - 60.0).max(0.0));
    }
    // スクロールは一度きりなので、囲む番組は egui のメモリーに置く。
    let marked_id = ui.scope_id().with("送った番組");
    let time = ui.input(|input| input.time);
    if let (Some(key), Some(start)) = (jump_to, listing.jump) {
        ui.data_mut(|data| data.insert_temp(marked_id, (key, start, time)));
    }
    let marked: Option<(Key, Moment, f32)> = ui
        .data(|data| data.get_temp::<(Key, Moment, f64)>(marked_id))
        .and_then(|(key, start, at)| {
            let left = MARK_SECONDS - (time - at);
            (left > 0.0).then(|| (key, start, (left as f32).min(1.0)))
        });
    if marked.is_some() {
        ui.ctx().request_repaint();
    }
    // 開いたときと種別を替えたときは、見ている局が左の端に来るようにスクロールする。
    let shoved_id = ui.scope_id().with("見出しの横の送り");
    let follow = super::mini::follow(
        ui,
        ui.scope_id().with("番組表を見ている局へ"),
        listing.watching,
        listing.band,
        listing.watching.and_then(|key| {
            listing
                .channels
                .iter()
                .position(|channel| channel.key == key)
                .map(|index| epg::COLUMN * index as f32)
        }),
    );
    // 縦のスクロールバーはいつも出ているので、その幅を除いた所が見える。
    let content = epg::COLUMN * listing.channels.len() as f32;
    let view = body_right.width() - child.spacing().scroll.allocated_width();
    if let Some(index) = jump_to.and_then(|key| {
        listing
            .channels
            .iter()
            .position(|channel| channel.key == key)
    }) {
        area = area.horizontal_scroll_offset(super::mini::within(
            epg::COLUMN * (index as f32 - 1.0),
            content,
            view,
        ));
    } else if let Some(offset) = follow {
        area = area.horizontal_scroll_offset(super::mini::within(offset, content, view));
    } else if let Some(offset) = ui.data_mut(|data| data.remove_temp::<f32>(shoved_id)) {
        area = area.horizontal_scroll_offset(offset);
    }
    let mut scrolled = area.show(&mut child, |ui| {
        let height = epg::HOUR * listing.hours as f32;
        let width = epg::COLUMN * listing.channels.len() as f32;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let mut picked = None;
        let visible = ui.clip_rect();
        for index in 0..listing.channels.len() {
            let left = rect.left() + epg::COLUMN * index as f32;
            if left > visible.right() {
                break;
            }
            // サブチャンネルは左へまとまるので、左に見えていない列から伸びるセルがこの列に掛かる。
            if left + epg::COLUMN * (1.0 + WIDEST as f32) < visible.left() {
                continue;
            }
            let column =
                Rect::from_min_size(pos2(left, rect.top()), vec2(epg::COLUMN, rect.height()));
            if let Some(happened) = one_channel(ui, column, index, listing, marked, skin) {
                picked = Some(happened);
            }
        }
        picked
    });
    if let Some(picked) = scrolled.inner.take() {
        action = Some(picked);
    }
    let offset = scrolled.state.unclamped_offset();
    // 時刻の列は下端のスクロールバーの上で止める（バーの横に時刻がはみ出さないように）。
    let seen_height = scrolled.inner_rect.height().min(body.height());
    if let Some(shoved) = sideways(ui, head_row, body_right, listing, &scrolled) {
        ui.data_mut(|data| data.insert_temp(shoved_id, shoved));
        ui.ctx().request_repaint();
    }

    let clock_column = Rect::from_min_size(body.min, vec2(epg::CLOCK_WIDTH, seen_height));
    clock(ui, clock_column, listing, skin, offset.y);
    let below_clock = Rect::from_min_max(
        pos2(body.left(), clock_column.bottom()),
        pos2(body_right.left(), body.bottom()),
    );
    if below_clock.height() > 0.0 {
        ui.painter()
            .rect_filled(below_clock, 0.0, skin.scroll_track);
    }
    if let Some(picked) = heads(ui, head_row, listing, skin, offset.x) {
        action = Some(picked);
    }
    let corner = Rect::from_min_size(head_row.min, vec2(epg::CLOCK_WIDTH, epg::HEAD_HEIGHT));
    ui.painter().rect_filled(corner, 0.0, skin.head_background);
    ui.painter().hline(
        corner.x_range(),
        corner.bottom(),
        Stroke::new(1.0, skin.head_edge),
    );
    action
}

/// 見出しの上では向きを問わず横にスクロールする。返す横のオフセットは次のフレームで効かせる
/// （このフレームの見出しだけ先に動かすと領域と一フレームずれる）。
fn sideways(
    ui: &mut Ui,
    head_row: Rect,
    body: Rect,
    listing: &Listing,
    scrolled: &egui::scroll_area::ScrollAreaOutput<Option<Action>>,
) -> Option<f32> {
    let mut shove = 0.0;
    if ui.rect_contains_pointer(head_row) {
        let delta = ui.input(|input| input.smooth_scroll_delta());
        shove -= delta.x + delta.y;
    }
    let typing = ui.ctx().memory(|memory| memory.focused().is_some());
    let stepped = ui.input(|input| {
        i32::from(input.key_pressed(egui::Key::ArrowRight))
            - i32::from(input.key_pressed(egui::Key::ArrowLeft))
    });
    if !typing {
        shove += epg::COLUMN * stepped as f32;
    }
    if shove == 0.0 {
        return None;
    }
    let seen = scrolled.inner_rect.width().min(body.width());
    let most = (epg::COLUMN * listing.channels.len() as f32 - seen).max(0.0);
    Some((scrolled.state.clamped_offset().x + shove).clamp(0.0, most))
}

fn heads(
    ui: &mut Ui,
    row: Rect,
    listing: &Listing,
    skin: &epg::Skin,
    offset: f32,
) -> Option<Action> {
    let mut action = None;
    // 切らないと時刻の列の上に局の名前が乗る。右端は縦のスクロールバーの上で止める。
    let bar = epg::SCROLL_BAR + epg::SCROLL_MARGIN * 2.0;
    let inner = Rect::from_min_max(
        pos2(row.left() + epg::CLOCK_WIDTH, row.top()),
        pos2(row.right() - bar, row.bottom()),
    );
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(inner.right(), row.top()), row.max),
        0.0,
        skin.head_background,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner);
    for (index, channel) in listing.channels.iter().enumerate() {
        let left = inner.left() + epg::COLUMN * index as f32 - offset;
        if left > inner.right() {
            break;
        }
        if left + epg::COLUMN < inner.left() {
            continue;
        }
        let column = Rect::from_min_size(pos2(left, row.top()), vec2(epg::COLUMN, row.height()));
        let watching = listing.watching == Some(channel.key);
        if head(&mut child, column, channel, watching, skin) {
            action = Some(Action::Tune(channel.key));
        }
    }
    action
}

/// 返すのは押されたものとバーの高さ（狭いウィンドウでは日付のタブが二行になる）。
fn bar(ui: &mut Ui, full: Rect, listing: &Listing, skin: &epg::Skin) -> (Option<Action>, f32) {
    let day_labels: Vec<String> = match listing.now {
        Some(now) => {
            let today = day_start(now);
            (0..DAYS)
                .map(|n| {
                    if n == 0 {
                        let (_, month, day, _, _, _) = civil(today);
                        return format!("今日 ({month}/{day} {})", weekday(today));
                    }
                    date_label(today + i64::from(n) * 86_400)
                })
                .collect()
        }
        None => Vec::new(),
    };
    let days_width: f32 = day_labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    label.clone(),
                    crate::ui::text::font::guide(crate::ui::theme::chips::SIZE),
                    skin.bar_text,
                )
                .size()
                .x
                + crate::ui::theme::chips::PAD_X * 2.0
                + crate::ui::theme::chips::GAP
        })
        .sum();
    // 日付のタブが右端を越えるなら二行目へ。高さを先に決めるため、種別と送りの幅は見積もる。
    let mut rows = 1.0;
    let rect_one = Rect::from_min_size(full.min, vec2(full.width(), epg::BAR_HEIGHT));
    let mut estimate = 12.0;
    for band in &listing.bands {
        estimate += ui
            .painter()
            .layout_no_wrap(
                band.name().to_owned(),
                crate::ui::text::font::guide(crate::ui::theme::chips::SIZE),
                skin.bar_text,
            )
            .size()
            .x
            + crate::ui::theme::chips::PAD_X * 2.0
            + crate::ui::theme::chips::GAP;
    }
    estimate += 16.0 + 90.0 + 12.0 + SEARCH_WIDTH + 12.0;
    for (label, mark) in [("前日", true), ("今", false), ("翌日", true)] {
        estimate += ui
            .painter()
            .layout_no_wrap(
                label.to_owned(),
                crate::ui::text::font::guide(epg::BAR_SIZE),
                skin.bar_text,
            )
            .size()
            .x
            + if mark { NAV_MARK + NAV_MARK_GAP } else { 0.0 }
            + 16.0
            + 4.0;
    }
    if !day_labels.is_empty() && estimate + 16.0 + days_width > rect_one.width() - 12.0 {
        rows = 2.0;
    }
    let rect = Rect::from_min_size(full.min, vec2(full.width(), epg::BAR_HEIGHT * rows));
    ui.painter().rect_filled(rect, 0.0, skin.bar_background);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.bar_edge),
    );
    let row_y = rect_one.center().y;
    let mut action = None;
    let names: Vec<String> = listing.bands.iter().map(|band| band.name()).collect();
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let selected = listing
        .bands
        .iter()
        .position(|band| *band == listing.band)
        .unwrap_or(0);
    let (pressed, mut left) = chips::show(
        ui,
        pos2(rect.left() + 12.0, row_y),
        &labels,
        selected,
        chips::paper(skin.mode),
    );
    if let Some(index) = pressed
        && let Some(band) = listing.bands.get(index)
    {
        action = Some(Action::Band(*band));
    }
    left += 16.0;
    let date = date_label(listing.from);
    let galley = ui.painter().layout_no_wrap(
        date,
        crate::ui::text::font::guide(epg::BAR_SIZE),
        skin.bar_text,
    );
    ui.painter().galley(
        pos2(left, row_y - galley.size().y / 2.0),
        galley.clone(),
        skin.bar_text,
    );
    left += galley.size().x + 12.0;
    for (label, mark, what) in [
        ("前日", Some(Icon::TriLeft), Action::Shift(-86_400)),
        ("今", None, Action::Now),
        ("翌日", Some(Icon::TriRight), Action::Shift(86_400)),
    ] {
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            crate::ui::text::font::guide(epg::BAR_SIZE),
            skin.bar_text,
        );
        let marked = if mark.is_some() {
            NAV_MARK + NAV_MARK_GAP
        } else {
            0.0
        };
        let button = Rect::from_min_size(
            pos2(left, row_y - 12.0),
            vec2(galley.size().x + marked + 16.0, 24.0),
        );
        let response = ui.interact(button, ui.scope_id().with(("送り", label)), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(button, 4.0, skin.head_background);
        }
        let mut text_left = button.left() + 8.0;
        match mark {
            Some(Icon::TriLeft) => {
                icons::paint_at(
                    ui.painter(),
                    pos2(text_left + NAV_MARK / 2.0, button.center().y),
                    NAV_MARK,
                    Icon::TriLeft,
                    skin.bar_text,
                );
                text_left += marked;
            }
            Some(icon) => icons::paint_at(
                ui.painter(),
                pos2(
                    text_left + galley.size().x + NAV_MARK_GAP + NAV_MARK / 2.0,
                    button.center().y,
                ),
                NAV_MARK,
                icon,
                skin.bar_text,
            ),
            None => {}
        }
        ui.painter().galley(
            pos2(text_left, button.center().y - galley.size().y / 2.0),
            galley,
            skin.bar_text,
        );
        if response.clicked() {
            action = Some(what);
        }
        left = button.right() + 4.0;
    }
    if let Some(query) = search_field(ui, rect_one) {
        action = Some(Action::Search(query));
    }
    if let Some(now) = listing.now {
        let today = day_start(now);
        let labels: Vec<&str> = day_labels.iter().map(String::as_str).collect();
        let selected =
            ((day_start(listing.from) - today) / 86_400).clamp(0, i64::from(DAYS) - 1) as usize;
        let at = if rows > 1.0 {
            pos2(
                rect.left() + 12.0,
                rect_one.bottom() + epg::BAR_HEIGHT / 2.0,
            )
        } else {
            pos2(left + 12.0, row_y)
        };
        let (pressed, _) = chips::show(ui, at, &labels, selected, chips::paper(skin.mode));
        if let Some(index) = pressed {
            action = Some(if index == 0 {
                Action::Now
            } else {
                Action::Day(index as u32)
            });
        }
    }
    (action, rect.height())
}

/// Enter で検索する文字列を返す。一覧は毎回届き直すので、入力中の文字列は egui のメモリーに置く。
fn search_field(ui: &mut Ui, row: Rect) -> Option<String> {
    let id = ui.scope_id().with("番組検索");
    let mut text = ui
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_default();
    let field = Rect::from_min_size(
        pos2(row.right() - 12.0 - SEARCH_WIDTH, row.center().y - 12.0),
        vec2(SEARCH_WIDTH, 24.0),
    );
    let clear = Rect::from_min_size(
        pos2(field.right() - 22.0, field.top()),
        vec2(22.0, field.height()),
    );
    let response = ui.put(
        field,
        egui::TextEdit::singleline(&mut text)
            .hint_text("番組検索")
            .margin(egui::Margin {
                left: 4,
                right: 24,
                top: 2,
                bottom: 2,
            })
            .font(crate::ui::text::font::guide(epg::BAR_SIZE)),
    );
    if !text.is_empty() {
        let pressed = ui.interact(clear, id.with("消す"), Sense::click());
        let colour = if pressed.hovered() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        icons::paint_at(ui.painter(), clear.center(), 14.0, Icon::Cross, colour);
        if pressed.on_hover_text("キーワードを消す").clicked() {
            text.clear();
            response.request_focus();
        }
    }
    let entered = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    ui.data_mut(|data| data.insert_temp(id, text.clone()));
    entered.then_some(text)
}

fn clock(ui: &mut Ui, column: Rect, listing: &Listing, skin: &epg::Skin, offset: f32) {
    let painter = ui.painter().with_clip_rect(column);
    painter.rect_filled(column, 0.0, skin.clock_background);
    painter.vline(
        column.right(),
        column.y_range(),
        Stroke::new(1.35, skin.clock_edge),
    );
    let (_, _, _, from_hour, _, _) = civil(listing.from);
    for hour in 0..listing.hours {
        let top = column.top() + epg::HOUR * hour as f32 - offset;
        if top > column.bottom() {
            break;
        }
        if top + epg::HOUR < column.top() {
            continue;
        }
        painter.hline(column.x_range(), top, Stroke::new(1.0, skin.head_edge));
        painter.text(
            pos2(column.center().x, top + 4.0),
            egui::Align2::CENTER_TOP,
            (from_hour + hour) % 24,
            FontId::monospace(epg::CLOCK_SIZE),
            skin.clock_text,
        );
    }
}

fn one_channel(
    ui: &mut Ui,
    column: Rect,
    index: usize,
    listing: &Listing,
    marked: Option<(Key, Moment, f32)>,
    skin: &epg::Skin,
) -> Option<Action> {
    let channel = &listing.channels[index];
    let mut action = None;
    let body = column;
    // 見えているところだけレンダリングする。全部置くと一フレームが秒の単位になる。
    let visible = ui.clip_rect();
    for programme in &channel.programmes {
        // 左の列（同じストリームの本チャンネル）が同じ番組をやっていれば、そのセルに含まれている。
        if covered(&listing.channels, index, programme) {
            continue;
        }
        let Some(mut rect) = place(programme, body, listing) else {
            continue;
        };
        // 右のサブチャンネルが同じ番組なら、その列までセルを伸ばす。
        let span = span_of(&listing.channels, index, programme);
        rect.max.x += epg::COLUMN * span as f32;
        if !rect.intersects(visible) {
            continue;
        }
        if one_programme(ui, rect, programme, listing.now, skin) == Some(grid::Click::Primary) {
            action = Some(Action::Open(Box::new(Chosen {
                key: channel.key,
                programme: programme.clone(),
            })));
        }
        if let Some((key, start, strength)) = marked
            && key == channel.key
            && programme.start == Some(start)
        {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                Stroke::new(epg::MARK_WIDTH, skin.mark.gamma_multiply(strength)),
                egui::StrokeKind::Inside,
            );
        }
    }
    // いまの時刻の線は局の列ごとに引く（時刻の列の上には出さない）。
    if let Some(now) = listing.now
        && let Some(y) = grid_of(listing).at_moment(now, body.top())
    {
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(body.left(), y), vec2(body.width(), epg::NOW_WIDTH)),
            0.0,
            skin.now,
        );
    }
    action
}

fn head(ui: &mut Ui, column: Rect, channel: &Channel, watching: bool, skin: &epg::Skin) -> bool {
    let rect = Rect::from_min_size(column.min, vec2(column.width(), epg::HEAD_HEIGHT));
    let response = ui.interact(
        rect,
        ui.scope_id().with(("局", channel.key)),
        Sense::click(),
    );
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        0.0,
        if watching {
            skin.head_watching
        } else {
            skin.head_background
        },
    );
    if response.hovered() && !watching {
        painter.rect_filled(rect, 0.0, skin.head_background);
    }
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.head_edge),
    );
    painter.vline(
        rect.right(),
        rect.y_range(),
        Stroke::new(1.0, skin.head_edge),
    );
    let text_colour = if watching {
        skin.head_watching_text
    } else {
        skin.head_text
    };
    let badge = Rect::from_min_size(
        pos2(
            rect.center().x - epg::BADGE_WIDTH / 2.0,
            rect.top() + epg::HEAD_PAD,
        ),
        vec2(epg::BADGE_WIDTH, epg::BADGE_HEIGHT),
    );
    crate::ui::logo(painter, badge, channel, epg::BADGE_SIZE);
    if let Some(key) = channel.remote_key {
        painter.text(
            pos2(
                rect.left() + epg::HEAD_PAD,
                rect.top() + epg::HEAD_PAD - 1.0,
            ),
            egui::Align2::LEFT_TOP,
            key,
            crate::ui::text::font::guide(epg::KEY_SIZE),
            skin.head_key,
        );
    }
    let name = crate::ui::one_line(
        painter,
        &channel.name,
        crate::ui::text::font::guide(epg::HEAD_SIZE),
        text_colour,
        (rect.width() - epg::HEAD_PAD * 2.0).max(20.0),
        0.0,
    );
    painter.galley(
        pos2(
            rect.center().x - name.size().x / 2.0,
            badge.bottom() + epg::HEAD_GAP,
        ),
        name,
        text_colour,
    );
    // スケジュールがまだ全部届いていない局には、どこまで来たかを細く出す。
    let (arrived, wanted) = channel.filled;
    if wanted > 0 && arrived < wanted {
        let ratio = arrived as f32 / wanted as f32;
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left(), rect.bottom() - 2.0),
                vec2(rect.width() * ratio, 2.0),
            ),
            0.0,
            skin.clock_edge,
        );
    }
    response.clicked()
}

fn grid_of(listing: &Listing) -> Grid {
    Grid {
        from: listing.from,
        hours: listing.hours,
        hour: epg::HOUR,
    }
}

/// 一つのセルが右へ伸びる列の数のいちばん多いところ（サブチャンネルは三つまで）。
pub const WIDEST: usize = 3;

/// イベントグループ記述子の「事象共有」で片方がもう片方を指定しているとき同じ番組。
fn same_programme(a: &Programme, b: &Programme) -> bool {
    a.start.is_some()
        && a.start == b.start
        && (a.shared.contains(&(b.service, b.event)) || b.shared.contains(&(a.service, a.event)))
}

fn siblings(a: &Channel, b: &Channel) -> bool {
    a.transport == b.transport && a.key.network == b.key.network
}

/// この列の番組が、右に何列続けて同じか。まとめるのは隣り合う列だけ。
pub fn span_of(channels: &[Channel], index: usize, programme: &Programme) -> usize {
    let Some(here) = channels.get(index) else {
        return 0;
    };
    let mut span = 0;
    for next in channels.iter().skip(index + 1).take(WIDEST) {
        if !siblings(here, next) || !next.programmes.iter().any(|p| same_programme(programme, p)) {
            break;
        }
        span += 1;
    }
    span
}

/// この列の番組が、左の列のセルに含まれているか。
pub fn covered(channels: &[Channel], index: usize, programme: &Programme) -> bool {
    if index == 0 {
        return false;
    }
    let here = &channels[index];
    let left = &channels[index - 1];
    siblings(here, left) && left.programmes.iter().any(|p| same_programme(programme, p))
}

fn place(programme: &Programme, body: Rect, listing: &Listing) -> Option<Rect> {
    grid_of(listing).place(body, programme.start?, Some(programme.ends()?))
}

fn one_programme(
    ui: &mut Ui,
    rect: Rect,
    programme: &Programme,
    now: Option<Moment>,
    skin: &epg::Skin,
) -> Option<grid::Click> {
    grid::cell(
        ui,
        rect,
        programme,
        now,
        &grid::Style {
            skin,
            edge: skin.edge,
            minute: skin.minute,
            title: skin.title,
            detail: skin.detail,
            edges: grid::Edges {
                top: true,
                right: true,
                left: false,
            },
            pad: epg::PAD,
            minute_size: epg::MINUTE_SIZE,
            title_size: epg::TITLE_SIZE,
            title_leading: epg::TITLE_LEADING,
            detail_text: Some((epg::DETAIL_SIZE, epg::DETAIL_LEADING)),
            top_pad: 2.0,
            tag: "番組",
        },
    )
}

pub fn genre_colour(genre: Option<&Genre>, skin: &epg::Skin) -> Color32 {
    match genre {
        Some(genre) => skin.genres[usize::from(genre.family().unwrap_or(genre.large) & 0x0f)],
        None => skin.no_genre,
    }
}

/// 本物の局の色ではなく、名前から決める（ロゴを読み込むまでの仮の色）。
pub fn badge_colour(name: &str) -> Color32 {
    use crate::ui::theme::epg::BADGE_COLOURS as COLOURS;
    let mut sum: u32 = 0;
    for byte in name.bytes() {
        sum = sum.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    COLOURS[(sum as usize) % COLOURS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(service: u16, start: i64, seconds: u32, title: &str) -> Programme {
        let mut programme = crate::ui::a_programme(Some(start), Some(seconds));
        programme.service = service;
        programme.network = 0x7fe1;
        programme.event = (start / 60) as u16 + 1 + u16::from(title.contains("野球"));
        programme.title = if service == 1025 {
            String::new()
        } else {
            title.to_owned()
        };
        // 1024〜1026 の「ニュース」は三つで一つ、「高校野球」は 1025 と 1026。
        let others: &[u16] = if title == "ニュース" {
            &[1024, 1025, 1026]
        } else if title == "高校野球" {
            &[1025, 1026]
        } else {
            &[]
        };
        programme.shared = others
            .iter()
            .filter(|other| **other != service)
            .map(|other| (*other, programme.event))
            .collect();
        programme
    }

    fn channel(service: u16, transport: u16, programmes: Vec<Programme>) -> Channel {
        Channel {
            key: Key {
                network: 0x7fe1,
                service,
            },
            transport,
            programmes,
            ..Channel::default()
        }
    }

    /// 同じストリームのサブチャンネルが同じ番組なら一つのセルにまとめ、違うときだけ分ける。
    #[test]
    fn sub_channels_share_one_cell_while_the_programme_is_the_same() {
        let news = |service| programme(service, 0, 1800, "ニュース");
        let channels = vec![
            channel(
                1024,
                0x0400,
                vec![news(1024), programme(1024, 1800, 1800, "ドラマ")],
            ),
            // サブチャンネルは題を持たないこともある（同じ番号なら同じ番組）。
            channel(
                1025,
                0x0400,
                vec![news(1025), programme(1025, 1800, 1800, "高校野球")],
            ),
            channel(
                1026,
                0x0400,
                vec![news(1026), programme(1026, 1800, 1800, "高校野球")],
            ),
            channel(2048, 0x0401, vec![news(2048)]),
        ];
        assert_eq!(span_of(&channels, 0, &channels[0].programmes[0]), 2);
        assert!(covered(&channels, 1, &channels[1].programmes[0]));
        assert!(covered(&channels, 2, &channels[2].programmes[0]));
        assert!(!covered(&channels, 3, &channels[3].programmes[0]));
        assert_eq!(span_of(&channels, 0, &channels[0].programmes[1]), 0);
        assert!(!covered(&channels, 1, &channels[1].programmes[1]));
        assert_eq!(span_of(&channels, 1, &channels[1].programmes[1]), 1);
        assert!(covered(&channels, 2, &channels[2].programmes[1]));
    }

    #[test]
    fn the_same_number_with_a_different_title_is_another_programme() {
        let mut weather = programme(504, 0, 86400, "天気チャンネル");
        let mut light = programme(506, 0, 86400, "情報チャンネル");
        weather.event = 36865;
        light.event = 36865;
        let channels = vec![
            channel(504, 0x0002, vec![weather]),
            channel(506, 0x0002, vec![light]),
        ];
        assert_eq!(span_of(&channels, 0, &channels[0].programmes[0]), 0);
        assert!(!covered(&channels, 1, &channels[1].programmes[0]));
    }

    /// 放送が指定していなければ、同じ番号・同じ時刻でも別の番組。
    #[test]
    fn the_same_number_and_time_without_a_sharing_flag_is_another_programme() {
        let mut weather = programme(504, 0, 86400, "天気チャンネル");
        let mut light = programme(506, 0, 86400, "情報チャンネル");
        weather.event = 36865;
        light.event = 36865;
        let channels = vec![
            channel(504, 0x0002, vec![weather]),
            channel(506, 0x0002, vec![light]),
        ];
        assert_eq!(span_of(&channels, 0, &channels[0].programmes[0]), 0);
        assert!(!covered(&channels, 1, &channels[1].programmes[0]));
    }

    #[test]
    fn a_channel_keeps_its_colour() {
        assert_eq!(
            badge_colour("ゼロウェーブ総合"),
            badge_colour("ゼロウェーブ総合")
        );
    }

    #[test]
    fn a_light_badge_is_pale() {
        for colour in crate::ui::theme::epg::BADGE_COLOURS {
            let pale = crate::ui::light_badge(colour);
            let sum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
            assert!(sum(pale) > sum(colour) + 300, "{colour:?} {pale:?}");
            assert!(pale.r().min(pale.g()).min(pale.b()) >= 200);
        }
    }

    #[test]
    fn a_programme_outside_the_hours_shown_is_not_placed() {
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(130.0, epg::HOUR * 4.0));
        let listing = Listing {
            from: 0,
            hours: 4,
            ..Listing::default()
        };
        let make = |start, seconds| crate::ui::a_programme(Some(start), Some(seconds));
        assert!(place(&make(-7200, 3600), body, &listing).is_none());
        assert!(place(&make(4 * 3600, 3600), body, &listing).is_none());
        let crossing = place(&make(-1800, 3600), body, &listing).expect("またぐものが置けない");
        assert_eq!(crossing.top(), 0.0);
        assert!((crossing.height() - epg::HOUR / 2.0).abs() < 0.1);
    }

    #[test]
    fn a_programme_too_short_to_press_is_not_placed() {
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(130.0, epg::HOUR));
        let listing = Listing {
            from: 0,
            hours: 1,
            ..Listing::default()
        };
        let tiny = crate::ui::a_programme(Some(0), Some(20));
        assert!(place(&tiny, body, &listing).is_none());
    }
}
