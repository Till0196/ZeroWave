// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! リモコンのウィンドウ。押されたものを返すだけ。

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::guide::Band;

use crate::ui::icons::{self, Icon};
use crate::ui::theme::remote as theme;
use crate::ui::theme::remote::Skin;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pressed {
    /// 種別を選んだ。選局はしない。
    Band(Band),
    Number {
        band: Band,
        number: u8,
    },
    VolumeUp,
    VolumeDown,
    Mute,
    MiniGuide,
    Guide,
    ChannelUp,
    ChannelDown,
    Record,
    Capture,
    PageUp,
    PageDown,
    Back10,
    Forward30,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
    Blue,
    Red,
    Green,
    Yellow,
    Data,
    Captions,
    NextSound,
    Rewind,
    PlayPause,
    FastForward,
    SkipStart,
    Stop,
    Pause,
    SkipEnd,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct State {
    /// 押していなければ見ている局の種別。
    pub band: Option<Band>,
}

#[derive(Debug, Clone)]
enum Face {
    Text(String),
    BoldThen(&'static str, &'static str),
    Icon(Icon),
    LargeIcon(Icon),
    IconThen(Icon, &'static str),
    IconOver(Icon, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Key,
    Chosen,
    Faint,
    Enter,
    Colour(Color32),
    Bare,
    Disabled,
}

/// `bands` は局のある種別。空ならネットワークの並びの先頭の四つ。
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    watching: Band,
    bands: &[Band],
    skin: &Skin,
) -> Option<Pressed> {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, skin.background);
    // ウィンドウの大きさに合わせて中身を拡大・縮小し、上の真ん中に置く。
    let scale = (full.width() / theme::DESIGN_WIDTH).min(full.height() / theme::DESIGN_HEIGHT);
    let d = |x: f32| x * scale;
    let full = Rect::from_center_size(
        pos2(full.center().x, full.top() + d(theme::DESIGN_HEIGHT) / 2.0),
        vec2(d(theme::DESIGN_WIDTH), d(theme::DESIGN_HEIGHT)),
    );
    let mut pressed = None;
    let mut press = |what: Option<Pressed>| {
        if what.is_some() {
            pressed = what;
        }
    };
    let left = full.left() + d(theme::PAD_X);
    let right = full.right() - d(theme::PAD_X);
    let mut top = full.top() + d(theme::PAD_TOP);
    let width = right - left;

    let band = state.band.unwrap_or(watching);
    {
        // 局のある種別を一行に四つまで、二行まで。
        let shown: Vec<Band> = if bands.is_empty() {
            Band::all()
                .into_iter()
                .filter(|band| *band != Band::OTHER)
                .take(theme::BANDS_PER_ROW)
                .collect()
        } else {
            bands
                .iter()
                .copied()
                .take(theme::BANDS_PER_ROW * theme::BAND_ROWS)
                .collect()
        };
        let bands: Vec<(String, Band)> =
            shown.into_iter().map(|band| (band.name(), band)).collect();
        let (rows, columns) = band_grid(bands.len());
        let each = (width - d(theme::KEY_GAP) * (columns as f32 - 1.0)) / columns as f32;
        for (n, (label, which)) in bands.iter().enumerate() {
            let (row, column) = (n / columns, n % columns);
            let rect = Rect::from_min_size(
                pos2(
                    left + (each + d(theme::KEY_GAP)) * column as f32,
                    top + (d(theme::BAND_HEIGHT) + d(theme::KEY_GAP)) * row as f32,
                ),
                vec2(each, d(theme::BAND_HEIGHT)),
            );
            let style = if *which == band {
                Style::Chosen
            } else {
                Style::Faint
            };
            if key(
                ui,
                scale,
                rect,
                label,
                Face::Text(label.clone()),
                style,
                skin,
                d(theme::BAND_ROUNDING),
            ) {
                state.band = Some(*which);
                press(Some(Pressed::Band(*which)));
            }
        }
        top += d(theme::BAND_HEIGHT) * rows as f32
            + d(theme::KEY_GAP) * (rows as f32 - 1.0)
            + d(theme::GAP);
    }

    {
        let each = (width - d(theme::KEY_GAP) * 2.0) / 3.0;
        for n in 1..=12u8 {
            let (column, row) = (((n - 1) % 3) as f32, ((n - 1) / 3) as f32);
            let rect = Rect::from_min_size(
                pos2(
                    left + (each + d(theme::KEY_GAP)) * column,
                    top + (d(theme::KEY_HEIGHT) + d(theme::KEY_GAP)) * row,
                ),
                vec2(each, d(theme::KEY_HEIGHT)),
            );
            let label: &'static str = DIGITS[usize::from(n)];
            if key_sized(
                ui,
                scale,
                rect,
                label,
                Face::Text(label.to_string()),
                Style::Key,
                skin,
                d(theme::ROUNDING),
                d(theme::DIGIT_SIZE),
            ) {
                press(Some(Pressed::Number { band, number: n }));
            }
        }
        top += d(theme::KEY_HEIGHT) * 4.0 + d(theme::KEY_GAP) * 3.0 + d(theme::GAP);
    }

    {
        let height = d(theme::ROCKER_KEY) * 2.0 + d(theme::ROCKER_LABEL);
        let volume = Rect::from_min_size(pos2(left, top), vec2(d(theme::ROCKER_WIDTH), height));
        press(rocker(
            ui,
            scale,
            volume,
            "音量",
            (Icon::Plus, Pressed::VolumeUp),
            (Icon::Minus, Pressed::VolumeDown),
            skin,
        ));
        let channel = Rect::from_min_size(
            pos2(right - d(theme::ROCKER_WIDTH), top),
            vec2(d(theme::ROCKER_WIDTH), height),
        );
        press(rocker(
            ui,
            scale,
            channel,
            "チャンネル",
            (Icon::TriUp, Pressed::ChannelUp),
            (Icon::TriDown, Pressed::ChannelDown),
            skin,
        ));
        let middle_left = volume.right() + d(theme::ROCKER_GAP);
        let middle_right = channel.left() - d(theme::ROCKER_GAP);
        let three = [
            (Face::IconThen(Icon::Mute, "消音"), Pressed::Mute, "消音"),
            (Face::Text("パネル".into()), Pressed::MiniGuide, "パネル"),
            (Face::Text("番組表".into()), Pressed::Guide, "番組表"),
        ];
        for (n, (face, what, id)) in three.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    middle_left,
                    top + (d(theme::KEY_HEIGHT) + d(theme::KEY_GAP)) * n as f32,
                ),
                vec2(middle_right - middle_left, d(theme::KEY_HEIGHT)),
            );
            if key(
                ui,
                scale,
                rect,
                id,
                face.clone(),
                Style::Key,
                skin,
                d(theme::ROUNDING),
            ) {
                press(Some(*what));
            }
        }
        top += height + d(theme::GAP);
    }

    {
        let each = (width - d(theme::KEY_GAP)) / 2.0;
        let record = Rect::from_min_size(pos2(left, top), vec2(each, d(theme::KEY_HEIGHT)));
        let capture = Rect::from_min_size(
            pos2(left + each + d(theme::KEY_GAP), top),
            vec2(each, d(theme::KEY_HEIGHT)),
        );
        if key(
            ui,
            scale,
            record,
            "録画",
            Face::IconThen(Icon::Record, "録画"),
            Style::Key,
            skin,
            d(theme::ROUNDING),
        ) {
            press(Some(Pressed::Record));
        }
        if key(
            ui,
            scale,
            capture,
            "キャプチャ",
            Face::IconThen(Icon::Camera, "キャプチャ"),
            Style::Key,
            skin,
            d(theme::ROUNDING),
        ) {
            press(Some(Pressed::Capture));
        }
        top += d(theme::KEY_HEIGHT) + d(theme::GAP);
    }

    {
        let height = d(theme::CLUSTER_PAD) * 2.0
            + d(theme::ROUND_KEY) * 2.0
            + d(theme::CLUSTER_GAP) * 2.0
            + d(theme::DISC);
        let area = Rect::from_min_size(pos2(left, top), vec2(width, height));
        press(cluster(ui, scale, area, skin));
        top += height + d(theme::GAP);
    }

    {
        let inner_left = left + d(theme::COLOUR_PAD_X);
        let inner_width = width - d(theme::COLOUR_PAD_X) * 2.0;
        let each = (inner_width - d(theme::COLOUR_GAP) * 3.0) / 4.0;
        let colours = [
            ("青", theme::BLUE, Pressed::Blue),
            ("赤", theme::RED, Pressed::Red),
            ("緑", theme::GREEN, Pressed::Green),
            ("黄", theme::YELLOW, Pressed::Yellow),
        ];
        for (n, (label, colour, what)) in colours.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(inner_left + (each + d(theme::COLOUR_GAP)) * n as f32, top),
                vec2(each, d(theme::COLOUR_HEIGHT)),
            );
            if key_sized(
                ui,
                scale,
                rect,
                label,
                Face::Text(label.to_string()),
                Style::Colour(*colour),
                skin,
                d(theme::COLOUR_ROUNDING),
                d(theme::COLOUR_SIZE),
            ) {
                press(Some(*what));
            }
        }
        top += d(theme::COLOUR_HEIGHT) + d(theme::GAP);
    }

    {
        let rows: [&[(Face, Pressed, &'static str)]; 3] = [
            &[
                (Face::BoldThen("d", "データ"), Pressed::Data, "d データ"),
                (Face::Text("字幕".into()), Pressed::Captions, "字幕"),
                (
                    Face::Text("音声切換".into()),
                    Pressed::NextSound,
                    "音声切換",
                ),
            ],
            &[
                (Face::Icon(Icon::Rewind), Pressed::Rewind, "早戻し"),
                (
                    Face::IconOver(Icon::PlayPause, "一時停止"),
                    Pressed::PlayPause,
                    "再生・一時停止",
                ),
                (
                    Face::Icon(Icon::FastForward),
                    Pressed::FastForward,
                    "早送り",
                ),
            ],
            &[
                (Face::Icon(Icon::SkipBack), Pressed::SkipStart, "先頭へ"),
                (Face::Icon(Icon::Stop), Pressed::Stop, "停止"),
                (Face::Icon(Icon::Pause), Pressed::Pause, "一時停止"),
                (Face::Icon(Icon::SkipForward), Pressed::SkipEnd, "末尾へ"),
            ],
        ];
        let height =
            d(theme::GROUP_PAD_Y) * 2.0 + d(theme::KEY_HEIGHT) * 3.0 + d(theme::KEY_GAP) * 2.0;
        let group = Rect::from_min_size(pos2(left, top), vec2(width, height));
        ui.painter()
            .rect_filled(group, d(theme::GROUP_ROUNDING), skin.group);
        let inner_left = left + d(theme::GROUP_PAD_X);
        let inner_width = width - d(theme::GROUP_PAD_X) * 2.0;
        for (r, row) in rows.iter().enumerate() {
            let count = row.len() as f32;
            let each = (inner_width - d(theme::KEY_GAP) * (count - 1.0)) / count;
            let y =
                top + d(theme::GROUP_PAD_Y) + (d(theme::KEY_HEIGHT) + d(theme::KEY_GAP)) * r as f32;
            for (n, (face, what, id)) in row.iter().enumerate() {
                let rect = Rect::from_min_size(
                    pos2(inner_left + (each + d(theme::KEY_GAP)) * n as f32, y),
                    vec2(each, d(theme::KEY_HEIGHT)),
                );
                // 停止はまだ繋いでいない。
                let style = if *what == Pressed::Stop {
                    Style::Disabled
                } else {
                    Style::Key
                };
                if key(
                    ui,
                    scale,
                    rect,
                    id,
                    face.clone(),
                    style,
                    skin,
                    d(theme::ROUNDING),
                ) {
                    press(Some(*what));
                }
            }
        }
    }
    pressed
}

const DIGITS: [&str; 13] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12",
];

