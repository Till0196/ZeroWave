// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像の無いサービス（ラジオ）の待ち画面。データ放送の文書が来るまで番組の情報を出す。
//! 位置は 960×540 の座標で固定し、中身の量では動かさない（長い物は「…」）。

use egui::{Align2, Color32, Rect, Stroke, Ui, pos2, vec2};
use zwcore::guide::{self, Programme};

use crate::draw::scene::Idle;

use crate::ui::theme::osd::{
    IDLE_BAR as BAR, IDLE_BOX, IDLE_DIM as DIM, IDLE_HEADING as HEADING, IDLE_RULE,
    IDLE_TEXT as TEXT, IDLE_TRACK,
};

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 540.0;
const BASE_WIDTH: f32 = 1920.0;
pub(super) const COLUMN: f32 = 640.0;
pub(super) const LEFT: f32 = (WIDTH - COLUMN) / 2.0;
pub(super) const CHANNEL_TOP: f32 = 60.0;
pub(super) const TITLE_TOP: f32 = 110.0;
pub(super) const TIME_MIDDLE: f32 = 168.0;
const GENRE_TOP: f32 = 191.0;
const DETAIL_TOP: f32 = 222.0;
pub(super) const EXTENDED_TOP: f32 = 281.0;
pub(super) const EXTENDED_HEIGHT: f32 = 150.0;
pub(super) const NEXT_RULE: f32 = 445.0;

/// 座標を画面へ変換するもの。映らないときの画面（[`super::trouble`]）も使う。
pub(super) struct Place {
    pub(super) s: f32,
    origin: egui::Pos2,
}

impl Place {
    pub(super) fn of(frame: Rect) -> Self {
        let s = scale(frame);
        Self {
            s,
            origin: frame.center() - vec2(WIDTH / 2.0, HEIGHT / 2.0) * s,
        }
    }

    pub(super) fn at(&self, x: f32, y: f32) -> egui::Pos2 {
        self.origin + vec2(x * self.s, y * self.s)
    }

