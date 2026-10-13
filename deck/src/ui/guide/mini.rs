// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! パネルの番組表。映像の下に並ぶパネルで、一時間が一行。背景は丸ごと塗る（穴があると映像が透ける）。
//! 見出しとつまみは下のパネル（[`super::panel`]）が表示する。ここは種別の列から下。

use egui::{FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::guide::{Band, Key, Moment, Programme, civil};

use crate::ui::grid::{self, Grid};

use crate::ui::guide::chips;
use crate::ui::guide::epg;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::mini;

#[derive(Debug, Clone, Copy)]
struct Cells {
    skin: crate::ui::theme::epg::Skin,
    mini: mini::Skin,
}

impl Cells {
    const fn of(theme: &crate::ui::theme::Theme) -> Self {
        Self {
            skin: theme.epg,
            mini: theme.mini,
        }
    }
}

/// 局と番組は番組表のウィンドウと同じものを使う（全局分は四万番組を超える）。
#[derive(Debug, Clone)]
pub struct Strip<'a> {
    pub channels: &'a [epg::Channel],
    pub from: Moment,
    pub rows: u32,
    /// 見ている局のいまの番組の放送時間と名前。
    pub now_when: String,
    pub now_title: String,
    pub band: Band,
    pub bands: &'a [Band],
    pub watching: Option<Key>,
    pub moment: Option<Moment>,
    pub theme: crate::ui::theme::Theme,
}

#[derive(Debug, Clone)]
pub enum Pressed {
    /// この局に選局する。番組ではなく局を選ぶのが、この画面の役割。
    Channel(Key),
    Detail(Key, Box<Programme>),
    Band(Band),
    Shift(i32),
}

pub fn body_height(rows: u32) -> f32 {
    mini::TAB_ROW + mini::HEAD_HEIGHT + mini::ROW_HEIGHT * rows as f32 + mini::FOOT
}

/// 少なくとも一行、多くて [`mini::MOST_ROWS`]。
pub fn rows_in(height: f32) -> u32 {
    let room = height - body_height(0);
    ((room / mini::ROW_HEIGHT + 0.001).floor().max(1.0) as u32).min(mini::MOST_ROWS)
}

pub fn show(ui: &mut Ui, strip: &Strip<'_>) -> Option<Pressed> {
    let cells = Cells::of(&strip.theme);
    let skin = &cells.mini;
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let tabs = Rect::from_min_size(full.min, vec2(full.width(), mini::TAB_ROW));
    let mut pressed = None;
    if let Some(band) = band_tabs(ui, tabs, strip, skin) {
        pressed = Some(Pressed::Band(band));
    }
    let bottom = full.bottom().min(full.top() + body_height(strip.rows));
    let below = Rect::from_min_max(pos2(full.left(), tabs.bottom()), pos2(full.right(), bottom));
    if let Some(picked) = channels(ui, below, strip, &cells) {
        pressed = Some(picked);
    }
    pressed
}

fn band_tabs(ui: &mut Ui, rect: Rect, strip: &Strip<'_>, skin: &mini::Skin) -> Option<Band> {
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, skin.tab_row_edge),
    );
    let names: Vec<String> = strip.bands.iter().map(|band| band.name()).collect();
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let selected = strip
        .bands
        .iter()
        .position(|band| *band == strip.band)
        .unwrap_or(0);
    let (pressed, _) = chips::show(
        ui,
        pos2(rect.left() + 12.0, rect.center().y),
        &labels,
        selected,
        strip.theme.chips,
    );
    pressed.and_then(|index| strip.bands.get(index).copied())
}