fn rocker(
    ui: &mut Ui,
    scale: f32,
    rect: Rect,
    label: &'static str,
    up: (Icon, Pressed),
    down: (Icon, Pressed),
    skin: &Skin,
) -> Option<Pressed> {
    let d = |x: f32| x * scale;
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), d(theme::ROCKER_KEY)));
    let middle = Rect::from_min_size(
        pos2(rect.left(), top.bottom()),
        vec2(rect.width(), d(theme::ROCKER_LABEL)),
    );
    let bottom = Rect::from_min_size(
        pos2(rect.left(), middle.bottom()),
        vec2(rect.width(), d(theme::ROCKER_KEY)),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, d(theme::ROUNDING), skin.key_fill);
    painter.rect_stroke(
        rect,
        d(theme::ROUNDING),
        Stroke::new(1.0, skin.key_edge),
        egui::StrokeKind::Inside,
    );
    painter.text(
        middle.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(d(theme::ROCKER_LABEL_SIZE)),
        skin.label,
    );
    let mut pressed = None;
    for (rect, (icon, what), rounding) in [
        (
            top,
            up,
            egui::CornerRadius {
                nw: d(theme::ROUNDING) as u8,
                ne: d(theme::ROUNDING) as u8,
                sw: 0,
                se: 0,
            },
        ),
        (
            bottom,
            down,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: d(theme::ROUNDING) as u8,
                se: d(theme::ROUNDING) as u8,
            },
        ),
    ] {
        let response = ui.interact(
            rect,
            ui.id().with(("リモコン", label, format!("{what:?}"))),
            Sense::click(),
        );
        let painter = ui.painter();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect.shrink(1.0), rounding, skin.key_pressed);
        } else if response.hovered() {
            painter.rect_filled(rect.shrink(1.0), rounding, skin.key_hover);
        }
        draw_icon(painter, icon, rect.center(), d(theme::ICON), skin.text);
        if response.clicked() {
            pressed = Some(what);
        }
    }
    pressed
}

