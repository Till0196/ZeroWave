// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組検索の別ウィンドウ。探すのは番組表のスレッド（`zwcore::guide::keeper`）で、
//! 結果は一覧（[`Listing::search`]）に入って届く。

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::guide::listing::{Asked, Hit, Listing, Search, marks};
use zwcore::guide::{Band, Genre, Key, Moment, airtime, civil, date_label, weekday};

use crate::ui::guide::chips;
use crate::ui::guide::epg::{Chosen, badge_colour, scroll_bars};
use crate::ui::icons::{self, Icon};
use crate::ui::text::badges;
use crate::ui::text::font::guide as font;
use crate::ui::theme::epg;

/// ウィンドウを閉じても、次に開くまで残す。
#[derive(Debug, Clone, Default)]
pub struct State {
    /// 変わったら番組表のスレッドへ要求し直す（[`Pressed::Ask`]）。
    pub asked: Asked,
    pub typed: String,
    /// 選んだ行（局と始まり）。結果が入れ替わっても同じ番組を指す。
    pub chosen: Option<(Key, Option<Moment>)>,
    pub fresh: bool,
}

impl State {
    pub fn ask(&mut self, query: &str) {
        query.clone_into(&mut self.typed);
        query.clone_into(&mut self.asked.query);
        self.chosen = None;
        self.fresh = true;
    }
}

#[derive(Debug, Clone)]
pub enum Pressed {
    Ask,
    Open(Box<Chosen>),
    Jump(Key, Moment),
}

/// 選べる大分類（ARIB STD-B10 付属 H）。0xC・0xD は予備、0xE は拡張なので並べない。
pub const LARGES: [u8; 13] = [
    0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xf,
];

