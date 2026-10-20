// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 下のパネルの「番組情報」のタブ。ラジオの待ち画面と同じ並びを高さの低い領域で読めるように置く。
//! 低くて広い領域では左右の二段組、高い領域や狭い領域では上から一段に。

use egui::{Align2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use zwcore::guide::{self, Programme};

use crate::draw::scene::Idle;
use crate::ui::text::font;
use crate::ui::theme::detail as look;

pub fn two_columns(size: Vec2) -> bool {
    size.y < look::INFO_STACK_HEIGHT && size.x >= look::INFO_TWO_MIN_WIDTH
}

fn split(inner: Rect) -> (Rect, Rect) {
    let left_width = (inner.width() * look::INFO_LEFT_SHARE)
        .clamp(look::INFO_LEFT_MIN, look::INFO_LEFT_MAX)
        .min(inner.width());
    let left = Rect::from_min_size(inner.min, vec2(left_width, inner.height()));
    let right = Rect::from_min_max(
        pos2(
            (left.right() + look::INFO_GUTTER).min(inner.right()),
            inner.top(),
        ),
        inner.max,
    );
    (left, right)
}

pub fn show(ui: &mut Ui, info: &Idle, skin: &look::Skin) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let inner = full.shrink2(vec2(look::INFO_PAD_X, look::INFO_PAD_Y));
    if !inner.is_positive() {
        return;
    }
    let painter = ui.painter().with_clip_rect(inner.intersect(ui.clip_rect()));
    let next_height = if info.next.is_some() {
        look::INFO_NEXT_HEIGHT
    } else {
        0.0
    };
    if two_columns(full.size()) {
        let (left, right) = split(inner);
        summary(
            &painter,
            left,
            left.bottom() - next_height,
            info,
            skin,
            false,
        );
        next(&painter, left, info.next.as_ref(), skin);
        painter.vline(
            left.right() + look::INFO_GUTTER / 2.0,
            inner.y_range(),
            Stroke::new(1.0, skin.edge),
        );
        notes(ui, right, &info.now.programme, skin);
    } else {
        let limit = inner.bottom() - next_height;
        let used = summary(&painter, inner, limit, info, skin, true);
        let rest = Rect::from_min_max(
            pos2(inner.left(), used + look::INFO_GAP),
            pos2(inner.right(), limit - look::INFO_GAP),
        );
        if rest.height() >= look::INFO_TEXT_LEADING {
            notes(ui, rest, &info.now.programme, skin);
        }
        next(&painter, inner, info.next.as_ref(), skin);
    }
}

/// 左の段（一段のときは上の部分）。`limit` より下には表示せず、表示した下端を返す。
fn summary(
    painter: &egui::Painter,
    rect: Rect,
    limit: f32,
    info: &Idle,
    skin: &look::Skin,
    wrap_title: bool,
) -> f32 {
    let detail = &info.now;
    let programme = &detail.programme;
    let mut y = rect.top();

    let middle = y + look::INFO_LOGO_HEIGHT / 2.0;
    let mut x = rect.left();
    if let Some(key) = detail.remote_key {
        crate::ui::key_circle(
            painter,
            pos2(x + look::KEY_BADGE / 2.0, middle),
            look::KEY_BADGE / 2.0,
            skin.key_fill,
            Some(Stroke::new(1.0, skin.edge)),
            key,
            look::KEY_SIZE,
            skin.key_text,
        );
        x += look::KEY_BADGE + 8.0;
    }
    if !detail.badge.is_empty() {
        let (ground, text) = crate::ui::badge_look(painter, detail.badge_colour);
        let pill = crate::ui::logo_pill(
            painter,
            pos2(x, middle),
            detail.logo.as_ref(),
            &detail.badge,
            ground,
            text,
            look::INFO_LOGO_SIZE,
            look::INFO_LOGO_HEIGHT,
            6.0,
            40.0,
        );
        x = pill.right() + 10.0;
    }
    let room = (rect.right() - x).max(0.0);
    let channel = crate::ui::one_line(
        painter,
        &detail.channel,
        font::guide(look::INFO_CHANNEL_SIZE),
        skin.text,
        room,
        0.0,
    );
    if info.band.is_empty() {
        painter.galley(pos2(x, middle - channel.size().y / 2.0), channel, skin.text);
    } else {
        let band = crate::ui::one_line(
            painter,
            &info.band,
            font::guide(look::INFO_BAND_SIZE),
            skin.dim,
            room,
            0.0,
        );
        let top = middle - (channel.size().y + band.size().y) / 2.0;
        let below = top + channel.size().y;
        painter.galley(pos2(x, top), channel, skin.text);
        painter.galley(pos2(x, below), band, skin.dim);
    }
    y += look::INFO_LOGO_HEIGHT + look::INFO_GAP;

    if y + look::INFO_TITLE_LEADING <= limit {
        let titled = crate::ui::text::badges::titled(&programme.title, &programme.detail);
        let title_font = font::guide(look::INFO_TITLE_SIZE);
        if wrap_title {
            let used = crate::ui::text::badges::draw_wrapped(
                painter,
                pos2(rect.left(), y),
                rect.width(),
                &titled,
                title_font,
                skin.text,
                look::INFO_TITLE_LEADING,
            );
            y += used.min(limit - y) + look::INFO_GAP;
        } else {
            crate::ui::text::badges::draw_line(
                painter,
                pos2(rect.left(), y + look::INFO_TITLE_LEADING / 2.0),
                rect.width(),
                &titled,
                title_font,
                skin.text,
                crate::ui::text::badges::Look::Coloured,
                1.0,
            );
            y += look::INFO_TITLE_LEADING + look::INFO_GAP;
        }
    }

    let row = look::INFO_TIME_SIZE * 1.5;
    if y + row <= limit {
        let middle = y + row / 2.0;
        let span = painter.text(
            pos2(rect.left(), middle),
            Align2::LEFT_CENTER,
            airtime(programme),
            font::guide(look::INFO_TIME_SIZE),
            skin.dim,
        );
        if let Some((fraction, left)) = progress(programme, detail.now) {
            let remain = painter.text(
                pos2(rect.right(), middle),
                Align2::RIGHT_CENTER,
                format!("残り {left} 分"),
                font::guide(look::INFO_REMAIN_SIZE),
                skin.dim,
            );
            let track = Rect::from_min_max(
                pos2(span.right() + 12.0, middle - look::INFO_BAR_HEIGHT / 2.0),
                pos2(remain.left() - 12.0, middle + look::INFO_BAR_HEIGHT / 2.0),
            );
            if track.width() > 8.0 {
                let round = look::INFO_BAR_HEIGHT / 2.0;
                painter.rect_filled(track, round, skin.track);
                let mut done = track;
                done.max.x = track.left() + track.width() * fraction;
                painter.rect_filled(done, round, skin.bar);
            }
        }
        y += row + look::INFO_GAP;
    }

    let genres = genres(programme);
    let row = look::INFO_GENRE_SIZE * 1.5;
    if !genres.is_empty() && y + row <= limit {
        let galley = crate::ui::one_line(
            painter,
            &genres,
            font::guide(look::INFO_GENRE_SIZE),
            skin.dim,
            rect.width(),
            0.0,
        );
        painter.galley(
            pos2(rect.left(), y + (row - galley.size().y) / 2.0),
            galley,
            skin.dim,
        );
        y += row;
    }
    y
}