fn cluster(ui: &mut Ui, scale: f32, area: Rect, skin: &Skin) -> Option<Pressed> {
    let d = |x: f32| x * scale;
    let mut pressed = None;
    let mut press = |what: Option<Pressed>| {
        if what.is_some() {
            pressed = what;
        }
    };
    let centre_x = area.center().x;
    let disc_top = area.top() + d(theme::CLUSTER_PAD) + d(theme::ROUND_KEY) + d(theme::CLUSTER_GAP);
    let disc = Rect::from_center_size(
        pos2(centre_x, disc_top + d(theme::DISC) / 2.0),
        vec2(d(theme::DISC), d(theme::DISC)),
    );
    let round = |centre: Pos2| {
        Rect::from_center_size(centre, vec2(d(theme::ROUND_KEY), d(theme::ROUND_KEY)))
    };
    let above = round(pos2(
        centre_x,
        area.top() + d(theme::CLUSTER_PAD) + d(theme::ROUND_KEY) / 2.0,
    ));
    let below = round(pos2(
        centre_x,
        disc.bottom() + d(theme::CLUSTER_GAP) + d(theme::ROUND_KEY) / 2.0,
    ));
    let before = round(pos2(
        disc.left() - d(theme::CLUSTER_GAP) - d(theme::ROUND_KEY) / 2.0,
        disc.center().y,
    ));
    let after = round(pos2(
        disc.right() + d(theme::CLUSTER_GAP) + d(theme::ROUND_KEY) / 2.0,
        disc.center().y,
    ));
    for (rect, icon, what, id) in [
        (above, Icon::DoubleUp, Pressed::PageUp, "ページ 上"),
        (below, Icon::DoubleDown, Pressed::PageDown, "ページ 下"),
        (before, Icon::DoubleLeft, Pressed::Back10, "10 秒戻す"),
        (after, Icon::DoubleRight, Pressed::Forward30, "30 秒送る"),
    ] {
        if key_round(ui, scale, rect, id, Face::Icon(icon), Style::Key, skin) {
            press(Some(what));
        }
    }
    {
        let painter = ui.painter();
        painter.circle_filled(disc.center(), d(theme::DISC) / 2.0, skin.disc);
        painter.circle_stroke(
            disc.center(),
            d(theme::DISC) / 2.0,
            Stroke::new(1.0, skin.key_edge),
        );
    }
    let inset = d(theme::ROUND_KEY) / 2.0 + d(8.0);
    for (centre, icon, what, id) in [
        (
            pos2(disc.center().x, disc.top() + inset),
            Icon::ChevUp,
            Pressed::Up,
            "上",
        ),
        (
            pos2(disc.center().x, disc.bottom() - inset),
            Icon::ChevDown,
            Pressed::Down,
            "下",
        ),
        (
            pos2(disc.left() + inset, disc.center().y),
            Icon::ChevLeft,
            Pressed::Left,
            "左",
        ),
        (
            pos2(disc.right() - inset, disc.center().y),
            Icon::ChevRight,
            Pressed::Right,
            "右",
        ),
    ] {
        if key_round(
            ui,
            scale,
            round(centre),
            id,
            Face::LargeIcon(icon),
            Style::Bare,
            skin,
        ) {
            press(Some(what));
        }
    }
    {
        let halo = d(theme::ENTER) / 2.0 + d(theme::ENTER_HALO);
        ui.painter()
            .circle_filled(disc.center(), halo, skin.background);
        let rect = Rect::from_center_size(disc.center(), vec2(d(theme::ENTER), d(theme::ENTER)));
        if key_round(
            ui,
            scale,
            rect,
            "決定",
            Face::Text("決定".into()),
            Style::Enter,
            skin,
        ) {
            press(Some(Pressed::Enter));
        }
    }
    {
        let rect = Rect::from_min_size(
            pos2(area.left(), below.center().y - d(theme::BACK_HEIGHT) / 2.0),
            vec2(d(theme::BACK_WIDTH), d(theme::BACK_HEIGHT)),
        );
        if key_sized(
            ui,
            scale,
            rect,
            "戻る",
            Face::Text("戻る".into()),
            Style::Key,
            skin,
            d(theme::BACK_HEIGHT) / 2.0,
            d(theme::BACK_SIZE),
        ) {
            press(Some(Pressed::Back));
        }
    }
    pressed
}

