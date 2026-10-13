// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 左下の音量の表示。パススルー中はバーの代わりに理由を出す。

use egui::{Color32, Rect, Ui, pos2, vec2};

use crate::draw::scene::Volume;
use crate::ui::icons::{self, Icon};

use crate::ui::theme::osd::{
    VOLUME_INK as INK, VOLUME_MUTED as MUTED, VOLUME_MUTED_BAR, VOLUME_SHADE, VOLUME_TRACK,
};

/// `opacity` は薄れと設定の不透明度を掛けた物、`bottom` は下から避ける高さ（全画面表示のバー）。
/// `hdr` はサーフェスが HDR か（[`super::osd::dim`]）。
pub fn show(ui: &mut Ui, frame: Rect, volume: &Volume, opacity: f32, bottom: f32, hdr: bool) {
    let dp = super::osd::scale_for(frame.width());
    let height = 44.0 * dp;
    let margin = 24.0 * dp;
    let icon = 22.0 * dp;
    let bar = 180.0 * dp;
    let font = crate::ui::text::font::osd(15.0 * dp);
    let painter = ui.painter().with_clip_rect(frame);
    let fade = |colour: Color32| colour.gamma_multiply(opacity);
    let label = if volume.passthrough {
        String::from("パススルー中は音量を変えられません")
    } else if volume.muted {
        String::from("消音")
    } else {
        volume.volume.to_string()
    };
    let text = painter.layout_no_wrap(label, font, INK);
    let number = text.size().x.max(40.0 * dp);
    let inner = if volume.passthrough {
        icon + 10.0 * dp + text.size().x
    } else {
        icon + 12.0 * dp + bar + 12.0 * dp + number
    };
    let left = frame.left() + margin;
    let bottom = frame.bottom() - margin - bottom;
    let pill = Rect::from_min_max(
        pos2(left, bottom - height),
        pos2(left + 12.0 * dp + inner + 16.0 * dp, bottom),
    );
    painter.rect_filled(pill, 0.0, super::osd::ground(VOLUME_SHADE, opacity, hdr));
    let middle = pill.center().y;
    let speaker = Rect::from_center_size(
        pos2(pill.left() + 12.0 * dp + icon / 2.0, middle),
        vec2(icon, icon),
    );
    let ink = if volume.passthrough {
        INK.gamma_multiply(0.6)
    } else {
        INK
    };
    draw_speaker(
        &painter,
        speaker,
        fade(ink),
        volume.muted && !volume.passthrough,
    );
    if volume.passthrough {
        painter.galley(
            pos2(speaker.right() + 10.0 * dp, middle - text.size().y / 2.0),
            text,
            fade(INK.gamma_multiply(0.8)),
        );
        return;
    }
    let track = Rect::from_min_size(
        pos2(speaker.right() + 12.0 * dp, middle - 2.5 * dp),
        vec2(bar, 5.0 * dp),
    );
    painter.rect_filled(track, 0.0, fade(VOLUME_TRACK));
    let filled = Rect::from_min_size(
        track.min,
        vec2(bar * volume.volume.min(100) as f32 / 100.0, track.height()),
    );
    let fill = if volume.muted { VOLUME_MUTED_BAR } else { INK };
    painter.rect_filled(filled, 0.0, fade(fill));
    let colour = if volume.muted { MUTED } else { INK };
    painter.galley(
        pos2(
            track.right() + 12.0 * dp + number - text.size().x,
            middle - text.size().y / 2.0,
        ),
        text,
        fade(colour),
    );
}

fn draw_speaker(painter: &egui::Painter, rect: Rect, ink: Color32, muted: bool) {
    icons::paint(painter, rect, Icon::Speaker, ink);
    if muted {
        let cross = MUTED.gamma_multiply(f32::from(ink.a()) / 255.0);
        icons::paint(painter, rect, Icon::MutedMark, cross);
    } else {
        icons::paint(painter, rect, Icon::Waves, ink);
    }
}