pub fn middles(large: u8) -> Vec<(u8, &'static str)> {
    (0x0..=0xf)
        .filter_map(|middle| {
            Genre::new(large, middle)
                .middle_name()
                .map(|name| (middle, name))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Day(String, bool),
    Hit(usize),
}

/// 結果は開始時刻の順に来る。
pub fn rows(hits: &[Hit], now: Option<Moment>) -> Vec<Row> {
    let mut out = Vec::new();
    let mut day: Option<i64> = None;
    for (index, hit) in hits.iter().enumerate() {
        let start = hit.programme.start.unwrap_or(0);
        let this = start.div_euclid(86_400);
        if day != Some(this) {
            day = Some(this);
            let all_ended = hits[index..]
                .iter()
                .take_while(|other| other.programme.start.unwrap_or(0).div_euclid(86_400) == this)
                .all(|other| ended(other, now));
            out.push(Row::Day(date_label(start), all_ended));
        }
        out.push(Row::Hit(index));
    }
    out
}

fn ended(hit: &Hit, now: Option<Moment>) -> bool {
    matches!((now, hit.programme.ends()), (Some(now), Some(ends)) if ends <= now)
}

/// 件数の欄。内訳は局のある種別ぜんぶ。
pub fn count_line(search: &Search, bands: &[Band]) -> String {
    let parts: Vec<String> = bands
        .iter()
        .map(|band| format!("{} {}", band.name(), search.count_of(*band)))
        .collect();
    let mut line = format!("{} 件", search.total());
    if bands.len() > 1 || search.asked.band.is_some() {
        line.push_str(&format!("（{}）", parts.join("・")));
    }
    if search.more {
        line.push_str(&format!("　先頭の {} 件を表示", search.hits.len()));
    }
    line
}

fn long_when(start: Moment, seconds: Option<u32>, ends: Option<Moment>) -> String {
    let (_, month, day, _, _, _) = civil(start);
    let mut out = format!(
        "{month}月{day}日（{}）{}",
        weekday(start),
        airtime(start, ends)
    );
    if let Some(seconds) = seconds {
        out.push_str(&format!("（{} 分）", seconds / 60));
    }
    out
}

/// 一致した語を強調した文字列。`rows` が 0 なら折り返して全部。
#[allow(clippy::too_many_arguments)]
fn marked(
    text: &str,
    query: &str,
    font: FontId,
    colour: Color32,
    mark: Color32,
    width: f32,
    rows: usize,
    leading: Option<f32>,
) -> LayoutJob {
    let format = |background| TextFormat {
        font_id: font.clone(),
        color: colour,
        background,
        line_height: leading,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for range in marks(text, query) {
        if range.start > at {
            job.append(&text[at..range.start], 0.0, format(Color32::TRANSPARENT));
        }
        job.append(&text[range.clone()], 0.0, format(mark));
        at = range.end;
    }
    if at < text.len() || job.sections.is_empty() {
        job.append(&text[at..], 0.0, format(Color32::TRANSPARENT));
    }
    job.wrap = if rows == 0 {
        TextWrapping {
            max_width: width,
            ..Default::default()
        }
    } else {
        TextWrapping {
            max_rows: rows,
            ..TextWrapping::truncate_at_width(width)
        }
    };
    job
}

pub fn show(
    ui: &mut Ui,
    state: &mut State,
    listing: &Listing,
    skin: &epg::Skin,
) -> Option<Pressed> {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let mut pressed = None;

    let top = Rect::from_min_size(full.min, vec2(full.width(), epg::FOUND_BAR));
    if top_bar(ui, top, state, listing, skin) {
        pressed = Some(Pressed::Ask);
    }
    let strip = Rect::from_min_size(
        pos2(full.left(), top.bottom()),
        vec2(full.width(), epg::FOUND_FILTER),
    );
    let search = listing
        .search
        .as_deref()
        .filter(|search| search.asked == state.asked);
    if filter_bar(ui, strip, state, search, listing, skin) {
        pressed = Some(Pressed::Ask);
    }
    if matches!(pressed, Some(Pressed::Ask)) {
        state.chosen = None;
        state.fresh = true;
    }

    let side = Rect::from_min_max(
        pos2(full.right() - epg::FOUND_SIDE, strip.bottom()),
        full.max,
    );
    let list = Rect::from_min_max(
        pos2(full.left(), strip.bottom()),
        pos2(side.left(), full.bottom()),
    );
    let shown = listing
        .search
        .as_deref()
        .filter(|_| !state.asked.query.trim().is_empty());
    if let Some(picked) = results(ui, list, state, shown, listing.now, skin) {
        pressed = Some(picked);
    }
    if let Some(picked) = side_pane(ui, side, state, shown, skin) {
        pressed = Some(picked);
    }
    pressed
}

fn top_bar(
    ui: &mut Ui,
    rect: Rect,
    state: &mut State,
    listing: &Listing,
    skin: &epg::Skin,
) -> bool {
    let mut changed = false;
    ui.painter().rect_filled(rect, 0.0, skin.bar_background);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.bar_edge),
    );
    let names: Vec<String> = std::iter::once(String::from("すべて"))
        .chain(listing.bands.iter().map(|band| band.name()))
        .collect();
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let selected = state
        .asked
        .band
        .and_then(|band| listing.bands.iter().position(|known| *known == band))
        .map_or(0, |at| at + 1);
    let (tab, _) = chips::show(
        ui,
        pos2(rect.left() + 12.0, rect.center().y),
        &labels,
        selected,
        chips::paper(skin.mode),
    );
    if let Some(index) = tab
        && index != selected
    {
        state.asked.band = index
            .checked_sub(1)
            .and_then(|at| listing.bands.get(at).copied());
        changed = true;
    }

    let field = Rect::from_min_size(
        pos2(
            rect.right() - 12.0 - epg::FOUND_SEARCH_WIDTH,
            rect.center().y - 14.0,
        ),
        vec2(epg::FOUND_SEARCH_WIDTH, 28.0),
    );
    ui.painter().rect_filled(field, 14.0, skin.field);
    ui.painter().rect_stroke(
        field,
        14.0,
        Stroke::new(1.0, skin.control_edge),
        egui::StrokeKind::Inside,
    );
    let clear = Rect::from_center_size(
        pos2(field.right() - 16.0, field.center().y),
        vec2(22.0, 22.0),
    );
    let text_rect = Rect::from_min_max(
        pos2(field.left() + 12.0, field.top() + 3.0),
        pos2(clear.left() - 4.0, field.bottom() - 3.0),
    );
    let response = ui.put(
        text_rect,
        egui::TextEdit::singleline(&mut state.typed)
            .hint_text("番組名や説明の語を入れて Enter")
            .frame(egui::Frame::NONE)
            .text_color(skin.title)
            .font(font(epg::FOUND_SIZE)),
    );
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        let query = state.typed.trim().to_owned();
        if query != state.asked.query {
            state.asked.query = query;
            changed = true;
        }
    }
    if !state.typed.is_empty() {
        let hit = ui.interact(clear, ui.scope_id().with("番組検索を消す"), Sense::click());
        if hit.hovered() {
            ui.painter()
                .circle_filled(clear.center(), 10.0, skin.control_edge);
        }
        icons::paint_at(ui.painter(), clear.center(), 14.0, Icon::Cross, skin.detail);
        if hit.on_hover_text("消す").clicked() {
            state.typed.clear();
            if !state.asked.query.is_empty() {
                state.asked.query.clear();
                changed = true;
            }
        }
    }
    changed
}