#[allow(clippy::too_many_arguments)]
fn key(
    ui: &mut Ui,
    scale: f32,
    rect: Rect,
    id: &str,
    face: Face,
    style: Style,
    skin: &Skin,
    rounding: f32,
) -> bool {
    key_sized(
        ui,
        scale,
        rect,
        id,
        face,
        style,
        skin,
        rounding,
        theme::KEY_SIZE * scale,
    )
}

fn key_round(
    ui: &mut Ui,
    scale: f32,
    rect: Rect,
    id: &str,
    face: Face,
    style: Style,
    skin: &Skin,
) -> bool {
    key_sized(
        ui,
        scale,
        rect,
        id,
        face,
        style,
        skin,
        rect.width() / 2.0,
        theme::KEY_SIZE * scale,
    )
}

#[allow(clippy::too_many_arguments)]
fn key_sized(
    ui: &mut Ui,
    scale: f32,
    rect: Rect,
    id: &str,
    face: Face,
    style: Style,
    skin: &Skin,
    rounding: f32,
    size: f32,
) -> bool {
    let d = |x: f32| x * scale;
    let sense = if style == Style::Disabled {
        Sense::hover()
    } else {
        Sense::click()
    };
    let response = ui.interact(rect, ui.id().with(("リモコン", id)), sense);
    let down = response.is_pointer_button_down_on();
    let hovered = response.hovered();
    let painter = ui.painter();
    let (fill, edge, text) = match style {
        Style::Key => (
            Some(if down {
                skin.key_pressed
            } else if hovered {
                skin.key_hover
            } else {
                skin.key_fill
            }),
            Some(skin.key_edge),
            skin.text,
        ),
        Style::Chosen => (
            Some(skin.chosen_fill),
            Some(skin.chosen_edge),
            skin.chosen_text,
        ),
        Style::Faint => (
            (down || hovered).then_some(skin.key_hover),
            Some(skin.key_edge),
            skin.label,
        ),
        Style::Enter if down => (
            Some(skin.chosen_fill),
            Some(skin.chosen_edge),
            skin.chosen_text,
        ),
        Style::Enter => (
            Some(if hovered {
                skin.key_pressed
            } else {
                skin.enter_fill
            }),
            Some(skin.enter_edge),
            skin.text,
        ),
        Style::Colour(colour) => (
            Some(if down || hovered {
                crate::ui::theme::lightened(colour, 1)
            } else {
                colour
            }),
            None,
            theme::COLOUR_TEXT,
        ),
        Style::Disabled => (Some(skin.key_fill), Some(skin.key_edge), skin.label),
        Style::Bare => (
            (down || hovered).then_some(if down {
                skin.key_pressed
            } else {
                skin.key_hover
            }),
            None,
            skin.text,
        ),
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, rounding, fill);
    }
    if let Some(edge) = edge {
        painter.rect_stroke(
            rect,
            rounding,
            Stroke::new(1.0, edge),
            egui::StrokeKind::Inside,
        );
    }
    let font = FontId::proportional(size);
    match face {
        Face::Text(label) => {
            painter.text(rect.center(), Align2::CENTER_CENTER, label, font, text);
        }
        Face::BoldThen(bold, label) => {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                bold,
                0.0,
                egui::TextFormat {
                    font_id: FontId::proportional(size * 1.25),
                    color: text,
                    valign: egui::Align::Center,
                    ..Default::default()
                },
            );
            job.append(
                label,
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: text,
                    valign: egui::Align::Center,
                    ..Default::default()
                },
            );
            let galley = painter.layout_job(job);
            painter.galley(rect.center() - galley.size() / 2.0, galley, text);
        }
        Face::Icon(icon) => draw_icon(painter, icon, rect.center(), d(theme::ICON), text),
        Face::LargeIcon(icon) => {
            draw_icon(painter, icon, rect.center(), d(theme::ICON_LARGE), text)
        }
        Face::IconThen(icon, label) => {
            let galley = painter.layout_no_wrap(label.to_owned(), font, text);
            let gap = d(6.0);
            let whole = d(theme::ICON) + gap + galley.size().x;
            let start = rect.center().x - whole / 2.0;
            draw_icon(
                painter,
                icon,
                pos2(start + d(theme::ICON) / 2.0, rect.center().y),
                d(theme::ICON),
                text,
            );
            painter.galley(
                pos2(
                    start + d(theme::ICON) + gap,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                text,
            );
        }
        Face::IconOver(icon, label) => {
            let small = FontId::proportional(d(theme::ROCKER_LABEL_SIZE));
            let galley = painter.layout_no_wrap(label.to_owned(), small, skin.label);
            let whole = d(theme::ICON) + d(2.0) + galley.size().y;
            let icon_y = rect.center().y - whole / 2.0 + d(theme::ICON) / 2.0;
            draw_icon(
                painter,
                icon,
                pos2(rect.center().x, icon_y),
                d(theme::ICON),
                text,
            );
            painter.galley(
                pos2(
                    rect.center().x - galley.size().x / 2.0,
                    icon_y + d(theme::ICON) / 2.0 + d(2.0),
                ),
                galley,
                skin.label,
            );
        }
    }
    response.clicked()
}