fn channels(ui: &mut Ui, rect: Rect, strip: &Strip<'_>, cells: &Cells) -> Option<Pressed> {
    let skin = &cells.mini;
    let mut pressed = None;
    // 左端の時刻の欄は見出しから下端まで塗る（空けると映像が透ける）。
    let clock = Rect::from_min_size(rect.min, vec2(mini::CLOCK_WIDTH, rect.height()));
    ui.painter().rect_filled(clock, 0.0, skin.bar);
    ui.painter().vline(
        clock.right(),
        (clock.top() + mini::HEAD_HEIGHT)..=(clock.bottom() - mini::FOOT),
        Stroke::new(1.35, skin.clock_edge),
    );
    let (_, _, _, from_hour, _, _) = civil(strip.from);
    let current = strip.moment.and_then(|moment| now_row(moment, strip));
    for row in 0..strip.rows {
        let top = rect.top() + mini::HEAD_HEIGHT + mini::ROW_HEIGHT * row as f32;
        let now = current == Some(row);
        if now {
            ui.painter().rect_filled(
                Rect::from_min_max(
                    pos2(clock.right() - 2.0, top),
                    pos2(clock.right() + 1.0, top + mini::ROW_HEIGHT),
                ),
                0.0,
                skin.now_edge,
            );
        }
        let hour = ((from_hour + row) % 24).to_string();
        let at = pos2(clock.center().x, top + 5.0);
        let font = FontId::monospace(mini::HOUR_SIZE);
        let colour = if now { skin.now_hour } else { skin.hour };
        ui.painter()
            .text(at, egui::Align2::CENTER_TOP, &hour, font.clone(), colour);
        if now {
            // 太字のフォントは持たないので、少しずらして重ねる。
            ui.painter().text(
                at + vec2(0.6, 0.0),
                egui::Align2::CENTER_TOP,
                &hour,
                font,
                colour,
            );
        }
        ui.painter().hline(
            rect.x_range(),
            top + mini::ROW_HEIGHT,
            Stroke::new(1.0, skin.row_edge),
        );
    }
    let up = Rect::from_min_size(clock.min, vec2(clock.width(), mini::HEAD_HEIGHT));
    let down = Rect::from_min_max(pos2(clock.left(), clock.bottom() - mini::FOOT), clock.max);
    for (rect, mark, way, tag) in [(up, "▲", -1, "前の時間へ"), (down, "▼", 1, "次の時間へ")]
    {
        let steps = step(ui, rect, mark, tag, skin);
        if steps > 0 {
            pressed = Some(Pressed::Shift(way * steps as i32));
        }
    }
    foot(
        ui,
        Rect::from_min_max(pos2(clock.right(), clock.bottom() - mini::FOOT), rect.max),
        strip,
        skin,
    );

    let columns = Rect::from_min_max(
        pos2(clock.right(), rect.top()),
        pos2(rect.right(), rect.bottom() - mini::FOOT),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(columns));
    // 縦にはスクロールしないパネルなので、ホイールをそのまま横のスクロールに使う。
    child.style_mut().always_scroll_the_only_direction = true;
    let mut area = egui::ScrollArea::horizontal()
        .id_salt("帯の局")
        .auto_shrink([false, false]);
    // 開いたときと種別を替えたときは、見ている局が左の端に来るようにスクロールする。
    if let Some(offset) = follow_watching(ui, strip) {
        let width = mini::COLUMN * strip.channels.len() as f32;
        area = area.horizontal_scroll_offset(within(offset, width, columns.width()));
    }
    let scrolled = area.show(&mut child, |ui| {
        let width = mini::COLUMN * strip.channels.len() as f32;
        let (area, _) = ui.allocate_exact_size(vec2(width, columns.height()), Sense::hover());
        let visible = ui.clip_rect();
        let mut pressed = None;
        // サブチャンネルのセルは左の列から伸びるので、左に少しはみ出した列も。
        let shown: Vec<(usize, Rect)> = strip
            .channels
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let left = area.left() + mini::COLUMN * index as f32;
                (
                    index,
                    Rect::from_min_size(pos2(left, area.top()), vec2(mini::COLUMN, area.height())),
                )
            })
            .filter(|(_, column)| {
                column.left() <= visible.right()
                    && column.left() + mini::COLUMN * (1.0 + epg::WIDEST as f32) >= visible.left()
            })
            .collect();
        let body_of = |column: &Rect| {
            Rect::from_min_max(
                pos2(column.left(), column.top() + mini::HEAD_HEIGHT),
                column.max,
            )
        };
        // 先に全部の列の背景を塗ってからセルをレンダリングする。交互だと伸ばしたセルの右半分を次の列の背景が塗り潰す。
        for (_, column) in &shown {
            let body = body_of(column);
            crate::ui::graded(
                ui.painter(),
                body,
                cells.skin.no_genre,
                cells.skin.genre_end,
            );
            ui.painter().vline(
                body.left(),
                body.y_range(),
                Stroke::new(1.0, cells.mini.edge),
            );
        }
        for (index, column) in &shown {
            let (index, column) = (*index, *column);
            let channel = &strip.channels[index];
            if head(
                ui,
                column,
                channel,
                strip.watching == Some(channel.key),
                skin,
            ) {
                pressed = Some(Pressed::Channel(channel.key));
            }
            let body = body_of(&column);
            for programme in &channel.programmes {
                if epg::covered(strip.channels, index, programme) {
                    continue;
                }
                let Some(mut cell) = place(programme, body, strip) else {
                    continue;
                };
                cell.max.x += mini::COLUMN * epg::span_of(strip.channels, index, programme) as f32;
                // 選局は局の名前の所だけ。番組は二度押しでその番組の説明。
                if one(ui, cell, programme, strip.moment, cells) == Some(grid::Click::Double) {
                    pressed = Some(Pressed::Detail(channel.key, Box::new(programme.clone())));
                }
            }
        }
        if let Some((_, column)) = shown
            .iter()
            .find(|(index, _)| strip.watching == Some(strip.channels[*index].key))
        {
            ui.painter().rect_stroke(
                *column,
                0.0,
                Stroke::new(1.0, skin.watch_edge),
                egui::StrokeKind::Inside,
            );
        }
        // `area` の左端はスクロールするとウィンドウの外へ出るので、見えている範囲に切り詰める。
        if let Some(y) = strip
            .moment
            .and_then(|moment| now_line(moment, strip, area.top()))
        {
            let line = Rect::from_min_max(
                pos2(area.left().max(visible.left()), y),
                pos2(
                    area.right().min(visible.right()),
                    y + crate::ui::theme::epg::NOW_WIDTH,
                ),
            );
            if line.width() > 0.0 {
                ui.painter().rect_filled(line, 0.0, cells.skin.now);
            }
        }
        pressed
    });
    if let Some(picked) = scrolled.inner {
        pressed = Some(picked);
    }
    pressed
}

