// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルスキャンの進行状況のウィンドウ。「完了」はそこまでを残し、「キャンセル」は捨てる。

use egui::{FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::scan::Scanning;
use crate::ui::theme::settings as skin;
use crate::ui::theme::settings::Skin as Look;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Done,
    Stop,
}

#[derive(Debug, Default)]
pub struct Shown {
    pub pressed: Option<Pressed>,
    /// ログは OS のテキストコントロールで（Windows と macOS）。egui は枠だけ。
    pub native: Vec<crate::ui::guide::detail::NativeBox>,
}

pub const WIDTH: f32 = 600.0;
pub const HEIGHT: f32 = 520.0;

const BUTTON: f32 = 30.0;
const GAP: f32 = 12.0;

pub fn show(root: &mut Ui, scanning: &Scanning, look: &Look) -> Shown {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let body = full.shrink(16.0);
    let mut at = body.top();
    // スキャン中は回転するインジケーターを添える（文字列が変わらない間も止まって見えないように）。
    let running = scanning.done_with.is_none() && !scanning.what.is_empty();
    let what = if scanning.what.is_empty() {
        String::from("待機中")
    } else {
        scanning.what.clone()
    };
    let text_left = if running {
        let spin = Rect::from_min_size(pos2(body.left(), at), vec2(16.0, 16.0));
        root.put(spin, egui::Spinner::new().size(14.0).color(look.primary));
        body.left() + 22.0
    } else {
        body.left()
    };
    let painter = root.painter();
    painter.text(
        pos2(text_left, at),
        egui::Align2::LEFT_TOP,
        what,
        FontId::proportional(skin::TEXT_SIZE + 1.0),
        look.text,
    );
    at += 24.0;
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        &scanning.measured,
        FontId::monospace(skin::TEXT_SIZE - 1.0),
        look.label,
    );
    at += 22.0;
    let bar = Rect::from_min_size(pos2(body.left(), at), vec2(body.width(), 8.0));
    painter.rect_filled(bar, 4.0, look.select_fill);
    painter.rect_stroke(
        bar,
        4.0,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let ratio = scanning.ratio();
    if ratio > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * ratio, bar.height())),
            4.0,
            look.primary,
        );
    }
    at = bar.bottom() + 8.0;
    let counted = if scanning.total > 0 {
        format!("{} / {}", scanning.done, scanning.total)
    } else {
        String::new()
    };
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        counted,
        FontId::monospace(skin::TEXT_SIZE - 1.0),
        look.label,
    );
    if !scanning.line.is_empty() {
        painter.text(
            pos2(body.right(), at),
            egui::Align2::RIGHT_TOP,
            format!("受信方式 {}", scanning.line),
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
    }
    at += 22.0;
    // ログは末尾に追従する。上へスクロールすれば外れ、末尾まで戻せばまた追従する。
    let log = Rect::from_min_max(
        pos2(body.left(), at),
        pos2(body.right(), (body.bottom() - BUTTON - GAP).max(at + 60.0)),
    );
    root.painter()
        .rect_filled(log, skin::SELECT_ROUNDING, look.select_fill);
    root.painter().rect_stroke(
        log,
        skin::SELECT_ROUNDING,
        Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let mut native = Vec::new();
    if cfg!(any(windows, target_os = "macos")) {
        native.push(crate::ui::guide::detail::NativeBox {
            rect: log.shrink2(vec2(8.0, 6.0)),
            sections: vec![(String::new(), scanning.log.join("\n"))],
            marks: Vec::new(),
            size: skin::TEXT_SIZE - 1.5,
            scroll: true,
            colours: Some([look.text, look.label, look.select_fill, look.body]),
            mono: false,
            tail: true,
        });
    } else {
        let mut inside = root.new_child(
            egui::UiBuilder::new()
                .max_rect(log.shrink2(vec2(8.0, 6.0)))
                .id_salt("スキャンの記録"),
        );
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(&mut inside, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for said in &scanning.log {
                    let colour = if said.starts_with(' ') || said.contains(": 受信") {
                        look.text
                    } else {
                        look.label
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(said)
                                .font(FontId::proportional(skin::TEXT_SIZE - 1.5))
                                .color(colour),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .selectable(true),
                    );
                }
            });
    }
    at = log.bottom() + GAP;
    let said = match &scanning.done_with {
        Some(Ok(summary)) => format!(
            "多重化ストリーム数 {}・サービス数 {}",
            summary.streams, summary.services
        ),
        Some(Err(why)) => why.clone(),
        None => String::new(),
    };
    root.painter().text(
        pos2(body.left(), at + 8.0),
        egui::Align2::LEFT_TOP,
        &said,
        FontId::proportional(skin::TEXT_SIZE),
        if matches!(scanning.done_with, Some(Err(_))) {
            look.typed
        } else {
            look.text
        },
    );
    let mut pressed = None;
    let mut right = body.right();
    let running = scanning.done_with.is_none();
    for (label, which, primary, shown) in [
        ("キャンセル", Pressed::Stop, false, running),
        ("完了", Pressed::Done, true, true),
    ] {
        if !shown {
            continue;
        }
        let width = 96.0;
        let button = Rect::from_min_size(pos2(right - width, at), vec2(width, 30.0));
        right = button.left() - 8.0;
        let response = root.interact(
            button,
            root.scope_id().with(("走査の釦", label)),
            Sense::click(),
        );
        let painter = root.painter();
        if primary {
            painter.rect_filled(button, skin::BUTTON_ROUNDING, look.primary);
        } else {
            let fill = if response.hovered() {
                look.button_edge
            } else {
                look.button_fill
            };
            painter.rect_filled(button, skin::BUTTON_ROUNDING, fill);
            painter.rect_stroke(
                button,
                skin::BUTTON_ROUNDING,
                Stroke::new(1.0, look.button_edge),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            button.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(skin::TEXT_SIZE),
            if primary {
                egui::Color32::WHITE
            } else {
                look.text
            },
        );
        if response.clicked() {
            pressed = Some(which);
        }
    }
    Shown { pressed, native }
}
