// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MMT/TLV の字幕・文字スーパー（ARIB-TTML）を表示する。TS の字幕とは文字のレンダリングだけを共有する。
//! 重ねる順は TR-B39 8.5.6.7（背景 → 囲み → 影 → 縁取り → 文字）。

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, Rect, Stroke, Ui, pos2, vec2};
use isdb::ttml::{Edge, Motion, Rgba, Run};
pub use zwcore::captions::ttml::{Picture, Shown};

use super::caption::{glyph, glyph_mesh};

fn colour(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

struct Plane {
    left: f32,
    top: f32,
    scale: f32,
}

impl Plane {
    fn at(&self, x: i32, y: i32, motion: (i32, i32)) -> egui::Pos2 {
        pos2(
            self.left + (x + motion.0) as f32 * self.scale,
            self.top + (y + motion.1) as f32 * self.scale,
        )
    }

    fn rect(&self, r: isdb::ttml::Rect, motion: (i32, i32)) -> Rect {
        Rect::from_min_size(
            self.at(r.x, r.y, motion),
            vec2(r.w as f32 * self.scale, r.h as f32 * self.scale),
        )
    }
}

/// `video` は映像の矩形（字幕プレーンはこれに一致）。
pub fn show(ui: &mut Ui, video: Rect, shown: &Shown, decor: &super::caption::Decor) {
    let doc = &shown.document;
    let (sw, sh) = doc.screen;
    if sw == 0 || sh == 0 {
        return;
    }
    let plane = Plane {
        left: video.left(),
        top: video.top(),
        scale: (video.width() / sw as f32).min(video.height() / sh as f32),
    };
    let at = shown.now_ms();
    let doc_begin = doc.begin();
    let painter = ui.painter().with_clip_rect(video);
    // 先に全部の文字をフォントのアトラスに載せる。後からアトラスが大きくなると、先に作ったメッシュの UV がずれる。
    for p in doc.shown_at(at) {
        for run in &p.runs {
            let size = run.style.font.1 as f32 * plane.scale;
            let _ = painter.layout_no_wrap(run.text.clone(), decor.font(size), Color32::WHITE);
        }
    }
    for image in doc.images_at(at) {
        let Some(picture) = shown.pictures.get(&image.src) else {
            continue;
        };
        let texture = picture_texture(painter.ctx(), &image.src, picture);
        painter.image(
            texture,
            plane.rect(image.rect, (0, 0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    for p in doc.shown_at(at) {
        let motion = p
            .animation
            .as_ref()
            .map(|a| a.motion(at.saturating_sub(p.begin.unwrap_or(doc_begin))))
            .unwrap_or(STILL);
        let clip = match p.clip {
            Some(clip) => plane.rect(clip, (motion.dx, motion.dy)).intersect(video),
            None => video,
        };
        // 囲みは矩形の外側。はみ出しを隠すのは文字だけ。
        if let Some(index) = p.region
            && let Some(region) = doc.regions.get(index)
        {
            let rect = plane.rect(region.rect, (motion.dx, motion.dy));
            let bg = motion.bg.unwrap_or(region.bg).faded(motion.opacity);
            if bg.a != 0 {
                painter.rect_filled(rect, 0.0, colour(bg));
            }
            let sides = region.border.map(|e| {
                e.map(|e| {
                    Stroke::new(
                        (e.width as f32 * plane.scale).max(1.0),
                        colour(e.color.faded(motion.opacity)),
                    )
                })
            });
            draw_edges_outside(&painter, rect, &sides);
        }
        let painter = painter.with_clip_rect(clip);
        for run in &p.runs {
            if !isdb::ttml::between(at, run.begin, run.end) {
                continue;
            }
            // span の動きは領域の動きに重ねる（子が親より強い、TR-B39 8.5.6.12）。
            let own = run
                .animation
                .as_ref()
                .map(|a| a.motion(at.saturating_sub(run.begin.or(p.begin).unwrap_or(doc_begin))))
                .unwrap_or(STILL);
            let combined = Motion {
                dx: motion.dx + own.dx,
                dy: motion.dy + own.dy,
                color: own.color.or(motion.color),
                bg: own.bg.or(motion.bg),
                opacity: motion.opacity * own.opacity,
            };
            draw_run(&painter, &plane, run, &combined, &shown.gaiji, decor);
        }
    }
}

const STILL: Motion = Motion {
    dx: 0,
    dy: 0,
    color: None,
    bg: None,
    opacity: 1.0,
};

fn draw_run(
    painter: &egui::Painter,
    plane: &Plane,
    run: &Run,
    motion: &Motion,
    gaiji: &BTreeMap<char, Arc<isdb::svg::Outline>>,
    decor: &super::caption::Decor,
) {
    let style = &run.style;
    let shift = (motion.dx, motion.dy);
    let fade = |c: Rgba| colour(c.faded(motion.opacity));
    let fg = fade(motion.color.unwrap_or(style.fg));
    let bg = fade(motion.bg.unwrap_or(style.bg));
    let scale = plane.scale;
    let (cw, ch) = (run.cell.0 as f32 * scale, run.cell.1 as f32 * scale);
    let bounds = plane.rect(run.bounds(), shift);
    // 背景は文字表示枠を塗る（TR-B39 8.5.5.1）。
    if let Some(bg) = decor.background(bg) {
        painter.rect_filled(bounds, 0.0, bg);
    }
    let edge =
        |e: Option<Edge>| e.map(|e| Stroke::new((e.width as f32 * scale).max(1.0), fade(e.color)));
    let sides = style.border.map(edge);
    draw_edges_outside(painter, bounds, &sides);
    // 横書きは行の上端から `at` 下、縦書きは行の右端から `at` 左（TR-B39 8.5.6.5）。
    if style.underline {
        let (at, width) = style.underline_at;
        let width = (width as f32 * scale).max(1.0);
        let stroke = Stroke::new(width, fg);
        if run.vertical {
            let x = bounds.right() - at as f32 * scale - width / 2.0;
            painter.line_segment([pos2(x, bounds.top()), pos2(x, bounds.bottom())], stroke);
        } else {
            let y = bounds.top() + at as f32 * scale + width / 2.0;
            painter.line_segment([pos2(bounds.left(), y), pos2(bounds.right(), y)], stroke);
        }
    }
    // 文字は枠の中央に文字の高さでレイアウトし、幅は伸縮（半角は 0.5）。
    let size = style.font.1 as f32 * scale;
    let squeeze = style.font.0 as f32 / style.font.1.max(1) as f32;
    let font = decor.font(size);
    let glyph_size = (style.font.0 as f32 * scale, style.font.1 as f32 * scale);
    let edging = decor
        .outline(style.outline.map(|outline| fade(outline.color)))
        .map(|color| {
            let (width, blur) = style.outline.map_or(((size / 18.0).max(1.0), 0.0), |o| {
                (o.width as f32 * scale, o.blur as f32 * scale)
            });
            (color, decor.outline_radius(size, width), blur)
        });
    for (letter, x, y) in run.cells() {
        if letter == ' ' {
            continue;
        }
        let cell = Rect::from_min_size(plane.at(x, y, shift), vec2(cw, ch));
        let center = cell.center();
        let rotate = run.vertical && rotates_in_vertical(letter);
        if let Some(outline) = gaiji.get(&letter) {
            // 外字は輪郭をラスタライズした画像でレンダリングする（縁取りと影はずらしたコピーを重ねる）。
            let texture = outline_texture(painter.ctx(), outline, glyph_size);
            let top_left = pos2(center.x - glyph_size.0 / 2.0, center.y - glyph_size.1 / 2.0);
            let draw_at = |at: egui::Pos2, color: Color32| {
                painter.image(
                    texture,
                    Rect::from_min_size(at, vec2(glyph_size.0, glyph_size.1)),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    color,
                );
            };
            if let Some(shadow) = style.shadow {
                draw_at(
                    pos2(
                        top_left.x + shadow.dx as f32 * scale,
                        top_left.y + shadow.dy as f32 * scale,
                    ),
                    fade(shadow.color),
                );
            }
            if let Some((color, width, blur)) = edging {
                for (offset, alpha) in ring(width, blur) {
                    let c = color;
                    let c = Color32::from_rgba_unmultiplied(
                        c.r(),
                        c.g(),
                        c.b(),
                        (f32::from(c.a()) * alpha) as u8,
                    );
                    draw_at(pos2(top_left.x + offset.x, top_left.y + offset.y), c);
                }
            }
            draw_at(top_left, fg);
            continue;
        }
        let draw = |at: egui::Pos2, color: Color32| {
            if rotate || style.italic || style.bold {
                let mut mesh = glyph_mesh(painter, at, letter, &font, color, squeeze);
                if rotate {
                    rotate_mesh(&mut mesh, at);
                }
                if style.italic {
                    for v in &mut mesh.vertices {
                        v.pos.x += (at.y - v.pos.y) * 0.2;
                    }
                }
                if style.bold {
                    let mut copy = mesh.clone();
                    copy.translate(vec2((size / 24.0).max(1.0), 0.0));
                    painter.add(egui::Shape::mesh(copy));
                }
                painter.add(egui::Shape::mesh(mesh));
            } else {
                glyph(painter, at, letter, &font, color, squeeze);
            }
        };
        if let Some(shadow) = style.shadow {
            let at = pos2(
                center.x + shadow.dx as f32 * scale,
                center.y + shadow.dy as f32 * scale,
            );
            if shadow.blur > 0 {
                for (offset, alpha) in ring(shadow.blur as f32 * scale, 0.0) {
                    let c = fade(shadow.color);
                    let c = Color32::from_rgba_unmultiplied(
                        c.r(),
                        c.g(),
                        c.b(),
                        (f32::from(c.a()) * alpha * 0.5) as u8,
                    );
                    draw(pos2(at.x + offset.x, at.y + offset.y), c);
                }
            }
            draw(at, fade(shadow.color));
        }
        if let Some((color, width, blur)) = edging {
            for (offset, alpha) in ring(width, blur) {
                let c = color;
                let c = Color32::from_rgba_unmultiplied(
                    c.r(),
                    c.g(),
                    c.b(),
                    (f32::from(c.a()) * alpha) as u8,
                );
                draw(pos2(center.x + offset.x, center.y + offset.y), c);
            }
        }
        draw(center, fg);
    }
}

/// 太さの円周に 16 点、その内側に 8 点。ぼかしがあれば外に薄い輪を足す。
fn ring(width: f32, blur: f32) -> Vec<(egui::Vec2, f32)> {
    let width = width.max(1.0);
    let mut out = Vec::new();
    for (radius, count, alpha) in [(width, 16, 1.0), (width * 0.5, 8, 1.0)] {
        for k in 0..count {
            let angle = std::f32::consts::TAU * k as f32 / count as f32;
            out.push((vec2(radius * angle.cos(), radius * angle.sin()), alpha));
        }
    }
    if blur > 0.5 {
        for step in 1..=2 {
            let radius = width + blur * step as f32 / 2.0;
            let alpha = 0.5 / step as f32;
            for k in 0..16 {
                let angle = std::f32::consts::TAU * k as f32 / 16.0;
                out.push((vec2(radius * angle.cos(), radius * angle.sin()), alpha));
            }
        }
    }
    out
}

fn draw_edges_outside(painter: &egui::Painter, rect: Rect, sides: &[Option<Stroke>; 4]) {
    if let Some(s) = sides[0] {
        let y = rect.top() - s.width / 2.0;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], s);
    }
    if let Some(s) = sides[1] {
        let x = rect.right() + s.width / 2.0;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], s);
    }
    if let Some(s) = sides[2] {
        let y = rect.bottom() + s.width / 2.0;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], s);
    }
    if let Some(s) = sides[3] {
        let x = rect.left() - s.width / 2.0;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], s);
    }
}