fn draw_icon(painter: &egui::Painter, icon: Icon, centre: Pos2, side: f32, colour: Color32) {
    let colour = if icon == Icon::Record {
        theme::RECORD
    } else {
        colour
    };
    icons::paint_at(painter, centre, side * theme::ICON_SCALE, icon, colour);
}

/// 四つまでは一行、それより多ければ二行に分けて同じ数ずつ。
fn band_grid(count: usize) -> (usize, usize) {
    let rows = count
        .div_ceil(theme::BANDS_PER_ROW)
        .clamp(1, theme::BAND_ROWS);
    (rows, count.div_ceil(rows).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_height_is_the_sum_of_the_rows() {
        let rows = [
            theme::BAND_HEIGHT * theme::BAND_ROWS as f32
                + theme::KEY_GAP * (theme::BAND_ROWS as f32 - 1.0),
            theme::KEY_HEIGHT * 4.0 + theme::KEY_GAP * 3.0,
            theme::ROCKER_KEY * 2.0 + theme::ROCKER_LABEL,
            theme::KEY_HEIGHT,
            theme::CLUSTER_PAD * 2.0
                + theme::ROUND_KEY * 2.0
                + theme::CLUSTER_GAP * 2.0
                + theme::DISC,
            theme::COLOUR_HEIGHT,
            theme::GROUP_PAD_Y * 2.0 + theme::KEY_HEIGHT * 3.0 + theme::KEY_GAP * 2.0,
        ];
        let sum: f32 = rows.iter().sum::<f32>()
            + theme::GAP * (rows.len() as f32 - 1.0)
            + theme::PAD_TOP
            + theme::PAD_TOP;
        assert!(
            (sum - theme::DESIGN_HEIGHT).abs() < 0.5,
            "{sum} vs {}",
            theme::DESIGN_HEIGHT
        );
    }

    #[test]
    fn the_three_in_the_middle_match_the_rockers() {
        assert_eq!(
            theme::KEY_HEIGHT * 3.0 + theme::KEY_GAP * 2.0,
            theme::ROCKER_KEY * 2.0 + theme::ROCKER_LABEL
        );
    }

    #[test]
    fn more_than_four_bands_take_a_second_row() {
        assert_eq!(band_grid(0), (1, 1));
        assert_eq!(band_grid(3), (1, 3));
        assert_eq!(band_grid(4), (1, 4));
        assert_eq!(band_grid(5), (2, 3));
        assert_eq!(band_grid(7), (2, 4));
        assert_eq!(band_grid(8), (2, 4));
    }

    #[test]
    fn a_number_carries_the_chosen_band() {
        let mut state = State::default();
        let (bs, cs) = (Band::of(0x0004), Band::of(0x0006));
        assert_eq!(state.band.unwrap_or(bs), bs);
        state.band = Some(cs);
        assert_eq!(
            Pressed::Number {
                band: state.band.unwrap_or(Band::of(0x7fe1)),
                number: 3
            },
            Pressed::Number {
                band: cs,
                number: 3
            }
        );
    }
}
