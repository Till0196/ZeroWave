// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 設定ウィンドウを画面なしでレンダリングして、ページごとに PNG にする（`SNAP_DIR`）。切れた文字列・重なった文字列も数える。
//! `SNAP_DIR=… cargo test --profile quick every_page_as_a_picture -- --ignored --nocapture`。
//! 設定は `SNAP_CONFIG`（`zerowave.toml`）があればそれを、無ければ既定。

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, TextureId, vec2};

use super::{File, PAGES, Page, State, skin};
use crate::ui::theme::Mode;

const SCALE: f32 = 1.5;

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

fn shoot(
    size: (f32, f32),
    mode: Mode,
    mut build: impl FnMut(&mut egui::Ui),
) -> (Vec<u8>, usize, usize, Vec<String>) {
    let context = egui::Context::default();
    crate::ui::text::font::install_or_say(&context);
    crate::ui::paper(&context, mode);
    context.set_pixels_per_point(SCALE);
    let input = || egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1))),
        ..egui::RawInput::default()
    };
    let mut textures: HashMap<TextureId, Texture> = HashMap::new();
    let mut output = None;
    // フォントは次のフレームから効き、中身の高さは前のフレームで測るので、何フレームか実行する。
    for _ in 0..4 {
        let mut full = context.run_ui(input(), &mut build);
        let deltas = full
            .textures_delta
            .set
            .iter()
            .flat_map(|(id, deltas)| deltas.iter().map(move |delta| (id, delta)));
        for (id, delta) in deltas {
            let egui::ImageData::Color(image) = &delta.image;
            let [width, height] = image.size;
            match delta.pos {
                None => {
                    textures.insert(
                        *id,
                        Texture {
                            width,
                            height,
                            pixels: image.pixels.clone(),
                        },
                    );
                }
                Some([x, y]) => {
                    if let Some(texture) = textures.get_mut(id) {
                        for row in 0..height {
                            for column in 0..width {
                                let at = (y + row) * texture.width + x + column;
                                if at < texture.pixels.len() {
                                    texture.pixels[at] = image.pixels[row * width + column];
                                }
                            }
                        }
                    }
                }
            }
        }
        // 写し終えた（片付けないと egui が手放すときに咎める）。
        full.textures_delta.clear();
        output = Some(full);
    }
    let output = output.expect("一フレーム以上実行した");
    let problems = problems(&output.shapes, size);
    let primitives = context.tessellate(output.shapes, SCALE);
    let width = (size.0 * SCALE).round() as usize;
    let height = (size.1 * SCALE).round() as usize;
    let mut canvas = vec![[0.0_f32; 4]; width * height];
    for primitive in &primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            continue;
        };
        let clip = primitive.clip_rect;
        let texture = textures.get(&mesh.texture_id);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] =
                [triangle[0], triangle[1], triangle[2]].map(|index| &mesh.vertices[index as usize]);
            fill(&mut canvas, width, height, clip, [a, b, c], texture);
        }
    }
    let mut bytes = Vec::with_capacity(width * height * 4);
    for pixel in canvas {
        let alpha = pixel[3].clamp(0.0, 1.0);
        for channel in &pixel[..3] {
            let straight = if alpha > 0.0 { channel / alpha } else { 0.0 };
            bytes.push((straight.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        bytes.push((alpha * 255.0).round() as u8);
    }
    (bytes, width, height, problems)
}

fn fill(
    canvas: &mut [[f32; 4]],
    width: usize,
    height: usize,
    clip: Rect,
    vertices: [&egui::epaint::Vertex; 3],
    texture: Option<&Texture>,
) {
    let points = vertices.map(|vertex| vertex.pos * SCALE);
    let area = edge(points[0], points[1], points[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let left = points
        .iter()
        .map(|p| p.x)
        .fold(f32::MAX, f32::min)
        .max(clip.left() * SCALE);
    let right = points
        .iter()
        .map(|p| p.x)
        .fold(f32::MIN, f32::max)
        .min(clip.right() * SCALE);
    let top = points
        .iter()
        .map(|p| p.y)
        .fold(f32::MAX, f32::min)
        .max(clip.top() * SCALE);
    let bottom = points
        .iter()
        .map(|p| p.y)
        .fold(f32::MIN, f32::max)
        .min(clip.bottom() * SCALE);
    if left >= right || top >= bottom {
        return;
    }
    let x0 = left.floor().max(0.0) as usize;
    let x1 = (right.ceil() as usize).min(width);
    let y0 = top.floor().max(0.0) as usize;
    let y1 = (bottom.ceil() as usize).min(height);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            if p.x < left || p.x > right || p.y < top || p.y > bottom {
                continue;
            }
            let w0 = edge(points[1], points[2], p) / area;
            let w1 = edge(points[2], points[0], p) / area;
            let w2 = edge(points[0], points[1], p) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let weights = [w0, w1, w2];
            let mut colour = [0.0_f32; 4];
            let mut uv = Pos2::ZERO;
            for (vertex, weight) in vertices.iter().zip(weights) {
                let [r, g, b, a] = vertex.color.to_array();
                colour[0] += f32::from(r) / 255.0 * weight;
                colour[1] += f32::from(g) / 255.0 * weight;
                colour[2] += f32::from(b) / 255.0 * weight;
                colour[3] += f32::from(a) / 255.0 * weight;
                uv += vertex.uv.to_vec2() * weight;
            }
            if let Some(texture) = texture {
                let texel = sample(texture, uv);
                for (channel, value) in colour.iter_mut().zip(texel) {
                    *channel *= value;
                }
            }
            let under = &mut canvas[y * width + x];
            let keep = 1.0 - colour[3];
            for channel in 0..4 {
                under[channel] = colour[channel] + under[channel] * keep;
            }
        }
    }
}

fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn sample(texture: &Texture, uv: Pos2) -> [f32; 4] {
    let x = (uv.x * texture.width as f32 - 0.5).clamp(0.0, texture.width as f32 - 1.0);
    let y = (uv.y * texture.height as f32 - 0.5).clamp(0.0, texture.height as f32 - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = (
        (x0 + 1).min(texture.width - 1),
        (y0 + 1).min(texture.height - 1),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| texture.pixels[y * texture.width + x].to_array();
    let mut out = [0.0; 4];
    for (channel, slot) in out.iter_mut().enumerate() {
        let top = f32::from(at(x0, y0)[channel]) * (1.0 - fx) + f32::from(at(x1, y0)[channel]) * fx;
        let bottom =
            f32::from(at(x0, y1)[channel]) * (1.0 - fx) + f32::from(at(x1, y1)[channel]) * fx;
        *slot = (top * (1.0 - fy) + bottom * fy) / 255.0;
    }
    out
}

fn problems(shapes: &[egui::epaint::ClippedShape], size: (f32, f32)) -> Vec<String> {
    let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
    let mut texts: Vec<(Rect, Rect, String)> = Vec::new();
    for clipped in shapes {
        collect(&clipped.shape, clipped.clip_rect, &mut texts);
    }
    let mut said = Vec::new();
    for (rect, clip, text) in &texts {
        let visible = clip.intersect(window);
        if rect.right() > visible.right() + 1.0 && rect.left() < visible.right() {
            said.push(format!(
                "右端がはみ出す（{:.0} > {:.0}）: {text}",
                rect.right(),
                visible.right()
            ));
        } else if rect.left() < visible.left() - 1.0 && rect.right() > visible.left() {
            said.push(format!("左端がはみ出す: {text}"));
        }
    }
    for (index, (one, clip_one, text_one)) in texts.iter().enumerate() {
        for (other, clip_other, text_other) in &texts[index + 1..] {
            let shown_one = one.intersect(*clip_one);
            let shown_other = other.intersect(*clip_other);
            let both = shown_one.intersect(shown_other);
            if both.width() > 2.0 && both.height() > 3.0 {
                said.push(format!("重なる: 「{text_one}」と「{text_other}」"));
            }
        }
    }
    said
}

fn collect(shape: &egui::Shape, clip: Rect, into: &mut Vec<(Rect, Rect, String)>) {
    match shape {
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect(shape, clip, into);
            }
        }
        egui::Shape::Text(text) => {
            let shown = text.galley.text().trim();
            if shown.is_empty() || text.galley.elided {
                return;
            }
            let rect = text.galley.rect.translate(text.pos.to_vec2());
            into.push((rect, clip, shown.chars().take(40).collect()));
        }
        _ => {}
    }
}

fn write_png(path: &std::path::Path, bytes: &[u8], width: usize, height: usize) {
    let file = std::fs::File::create(path).expect("書ける");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(bytes))
        .expect("PNG にできる");
}

fn config() -> File {
    std::env::var_os("SNAP_CONFIG")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| File::parse(&text).ok())
        .unwrap_or_default()
}

