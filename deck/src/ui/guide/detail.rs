// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組説明の別ウィンドウ。どこから開いても同じ一枚で、開いていれば中身を入れ替える。
//! 字幕は「あり」のときだけ出す（Mirakurun からはフラグが偽のまま来る）。

use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::guide::{self, Described, Moment, Programme, Told};

use crate::ui::theme::detail;

#[derive(Debug, Clone)]
pub struct Detail {
    pub programme: Programme,
    pub channel: String,
    pub remote_key: Option<u8>,
    pub badge: String,
    pub badge_colour: Color32,
    pub logo: Option<std::sync::Arc<zwcore::guide::Logo>>,
    /// 同じネットワークで同時に流している他のサービスの名前。
    pub shared: Vec<String>,
    /// メニューから開いたウィンドウだけ `Some`。視聴中の局に付いていく。
    pub follow: Option<Follow>,
    pub now: Option<Moment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    Now,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Close,
    Follow(Follow),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeBox {
    pub rect: Rect,
    pub sections: Vec<(String, String)>,
    /// 本文の囲い（`MARK_OPEN`…`MARK_CLOSE`）ごとの、バッジの文字と色。
    pub marks: Vec<(String, Color32)>,
    pub size: f32,
    pub scroll: bool,
    pub colours: Option<[Color32; 4]>,
    pub mono: bool,
    pub tail: bool,
}

#[derive(Debug, Default)]
pub struct Shown {
    pub pressed: Option<Pressed>,
    /// OS のテキストコントロールに出してほしい矩形（Windows と macOS）。egui は枠だけをレンダリングする。
    pub native: Vec<NativeBox>,
}

/// OS のテキストコントロールの中のバッジの囲い（私用領域の文字）。コントロールが囲いの中をインラインの画像に置き換える。
pub const MARK_OPEN: char = '\u{E000}';
pub const MARK_CLOSE: char = '\u{E001}';

pub fn show(ui: &mut Ui, detail: &Detail, skin: &detail::Skin) -> Shown {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let inner = Rect::from_min_max(
        pos2(full.left() + detail::PAD, full.top() + detail::PAD_TOP),
        pos2(full.right() - detail::PAD, full.bottom() - detail::PAD_TOP),
    );
    let mut pressed = None;
    let mut top = inner.top();
    if let Some(follow) = detail.follow {
        if let Some(chosen) = tabs(ui, inner, top, follow, skin) {
            pressed = Some(Pressed::Follow(chosen));
        }
        top += detail::TAB_HEIGHT + detail::GAP;
    }
    let foot = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - detail::FOOT_HEIGHT),
        inner.max,
    );
    let area = Rect::from_min_max(
        pos2(inner.left(), top),
        pos2(inner.right(), foot.top() - detail::GAP),
    );
    let native = body(
        ui,
        area,
        detail,
        skin,
        cfg!(any(windows, target_os = "macos")),
    );
    if footer(ui, foot, detail.programme.told, skin) {
        pressed = Some(Pressed::Close);
    }
    Shown { pressed, native }
}

pub fn of(
    guide: &zwcore::guide::Guide,
    key: zwcore::guide::Key,
    programme: Programme,
    follow: Option<Follow>,
) -> Detail {
    let service = guide.service(key);
    let name = service
        .map(|service| service.name.clone())
        .unwrap_or_default();
    let mut shared: Vec<String> = Vec::new();
    for (sid, _) in &programme.shared {
        if *sid == key.service {
            continue;
        }
        let other = zwcore::guide::Key {
            network: key.network,
            service: *sid,
        };
        if let Some(service) = guide.service(other)
            && !shared.contains(&service.name)
        {
            shared.push(service.name.clone());
        }
    }
    Detail {
        programme,
        channel: name.clone(),
        remote_key: service.and_then(|service| service.remote_key),
        badge: service.map_or_else(
            || crate::ui::guide::epg::badge_of(&name),
            zwcore::guide::Service::badge,
        ),
        badge_colour: crate::ui::guide::epg::badge_colour(&name),
        logo: guide.logo_of(key).cloned(),
        shared,
        follow,
        now: Some(guide.now_or_machine()),
    }
}