    pub(super) fn font(&self, size: f32) -> egui::FontId {
        crate::ui::text::font::guide(size * self.s)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn block(
        &self,
        painter: &egui::Painter,
        text: String,
        size: f32,
        colour: Color32,
        width: f32,
        rows: usize,
        line: f32,
    ) -> std::sync::Arc<egui::Galley> {
        let mut job = egui::text::LayoutJob::single_section(
            text,
            egui::TextFormat {
                font_id: self.font(size),
                color: colour,
                line_height: Some(size * line * self.s),
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: width * self.s,
            max_rows: rows,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        painter.layout_job(job)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn station(
    painter: &egui::Painter,
    place: &Place,
    logo: Option<&std::sync::Arc<zwcore::guide::Logo>>,
    badge: &str,
    badge_colour: Color32,
    channel: &str,
    band: &str,
) {
    let s = place.s;
    let row = CHANNEL_TOP + 18.0;
    let pill = crate::ui::logo_pill(
        painter,
        place.at(LEFT, row),
        logo,
        badge,
        badge_colour,
        Color32::WHITE,
        12.0 * s,
        36.0 * s,
        8.0 * s,
        64.0 * s,
    );
    let beside = (pill.right() - place.at(0.0, 0.0).x) / s + 12.0;
    painter.text(
        place.at(beside, row - 9.0),
        Align2::LEFT_CENTER,
        channel,
        place.font(13.0),
        TEXT,
    );
    painter.text(
        place.at(beside, row + 9.0),
        Align2::LEFT_CENTER,
        band,
        place.font(11.0),
        DIM,
    );
}

/// 文字の入る矩形でクリップした Painter を返す。
pub(super) fn ground(painter: &egui::Painter, place: &Place) -> egui::Painter {
    let area = Rect::from_min_max(
        place.at(LEFT, EXTENDED_TOP),
        place.at(LEFT + COLUMN, EXTENDED_TOP + EXTENDED_HEIGHT),
    );
    painter.rect_filled(area, 6.0 * place.s, IDLE_BOX);
    painter.with_clip_rect(area.shrink2(vec2(14.0 * place.s, 12.0 * place.s)))
}

pub(super) fn below(painter: &egui::Painter, place: &Place, label: &str, text: String) {
    painter.hline(
        place.at(LEFT, 0.0).x..=place.at(LEFT + COLUMN, 0.0).x,
        place.at(0.0, NEXT_RULE).y,
        Stroke::new(1.0, IDLE_RULE),
    );
    let middle = NEXT_RULE + 10.0 + 9.0;
    let label = painter.text(
        place.at(LEFT, middle),
        Align2::LEFT_CENTER,
        label,
        place.font(12.0),
        DIM,
    );
    let left = (label.right() - place.at(0.0, 0.0).x) / place.s + 10.0;
    let galley = place.block(painter, text, 12.0, DIM, LEFT + COLUMN - left, 1, 1.4);
    let height = galley.size().y;
    painter.galley(
        egui::pos2(
            label.right() + 10.0 * place.s,
            place.at(0.0, middle).y - height / 2.0,
        ),
        galley,
        DIM,
    );
}

pub fn show(ui: &mut Ui, frame: Rect, idle: &Idle) {
    let place = Place::of(frame);
    let s = place.s;
    let at = |x: f32, y: f32| place.at(x, y);
    let painter = ui.painter().with_clip_rect(frame);
    let font = |size: f32| place.font(size);
    let block = |text: String, size: f32, colour: Color32, width: f32, rows: usize, line: f32| {
        place.block(&painter, text, size, colour, width, rows, line)
    };
    let programme = &idle.now.programme;
    let right = LEFT + COLUMN;

    station(
        &painter,
        &place,
        idle.now.logo.as_ref(),
        &idle.now.badge,
        idle.now.badge_colour,
        &idle.now.channel,
        &format!("{}　ラジオ", idle.band),
    );

    let title = crate::ui::text::badges::titled(&programme.title, &programme.detail);
    let galley = block(title.name.clone(), 24.0, TEXT, COLUMN, 1, 1.4);
    painter.galley(at(LEFT, TITLE_TOP), galley, TEXT);

    let span_rect = painter.text(
        at(LEFT, TIME_MIDDLE),
        Align2::LEFT_CENTER,
        when(programme),
        font(13.0),
        DIM,
    );
    if let (Some(start), Some(ends), Some(now)) = (programme.start, programme.ends(), idle.now.now)
        && ends > start
    {
        let fraction = ((now - start) as f32 / (ends - start) as f32).clamp(0.0, 1.0);
        let left_minutes = ((ends - now).max(0) + 59) / 60;
        let remain = painter.text(
            at(right, TIME_MIDDLE),
            Align2::RIGHT_CENTER,
            format!("残り {left_minutes} 分"),
            font(11.5),
            DIM,
        );
        let track = Rect::from_min_max(
            pos2(span_rect.right() + 14.0 * s, at(0.0, TIME_MIDDLE - 2.0).y),
            pos2(remain.left() - 14.0 * s, at(0.0, TIME_MIDDLE + 2.0).y),
        );
        if track.width() > 0.0 {
            painter.rect_filled(track, 2.0 * s, IDLE_TRACK);
            let mut done = track;
            done.max.x = track.left() + track.width() * fraction;
            painter.rect_filled(done, 2.0 * s, BAR);
        }
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
        let galley = block(genres.join("　"), 11.5, DIM, COLUMN, 1, 1.4);
        painter.galley(at(LEFT, GENRE_TOP), galley, DIM);
    }

    if !programme.detail.is_empty() {
        let galley = block(
            crate::ui::text::badges::cleaned(&programme.detail),
            12.5,
            TEXT,
            COLUMN,
            2,
            1.75,
        );
        painter.galley(at(LEFT, DETAIL_TOP), galley, TEXT);
    }

    // 拡張形式は決まった高さの背景の中に。入らない分は背景で切る。
    if !programme.extended.is_empty() {
        let inside = ground(&painter, &place);
        let mut y = EXTENDED_TOP + 12.0;
        for (heading, value) in &programme.extended {
            if y >= EXTENDED_TOP + EXTENDED_HEIGHT - 12.0 {
                break;
            }
            if !heading.is_empty() {
                let galley = block(heading.clone(), 11.5, HEADING, COLUMN - 28.0, 1, 1.4);
                let height = galley.size().y / s;
                inside.galley(at(LEFT + 14.0, y), galley, HEADING);
                y += height + 8.0;
            }
            let galley = block(value.clone(), 12.5, TEXT, COLUMN - 28.0, usize::MAX, 1.75);
            let height = galley.size().y / s;
            inside.galley(at(LEFT + 14.0, y), galley, TEXT);
            y += height + 8.0;
        }
    }

    if let Some(next) = &idle.next {
        let start = next
            .start
            .map(|start| {
                let (_, _, _, hour, minute, _) = guide::civil(start);
                format!("{hour}:{minute:02}")
            })
            .unwrap_or_default();
        below(&painter, &place, "次", format!("{start}　{}", next.title));
    }
}

/// 1920 幅で 2、そこから OSD と同じ倍率で伸び縮みする。枠の高さに対する比は超えない。
fn scale(frame: Rect) -> f32 {
    let osd = super::osd::scale_for(frame.width()) / super::osd::scale_for(BASE_WIDTH);
    let fits = frame.height() / HEIGHT / 2.0;
    BASE_WIDTH / WIDTH * osd.min(fits)
}

fn when(programme: &Programme) -> String {
    let Some(start) = programme.start else {
        return String::new();
    };
    guide::airtime(start, programme.ends())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> Idle {
        let start = 1_791_000_000;
        let mut programme = crate::ui::a_programme(Some(start), Some(3600));
        programme.title = "長い番組名の例　".repeat(8);
        programme.detail = "番組の短い説明がとても長いときの例です。".repeat(8);
        programme.genres = vec![guide::Genre::new(4, 0), guide::Genre::new(4, 1)];
        programme.extended = (0..6)
            .map(|nth| {
                (
                    format!("項目{nth}"),
                    "拡張形式の本文が長く続く例。".repeat(10),
                )
            })
            .collect();
        let mut next = crate::ui::a_programme(Some(start + 3600), Some(1800));
        next.title = "次の番組の名前".repeat(10);
        Idle {
            now: crate::ui::guide::detail::Detail {
                programme,
                channel: String::from("ゼロウェーブラジオ"),
                remote_key: None,
                badge: String::from("ZW"),
                badge_colour: Color32::DARK_BLUE,
                logo: None,
                shared: Vec::new(),
                follow: None,
                now: Some(start + 1200),
            },
            band: String::from("BS 531"),
            next: Some(next),
        }
    }

    /// どの文字も決めた場所に収まり、横に並ぶ物どうしは縦に重ならない。
    #[test]
    fn the_layout_stays_put_and_nothing_overlaps() {
        let at_base = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0));
        assert!((scale(at_base) - 2.0).abs() < 1e-4);
        for width in [640.0, 960.0, 1280.0, 1920.0, 3840.0] {
            let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, width * 9.0 / 16.0));
            let ctx = egui::Context::default();
            crate::ui::text::font::install(&ctx);
            let input = || egui::RawInput {
                screen_rect: Some(frame),
                ..Default::default()
            };
            let idle = idle();
            let mut output = ctx.run_ui(input(), |_| {});
            for _ in 0..2 {
                output.textures_delta.clear();
                output = ctx.run_ui(input(), |ui| show(ui, frame, &idle));
            }
            output.textures_delta.clear();
            let s = scale(frame);
            let origin = frame.center() - vec2(WIDTH / 2.0, HEIGHT / 2.0) * s;
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
            assert!(texts.len() >= 10, "{}", texts.len());
            for rect in &texts {
                assert!(frame.contains_rect(*rect), "{rect:?}");
                assert!(
                    rect.left() >= origin.x + LEFT * s - 0.5
                        && rect.right() <= origin.x + (LEFT + COLUMN) * s + 0.5
                );
            }
            for (nth, one) in texts.iter().enumerate() {
                for other in &texts[nth + 1..] {
                    let side_by_side = one.x_range().intersects(other.x_range());
                    let overlap = one.y_range().intersection(other.y_range()).span();
                    assert!(
                        !side_by_side || overlap <= 1.0,
                        "{width}: {one:?} と {other:?} が重なる"
                    );
                }
            }
            let y = |at: f32| origin.y + at * s;
            assert!(texts.iter().any(|rect| rect.top() > y(NEXT_RULE)));
            assert!(
                texts
                    .iter()
                    .filter(|rect| rect.top() < y(EXTENDED_TOP))
                    .all(|rect| rect.bottom() <= y(EXTENDED_TOP) + 1.0)
            );
        }
    }
}