/// 押したときに一つ、[`REPEAT_AFTER`] 秒からは [`REPEAT_EVERY`] 秒ごとに一つ。
const REPEAT_AFTER: f64 = 0.45;
const REPEAT_EVERY: f64 = 0.18;

pub(crate) fn steps_by(held: f64) -> u32 {
    if held < REPEAT_AFTER {
        1
    } else {
        2 + ((held - REPEAT_AFTER) / REPEAT_EVERY) as u32
    }
}

/// このフレームでスクロールする数を返す。
fn step(ui: &mut Ui, rect: Rect, mark: &str, tag: &str, skin: &mini::Skin) -> u32 {
    let id = ui.id().with(("帯の送り", tag));
    let response = ui.interact(rect, id, Sense::click()).on_hover_text(tag);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, skin.step);
    painter.hline(
        rect.x_range(),
        if mark == "▲" {
            rect.bottom()
        } else {
            rect.top()
        },
        Stroke::new(1.0, skin.step_edge),
    );
    icons::paint_at(
        painter,
        rect.center(),
        mini::STEP_SIZE,
        if mark == "▲" {
            Icon::TriUp
        } else {
            Icon::TriDown
        },
        if response.hovered() {
            skin.foot_clock
        } else {
            skin.step_text
        },
    );
    if !response.is_pointer_button_down_on() {
        ui.data_mut(|data| data.remove::<(f64, u32)>(id));
        return 0;
    }
    let now = ui.input(|input| input.time);
    let (since, fired) = ui.data(|data| data.get_temp::<(f64, u32)>(id).unwrap_or((now, 0)));
    let due = steps_by(now - since);
    ui.data_mut(|data| data.insert_temp(id, (since, due.max(fired))));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(REPEAT_EVERY / 2.0));
    due.saturating_sub(fired)
}