fn tabs(ui: &mut Ui, inner: Rect, top: f32, follow: Follow, skin: &detail::Skin) -> Option<Follow> {
    let font = crate::ui::text::font::guide(detail::TAB_SIZE);
    let mut x = inner.left();
    let mut chosen = None;
    for (label, which) in [("いま放送中", Follow::Now), ("次の番組", Follow::Next)] {
        let width = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font.clone(), skin.text)
            .size()
            .x;
        let rect = Rect::from_min_size(
            pos2(x, top),
            vec2(width + detail::TAB_PAD_X * 2.0, detail::TAB_HEIGHT),
        );
        let response = ui.interact(
            rect,
            ui.scope_id().with(("番組説明のタブ", label)),
            Sense::click(),
        );
        let on = which == follow;
        if on || response.hovered() {
            ui.painter()
                .rect_filled(rect, detail::TAB_ROUNDING, skin.tab);
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            if on { skin.text } else { skin.dim },
        );
        if response.clicked() && !on {
            chosen = Some(which);
        }
        x = rect.right() + 4.0;
    }
    chosen
}

fn body(
    ui: &mut Ui,
    inner: Rect,
    detail: &Detail,
    skin: &detail::Skin,
    native_text: bool,
) -> Vec<NativeBox> {
    let painter = ui.painter().with_clip_rect(inner);
    let mut at = inner.top();
    let mut native = Vec::new();

    let titled = crate::ui::text::badges::titled(&detail.programme.title, &detail.programme.detail);
    let title_font = crate::ui::text::font::guide(detail::TITLE_SIZE);
    let title_room = inner.width();
    if native_text {
        // 題名も OS のコントロールで（選択・コピーできる）。高さは egui で折り返して測る。
        let mut body = String::new();
        let mut marks = Vec::new();
        let mut mark = |body: &mut String, attr: &crate::ui::text::badges::Attr| {
            body.push_str(&format!("{MARK_OPEN}{}{MARK_CLOSE}", attr.label));
            marks.push((attr.label.to_owned(), attr.colour));
        };
        for attr in &titled.pre {
            mark(&mut body, attr);
            body.push(' ');
        }
        body.push_str(&titled.name);
        for attr in &titled.post {
            body.push(' ');
            mark(&mut body, attr);
        }
        let measured: String = body
            .chars()
            .filter(|c| *c != MARK_OPEN && *c != MARK_CLOSE)
            .collect();
        let rows = painter
            .layout(measured, title_font.clone(), skin.text, title_room)
            .rows
            .len()
            .max(1);
        let height = rows as f32 * detail::TITLE_LEADING + 6.0;
        native.push(NativeBox {
            rect: Rect::from_min_size(pos2(inner.left(), at), vec2(title_room, height)),
            sections: vec![(String::new(), body)],
            marks,
            size: detail::TITLE_SIZE,
            scroll: false,
            colours: None,
            mono: false,
            tail: false,
        });
        at += height + 6.0;
    } else {
        let used = crate::ui::text::badges::draw_wrapped(
            &painter,
            pos2(inner.left(), at),
            title_room,
            &titled,
            title_font,
            skin.text,
            detail::TITLE_LEADING,
        );
        at += used + 6.0;
    }

    let mut said = when(&detail.programme);
    if let Some(left) = detail
        .now
        .and_then(|now| minutes_left(&detail.programme, now))
    {
        said.push_str(&format!("　残り {left} 分"));
    }
    painter.text(
        pos2(inner.left(), at),
        egui::Align2::LEFT_TOP,
        said,
        crate::ui::text::font::guide(detail::WHEN_SIZE),
        skin.dim,
    );
    at += 24.0;

    at = channel(ui, inner, at, detail, skin);
    at += detail::GAP;

    let box_rect = Rect::from_min_max(pos2(inner.left(), at), inner.max);
    if box_rect.height() > 40.0
        && let Some(fields) = fields(ui, box_rect, &detail.programme, skin, native_text)
    {
        native.push(fields);
    }
    native
}

fn footer(ui: &mut Ui, foot: Rect, told: Told, skin: &detail::Skin) -> bool {
    ui.painter().text(
        pos2(foot.left(), foot.center().y),
        egui::Align2::LEFT_CENTER,
        source(told),
        crate::ui::text::font::guide(detail::FOOT_SIZE),
        skin.dim,
    );
    let button = Rect::from_min_size(
        pos2(
            foot.right() - detail::CLOSE_WIDTH,
            foot.center().y - detail::CLOSE_HEIGHT / 2.0,
        ),
        vec2(detail::CLOSE_WIDTH, detail::CLOSE_HEIGHT),
    );
    let response = ui.interact(
        button,
        ui.scope_id().with("番組説明を閉じる"),
        Sense::click(),
    );
    let painter = ui.painter();
    painter.rect_filled(
        button,
        detail::CLOSE_ROUNDING,
        if response.hovered() {
            skin.tab
        } else {
            skin.box_fill
        },
    );
    painter.rect_stroke(
        button,
        detail::CLOSE_ROUNDING,
        Stroke::new(1.0, skin.button_edge),
        egui::StrokeKind::Inside,
    );
    painter.text(
        button.center(),
        egui::Align2::CENTER_CENTER,
        "閉じる",
        crate::ui::text::font::guide(detail::CHANNEL_SIZE),
        skin.text,
    );
    response.clicked()
}