fn filter_bar(
    ui: &mut Ui,
    rect: Rect,
    state: &mut State,
    search: Option<&Search>,
    listing: &Listing,
    skin: &epg::Skin,
) -> bool {
    let mut changed = false;
    ui.painter().rect_filled(rect, 0.0, skin.side_background);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.bar_edge),
    );
    let inner = rect.shrink2(vec2(epg::FOUND_PAD, 0.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 8.0;
    let label = |ui: &mut Ui, text: &str| {
        ui.label(
            egui::RichText::new(text)
                .font(font(11.0))
                .color(skin.head_key),
        );
    };

    label(ui, "ジャンル");
    let large = state.asked.genre.map(|(large, _)| large);
    let large_name = large.map_or("すべて", |large| Genre::new(large, 0).name());
    let mut picked: Option<Option<(u8, Option<u8>)>> = None;
    egui::ComboBox::from_id_salt("番組検索の大分類")
        .selected_text(egui::RichText::new(large_name).font(font(epg::FOUND_SIZE)))
        .width(150.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(large.is_none(), "すべて").clicked() {
                picked = Some(None);
            }
            for one in LARGES {
                if ui
                    .selectable_label(large == Some(one), Genre::new(one, 0).name())
                    .clicked()
                {
                    picked = Some(Some((one, None)));
                }
            }
        });
    if let Some(large) = large {
        let choices = middles(large);
        if !choices.is_empty() {
            let middle = state.asked.genre.and_then(|(_, middle)| middle);
            let middle_name = middle
                .and_then(|middle| Genre::new(large, middle).middle_name())
                .unwrap_or("すべて");
            egui::ComboBox::from_id_salt("番組検索の中分類")
                .selected_text(egui::RichText::new(middle_name).font(font(epg::FOUND_SIZE)))
                .width(150.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(middle.is_none(), "すべて").clicked() {
                        picked = Some(Some((large, None)));
                    }
                    for (one, name) in choices {
                        if ui.selectable_label(middle == Some(one), name).clicked() {
                            picked = Some(Some((large, Some(one))));
                        }
                    }
                });
        }
    }
    if let Some(genre) = picked
        && genre != state.asked.genre
    {
        state.asked.genre = genre;
        changed = true;
    }

    ui.add_space(8.0);
    label(ui, "期間");
    if let Some(index) = segments(
        ui,
        "期間",
        &["これから", "過去も含む"],
        usize::from(state.asked.past),
        skin,
    ) {
        state.asked.past = index == 1;
        changed = true;
    }
    ui.add_space(8.0);
    label(ui, "検索対象");
    if let Some(index) = segments(
        ui,
        "検索対象",
        &["番組名", "番組名と説明"],
        usize::from(!state.asked.titles_only),
        skin,
    ) {
        state.asked.titles_only = index == 0;
        changed = true;
    }

    // 内訳が入らなければ合計だけ、それも入らなければ「…」で切る（ポインターを重ねると全部）。
    let (told, short) = if state.asked.query.trim().is_empty() {
        (String::new(), String::new())
    } else {
        match search {
            Some(search) => (
                count_line(search, &listing.bands),
                format!("{} 件", search.total()),
            ),
            None => (String::from("探しています…"), String::from("探しています…")),
        }
    };
    let room = (inner.right() - ui.min_rect().right() - 16.0).max(0.0);
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font(epg::FOUND_SIZE), skin.head_text)
            .size()
            .x
    };
    let shown = if measure(&told) <= room {
        &told
    } else {
        &short
    };
    let mut job = egui::text::LayoutJob::simple_singleline(
        shown.clone(),
        font(epg::FOUND_SIZE),
        skin.head_text,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: room,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let at = pos2(
        inner.right() - galley.size().x,
        inner.center().y - galley.size().y / 2.0,
    );
    if shown != &told || galley.elided {
        ui.interact(
            Rect::from_min_size(at, galley.size()),
            ui.scope_id().with("件数"),
            egui::Sense::hover(),
        )
        .on_hover_text(told.clone());
    }
    ui.painter().galley(at, galley, skin.head_text);
    changed
}

