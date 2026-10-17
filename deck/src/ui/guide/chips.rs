// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送の種別を選ぶ丸いタブ。パネルの番組表と番組表のウィンドウの両方に出る。

use egui::{Pos2, Rect, Sense, Ui, pos2, vec2};

use crate::ui::theme::chips;

pub type Look = chips::Skin;

/// 別ウィンドウの上。明暗で変わる（暗い番組表では白地向けの色が沈む）。
pub fn paper(mode: crate::ui::theme::Mode) -> Look {
    chips::skin(mode)
}

/// 押されたものの番号と、右端の位置を返す。
pub fn show(
    ui: &mut Ui,
    at: Pos2,
    labels: &[&str],
    selected: usize,
    look: Look,
) -> (Option<usize>, f32) {
    let mut pressed = None;
    let mut left = at.x;
    for (index, label) in labels.iter().enumerate() {
        let chosen = index == selected;
        let galley = ui.painter().layout_no_wrap(
            (*label).to_owned(),
            crate::ui::text::font::guide(chips::SIZE),
            if chosen {
                look.selected_text
            } else {
                look.text
            },
        );
        let rect = Rect::from_min_size(
            pos2(left, at.y - chips::HEIGHT / 2.0),
            vec2(galley.size().x + chips::PAD_X * 2.0, chips::HEIGHT),
        );
        // 一つの領域に二組あっても識別子が重ならないよう、組の先頭の位置も混ぜる。
        let response = ui.interact(
            rect,
            ui.scope_id()
                .with(("chips", at.x.to_bits(), at.y.to_bits(), index)),
            Sense::click(),
        );
        let painter = ui.painter();
        if chosen {
            painter.rect_filled(rect, chips::ROUNDING, look.selected_fill);
        } else if response.hovered() {
            painter.rect_filled(
                rect,
                chips::ROUNDING,
                look.selected_fill.gamma_multiply(0.4),
            );
        }
        painter.galley(
            pos2(
                rect.left() + chips::PAD_X,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            look.text,
        );
        if response.clicked() {
            pressed = Some(index);
        }
        left = rect.right() + chips::GAP;
    }
    (pressed, left)
}