fn source(told: Told) -> &'static str {
    match told {
        Told::Broadcast => "番組情報：放送波（番組表）",
        Told::Now => "番組情報：放送波（現在・次）",
        Told::Mirakurun => "番組情報：Mirakurun",
    }
}

fn when(programme: &Programme) -> String {
    let Some(start) = programme.start else {
        return String::from("—");
    };
    let (year, ..) = guide::civil(guide::shown(start));
    let mut out = format!(
        "{year}/{} {}",
        guide::date_label(start),
        guide::airtime(start, programme.ends())
    );
    if let Some(seconds) = programme.seconds {
        out.push_str(&format!("　（{}分）", seconds / 60));
    }
    out
}

/// それ以外と終了時刻の無い番組は `None`。
fn minutes_left(programme: &Programme, now: Moment) -> Option<i64> {
    let start = programme.start?;
    let ends = programme.ends()?;
    (start <= now && now < ends).then(|| (ends - now + 59) / 60)
}

fn channel(ui: &mut Ui, inner: Rect, top: f32, detail: &Detail, skin: &detail::Skin) -> f32 {
    let row = Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), 32.0));
    let painter = ui.painter().with_clip_rect(inner);
    let mut at = row.left();
    if let Some(key) = detail.remote_key {
        crate::ui::key_circle(
            &painter,
            pos2(at + detail::KEY_BADGE / 2.0, row.center().y),
            detail::KEY_BADGE / 2.0,
            skin.key_fill,
            Some(Stroke::new(1.0, skin.edge)),
            key,
            detail::KEY_SIZE,
            skin.key_text,
        );
        at += detail::KEY_BADGE + 8.0;
    }
    if !detail.badge.is_empty() {
        let (ground, text) = crate::ui::badge_look(&painter, detail.badge_colour);
        let logo = crate::ui::logo_pill(
            &painter,
            pos2(at, row.center().y),
            detail.logo.as_ref(),
            &detail.badge,
            ground,
            text,
            10.0,
            19.0,
            5.0,
            0.0,
        );
        at = logo.right() + 8.0;
    }
    let name = painter.text(
        pos2(at, row.center().y),
        egui::Align2::LEFT_CENTER,
        &detail.channel,
        crate::ui::text::font::guide(detail::CHANNEL_SIZE),
        skin.text,
    );
    if !detail.shared.is_empty() {
        painter.text(
            pos2(name.right() + 14.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            format!("同じ番組：{}", detail.shared.join("、")),
            crate::ui::text::font::guide(detail::FIELD_SIZE),
            skin.dim,
        );
    }
    painter.hline(inner.x_range(), row.bottom(), Stroke::new(1.0, skin.edge));
    row.bottom()
}

fn sections(programme: &Programme) -> Vec<(String, String)> {
    let mut out = Vec::new();

    if !programme.detail.is_empty() {
        out.push((
            String::new(),
            crate::ui::text::badges::cleaned(&programme.detail),
        ));
    }
    for (heading, value) in &programme.extended {
        out.push((heading.clone(), value.clone()));
    }

    let genres: Vec<String> = programme
        .genres
        .iter()
        .map(|genre| match genre.middle_name() {
            Some(middle) => format!("{} - {middle}", genre.name()),
            None => genre.name().to_owned(),
        })
        .collect();
    if !genres.is_empty() {
        out.push((String::from("ジャンル"), genres.join("\n")));
    }

    if let Some(series) = &programme.details.series {
        let mut lines = Vec::new();
        if !series.name.is_empty() {
            lines.push(series.name.clone());
        }
        let mut parts = Vec::new();
        if series.episode != 0 {
            if series.last_episode != 0 {
                parts.push(format!(
                    "第{}話（全{}話）",
                    series.episode, series.last_episode
                ));
            } else {
                parts.push(format!("第{}話", series.episode));
            }
        }
        let pattern = series.pattern_name();
        if pattern != "予備" {
            parts.push(pattern.to_owned());
        }
        if series.repeat_label != 0 {
            parts.push(String::from("再放送"));
        }
        if !parts.is_empty() {
            lines.push(parts.join("・"));
        }
        if !lines.is_empty() {
            out.push((String::from("シリーズ"), lines.join("\n")));
        }
    }

    let mut told = Vec::new();
    if let Some(picture) = programme.video.as_ref().and_then(guide::Picture::name) {
        told.push(format!("映像：{picture}"));
    }
    match programme.audios.len() {
        0 => {}
        1 => told.push(format!("音声：{}", programme.audios[0].describe())),
        _ => told.extend(
            programme
                .audios
                .iter()
                .enumerate()
                .map(|(nth, sound)| format!("音声{}：{}", nth + 1, sound.describe())),
        ),
    }
    if programme.caption {
        told.push(String::from("字幕：あり"));
    }
    if !told.is_empty() {
        out.push((String::from("映像・音声"), told.join("\n")));
    }

    let mut other = Vec::new();
    if let Some(age) = programme.details.minimum_age() {
        other.push(format!("視聴年齢：{age} 歳以上"));
    }
    if let Some(groups) = &programme.details.groups
        && groups.group_type == 0
        && groups.groups.len() > 1
    {
        other.push(format!("マルチビュー：{} 画面", groups.groups.len()));
    }
    if let Some(region) = &programme.details.region
        && region.kind == 0x01
        && !region.prefectures.is_empty()
    {
        let names: Vec<&str> = region
            .prefectures
            .iter()
            .filter_map(|bit| isdb::descriptors::prefecture_name(*bit))
            .collect();
        if !names.is_empty() {
            other.push(format!("放送地域：{}", names.join("・")));
        }
    }
    if !other.is_empty() {
        out.push((String::from("その他"), other.join("\n")));
    }

    // 記述子から読んだ物をそのまま（コピー制御、コンテント利用、契約、シリーズ、リンクなど）。
    let described = guide::details_lines(&programme.details);
    if !described.is_empty() {
        out.push((String::from("詳細情報"), described.join("\n")));
    }

    if out.is_empty() {
        out.push((String::new(), String::from("—")));
    }
    out
}