fn notes(ui: &mut Ui, rect: Rect, programme: &Programme, skin: &look::Skin) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let text_font = font::guide(look::INFO_TEXT_SIZE);
    let heading_font = font::guide(look::INFO_HEADING_SIZE);
    let entries = entries(programme);
    egui::ScrollArea::vertical()
        .id_salt("パネルの番組情報")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.set_width(rect.width() - look::SCROLL_ROOM);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            if entries.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("番組の説明はありません")
                            .font(text_font.clone())
                            .color(skin.dim),
                    )
                    .sense(Sense::hover()),
                );
                return;
            }
            let style = crate::ui::guide::copyable::Look {
                heading_font: heading_font.clone(),
                heading: skin.heading,
                text_font: text_font.clone(),
                text: skin.text,
                leading: Some(look::INFO_TEXT_LEADING),
            };
            let id = ui.scope_id().with("パネルの番組情報の本文");
            crate::ui::guide::copyable::show(
                ui,
                id,
                &entries,
                &style,
                rect.width() - look::SCROLL_ROOM,
            );
        });
}

fn next(painter: &egui::Painter, rect: Rect, next: Option<&Programme>, skin: &look::Skin) {
    let Some(next) = next else {
        return;
    };
    let top = rect.bottom() - look::INFO_NEXT_HEIGHT;
    painter.hline(rect.x_range(), top, Stroke::new(1.0, skin.edge));
    let middle = top + look::INFO_NEXT_HEIGHT / 2.0 + 1.0;
    let label = painter.text(
        pos2(rect.left(), middle),
        Align2::LEFT_CENTER,
        "次",
        font::guide(look::INFO_NEXT_SIZE),
        skin.heading,
    );
    let galley = crate::ui::one_line(
        painter,
        &next_text(next),
        font::guide(look::INFO_NEXT_SIZE),
        skin.dim,
        (rect.right() - label.right() - 10.0).max(0.0),
        0.0,
    );
    painter.galley(
        pos2(label.right() + 10.0, middle - galley.size().y / 2.0),
        galley,
        skin.dim,
    );
}

fn airtime(programme: &Programme) -> String {
    programme.start.map_or_else(
        || String::from("—"),
        |start| guide::airtime(start, programme.ends()),
    )
}

/// いま放送中なら、進んだ割合と残りの分（切り上げ）。
fn progress(programme: &Programme, now: Option<guide::Moment>) -> Option<(f32, i64)> {
    let start = programme.start?;
    let ends = programme.ends()?;
    let now = now?;
    if ends <= start || now < start || now >= ends {
        return None;
    }
    let fraction = ((now - start) as f32 / (ends - start) as f32).clamp(0.0, 1.0);
    Some((fraction, (ends - now + 59) / 60))
}

