// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映らないときの画面（E200 番台）。ラジオの待ち画面（[`super::idle`]）と同じ枠とウィジェットで表示する。

use egui::{Color32, Rect, Stroke, Ui, vec2};

use super::idle::{
    COLUMN, EXTENDED_TOP, LEFT, Place, TIME_MIDDLE, TITLE_TOP, below, ground, station,
};
use crate::draw::scene::Trouble;
use crate::ui::theme::osd::{
    IDLE_HEADING as HEADING, IDLE_TEXT as TEXT, TROUBLE_CODE, TROUBLE_DIM,
};

/// 対処の文の上端（ジャンルの行が無いので、番号のラベルのすぐ下）。
const GUIDE_TOP: f32 = 196.0;
const HEADING_WIDTH: f32 = 120.0;
const HEADING_GAP: f32 = 16.0;

/// `frame` は映像の場所（無いときは 16:9 の枠）。`cover` はその外側も含めて塗る場所。
pub fn show(ui: &mut Ui, cover: Rect, frame: Rect, trouble: &Trouble) {
    // E201 は最後のフレームを暗くして下に残す。ほかは黒で覆う。
    ui.painter().rect_filled(
        cover,
        0.0,
        if trouble.dim {
            TROUBLE_DIM
        } else {
            Color32::BLACK
        },
    );
    let place = Place::of(frame);
    let s = place.s;
    let painter = ui.painter().with_clip_rect(frame);

    if let Some(at) = &trouble.station {
        station(
            &painter,
            &place,
            at.logo.as_ref(),
            &at.badge,
            at.badge_colour,
            &at.channel,
            &at.band,
        );
    }

    let galley = place.block(
        &painter,
        trouble.title.to_owned(),
        24.0,
        TEXT,
        COLUMN,
        1,
        1.4,
    );
    painter.galley(place.at(LEFT, TITLE_TOP), galley, TEXT);

    let code = painter.layout_no_wrap(trouble.code.to_owned(), place.font(11.5), TROUBLE_CODE);
    let tag = Rect::from_min_size(
        place.at(LEFT, TIME_MIDDLE - 9.0),
        vec2(code.size().x + 14.0 * s, 18.0 * s),
    );
    painter.rect_stroke(
        tag,
        4.0 * s,
        Stroke::new(1.0 * s.max(1.0), TROUBLE_CODE),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(tag.left() + 7.0 * s, tag.center().y - code.size().y / 2.0),
        code,
        TROUBLE_CODE,
    );

    let galley = place.block(
        &painter,
        trouble.guide.to_owned(),
        12.5,
        TEXT,
        COLUMN,
        2,
        1.75,
    );
    painter.galley(place.at(LEFT, GUIDE_TOP), galley, TEXT);

    if !trouble.rows.is_empty() {
        let inside = ground(&painter, &place);
        let row = 12.5 * 1.75 + 2.0;
        for (nth, (heading, value)) in trouble.rows.iter().enumerate() {
            let y = EXTENDED_TOP + 12.0 + row * nth as f32;
            let middle = y + row / 2.0 - 1.0;
            let galley = place.block(
                &painter,
                heading.clone(),
                11.5,
                HEADING,
                HEADING_WIDTH,
                1,
                1.75,
            );
            let height = galley.size().y;
            inside.galley(
                egui::pos2(
                    place.at(LEFT + 14.0, 0.0).x,
                    place.at(0.0, middle).y - height / 2.0,
                ),
                galley,
                HEADING,
            );
            let left = LEFT + 14.0 + HEADING_WIDTH + HEADING_GAP;
            let galley = place.block(
                &painter,
                value.clone(),
                12.5,
                TEXT,
                LEFT + COLUMN - 14.0 - left,
                1,
                1.75,
            );
            let height = galley.size().y;
            inside.galley(
                egui::pos2(
                    place.at(left, 0.0).x,
                    place.at(0.0, middle).y - height / 2.0,
                ),
                galley,
                TEXT,
            );
        }
    }

    if let Some((label, text)) = &trouble.below {
        below(&painter, &place, label, text.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn trouble() -> Trouble {
        Trouble {
            code: "E203",
            title: "このチャンネルは放送を休止しています",
            guide: "放送が始まるまでお待ちください。放送時間は番組表で確認できます。",
            station: Some(crate::draw::scene::Station {
                channel: String::from("ゼロウェーブテレビ").repeat(4),
                badge: String::from("ZW"),
                badge_colour: Color32::DARK_BLUE,
                logo: None,
                band: String::from("地デジ 071"),
            }),
            rows: vec![
                (
                    String::from("状態"),
                    String::from("放送休止中（正常に受信しています）"),
                ),
                (String::from("受信状態"), String::from("良好")),
            ],
            below: Some((
                String::from("次"),
                String::from("5:00　モーサテ").repeat(20),
            )),
            dim: false,
        }
    }

    /// 文字は決めた列に収まり、横に並ぶ物どうしは縦に重ならない。
    #[test]
    fn the_screen_keeps_to_the_radio_layout() {
        for width in [640.0, 960.0, 1920.0, 3840.0] {
            let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, width * 9.0 / 16.0));
            let ctx = egui::Context::default();
            crate::ui::text::font::install(&ctx);
            let input = || egui::RawInput {
                screen_rect: Some(frame),
                ..Default::default()
            };
            let shown = trouble();
            let mut output = ctx.run_ui(input(), |_| {});
            for _ in 0..2 {
                output.textures_delta.clear();
                output = ctx.run_ui(input(), |ui| show(ui, frame, frame, &shown));
            }
            output.textures_delta.clear();
            let place = Place::of(frame);
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
            assert!(texts.len() >= 12, "{}", texts.len());
            let left = place.at(LEFT, 0.0).x - 0.5;
            let right = place.at(LEFT + COLUMN, 0.0).x + 0.5;
            for rect in &texts {
                assert!(frame.contains_rect(*rect), "{rect:?}");
                assert!(rect.left() >= left && rect.right() <= right, "{rect:?}");
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
        }
    }
}