fn fields(
    ui: &mut Ui,
    rect: Rect,
    programme: &Programme,
    skin: &detail::Skin,
    native_text: bool,
) -> Option<NativeBox> {
    ui.painter()
        .rect_filled(rect, detail::BOX_ROUNDING, skin.box_fill);
    ui.painter().rect_stroke(
        rect,
        detail::BOX_ROUNDING,
        Stroke::new(1.0, skin.edge),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(detail::BOX_PAD);
    let entries = sections(programme);

    if native_text {
        return Some(NativeBox {
            rect: inner,
            sections: entries,
            marks: Vec::new(),
            size: detail::FIELD_SIZE,
            scroll: true,
            colours: None,
            mono: false,
            tail: false,
        });
    }
    let font = crate::ui::text::font::guide(detail::FIELD_SIZE);
    let look = crate::ui::guide::copyable::Look {
        heading_font: font.clone(),
        heading: skin.dim,
        text_font: font,
        text: skin.text,
        leading: Some(detail::FIELD_LEADING),
    };
    let id = ui.scope_id().with("番組説明の本文");
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    egui::ScrollArea::vertical()
        .id_salt("番組説明")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            crate::ui::guide::copyable::show(
                ui,
                id,
                &entries,
                &look,
                inner.width() - detail::SCROLL_ROOM,
            );
        });
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a_programme as programme;

    #[test]
    fn when_it_runs_reads_as_a_date_and_a_span() {
        let said = when(&programme(Some(1_788_870_600), Some(5400)));
        assert_eq!(said, "2026/9/8（火） 12:30〜14:00　（90分）");
    }

    #[test]
    fn a_programme_with_no_time_says_so() {
        assert_eq!(when(&programme(None, None)), "—");
    }

    #[test]
    fn minutes_left_only_while_it_airs() {
        let p = programme(Some(1_000), Some(3600));
        assert_eq!(minutes_left(&p, 1_000), Some(60));
        assert_eq!(minutes_left(&p, 1_000 + 3599), Some(1));
        assert_eq!(minutes_left(&p, 999), None);
        assert_eq!(minutes_left(&p, 1_000 + 3600), None);
    }

    #[test]
    fn a_programme_that_says_nothing_shows_a_dash() {
        let mut p = programme(Some(1_000), Some(3600));
        p.detail.clear();
        p.extended.clear();
        p.genres.clear();
        p.video = None;
        p.audios.clear();
        p.caption = false;
        p.details = zwcore::guide::Details::default();
        assert_eq!(sections(&p), vec![(String::new(), String::from("—"))]);
    }

    #[test]
    fn captions_are_said_only_when_present() {
        let mut p = programme(Some(1_000), Some(3600));
        p.video = None;
        p.audios.clear();
        p.caption = true;
        let said = sections(&p);
        assert!(said.contains(&(String::from("映像・音声"), String::from("字幕：あり"))));
        p.caption = false;
        assert!(
            !sections(&p)
                .iter()
                .any(|(heading, _)| heading == "映像・音声")
        );
    }
}