fn segments(
    ui: &mut Ui,
    tag: &str,
    options: &[&str],
    selected: usize,
    skin: &epg::Skin,
) -> Option<usize> {
    let galleys: Vec<_> = options
        .iter()
        .map(|option| {
            ui.painter()
                // 色はレンダリングするときに選んだかで決める（ここで決めると後から変えられない）。
                .layout_no_wrap(
                    (*option).to_owned(),
                    font(epg::FOUND_SIZE),
                    Color32::PLACEHOLDER,
                )
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|galley| galley.size().x + 20.0)
        .collect();
    let (rect, _) = ui.allocate_exact_size(vec2(widths.iter().sum(), 26.0), Sense::hover());
    let mut pressed = None;
    let mut left = rect.left();
    for (index, (galley, width)) in galleys.into_iter().zip(widths).enumerate() {
        let piece = Rect::from_min_size(pos2(left, rect.top()), vec2(width, rect.height()));
        let response = ui.interact(
            piece,
            ui.scope_id().with(("切り替え", tag, index)),
            Sense::click(),
        );
        let chosen = index == selected;
        let corner = if index == 0 {
            egui::CornerRadius {
                nw: 6,
                sw: 6,
                ..Default::default()
            }
        } else if index + 1 == options.len() {
            egui::CornerRadius {
                ne: 6,
                se: 6,
                ..Default::default()
            }
        } else {
            egui::CornerRadius::ZERO
        };
        if chosen {
            ui.painter().rect_filled(piece, corner, skin.accent);
        } else if response.hovered() {
            ui.painter().rect_filled(piece, corner, skin.chosen_row);
        }
        let colour = if chosen { skin.accent_text } else { skin.title };
        ui.painter().galley(
            pos2(
                piece.center().x - galley.size().x / 2.0,
                piece.center().y - galley.size().y / 2.0,
            ),
            galley,
            colour,
        );
        if response.clicked() && !chosen {
            pressed = Some(index);
        }
        left += width;
    }
    ui.painter().rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, skin.control_edge),
        egui::StrokeKind::Inside,
    );
    pressed
}

fn badge(painter: &egui::Painter, rect: Rect, hit: &Hit) {
    if crate::ui::draw_logo(painter, rect, hit.logo.as_ref(), Color32::WHITE) {
        return;
    }
    let (ground, text) = crate::ui::badge_look(painter, badge_colour(&hit.channel));
    painter.rect_filled(rect, 2.0, ground);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &hit.badge,
        FontId::proportional(epg::BADGE_SIZE),
        text,
    );
}

