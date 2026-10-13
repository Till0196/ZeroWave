// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 字幕・文字スーパー（TS、ARIB STD-B24）を表示する。置き方は [`isdb::caption::Presenter`] が決め、
//! ここは画面の座標を映像の矩形に縮尺して置くだけ。

use std::time::Duration;

use egui::{Align2, Color32, FontId, Rect, Stroke, Ui, pos2, vec2};
use isdb::caption::{Color, DrcsGlyph, Layout, Text, is_drcs};

fn colour(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// 背景と縁取りの、放送の指定の上書き（`[caption]`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Broadcast,
    Always,
    Never,
}

impl Mode {
    fn parse(spelled: &str) -> Self {
        match spelled {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Broadcast,
        }
    }
}

/// 字幕と文字スーパーでフォントだけが違う。ARIB の字幕も TTML もこれでレンダリングする。
#[derive(Debug, Clone, Copy)]
pub struct Decor {
    pub superimpose: bool,
    pub background: Mode,
    pub opacity: f32,
    pub outline: Mode,
    /// 縁取りの太さ（文字の高さに対する比）。0 で放送のとおり。
    pub outline_width: f32,
}

impl Default for Decor {
    fn default() -> Self {
        Self {
            superimpose: false,
            background: Mode::Broadcast,
            opacity: crate::config::file::CAPTION_OPACITY as f32 / 100.0,
            outline: Mode::Broadcast,
            outline_width: 0.0,
        }
    }
}

impl Decor {
    pub fn of(caption: &crate::config::file::Caption, superimpose: bool) -> Self {
        Self {
            superimpose,
            background: Mode::parse(&caption.background),
            opacity: caption.opacity.min(100) as f32 / 100.0,
            outline: Mode::parse(&caption.outline),
            outline_width: caption.outline_width.min(50) as f32 / 100.0,
        }
    }

    pub fn font(&self, size: f32) -> FontId {
        if self.superimpose {
            crate::ui::text::font::superimpose(size)
        } else {
            crate::ui::text::font::caption(size)
        }
    }

    /// 透明度はいつも設定の不透明度で、放送に従うのは付けるかと色だけ。
    pub fn background(&self, broadcast: Color32) -> Option<Color32> {
        match self.background {
            Mode::Never => return None,
            Mode::Broadcast if broadcast.a() == 0 => return None,
            _ => {}
        }
        let alpha = 255.0 * self.opacity;
        let [r, g, b, _] = broadcast.to_srgba_unmultiplied();
        let (r, g, b) = if broadcast.a() == 0 {
            (0, 0, 0)
        } else {
            (r, g, b)
        };
        Some(Color32::from_rgba_unmultiplied(
            r,
            g,
            b,
            alpha.clamp(0.0, 255.0) as u8,
        ))
    }

    pub fn outline(&self, broadcast: Option<Color32>) -> Option<Color32> {
        match self.outline {
            Mode::Never => None,
            Mode::Broadcast => broadcast,
            Mode::Always => broadcast.or(Some(Color32::BLACK)),
        }
    }

    /// `size` は文字の高さ、`broadcast` は放送どおりの太さ。
    pub fn outline_radius(&self, size: f32, broadcast: f32) -> f32 {
        if self.outline_width <= 0.0 {
            broadcast
        } else {
            (size * self.outline_width).max(1.0)
        }
    }
}