fn foot(ui: &mut Ui, rect: Rect, strip: &Strip<'_>, skin: &mini::Skin) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, skin.foot);
    painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, skin.step_edge));
    let font = crate::ui::text::font::guide(mini::FOOT_SIZE);
    let mut x = rect.left() + 10.0;
    if !strip.now_when.is_empty() {
        let galley = painter.layout_no_wrap(strip.now_when.clone(), font.clone(), skin.foot_dim);
        let width = galley.size().x;
        painter.galley(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            skin.foot_dim,
        );
        x += width + 10.0;
    }
    let title = crate::ui::one_line(
        &painter,
        &strip.now_title,
        font,
        skin.foot_text,
        (rect.right() - 10.0 - x).max(0.0),
        0.0,
    );
    painter.galley(
        pos2(x, rect.center().y - title.size().y / 2.0),
        title,
        skin.foot_text,
    );
}

fn now_row(moment: Moment, strip: &Strip<'_>) -> Option<u32> {
    let since = moment - strip.from;
    (0..i64::from(strip.rows) * 3600)
        .contains(&since)
        .then_some((since / 3600) as u32)
}

fn watching_index(strip: &Strip<'_>) -> Option<usize> {
    let watching = strip.watching?;
    strip
        .channels
        .iter()
        .position(|channel| channel.key == watching)
}

/// パネルを開いたときと、見ている局か種別が替わったときだけ、見ている局の列を左の端へ寄せるスクロール位置。
fn follow_watching(ui: &Ui, strip: &Strip<'_>) -> Option<f32> {
    follow(
        ui,
        ui.id().with("帯の局を見ている局へ"),
        strip.watching,
        strip.band,
        watching_offset(strip),
    )
}

/// 見ている局がまだ並びに無ければ（番組表の読み込み中）、記録せずに次のフレームでまた試す。
pub(super) fn follow(
    ui: &Ui,
    id: egui::Id,
    watching: Option<Key>,
    band: Band,
    offset: Option<f32>,
) -> Option<f32> {
    let pass = ui.ctx().cumulative_pass_nr();
    let seen: Option<(Option<Key>, Band, u64)> = ui.data(|data| data.get_temp(id));
    let fresh = match seen {
        Some((before, before_band, at)) => {
            before != watching || before_band != band || at + 1 < pass
        }
        None => true,
    };
    if fresh && offset.is_none() && watching.is_some() {
        return None;
    }
    ui.data_mut(|data| data.insert_temp(id, (watching, band, pass)));
    if fresh { offset } else { None }
}

/// 端まで寄せられないときは、スクロールできる所までにする（行き過ぎると次のフレームで戻ってガタつく）。
pub(super) fn within(offset: f32, content: f32, view: f32) -> f32 {
    offset.min(content - view).max(0.0)
}

fn watching_offset(strip: &Strip<'_>) -> Option<f32> {
    watching_index(strip).map(|index| mini::COLUMN * index as f32)
}

fn now_line(moment: Moment, strip: &Strip<'_>, top: f32) -> Option<f32> {
    let since = moment - strip.from;
    if since < 0 || since >= i64::from(strip.rows) * 3600 {
        return None;
    }
    Some(top + mini::HEAD_HEIGHT + mini::ROW_HEIGHT * since as f32 / 3600.0)
}

impl Strip<'_> {
    fn grid(&self) -> Grid {
        Grid {
            from: self.from,
            hours: self.rows,
            hour: mini::ROW_HEIGHT,
        }
    }
}

