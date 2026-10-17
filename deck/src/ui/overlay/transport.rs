// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画の操作バー。放送や長さの読めない録画では出ない。
//! ドラッグしている間はポインタの下の位置を出し、放したときに一度だけ再生側へ伝える。

use egui::{Color32, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{status, transport as theme};
use zwcore::player::Progress;

#[derive(Debug, Default, Clone, Copy)]
pub struct State {
    /// ドラッグしている最中なら、いまポインタの下の位置（0〜1）。
    pub dragging: Option<f32>,
    pub hot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pressed {
    Seek(f32),
    SeekBy(i64),
    Pause,
    /// もう一度なら等速へ。
    Pace(f64),
}

const BUTTONS: &[(Face, Pressed)] = &[
    (Face::Text("-60"), Pressed::SeekBy(-60_000)),
    (Face::Text("-15"), Pressed::SeekBy(-15_000)),
    (Face::Text("-5"), Pressed::SeekBy(-5_000)),
    (Face::Icon(Icon::Pause), Pressed::Pause),
    (Face::Text("+5"), Pressed::SeekBy(4_000)),
    (Face::Text("+15"), Pressed::SeekBy(14_000)),
    (Face::Text("+60"), Pressed::SeekBy(59_000)),
    (Face::Text("×2.0"), Pressed::Pace(2.0)),
    (Face::Text("×0.5"), Pressed::Pace(0.5)),
];

#[derive(Debug, Clone, Copy)]
enum Face {
    Text(&'static str),
    Icon(Icon),
}

pub fn show(
    ui: &mut Ui,
    progress: &Progress,
    skin: &status::Skin,
    state: &mut State,
) -> Option<Pressed> {
    let band = ui.max_rect();
    let em = status::TEXT_SIZE;
    let mut pressed = None;
    let mut left = band.left() + status::PAD;
    for (face, what) in BUTTONS {
        let rect = Rect::from_min_max(
            pos2(left, band.top() + 2.0),
            pos2(left + theme::BUTTON_EMS * em, band.bottom() - 2.0),
        );
        left = rect.right() + theme::BUTTON_GAP;
        let response = ui.interact(
            rect,
            ui.scope_id().with(("録画の釦", format!("{what:?}"))),
            Sense::click(),
        );
        let pushed = match what {
            Pressed::Pace(pace) => (progress.pace - pace).abs() < 1e-6,
            _ => false,
        };
        let painter = ui.painter();
        if pushed {
            painter.rect_filled(rect, 3.0, skin.pushed);
        } else if response.hovered() {
            painter.rect_filled(rect, 3.0, skin.divider);
        }
        let face = match (face, what) {
            (Face::Icon(_), Pressed::Pause) if progress.paused => Face::Icon(Icon::Play),
            (face, _) => *face,
        };
        match face {
            Face::Text(label) => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(theme::BUTTON_SIZE),
                    skin.value,
                );
            }
            Face::Icon(icon) => {
                icons::paint_at(painter, rect.center(), theme::ICON, icon, skin.value)
            }
        }
        if response.clicked() {
            pressed = Some(*what);
        }
    }
    let shown = state
        .dragging
        .map(|fraction| (progress.length as f64 * f64::from(fraction)) as i64)
        .unwrap_or(progress.position);
    let text = format!(
        "{} / {}",
        zwcore::input::span::clock_text(shown),
        zwcore::input::span::clock_text(progress.length)
    );
    // 数字を 0 に置き換えた文字列で測る（秒が進んでも幅が揺れない）。
    let sample: String = text
        .chars()
        .map(|letter| if letter.is_ascii_digit() { '0' } else { letter })
        .collect();
    let width = ui
        .painter()
        .layout_no_wrap(sample, FontId::monospace(status::TEXT_SIZE), skin.value)
        .size()
        .x;
    let position = Rect::from_min_max(
        pos2(band.right() - status::CLOCK_PAD - width, band.top()),
        pos2(band.right() - status::CLOCK_PAD, band.bottom()),
    );
    let painter = ui.painter().with_clip_rect(position);
    let galley = painter.layout_no_wrap(text, FontId::monospace(status::TEXT_SIZE), skin.value);
    let y = crate::ui::seat_em(&galley, position.center().y - em / 2.0, em);
    let width = galley.size().x;
    painter.galley(pos2(position.right() - width, y), galley, skin.value);
    let track = Rect::from_min_max(
        pos2(left + status::PAD, band.top()),
        pos2(position.left() - status::PAD * 1.5, band.bottom()),
    );
    if track.width() >= theme::TRACK_LEAST
        && let Some(at) = bar(ui, track, progress, skin, state)
    {
        pressed = Some(Pressed::Seek(at));
    }
    pressed
}

fn bar(
    ui: &mut Ui,
    rect: Rect,
    progress: &Progress,
    skin: &status::Skin,
    state: &mut State,
) -> Option<f32> {
    let painter = ui.painter().with_clip_rect(rect);
    let middle = rect.center().y;
    let response = ui.interact(
        rect,
        ui.scope_id().with("録画の棒"),
        Sense::click_and_drag(),
    );
    // 端の丸が切れないよう、丸の半径の分だけ内側に。
    let span = Rect::from_min_max(
        pos2(rect.left() + theme::KNOB, rect.top()),
        pos2(rect.right() - theme::KNOB, rect.bottom()),
    );
    let rect = span;
    let at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    let hot = response.hovered() || response.dragged() || state.dragging.is_some();
    state.hot = hot;
    let mut chosen = None;
    if (response.drag_started() || response.dragged())
        && let Some(pointer) = pointer
    {
        state.dragging = Some(at(pointer.x));
    }
    if response.drag_stopped() {
        chosen = state.dragging.take().or_else(|| pointer.map(|p| at(p.x)));
    } else if response.clicked() {
        chosen = pointer.map(|p| at(p.x));
        state.dragging = None;
    }
    let fraction = state.dragging.unwrap_or(progress.fraction.clamp(0.0, 1.0));
    let thickness = if hot { theme::TRACK_HOT } else { theme::TRACK };
    let line = |from: f32, to: f32, colour: Color32| {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(from, middle - thickness / 2.0),
                pos2(to, middle + thickness / 2.0),
            ),
            thickness / 2.0,
            colour,
        );
    };
    let knob_x = rect.left() + rect.width() * fraction;
    line(rect.left(), rect.right(), skin.seek_back);
    line(rect.left(), knob_x, skin.seek_done);
    if hot {
        painter.circle_filled(pos2(knob_x, middle), theme::KNOB, skin.seek_done);
        // ポインタの下の時刻はバーの外に出るので、いちばん上のレイヤーにレンダリングする。
        if let Some(p) = pointer.filter(|_| state.dragging.is_none()) {
            let ticks = (progress.length as f64 * f64::from(at(p.x))) as i64;
            let text = zwcore::input::span::clock_text(ticks);
            let top = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("録画の棒の時刻"),
            ));
            let galley = top.layout_no_wrap(text, FontId::monospace(theme::HINT_SIZE), skin.value);
            let size = galley.size() + vec2(8.0, 4.0);
            let x = (p.x - size.x / 2.0).clamp(rect.left(), rect.right() - size.x);
            let hint = Rect::from_min_size(pos2(x, rect.top() - size.y - 4.0), size);
            top.rect_filled(hint, 3.0, skin.background);
            top.rect_stroke(
                hint,
                3.0,
                egui::Stroke::new(1.0, skin.divider),
                egui::StrokeKind::Inside,
            );
            top.galley(hint.min + vec2(4.0, 2.0), galley, skin.value);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress() -> Progress {
        Progress {
            fraction: 0.25,
            position: 90_000 * 60,
            length: 90_000 * 240,
            paused: false,
            buffering: false,
            pace: 1.0,
        }
    }

    fn run(
        ctx: &egui::Context,
        band: Rect,
        events: Vec<egui::Event>,
        state: &mut State,
        progress: &Progress,
    ) -> Option<Pressed> {
        let mut got = None;
        let mut input = egui::RawInput {
            screen_rect: Some(band),
            ..Default::default()
        };
        input.events = events;
        let mut output = ctx.run_ui(input, |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(band));
            got = show(&mut child, progress, &status::DARK, state);
        });
        output.textures_delta.clear();
        got
    }

    fn click(
        ctx: &egui::Context,
        band: Rect,
        at: egui::Pos2,
        state: &mut State,
    ) -> Option<Pressed> {
        let progress = progress();
        run(ctx, band, Vec::new(), state, &progress);
        run(
            ctx,
            band,
            vec![egui::Event::PointerMoved(at)],
            state,
            &progress,
        );
        run(
            ctx,
            band,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            state,
            &progress,
        );
        run(
            ctx,
            band,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            state,
            &progress,
        )
    }

    #[test]
    fn the_bar_and_the_buttons_answer_where_they_are_pressed() {
        let ctx = egui::Context::default();
        let band = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 22.0));
        let mut state = State::default();
        let em = status::TEXT_SIZE;
        let _ = run(&ctx, band, Vec::new(), &mut state, &progress());
        let text = format!(
            "{} / {}",
            zwcore::input::span::clock_text(progress().position),
            zwcore::input::span::clock_text(progress().length)
        );
        let width = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    text,
                    FontId::monospace(status::TEXT_SIZE),
                    egui::Color32::WHITE,
                )
                .size()
                .x
        });
        let x = band.right() - status::CLOCK_PAD - width - status::PAD * 1.5 - 1.0;
        let at = click(&ctx, band, pos2(x, 11.0), &mut state);
        assert!(matches!(at, Some(Pressed::Seek(f)) if f > 0.98), "{at:?}");
        let x = band.left() + status::PAD + theme::BUTTON_EMS * em / 2.0;
        assert_eq!(
            click(&ctx, band, pos2(x, 11.0), &mut state),
            Some(Pressed::SeekBy(-60_000))
        );
        let x = x + 3.0 * (theme::BUTTON_EMS * em + theme::BUTTON_GAP);
        assert_eq!(
            click(&ctx, band, pos2(x, 11.0), &mut state),
            Some(Pressed::Pause)
        );
    }
}
