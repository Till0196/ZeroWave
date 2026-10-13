// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フォントのサンプル画像。egui にフォントを足すとフォントアトラスを作り直して重いので、OS のフォントでレンダリングする。
//! レンダリングは別のスレッド一本で行い、画像はウィンドウ（egui のコンテキスト）ごとにキャッシュして使い回す。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use egui::{ColorImage, TextureHandle, TextureId, Vec2, vec2};
use zwsdk::host::fonts::Face;

/// キャッシュする画像の数。越えたら長く使っていない物から捨てる。
const KEEP: usize = 400;
const STALE: u64 = 3;
const FONTS_KEPT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Ask {
    path: PathBuf,
    index: u32,
    weight: u16,
    text: String,
    px: u32,
}

enum Slot {
    Asked,
    Drawn(ColorImage),
    Shown(TextureHandle),
    Failed,
}

struct Entry {
    slot: Slot,
    used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Ask, Entry>,
    frame: u64,
}

type Shared = Arc<Mutex<Cache>>;

struct Job {
    face: &'static Face,
    ask: Ask,
    cache: Weak<Mutex<Cache>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub texture: TextureId,
    pub size: Vec2,
}

fn cache_of(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(egui::Id::new("font-samples"), Shared::default)
            .clone()
    })
}

pub fn sample(ctx: &egui::Context, face: &'static Face, text: &str, size: f32) -> Option<Sample> {
    let scale = ctx.pixels_per_point();
    let ask = Ask {
        path: face.path.clone(),
        index: face.index,
        weight: face.weight,
        text: text.to_owned(),
        px: (size * scale).round().max(1.0) as u32,
    };
    let shared = cache_of(ctx);
    let mut cache = shared.lock().unwrap_or_else(|e| e.into_inner());
    let frame = ctx.cumulative_frame_nr();
    cache.frame = frame;
    let new = !cache.entries.contains_key(&ask);
    let entry = cache.entries.entry(ask.clone()).or_insert(Entry {
        slot: Slot::Asked,
        used: frame,
    });
    entry.used = frame;
    if matches!(entry.slot, Slot::Drawn(_))
        && let Slot::Drawn(image) = std::mem::replace(&mut entry.slot, Slot::Failed)
    {
        let texture = ctx.load_texture("font-sample", image, egui::TextureOptions::LINEAR);
        entry.slot = Slot::Shown(texture);
    }
    let shown = match &entry.slot {
        Slot::Shown(texture) => Some(Sample {
            texture: texture.id(),
            size: texture.size_vec2() / scale,
        }),
        Slot::Asked => {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            None
        }
        Slot::Drawn(_) | Slot::Failed => None,
    };
    if new {
        send(Job {
            face,
            ask,
            cache: Arc::downgrade(&shared),
        });
    }
    forget_old(&mut cache);
    shown
}

fn forget_old(cache: &mut Cache) {
    if cache.entries.len() <= KEEP {
        return;
    }
    let mut ages: Vec<u64> = cache.entries.values().map(|entry| entry.used).collect();
    ages.sort_unstable();
    let cut = ages[cache.entries.len() - KEEP];
    cache.entries.retain(|_, entry| entry.used >= cut);
}

fn send(job: Job) {
    static JOBS: OnceLock<Mutex<Option<Sender<Job>>>> = OnceLock::new();
    let jobs = JOBS.get_or_init(|| {
        let (to, from) = channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name(String::from("フォントの見本"))
            .spawn(move || {
                let mut fonts: Vec<((PathBuf, u32), zwsdk::draw::Fonts)> = Vec::new();
                for job in from {
                    work(&mut fonts, job);
                }
            });
        match spawned {
            Ok(_) => Mutex::new(Some(to)),
            Err(error) => {
                tracing::warn!(
                    "フォントのサンプルをレンダリングするスレッドを起動できない: {error}"
                );
                Mutex::new(None)
            }
        }
    });
    if let Some(to) = jobs.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = to.send(job);
    }
}

fn work(fonts: &mut Vec<((PathBuf, u32), zwsdk::draw::Fonts)>, job: Job) {
    let Some(shared) = job.cache.upgrade() else {
        return;
    };
    {
        let mut cache = shared.lock().unwrap_or_else(|e| e.into_inner());
        let frame = cache.frame;
        let Some(entry) = cache.entries.get(&job.ask) else {
            return;
        };
        if entry.used + STALE < frame {
            cache.entries.remove(&job.ask);
            return;
        }
    }
    let mark = (job.ask.path.clone(), job.ask.index);
    let at = match fonts.iter().position(|(had, _)| *had == mark) {
        Some(at) => at,
        None => {
            let Some(bytes) = job.face.bytes() else {
                put(&shared, &job.ask, Slot::Failed);
                return;
            };
            let faces = zwsdk::fonts::Faces {
                sans: Some(bytes),
                mono: None,
            };
            if fonts.len() >= FONTS_KEPT {
                fonts.remove(0);
            }
            fonts.push((
                mark,
                zwsdk::draw::Fonts::new(zwsdk::draw::font_context(&faces)),
            ));
            fonts.len() - 1
        }
    };
    let drawn = draw(&mut fonts[at].1, job.ask.weight, &job.ask.text, job.ask.px);
    put(&shared, &job.ask, drawn.map_or(Slot::Failed, Slot::Drawn));
}

fn put(shared: &Shared, ask: &Ask, slot: Slot) {
    let mut cache = shared.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.entries.get_mut(ask) {
        entry.slot = slot;
    }
}

fn draw(fonts: &mut zwsdk::draw::Fonts, weight: u16, text: &str, px: u32) -> Option<ColorImage> {
    let font = format!("{weight} {px}px sans-serif");
    let mut probe = zwsdk::draw::Canvas::new(1.0, 1.0);
    probe.state.font = font.clone();
    let line = probe.measure_text(fonts, text);
    let width = (line.width.ceil() as u32 + 2).clamp(1, 4096);
    let height = ((line.ascent + line.descent).ceil() as u32 + 2).clamp(1, 512);
    let mut canvas = zwsdk::draw::Canvas::new(f64::from(width), f64::from(height));
    canvas.state.font = font;
    canvas.state.fill = zwsdk::draw::colour("#ffffff");
    canvas.fill_text(fonts, text, 1.0, f64::from(line.ascent) + 1.0);
    let rgba = zwsdk::draw::render(
        &canvas.scene_for(f64::from(width), f64::from(height)),
        width,
        height,
    );
    if rgba.len() != (width * height * 4) as usize {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

pub fn paint_left(painter: &egui::Painter, rect: egui::Rect, sample: Sample, tint: egui::Color32) {
    let at = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - sample.size.y / 2.0),
        sample.size,
    );
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .image(
            sample.texture,
            at,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
}

pub fn paint_centred(
    painter: &egui::Painter,
    rect: egui::Rect,
    sample: Sample,
    tint: egui::Color32,
) {
    let fit = (rect.width() / sample.size.x)
        .min(rect.height() / sample.size.y)
        .min(1.0);
    let size = vec2(sample.size.x * fit, sample.size.y * fit);
    painter.image(
        sample.texture,
        egui::Rect::from_center_size(rect.center(), size),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );
}