fn place(programme: &Programme, body: Rect, strip: &Strip<'_>) -> Option<Rect> {
    strip.grid().place(body, programme.start?, programme.ends())
}

fn head(
    ui: &mut Ui,
    column: Rect,
    channel: &epg::Channel,
    watching: bool,
    skin: &mini::Skin,
) -> bool {
    let rect = Rect::from_min_size(column.min, vec2(column.width(), mini::HEAD_HEIGHT));
    let response = ui.interact(rect, ui.id().with(("帯の局", channel.key)), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        0.0,
        if watching {
            skin.head_watching
        } else {
            skin.bar
        },
    );
    painter.vline(rect.left(), rect.y_range(), Stroke::new(1.0, skin.bar_edge));
    if response.hovered() && !watching {
        painter.rect_filled(rect, 0.0, skin.bar_edge);
    }
    let text_colour = if watching {
        skin.head_watching_text
    } else {
        skin.channel
    };
    crate::ui::nameplate(
        painter,
        rect,
        channel,
        &crate::ui::Plate {
            key_size: mini::KEY_SIZE,
            key_colour: skin.key,
            badge: vec2(mini::BADGE_WIDTH, mini::BADGE_HEIGHT),
            badge_size: mini::BADGE_SIZE,
            name_size: mini::CHANNEL_SIZE,
            gap: 4.0,
            margin: 10.0,
        },
        text_colour,
    );
    response.clicked()
}