fn genres(programme: &Programme) -> String {
    programme
        .genres
        .iter()
        .map(|genre| match genre.middle_name() {
            Some(middle) => format!("{} - {middle}", genre.name()),
            None => genre.name().to_owned(),
        })
        .collect::<Vec<_>>()
        .join("　")
}

/// 放送で送られていなければ空。
fn entries(programme: &Programme) -> Vec<(String, String)> {
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
    out
}

fn next_text(next: &Programme) -> String {
    let start = next
        .start
        .map(|start| {
            let (_, _, _, hour, minute, _) = guide::civil(guide::shown(start));
            format!("{hour}:{minute:02}")
        })
        .unwrap_or_default();
    let title = crate::ui::text::badges::titled(&next.title, &next.detail).name;
    if start.is_empty() {
        title
    } else {
        format!("{start}　{title}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    fn info() -> Idle {
        let start = 1_791_000_000;
        let mut programme = crate::ui::a_programme(Some(start), Some(3600));
        programme.title = "長い番組名の例　".repeat(6);
        programme.detail = "番組の短い説明がとても長いときの例です。".repeat(6);
        programme.genres = vec![guide::Genre::new(4, 0), guide::Genre::new(4, 1)];
        programme.extended = (0..4)
            .map(|nth| (format!("項目{nth}"), "拡張形式の本文が続く例。".repeat(8)))
            .collect();
        let mut next = crate::ui::a_programme(Some(start + 3600), Some(1800));
        next.title = String::from("次の番組の名前");
        Idle {
            now: crate::ui::guide::detail::Detail {
                programme,
                channel: String::from("ゼロウェーブテレビ"),
                remote_key: Some(4),
                badge: String::from("ZW"),
                badge_colour: Color32::DARK_BLUE,
                logo: None,
                shared: Vec::new(),
                follow: None,
                now: Some(start + 1200),
            },
            band: String::from("地デジ 041"),
            next: Some(next),
        }
    }

    #[test]
    fn a_low_wide_band_splits_in_two() {
        assert!(two_columns(vec2(1200.0, 180.0)));
        assert!(!two_columns(vec2(1200.0, 420.0)));
        assert!(!two_columns(vec2(500.0, 180.0)));
        let (left, right) = split(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 160.0)));
        assert!(left.width() >= look::INFO_LEFT_MIN && left.width() <= look::INFO_LEFT_MAX);
        assert!(right.left() >= left.right() + look::INFO_GUTTER - 0.01);
    }

    #[test]
    fn the_progress_is_said_only_while_it_airs() {
        let p = crate::ui::a_programme(Some(1_000), Some(3600));
        let (fraction, left) = progress(&p, Some(1_000 + 1800)).unwrap();
        assert!((fraction - 0.5).abs() < 1e-3);
        assert_eq!(left, 30);
        assert!(progress(&p, Some(999)).is_none());
        assert!(progress(&p, None).is_none());
        assert_eq!(airtime(&crate::ui::a_programme(None, None)), "—");
    }

    #[test]
    fn everything_stays_inside_the_panel() {
        for size in [
            vec2(1200.0, 134.0),
            vec2(1200.0, 190.0),
            vec2(760.0, 240.0),
            vec2(600.0, 190.0),
            vec2(900.0, 480.0),
        ] {
            let frame = Rect::from_min_size(pos2(0.0, 0.0), size);
            let ctx = egui::Context::default();
            crate::ui::text::font::install(&ctx);
            let input = || egui::RawInput {
                screen_rect: Some(frame),
                ..Default::default()
            };
            let info = info();
            let skin = crate::ui::theme::detail::DARK;
            let mut output = ctx.run_ui(input(), |_| {});
            for _ in 0..2 {
                output.textures_delta.clear();
                output = ctx.run_ui(input(), |ui| show(ui, &info, &skin));
            }
            output.textures_delta.clear();
            let texts: Vec<Rect> = output
                .shapes
                .iter()
                .filter(|clipped| matches!(clipped.shape, egui::Shape::Text(_)))
                .map(|clipped| {
                    clipped
                        .shape
                        .visual_bounding_rect()
                        .intersect(clipped.clip_rect)
                })
                .filter(|rect| rect.is_positive())
                .collect();
            assert!(texts.len() >= 5, "{size:?}: {}", texts.len());
            for rect in &texts {
                assert!(frame.expand(0.5).contains_rect(*rect), "{size:?}: {rect:?}");
            }
            if two_columns(size) {
                let inner = frame.shrink2(vec2(look::INFO_PAD_X, look::INFO_PAD_Y));
                let (left, right) = split(inner);
                for rect in &texts {
                    assert!(
                        rect.right() <= left.right() + 0.5 || rect.left() >= right.left() - 0.5,
                        "{size:?}: {rect:?} が段をまたぐ"
                    );
                }
            }
        }
    }
}