fn summary(hit: &Hit) -> String {
    let programme = &hit.programme;
    let text = if programme.detail.trim().is_empty() {
        programme
            .extended
            .first()
            .map(|(_, body)| body.as_str())
            .unwrap_or_default()
    } else {
        programme.detail.as_str()
    };
    badges::cleaned(text).replace(['\n', '\r'], " ")
}

fn results(
    ui: &mut Ui,
    area: Rect,
    state: &mut State,
    search: Option<&Search>,
    now: Option<Moment>,
    skin: &epg::Skin,
) -> Option<Pressed> {
    let Some(search) = search else {
        let hint = if state.asked.query.trim().is_empty() {
            "番組名や説明の語を入れて Enter で探します"
        } else {
            "探しています…"
        };
        ui.painter().text(
            area.center(),
            Align2::CENTER_CENTER,
            hint,
            font(epg::FOUND_SIZE),
            skin.detail,
        );
        return None;
    };
    if search.hits.is_empty() {
        ui.painter().text(
            area.center(),
            Align2::CENTER_CENTER,
            format!("「{}」に合う番組はありません", search.asked.query),
            font(epg::FOUND_SIZE),
            skin.detail,
        );
        return None;
    }
    let laid = rows(&search.hits, now);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    scroll_bars(&mut child, skin);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("番組検索の結果")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
    // 先頭へ戻すのは新しい結果が来てから（古い結果で戻しても、届いた結果は途中から出る）。
    if search.asked == state.asked && std::mem::take(&mut state.fresh) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let query = search.asked.query.as_str();
    scroll
        .show(&mut child, |ui| {
            let mut pressed = None;
            for row in &laid {
                match row {
                    Row::Day(label, all_ended) => {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), epg::FOUND_DAY),
                            Sense::hover(),
                        );
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let text = if *all_ended {
                            format!("{label}（終わった番組）")
                        } else {
                            label.clone()
                        };
                        ui.painter().text(
                            pos2(rect.left() + epg::FOUND_PAD, rect.bottom() - 4.0),
                            Align2::LEFT_BOTTOM,
                            text,
                            font(11.5),
                            skin.day_text,
                        );
                    }
                    Row::Hit(index) => {
                        let hit = &search.hits[*index];
                        let (rect, response) = ui.allocate_exact_size(
                            vec2(ui.available_width(), epg::FOUND_ROW),
                            Sense::click(),
                        );
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let key = (hit.key, hit.programme.start);
                        if response.clicked() || response.double_clicked() {
                            state.chosen = Some(key);
                        }
                        if response.double_clicked() {
                            pressed = Some(Pressed::Open(Box::new(Chosen {
                                key: hit.key,
                                programme: hit.programme.clone(),
                            })));
                        }
                        let chosen = state.chosen == Some(key);
                        one_row(ui, rect, hit, query, chosen, response.hovered(), now, skin);
                    }
                }
            }
            pressed
        })
        .inner
}