/// 設定の画面の見本。放送が「背景は黒、縁取りは黒」を指定しているものとして表示する。
pub fn sample(painter: &egui::Painter, rect: Rect, decor: &Decor, size: f32) {
    let text = "［字幕の見本］　♪〜";
    let font = decor.font(size);
    let advances: Vec<f32> = text
        .chars()
        .map(|letter| {
            painter
                .layout_no_wrap(letter.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .collect();
    let width: f32 = advances.iter().sum();
    let pad = vec2(size * 0.6, size * 0.2);
    let line = Rect::from_center_size(rect.center(), vec2(width, size * 1.2));
    if let Some(ground) = decor.background(Color32::BLACK) {
        painter.rect_filled(line.expand2(pad), 0.0, ground);
    }
    let outline = decor.outline(Some(Color32::BLACK));
    let radius = decor.outline_radius(size, (size / 18.0).max(1.0));
    let mut x = line.left();
    for (letter, advance) in text.chars().zip(advances) {
        let center = pos2(x + advance / 2.0, line.center().y);
        if let Some(o) = outline {
            let mesh = glyph_mesh(painter, center, letter, &font, o, 1.0);
            for (radius, count) in [(radius, 16), (radius * 0.5, 8)] {
                for k in 0..count {
                    let angle = std::f32::consts::TAU * k as f32 / count as f32;
                    let mut copy = mesh.clone();
                    copy.translate(vec2(radius * angle.cos(), radius * angle.sin()));
                    painter.add(egui::Shape::mesh(copy));
                }
            }
        }
        glyph(painter, center, letter, &font, Color32::WHITE, 1.0);
        x += advance;
    }
}

pub fn animates(layout: &Layout) -> bool {
    layout.motion.is_some()
        || layout
            .texts
            .iter()
            .any(|t| t.flash != 0 || t.delay_ms != 0 || t.until_ms.is_some())
}

/// `elapsed` は文を受けてからの時間（点滅、待ち、動き）。
pub fn show(ui: &mut Ui, video: Rect, layout: &Layout, elapsed: Duration, decor: &Decor) {
    let (sw, sh) = layout.screen;
    if sw == 0 || sh == 0 {
        return;
    }
    let scale = (video.width() / sw as f32).min(video.height() / sh as f32);
    let painter = ui.painter().with_clip_rect(video);
    // 先に全部の文字をフォントのアトラスに載せる。後からアトラスが大きくなると、先に作ったメッシュの UV がずれる。
    for text in &layout.texts {
        let size = text.size.1 as f32 * scale;
        let _ = painter.layout_no_wrap(text.text.clone(), decor.font(size), Color32::WHITE);
    }
    let ms = elapsed.as_millis() as u32;
    let (dx, dy) = match &layout.motion {
        Some(m) => {
            let t = ms.saturating_sub(m.delay_ms);
            let f = if m.duration_ms == 0 {
                1.0
            } else if m.step_end {
                if t >= m.duration_ms { 1.0 } else { 0.0 }
            } else {
                (t as f32 / m.duration_ms as f32).min(1.0)
            };
            (
                m.from.0 as f32 + (m.to.0 - m.from.0) as f32 * f,
                m.from.1 as f32 + (m.to.1 - m.from.1) as f32 * f,
            )
        }
        None => (0.0, 0.0),
    };
    let at = move |x: i32, y: i32| {
        pos2(
            video.left() + (x as f32 + dx) * scale,
            video.top() + (y as f32 + dy) * scale,
        )
    };
    let blink_on = (ms / 500).is_multiple_of(2);
    // ラスタ色（RCS）：背景の設定に従い、放送で指定されない表示領域は塗らない。
    if layout.raster.a != 0
        && let Some(raster) = decor.background(colour(layout.raster))
    {
        let (ax, ay, aw, ah) = layout.area;
        painter.rect_filled(
            Rect::from_min_size(at(ax, ay), vec2(aw as f32 * scale, ah as f32 * scale)),
            0.0,
            raster,
        );
    }
    let shown: Vec<&Text> = layout
        .texts
        .iter()
        .filter(|text| !(ms < text.delay_ms || text.until_ms.is_some_and(|until| ms >= until)))
        .filter(|text| match text.flash {
            1 => blink_on,
            2 => !blink_on,
            _ => true,
        })
        .collect();
    // 局は次の行を「前の行の下端 − 1」に置くことがあり、半透明の背景が重なって濃い筋になるので、
    // 二画素以内の食い込みは下端まで下げてから塗る。
    let mut boxes: Vec<(i32, i32, i32, i32)> = shown
        .iter()
        .map(|text| {
            let count = text.text.chars().count() as i32;
            (
                text.x,
                text.y,
                text.x + text.cell.0 as i32 * count,
                text.y + text.cell.1 as i32,
            )
        })
        .collect();
    for i in 0..boxes.len() {
        for j in 0..boxes.len() {
            if i == j {
                continue;
            }
            let (l, t, r, _) = boxes[i];
            let (l2, t2, r2, b2) = boxes[j];
            if t2 < t && b2 > t && b2 - t <= 2 && l < r2 && l2 < r {
                boxes[i].1 = b2;
            }
        }
    }
    for (text, (l, t, r, b)) in shown.iter().zip(&boxes) {
        if r > l
            && b > t
            && let Some(bg) = decor.background(colour(text.bg))
        {
            painter.rect_filled(Rect::from_min_max(at(*l, *t), at(*r, *b)), 0.0, bg);
        }
    }
    for text in &shown {
        draw_text(&painter, text, scale, &at, &layout.drcs, decor);
    }
}

/// 一番近い大きさの字形を選び、枠に伸縮する。矩形を並べると egui の縁のぼかしで格子が
/// 見えるので、一枚の画像にしてレンダリングする。
pub(super) fn draw_drcs(
    painter: &egui::Painter,
    glyphs: &[DrcsGlyph],
    origin: egui::Pos2,
    size: (f32, f32),
    fg: Color32,
) {
    let Some(glyph) = glyphs
        .iter()
        .filter(|g| g.width > 0 && g.height > 0)
        .min_by_key(|g| (f32::from(g.width) - size.0).abs() as u32)
    else {
        return;
    };
    let texture = glyph_texture(painter.ctx(), glyph);
    painter.image(
        texture,
        Rect::from_min_size(origin, vec2(size.0, size.1)),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        fg,
    );
}

fn glyph_texture(ctx: &egui::Context, glyph: &DrcsGlyph) -> egui::TextureId {
    crate::ui::alpha_texture(
        ctx,
        usize::from(glyph.width),
        usize::from(glyph.height),
        glyph.levels,
        &glyph.bitmap,
    )
}

fn draw_text(
    painter: &egui::Painter,
    text: &Text,
    scale: f32,
    at: &dyn Fn(i32, i32) -> egui::Pos2,
    drcs: &std::collections::BTreeMap<char, Vec<DrcsGlyph>>,
    decor: &Decor,
) {
    let (cw, ch) = (text.cell.0 as f32 * scale, text.cell.1 as f32 * scale);
    let count = text.text.chars().count() as f32;
    let origin = at(text.x, text.y);
    if text.enclosure != 0 {
        let box_ = Rect::from_min_size(origin, vec2(cw * count, ch));
        let stroke = Stroke::new((2.0 * scale).max(1.0), colour(text.fg));
        if text.enclosure & 1 != 0 {
            painter.line_segment([box_.right_top(), box_.right_bottom()], stroke);
        }
        if text.enclosure & 2 != 0 {
            painter.line_segment([box_.left_bottom(), box_.right_bottom()], stroke);
        }
        if text.enclosure & 4 != 0 {
            painter.line_segment([box_.left_top(), box_.left_bottom()], stroke);
        }
        if text.enclosure & 8 != 0 {
            painter.line_segment([box_.left_top(), box_.right_top()], stroke);
        }
    }
    // 文字の大きさは枠の中の文字（SSM）の高さ。幅が違えば横に伸縮する。
    let size = text.size.1 as f32 * scale;
    let squeeze = text.size.0 as f32 / text.size.1.max(1) as f32;
    let font = decor.font(size);
    let fg = colour(text.fg);
    let outline = decor.outline(text.outline.map(colour));
    let radius = decor.outline_radius(size, (size / 18.0).max(1.0));
    let glyph_size = (text.size.0 as f32 * scale, text.size.1 as f32 * scale);
    let mut x = origin.x;
    for letter in text.text.chars() {
        let center = pos2(x + cw / 2.0, origin.y + ch / 2.0);
        // DRCS と外字のフォント（ARIB-TTML の SVG フォント）は字形の画素で。字形の無い DRCS は空けておく。
        if let Some(glyphs) = drcs.get(&letter) {
            let top_left = pos2(center.x - glyph_size.0 / 2.0, center.y - glyph_size.1 / 2.0);
            // 字形の画像は α のマップなので、色を替えてずらしたコピーを重ねればフォントの文字と同じ縁になる。
            if let Some(o) = outline {
                let d = radius;
                for (radius, count) in [(d, 16), (d * 0.5, 8)] {
                    for k in 0..count {
                        let angle = std::f32::consts::TAU * k as f32 / count as f32;
                        let shifted = pos2(
                            top_left.x + radius * angle.cos(),
                            top_left.y + radius * angle.sin(),
                        );
                        draw_drcs(painter, glyphs, shifted, glyph_size, o);
                    }
                }
            }
            draw_drcs(painter, glyphs, top_left, glyph_size, fg);
            x += cw;
            continue;
        }
        if is_drcs(letter) {
            x += cw;
            continue;
        }
        if let Some(o) = outline {
            // 縁取りは文字のメッシュをずらしたコピーを円周に沿って重ねる（四方だけだと斜めが欠ける）。
            let d = radius;
            let mesh = glyph_mesh(painter, center, letter, &font, o, squeeze);
            for (radius, count) in [(d, 16), (d * 0.5, 8)] {
                for k in 0..count {
                    let angle = std::f32::consts::TAU * k as f32 / count as f32;
                    let mut copy = mesh.clone();
                    copy.translate(vec2(radius * angle.cos(), radius * angle.sin()));
                    painter.add(egui::Shape::mesh(copy));
                }
            }
        }
        glyph(painter, center, letter, &font, fg, squeeze);
        x += cw;
    }
}

/// `squeeze` は横の伸縮（0.5 で半角）。egui は等方にしか伸縮できないので、
/// メッシュを作ってから頂点の x を潰す。
pub(super) fn glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    letter: char,
    font: &FontId,
    color: Color32,
    squeeze: f32,
) {
    if (squeeze - 1.0).abs() < 0.01 {
        let galley = painter.layout_no_wrap(letter.to_string(), font.clone(), color);
        let pos = seat(&galley, center, font.size, 1.0);
        painter.galley(pos, galley, color);
        return;
    }
    let mesh = glyph_mesh(painter, center, letter, font, color, squeeze);
    painter.add(egui::Shape::mesh(mesh));
}

/// 文字の枠の中心が `center` に来るように置く左上（ベースラインで据える）。
fn seat(galley: &egui::Galley, center: egui::Pos2, em: f32, squeeze: f32) -> egui::Pos2 {
    pos2(
        center.x - galley.size().x * squeeze / 2.0,
        crate::ui::seat_em(galley, center.y - em / 2.0, em),
    )
}

pub(super) fn glyph_mesh(
    painter: &egui::Painter,
    center: egui::Pos2,
    letter: char,
    font: &FontId,
    color: Color32,
    squeeze: f32,
) -> egui::epaint::Mesh {
    let galley = painter.layout_no_wrap(letter.to_string(), font.clone(), color);
    let pos = seat(&galley, center, font.size, squeeze);
    let shape = egui::epaint::TextShape::new(pos, galley, color);
    let atlas = painter.ctx().fonts(|f| f.font_image_size());
    let mut tessellator = egui::epaint::Tessellator::new(
        painter.pixels_per_point(),
        egui::epaint::TessellationOptions::default(),
        atlas,
        Vec::new(),
    );
    let mut mesh = egui::epaint::Mesh::default();
    tessellator.tessellate_text(&shape, &mut mesh);
    for vertex in &mut mesh.vertices {
        vertex.pos.x = pos.x + (vertex.pos.x - pos.x) * squeeze;
    }
    mesh
}

pub fn emergency(ui: &mut Ui, video: Rect, name: &str) {
    let height = (video.height() * 0.08).clamp(28.0, 72.0);
    let band = Rect::from_min_size(video.min, vec2(video.width(), height));
    let painter = ui.painter().with_clip_rect(band);
    painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(200, 0, 0, 230));
    let text = format!("緊急警報放送  {name}");
    painter.text(
        pos2(band.left() + height * 0.4, band.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(height * 0.6),
        Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decor(background: Mode, opacity: f32) -> Decor {
        Decor {
            background,
            opacity,
            ..Decor::default()
        }
    }

    #[test]
    fn the_broadcast_background_takes_the_set_opacity() {
        let grey = Color32::from_rgba_unmultiplied(10, 20, 30, 128);
        let shown = decor(Mode::Broadcast, 1.0).background(grey).unwrap();
        assert_eq!(shown.to_srgba_unmultiplied(), [10, 20, 30, 255]);
        let half = decor(Mode::Broadcast, 0.5).background(grey).unwrap();
        assert_eq!(half.to_srgba_unmultiplied()[3], 127);
        assert!(
            decor(Mode::Broadcast, 1.0)
                .background(Color32::TRANSPARENT)
                .is_none()
        );
    }

    #[test]
    fn always_and_never_override_the_broadcast() {
        let shown = decor(Mode::Always, 0.6)
            .background(Color32::TRANSPARENT)
            .unwrap();
        assert_eq!(shown.to_srgba_unmultiplied(), [0, 0, 0, 153]);
        let black = decor(Mode::Always, 1.0)
            .background(Color32::TRANSPARENT)
            .unwrap();
        assert_eq!(black.to_srgba_unmultiplied(), [0, 0, 0, 255]);
        assert!(decor(Mode::Never, 1.0).background(Color32::WHITE).is_none());
    }

    #[test]
    fn outlines_can_be_forced_and_thickened() {
        let forced = Decor {
            outline: Mode::Always,
            outline_width: 0.1,
            ..Decor::default()
        };
        assert_eq!(forced.outline(None), Some(Color32::BLACK));
        assert_eq!(forced.outline_radius(40.0, 2.0), 4.0);
        assert_eq!(Decor::default().outline_radius(40.0, 2.0), 2.0);
        let none = Decor {
            outline: Mode::Never,
            ..Decor::default()
        };
        assert_eq!(none.outline(Some(Color32::WHITE)), None);
    }
}