fn one(
    ui: &mut Ui,
    rect: Rect,
    programme: &Programme,
    moment: Option<Moment>,
    cells: &Cells,
) -> Option<grid::Click> {
    grid::cell(
        ui,
        rect,
        programme,
        moment,
        &grid::Style {
            skin: &cells.skin,
            edge: cells.mini.edge,
            minute: cells.mini.minute,
            title: cells.mini.title,
            detail: cells.mini.title,
            edges: grid::Edges {
                top: false,
                right: false,
                left: true,
            },
            pad: mini::PAD,
            minute_size: mini::MINUTE_SIZE,
            title_size: mini::TITLE_SIZE,
            title_leading: mini::TITLE_LEADING,
            detail_text: None,
            top_pad: 4.0,
            tag: "帯の番組",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_strip(from: Moment, rows: u32) -> Strip<'static> {
        Strip {
            channels: &[],
            from,
            rows,
            now_when: String::new(),
            now_title: String::new(),
            band: Band::default(),
            bands: &[],
            watching: None,
            moment: None,
            theme: crate::ui::theme::Theme::of(crate::ui::theme::Mode::Dark),
        }
    }

    #[test]
    fn the_now_line_sits_inside_its_hour_row() {
        let strip = a_strip(3600, 2);
        assert_eq!(now_line(3600, &strip, 0.0), Some(mini::HEAD_HEIGHT));
        let half = now_line(3600 + 3600 + 1800, &strip, 0.0).unwrap();
        assert!((half - (mini::HEAD_HEIGHT + mini::ROW_HEIGHT * 1.5)).abs() < 0.01);
        assert_eq!(now_line(3599, &strip, 0.0), None);
        assert_eq!(now_line(3600 + 7200, &strip, 0.0), None);
    }

    fn five_channels() -> Vec<epg::Channel> {
        (1..=5)
            .map(|service| epg::Channel {
                key: Key {
                    network: 1,
                    service,
                },
                ..epg::Channel::default()
            })
            .collect()
    }

    #[test]
    fn the_current_hour_row() {
        let strip = a_strip(3600, 3);
        assert_eq!(now_row(3600, &strip), Some(0));
        assert_eq!(now_row(3600 + 3599, &strip), Some(0));
        assert_eq!(now_row(3600 * 3 + 10, &strip), Some(2));
        assert_eq!(now_row(3599, &strip), None);
        assert_eq!(now_row(3600 * 4, &strip), None);
    }

    #[test]
    fn the_watching_column_is_scrolled_to_the_left() {
        let channels = five_channels();
        let mut strip = a_strip(0, 2);
        strip.channels = &channels;
        assert_eq!(watching_offset(&strip), None);
        strip.watching = Some(channels[3].key);
        assert_eq!(watching_index(&strip), Some(3));
        assert!((watching_offset(&strip).unwrap() - mini::COLUMN * 3.0).abs() < 1e-3);
        strip.watching = Some(Key {
            network: 9,
            service: 9,
        });
        assert_eq!(watching_offset(&strip), None);
    }

    #[test]
    fn the_strip_follows_only_when_opened_or_changed() {
        let channels = five_channels();
        let mut strip = a_strip(0, 2);
        strip.channels = &channels;
        strip.watching = Some(channels[2].key);
        let ctx = egui::Context::default();
        let mut said = Vec::new();
        let run = |strip: &Strip<'_>, said: &mut Vec<Option<f32>>| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                said.push(follow_watching(ui, strip));
            });
        };
        run(&strip, &mut said);
        run(&strip, &mut said);
        strip.watching = Some(channels[4].key);
        run(&strip, &mut said);
        run(&strip, &mut said);
        assert!(said[0].is_some(), "開いたとき");
        assert_eq!(said[1], None, "そのまま");
        assert!(said[2].is_some(), "局を替えたとき");
        assert_eq!(said[3], None);
    }

    #[test]
    fn following_waits_for_the_channels() {
        let ctx = egui::Context::default();
        let key = Key {
            network: 1,
            service: 3,
        };
        let mut said = Vec::new();
        for offset in [None, None, Some(240.0), Some(240.0)] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                said.push(follow(ui, ui.id().with("試し"), Some(key), Band(0), offset));
            });
        }
        assert_eq!(said, [None, None, Some(240.0), None]);
    }

    #[test]
    fn holding_a_step_repeats() {
        assert_eq!(steps_by(0.0), 1);
        assert_eq!(steps_by(REPEAT_AFTER - 0.01), 1);
        assert_eq!(steps_by(REPEAT_AFTER), 2);
        assert_eq!(steps_by(REPEAT_AFTER + REPEAT_EVERY * 3.0 + 0.01), 5);
    }

    #[test]
    fn rows_follow_the_height() {
        assert_eq!(rows_in(body_height(2)), 2);
        assert_eq!(rows_in(body_height(4)), 4);
        assert_eq!(rows_in(body_height(4) - 1.0), 3);
        assert_eq!(rows_in(body_height(3) + mini::ROW_HEIGHT * 0.9), 3);
        assert_eq!(rows_in(0.0), 1);
        assert_eq!(rows_in(100_000.0), mini::MOST_ROWS);
    }

    #[test]
    fn programmes_are_placed_like_the_guide() {
        let strip = a_strip(7200, 2);
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, mini::ROW_HEIGHT * 2.0));
        let early = crate::ui::a_programme(Some(0), Some(7200 + 1800));
        let placed = place(&early, body, &strip).unwrap();
        assert_eq!(placed.top(), 0.0);
        assert!((placed.bottom() - mini::ROW_HEIGHT * 0.5).abs() < 0.01);
        let long = crate::ui::a_programme(Some(7200 + 1800), Some(4 * 3600));
        let placed = place(&long, body, &strip).unwrap();
        assert!((placed.top() - mini::ROW_HEIGHT * 0.5).abs() < 0.01);
        assert!((placed.bottom() - mini::ROW_HEIGHT * 2.0).abs() < 0.01);
        let later = crate::ui::a_programme(Some(7200 + 3 * 3600), Some(1800));
        assert!(place(&later, body, &strip).is_none());
        let open = crate::ui::a_programme(Some(7200 + 3600), None);
        let placed = place(&open, body, &strip).unwrap();
        assert!((placed.bottom() - mini::ROW_HEIGHT * 2.0).abs() < 0.01);
    }
}