#[allow(clippy::too_many_arguments)]
fn one_row(
    ui: &Ui,
    rect: Rect,
    hit: &Hit,
    query: &str,
    chosen: bool,
    hovered: bool,
    now: Option<Moment>,
    skin: &epg::Skin,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if chosen {
        painter.rect_filled(rect, 0.0, skin.chosen_row);
    } else if hovered {
        painter.rect_filled(rect, 0.0, skin.row_hover);
    }
    painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, skin.edge));
    let fade = if ended(hit, now) {
        epg::FOUND_ENDED
    } else {
        1.0
    };
    let title_colour = skin.title.gamma_multiply(fade);
    let other_colour = skin.minute.gamma_multiply(fade);
    let detail_colour = skin.detail.gamma_multiply(fade);
    let middle = rect.center().y;
    let programme = &hit.programme;

    let mut x = rect.left() + epg::FOUND_PAD;
    if let Some(start) = programme.start {
        painter.text(
            pos2(x, middle),
            Align2::LEFT_CENTER,
            airtime(start, programme.ends()),
            font(epg::FOUND_SIZE),
            other_colour,
        );
    }
    x += epg::FOUND_TIME + epg::FOUND_GAP;
    let mark = Rect::from_min_size(
        pos2(x, middle - epg::BADGE_HEIGHT / 2.0),
        vec2(epg::BADGE_WIDTH, epg::BADGE_HEIGHT),
    );
    badge(&painter, mark, hit);
    let name = crate::ui::one_line(
        &painter,
        &hit.channel,
        font(epg::FOUND_SIZE),
        other_colour,
        epg::FOUND_CHANNEL - epg::BADGE_WIDTH - 6.0,
        0.0,
    );
    painter.galley(
        pos2(mark.right() + 6.0, middle - name.size().y / 2.0),
        name,
        other_colour,
    );
    x += epg::FOUND_CHANNEL + epg::FOUND_GAP;

    let genre_left = rect.right() - epg::FOUND_PAD - epg::FOUND_GENRE;
    let room = (genre_left - epg::FOUND_GAP - x).max(0.0);
    let titled = badges::titled(&programme.title, &programme.detail);
    let size = epg::FOUND_TITLE_SIZE;
    let look = badges::Look::Coloured;
    let pre = badges::width(&painter, &titled.pre, size, look);
    let post = badges::width(&painter, &titled.post, size, look);
    let title_top = rect.top() + 7.0;
    let galley = painter.layout_job(marked(
        &titled.name,
        query,
        font(size),
        title_colour,
        skin.found_mark,
        (room - pre - post).max(0.0),
        1,
        None,
    ));
    let title_middle = title_top + galley.size().y / 2.0;
    let mut at = badges::draw(
        &painter,
        pos2(x, title_middle),
        &titled.pre,
        size,
        look,
        title_colour,
        fade,
    );
    painter.galley(pos2(at, title_top), galley.clone(), title_colour);
    at += galley.size().x;
    if !titled.post.is_empty() {
        badges::draw(
            &painter,
            pos2(at + 3.0, title_middle),
            &titled.post,
            size,
            look,
            title_colour,
            fade,
        );
    }
    let summary = summary(hit);
    if !summary.is_empty() {
        let galley = painter.layout_job(marked(
            &summary,
            query,
            font(epg::FOUND_DETAIL_SIZE),
            detail_colour,
            skin.found_mark,
            room,
            1,
            None,
        ));
        painter.galley(
            pos2(x, rect.bottom() - 7.0 - galley.size().y),
            galley,
            detail_colour,
        );
    }

    if let Some(genre) = programme.genre() {
        let galley = crate::ui::one_line(
            &painter,
            genre.name(),
            font(11.0),
            other_colour,
            epg::FOUND_GENRE - 16.0,
            0.0,
        );
        let chip = Rect::from_min_size(
            pos2(genre_left, middle - 10.0),
            vec2(galley.size().x + 16.0, 20.0),
        );
        painter.rect_stroke(
            chip,
            10.0,
            Stroke::new(1.0, skin.control_edge),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            pos2(chip.left() + 8.0, chip.center().y - galley.size().y / 2.0),
            galley,
            other_colour,
        );
    }
}

