// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! egui の出力を CPU でラスタライズする。三角形ごとに重心座標で色とテクスチャの座標を補間し、
//! 乗算済みの「上に重ねる」で書く。縁のぼかしは egui が頂点のアルファで作っている。

use std::collections::HashMap;
use std::sync::Arc;

use egui::epaint::textures::TexturesDelta;
use egui::epaint::{
    ClippedPrimitive, ColorImage, ImageData, ImageDelta, Primitive, TextureId, Vertex,
};

/// egui が送ってきたテクスチャのコピー。GL のコンテキストが作り直されたら全部をアップロードし直すのにも使う
/// （egui は差分しか送らない）。
#[derive(Debug, Default)]
pub struct Textures {
    kept: HashMap<TextureId, (Arc<ColorImage>, egui::TextureOptions)>,
}

impl Textures {
    pub fn set(&mut self, delta: &TexturesDelta) {
        for (id, all) in &delta.set {
            for change in all {
                let ImageData::Color(image) = &change.image;
                match change.pos {
                    Some([x0, y0]) => {
                        let Some((kept, _)) = self.kept.get_mut(id) else {
                            continue;
                        };
                        let kept = Arc::make_mut(kept);
                        let [width, height] = image.size;
                        let stride = kept.size[0];
                        for y in 0..height {
                            let to = (y0 + y) * stride + x0;
                            let from = y * width;
                            if let (Some(target), Some(source)) = (
                                kept.pixels.get_mut(to..to + width),
                                image.pixels.get(from..from + width),
                            ) {
                                target.copy_from_slice(source);
                            }
                        }
                    }
                    None => {
                        self.kept.insert(*id, (Arc::clone(image), change.options));
                    }
                }
            }
        }
    }

    pub fn free(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.kept.remove(id);
        }
    }

    pub fn whole(&self) -> TexturesDelta {
        let mut delta = TexturesDelta::default();
        for (id, (image, options)) in &self.kept {
            delta.set.entry(*id).or_default().push(ImageDelta::full(
                ImageData::Color(Arc::clone(image)),
                *options,
            ));
        }
        delta
    }
}

#[derive(Debug, Default)]
pub struct Soft;

impl Soft {
    /// `out`（乗算済み RGBA、上から下）を透明にしてから塗る。
    pub fn paint(
        textures: &Textures,
        primitives: &[ClippedPrimitive],
        pixels_per_point: f32,
        width: usize,
        height: usize,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        out.resize(width * height * 4, 0);
        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let Some((image, options)) = textures.kept.get(&mesh.texture_id) else {
                continue;
            };
            let texture = Texture {
                image,
                linear: options.magnification == egui::TextureFilter::Linear,
            };
            let clip = primitive.clip_rect;
            let clip = (
                (clip.min.x * pixels_per_point).round().max(0.0) as usize,
                (clip.min.y * pixels_per_point).round().max(0.0) as usize,
                ((clip.max.x * pixels_per_point).round().max(0.0) as usize).min(width),
                ((clip.max.y * pixels_per_point).round().max(0.0) as usize).min(height),
            );
            if clip.0 >= clip.2 || clip.1 >= clip.3 {
                continue;
            }
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let corner = |at: u32| mesh.vertices.get(at as usize).copied();
                let (Some(a), Some(b), Some(c)) = (
                    corner(triangle[0]),
                    corner(triangle[1]),
                    corner(triangle[2]),
                ) else {
                    continue;
                };
                fill(
                    [a, b, c],
                    pixels_per_point,
                    &texture,
                    clip,
                    width,
                    out.as_mut_slice(),
                );
            }
        }
    }
}

struct Texture<'a> {
    image: &'a ColorImage,
    linear: bool,
}