#[test]
#[ignore = "画像を書き出すだけ。SNAP_DIR を指定して実行する"]
fn every_page_as_a_picture() {
    let folder = std::path::PathBuf::from(std::env::var_os("SNAP_DIR").expect("SNAP_DIR"));
    std::fs::create_dir_all(&folder).expect("作れる");
    let sizes: Vec<(f32, f32)> = std::env::var("SNAP_SIZES")
        .ok()
        .map(|said| {
            said.split(',')
                .filter_map(|one| {
                    let (w, h) = one.trim().split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
                .collect()
        })
        .filter(|sizes: &Vec<(f32, f32)>| !sizes.is_empty())
        .unwrap_or_else(|| vec![(skin::WIDTH, skin::HEIGHT - skin::BAR_HEIGHT)]);
    let loaded = config();
    let guide = zwcore::guide::Guide::new();
    let pages: Vec<Page> = PAGES
        .iter()
        .flat_map(|(_, pages)| pages.iter().copied())
        .collect();
    let mut total = 0;
    for &(width, height) in &sizes {
        for (mode, look, name) in [
            (Mode::Dark, &crate::ui::theme::settings::DARK, "dark"),
            (Mode::Light, &crate::ui::theme::settings::LIGHT, "light"),
        ] {
            for page in &pages {
                let mut state = State {
                    page: *page,
                    ..State::default()
                };
                let mut edited = loaded.clone();
                let (bytes, w, h, problems) = shoot((width, height), mode, |ui| {
                    let _ = super::show(ui, &mut state, &mut edited, &loaded, &guide, &[], look);
                });
                let file = folder.join(format!(
                    "{page:?}-{name}-{}x{}.png",
                    width as u32, height as u32
                ));
                write_png(&file, &bytes, w, h);
                if name == "dark" {
                    for problem in &problems {
                        println!("{page:?}（{width}x{height}）: {problem}");
                    }
                    total += problems.len();
                }
            }
        }
    }
    println!("問題 {total} 件");
}