fn side_pane(
    ui: &mut Ui,
    rect: Rect,
    state: &State,
    search: Option<&Search>,
    skin: &epg::Skin,
) -> Option<Pressed> {
    ui.painter().rect_filled(rect, 0.0, skin.side_background);
    ui.painter()
        .vline(rect.left(), rect.y_range(), Stroke::new(1.0, skin.bar_edge));
    let inner = rect.shrink(16.0);
    let hit = state.chosen.and_then(|(key, start)| {
        search?
            .hits
            .iter()
            .find(|hit| hit.key == key && hit.programme.start == start)
    });
    let Some(hit) = hit else {
        ui.painter().text(
            inner.center(),
            Align2::CENTER_CENTER,
            "番組を選ぶと、ここに詳しく出ます",
            font(epg::FOUND_SIZE),
            skin.detail,
        );
        return None;
    };
    let query = search.map_or("", |search| search.asked.query.as_str());
    let programme = &hit.programme;
    let painter = ui.painter().with_clip_rect(rect);
    let mut y = inner.top();

    let mark = Rect::from_min_size(
        pos2(inner.left(), y),
        vec2(epg::BADGE_WIDTH, epg::BADGE_HEIGHT),
    );
    badge(&painter, mark, hit);
    painter.text(
        pos2(mark.right() + 8.0, mark.center().y),
        Align2::LEFT_CENTER,
        &hit.channel,
        font(epg::FOUND_SIZE),
        skin.minute,
    );
    y = mark.bottom() + 10.0;

    let titled = badges::titled(&programme.title, &programme.detail);
    y += badges::draw_wrapped(
        &painter,
        pos2(inner.left(), y),
        inner.width(),
        &titled,
        font(epg::FOUND_SIDE_TITLE),
        skin.title,
        epg::FOUND_SIDE_TITLE * 1.4,
    );
    y += 6.0;
    if let Some(start) = programme.start {
        let when = painter.layout(
            long_when(start, programme.seconds, programme.ends()),
            font(epg::FOUND_SIZE),
            skin.detail,
            inner.width(),
        );
        let height = when.size().y;
        painter.galley(pos2(inner.left(), y), when, skin.detail);
        y += height + 10.0;
    }

    let buttons_top = inner.bottom() - 30.0;
    let text_area = Rect::from_min_max(
        pos2(inner.left(), y),
        pos2(inner.right(), buttons_top - 10.0),
    );
    if text_area.height() > 20.0 {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_area));
        scroll_bars(&mut child, skin);
        egui::ScrollArea::vertical()
            .id_salt(("番組検索の説明", hit.key, programme.start))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let width = ui.available_width();
                let paragraph = |ui: &mut Ui, text: &str, colour: Color32| {
                    let job = marked(
                        text,
                        query,
                        font(epg::FOUND_SIZE),
                        colour,
                        skin.found_mark,
                        width,
                        0,
                        Some(epg::FOUND_SIZE * 1.75),
                    );
                    ui.label(job);
                };
                let detail = badges::cleaned(&programme.detail);
                if !detail.trim().is_empty() {
                    paragraph(ui, &detail, skin.title);
                }
                for (head, body) in &programme.extended {
                    ui.add_space(4.0);
                    paragraph(ui, head, skin.day_text);
                    paragraph(ui, body, skin.title);
                }
            });
    }

    let mut pressed = None;
    let mut left = inner.left();
    for (label, primary) in [("番組表でこの時間を見る", true), ("番組説明を開く", false)]
    {
        // 文字の色は主ボタンか副ボタンかで決める（ここで決めるとレンダリングするときに変えられない）。
        let galley = painter.layout_no_wrap(
            label.to_owned(),
            font(epg::FOUND_SIZE),
            Color32::PLACEHOLDER,
        );
        let button =
            Rect::from_min_size(pos2(left, buttons_top), vec2(galley.size().x + 28.0, 30.0));
        let response = ui.interact(
            button,
            ui.scope_id().with(("番組検索の釦", label)),
            Sense::click(),
        );
        let (fill, text) = match (primary, response.hovered()) {
            (true, false) => (skin.accent, skin.accent_text),
            (true, true) => (skin.accent_hover, skin.accent_text),
            (false, false) => (skin.chosen_row, skin.title),
            (false, true) => (skin.button_hover, skin.title),
        };
        painter.rect_filled(button, 6.0, fill);
        if !primary {
            painter.rect_stroke(
                button,
                6.0,
                Stroke::new(1.0, skin.control_edge),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley(
            pos2(
                button.left() + 14.0,
                button.center().y - galley.size().y / 2.0,
            ),
            galley,
            text,
        );
        if response.clicked() {
            pressed = if primary {
                programme.start.map(|start| Pressed::Jump(hit.key, start))
            } else {
                Some(Pressed::Open(Box::new(Chosen {
                    key: hit.key,
                    programme: programme.clone(),
                })))
            };
        }
        left = button.right() + 8.0;
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(network: u16, start: Moment, title: &str) -> Hit {
        let mut programme = crate::ui::a_programme(Some(start), Some(1800));
        programme.network = network;
        programme.title = title.to_owned();
        Hit {
            key: programme.key(),
            channel: String::from("架空テレビ"),
            badge: String::from("架"),
            logo: None,
            programme,
        }
    }

    #[test]
    fn results_are_split_by_day() {
        let day = 86_400;
        let hits = vec![
            hit(0x7fe1, 10 * 3600, "朝"),
            hit(0x7fe1, 20 * 3600, "夜"),
            hit(0x7fe1, day + 3600, "翌日"),
        ];
        let laid = rows(&hits, Some(12 * 3600));
        assert_eq!(
            laid,
            vec![
                Row::Day(date_label(10 * 3600), false),
                Row::Hit(0),
                Row::Hit(1),
                Row::Day(date_label(day + 3600), false),
                Row::Hit(2),
            ]
        );
        let laid = rows(&hits, Some(3 * day));
        assert_eq!(laid[0], Row::Day(date_label(10 * 3600), true));
        assert_eq!(laid[3], Row::Day(date_label(day + 3600), true));
    }

    /// 件数：合計と、局のある種別ごとの内訳（一致が無ければ 0）。
    #[test]
    fn the_count_line_breaks_down_by_band() {
        let ground = Band::of(0x7fe1);
        let bs = Band::of(0x0004);
        let cs = Band::of(0x0006);
        let search = Search {
            asked: Asked {
                query: String::from("灯台"),
                ..Asked::default()
            },
            hits: vec![hit(0x7fe1, 0, "灯台")],
            more: false,
            counts: vec![(ground, 3), (bs, 1)],
        };
        let line = count_line(&search, &[ground, bs, cs]);
        assert_eq!(
            line,
            format!(
                "4 件（{} 3・{} 1・{} 0）",
                ground.name(),
                bs.name(),
                cs.name()
            )
        );
        let search = Search {
            asked: Asked {
                band: Some(bs),
                ..search.asked.clone()
            },
            more: true,
            ..search
        };
        assert!(count_line(&search, &[ground, bs, cs]).starts_with("1 件（"));
        assert!(count_line(&search, &[ground, bs, cs]).ends_with("先頭の 1 件を表示"));
    }

    /// 大分類は予備と拡張を除いた十三、中分類は名前のある物と「その他」。
    #[test]
    fn genres_offer_their_middle_classes() {
        assert_eq!(LARGES.len(), 13);
        let drama: Vec<&str> = middles(0x3).into_iter().map(|(_, name)| name).collect();
        assert_eq!(drama, vec!["国内ドラマ", "海外ドラマ", "時代劇", "その他"]);
        assert!(middles(0xf).is_empty());
    }

    #[test]
    fn a_new_query_from_the_guide_keeps_the_other_conditions() {
        let mut state = State {
            asked: Asked {
                past: true,
                genre: Some((0x3, None)),
                ..Asked::default()
            },
            chosen: Some((Key::default(), Some(0))),
            ..State::default()
        };
        state.ask("灯台");
        assert_eq!(state.asked.query, "灯台");
        assert_eq!(state.typed, "灯台");
        assert!(state.asked.past);
        assert_eq!(state.asked.genre, Some((0x3, None)));
        assert!(state.chosen.is_none());
        assert!(state.fresh);
    }

    #[test]
    fn the_side_pane_says_the_whole_date() {
        assert_eq!(
            long_when(21 * 3600, Some(3600), Some(22 * 3600)),
            "1月1日（木）21:00〜22:00（60 分）"
        );
    }
}