/// 縦書きで向きを変える文字（TR-B39 表 8-4 と、長音・括弧）。
fn rotates_in_vertical(letter: char) -> bool {
    matches!(
        letter,
        '\u{2010}'
            | '\u{2015}'
            | '\u{2016}'
            | '\u{2025}'
            | '\u{2026}'
            | '\u{25B6}'
            | '\u{25C0}'
            | '\u{30FC}'
            | '\u{301C}'
            | '\u{FF5E}'
            | '\u{FF0D}'
            | '\u{FF1D}'
            | '（'
            | '）'
            | '「'
            | '」'
            | '『'
            | '』'
            | '［'
            | '］'
            | '｛'
            | '｝'
            | '〈'
            | '〉'
            | '《'
            | '》'
            | '【'
            | '】'
            | '〔'
            | '〕'
    )
}

fn rotate_mesh(mesh: &mut egui::epaint::Mesh, center: egui::Pos2) {
    for v in &mut mesh.vertices {
        let dx = v.pos.x - center.x;
        let dy = v.pos.y - center.y;
        v.pos = pos2(center.x - dy, center.y + dx);
    }
}

/// 輪郭と大きさごとに一度だけラスタライズする。
fn outline_texture(
    ctx: &egui::Context,
    outline: &Arc<isdb::svg::Outline>,
    size: (f32, f32),
) -> egui::TextureId {
    use std::collections::HashMap;
    use std::sync::Mutex;
    // キーは輪郭のアドレスなので、アドレスが使い回されないよう輪郭のコピーも持つ。
    type Drawn = HashMap<(usize, usize, usize), (Arc<isdb::svg::Outline>, Vec<u8>)>;
    static DRAWN: Mutex<Option<Drawn>> = Mutex::new(None);
    let width = (size.0.round() as usize).max(1);
    let height = (size.1.round() as usize).max(1);
    let key = (Arc::as_ptr(outline) as usize, width, height);
    let mut drawn = DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    let drawn = drawn.get_or_insert_with(HashMap::new);
    if drawn.len() > 512 {
        drawn.clear();
    }
    let (_, bitmap) = drawn
        .entry(key)
        .or_insert_with(|| (Arc::clone(outline), outline.rasterise(width, height)));
    crate::ui::alpha_texture(ctx, width, height, 255, bitmap)
}

/// キーは画像の `Arc` のアドレス（コピーも持って使い回しを防ぐ）。テクスチャの番号は egui のコンテキストの
/// 中でしか通じないので、コンテキストごとに持つ。
type PictureKept = std::collections::HashMap<usize, (Arc<Picture>, egui::TextureHandle)>;

/// 画像は `Arc` のアドレスで調べる（表示のたびに画素全部のハッシュを取らない）。
fn picture_texture(ctx: &egui::Context, name: &str, picture: &Arc<Picture>) -> egui::TextureId {
    let key = Arc::as_ptr(picture) as usize;
    let place = egui::Id::new("字幕の画像");
    if let Some(id) = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<PictureKept>(place)
            .get(&key)
            .map(|(_, handle)| handle.id())
    }) {
        return id;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied(picture.size(), &picture.rgba);
    let handle = ctx.load_texture(
        format!("ttml-{name}-{key:016x}"),
        image,
        egui::TextureOptions::LINEAR,
    );
    let id = handle.id();
    ctx.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<PictureKept>(place);
        if kept.len() > 30 {
            kept.clear();
        }
        kept.insert(key, (Arc::clone(picture), handle));
    });
    id
}