/// 一つの三角形を塗る。重みは x について一次なので、行ごとに中に入る範囲だけを足し算で進む。
fn fill(
    corners: [Vertex; 3],
    scale: f32,
    texture: &Texture<'_>,
    clip: (usize, usize, usize, usize),
    width: usize,
    out: &mut [u8],
) {
    let p = corners.map(|v| (v.pos.x * scale, v.pos.y * scale));
    // 符号付きの面積。向きは問わない（egui はどちら回りも出す）。
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < f32::EPSILON {
        return;
    }
    let left = p
        .iter()
        .map(|q| q.0)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip.0 as f32);
    let right = p
        .iter()
        .map(|q| q.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip.2 as f32);
    let top = p
        .iter()
        .map(|q| q.1)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip.1 as f32);
    let bottom = p
        .iter()
        .map(|q| q.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip.3 as f32);
    if left >= right || top >= bottom {
        return;
    }
    let inverse = 1.0 / area;
    let weight = |from: (f32, f32), to: (f32, f32)| {
        let a = -(to.1 - from.1) * inverse;
        let b = (to.0 - from.0) * inverse;
        let c = ((to.1 - from.1) * from.0 - (to.0 - from.0) * from.1) * inverse;
        (a, b, c)
    };
    let weights = [weight(p[1], p[2]), weight(p[2], p[0]), weight(p[0], p[1])];
    let colour = corners.map(|v| v.color.to_array().map(f32::from));
    let uv = corners.map(|v| (v.uv.x, v.uv.y));
    let flat_uv = uv[0] == uv[1] && uv[1] == uv[2];
    let flat_colour = colour[0] == colour[1] && colour[1] == colour[2];
    let constant = (flat_uv && flat_colour).then(|| {
        let texel = sample(texture, uv[0].0, uv[0].1);
        std::array::from_fn::<f32, 4, _>(|k| texel[k] * colour[0][k] / 255.0)
    });
    let flat_texel = flat_uv.then(|| sample(texture, uv[0].0, uv[0].1));
    if constant.is_some_and(|source| source[3] <= 0.0 && source[..3].iter().all(|v| *v <= 0.0)) {
        return;
    }
    // 端の画素を二つの三角形で二度塗らないよう、ほんの少し内側だけを中とする。
    const INSIDE: f32 = -1e-5;
    for y in top as usize..bottom as usize {
        let cy = y as f32 + 0.5;
        let (mut from, mut to) = (left, right - 1.0);
        for (a, b, c) in weights {
            let base = b * cy + c;
            if a.abs() < 1e-12 {
                if base < INSIDE {
                    from = f32::INFINITY;
                }
                continue;
            }
            let at = (INSIDE - base) / a - 0.5;
            if a > 0.0 {
                from = from.max(at.ceil());
            } else {
                to = to.min(at.floor());
            }
        }
        if from > to {
            continue;
        }
        let (from, to) = (from as usize, to as usize);
        let row = y * width * 4;
        for x in from..=to {
            let source = match constant {
                Some(source) => source,
                None => {
                    let cx = x as f32 + 0.5;
                    let w = weights.map(|(a, b, c)| a * cx + b * cy + c);
                    let texel = match flat_texel {
                        Some(texel) => texel,
                        None => sample(
                            texture,
                            w[0] * uv[0].0 + w[1] * uv[1].0 + w[2] * uv[2].0,
                            w[0] * uv[0].1 + w[1] * uv[1].1 + w[2] * uv[2].1,
                        ),
                    };
                    std::array::from_fn(|k| {
                        let vertex = if flat_colour {
                            colour[0][k]
                        } else {
                            w[0] * colour[0][k] + w[1] * colour[1][k] + w[2] * colour[2][k]
                        };
                        texel[k] * vertex / 255.0
                    })
                }
            };
            if source[3] <= 0.0 && source[0] <= 0.0 && source[1] <= 0.0 && source[2] <= 0.0 {
                continue;
            }
            let at = row + x * 4;
            let Some(target) = out.get_mut(at..at + 4) else {
                continue;
            };
            let keep = 1.0 - source[3] / 255.0;
            for k in 0..4 {
                let mixed = source[k] + f32::from(target[k]) * keep;
                target[k] = (mixed + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn sample(texture: &Texture<'_>, u: f32, v: f32) -> [f32; 4] {
    let [w, h] = texture.image.size;
    if w == 0 || h == 0 {
        return [255.0; 4];
    }
    let pixels = &texture.image.pixels;
    let at = |x: usize, y: usize| {
        pixels[y.min(h - 1) * w + x.min(w - 1)]
            .to_array()
            .map(f32::from)
    };
    let x = (u * w as f32 - 0.5).max(0.0);
    let y = (v * h as f32 - 0.5).max(0.0);
    if !texture.linear {
        return at((x + 0.5) as usize, (y + 0.5) as usize);
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = (x.fract(), y.fract());
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    let mut out = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let low = c[k] + (d[k] - c[k]) * fx;
        out[k] = top + (low - top) * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_white_rectangle_comes_out_white_and_only_there() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::background());
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 10.0)),
                0.0,
                egui::Color32::WHITE,
            );
        });
        let primitives = context.tessellate(output.shapes, 1.0);
        let mut textures = Textures::default();
        textures.set(&output.textures_delta);
        output.textures_delta.clear();
        let mut out = Vec::new();
        Soft::paint(&textures, &primitives, 1.0, 64, 40, &mut out);
        let at = |x: usize, y: usize| &out[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(15, 15), &[255, 255, 255, 255]);
        assert_eq!(at(5, 5), &[0, 0, 0, 0]);
        assert_eq!(at(40, 15), &[0, 0, 0, 0]);
    }

    #[test]
    fn text_puts_something_on_the_picture() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::background());
            painter.text(
                egui::pos2(2.0, 2.0),
                egui::Align2::LEFT_TOP,
                "ZeroWave",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
        });
        let primitives = context.tessellate(output.shapes, 1.0);
        let mut textures = Textures::default();
        textures.set(&output.textures_delta);
        output.textures_delta.clear();
        let mut out = Vec::new();
        Soft::paint(&textures, &primitives, 1.0, 120, 30, &mut out);
        let lit = out.as_chunks::<4>().0.iter().filter(|p| p[3] > 128).count();
        assert!(lit > 50, "文字の画素が {lit} しかない");
    }
}
