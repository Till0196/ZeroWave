// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面のレンダリング。映像の上に重ねる。

pub mod face;
pub mod grid;
pub mod guide;
pub mod icons;
pub mod overlay;
pub mod panes;
pub mod text;
pub mod theme;

pub use face::Face;
pub use overlay::status::Status;

/// egui の既定の行送りはデザインより広く、番組表の枠に入る行数が変わるので指定する。
pub fn laid(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    colour: egui::Color32,
    width: f32,
    leading: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color: colour,
            line_height: Some(leading),
            ..Default::default()
        },
    );
    job.wrap.max_width = width;
    painter.layout_job(job)
}

pub fn one_line(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    colour: egui::Color32,
    width: f32,
    spacing: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: font,
            color: colour,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.layout_job(job)
}

#[cfg(test)]
pub fn a_programme(
    start: Option<zwcore::guide::Moment>,
    seconds: Option<u32>,
) -> zwcore::guide::Programme {
    zwcore::guide::Programme {
        title: String::from("題"),
        ..zwcore::guide::Programme::scheduled(
            zwcore::guide::Key {
                service: 1,
                network: 1,
            },
            1,
            start,
            seconds,
        )
    }
}

/// 本体の画像（RGBA）を egui の形に。コピーが一回なので、画像が替わったときだけ呼ぶ。
pub fn rgba_image(picture: &zwcore::image::Rgba) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(picture.size(), &picture.rgba)
}

/// 局のラベル。番号・マーク・名前を一列に並べて中央に置く。背景や線は呼ぶ側が塗る。
#[derive(Debug, Clone, Copy)]
pub struct Plate {
    pub key_size: f32,
    pub key_colour: egui::Color32,
    pub badge: egui::Vec2,
    pub badge_size: f32,
    pub name_size: f32,
    pub gap: f32,
    pub margin: f32,
}

pub fn nameplate(
    painter: &egui::Painter,
    rect: egui::Rect,
    channel: &guide::epg::Channel,
    plate: &Plate,
    text: egui::Color32,
) {
    let key = channel.remote_key.map(|key| {
        painter.layout_no_wrap(
            key.to_string(),
            egui::FontId::proportional(plate.key_size),
            plate.key_colour,
        )
    });
    let key_room = key.as_ref().map_or(0.0, |key| key.size().x + plate.gap);
    let name = one_line(
        painter,
        &channel.name,
        egui::FontId::proportional(plate.name_size),
        text,
        (rect.width() - plate.margin - key_room - plate.badge.x - plate.gap).max(20.0),
        0.0,
    );
    let together = key_room + plate.badge.x + plate.gap + name.size().x;
    let mut left = rect.center().x - together / 2.0;
    if let Some(key) = key {
        painter.galley(
            egui::pos2(left, rect.center().y - key.size().y / 2.0),
            key,
            plate.key_colour,
        );
        left += key_room;
    }
    let badge = egui::Rect::from_min_size(
        egui::pos2(left, rect.center().y - plate.badge.y / 2.0),
        plate.badge,
    );
    logo(painter, badge, channel, plate.badge_size);
    painter.galley(
        egui::pos2(
            badge.right() + plate.gap,
            rect.center().y - name.size().y / 2.0,
        ),
        name,
        text,
    );
}

type LogoKept = std::collections::HashMap<
    usize,
    (
        std::sync::Arc<zwcore::guide::Logo>,
        Option<egui::TextureHandle>,
        u64,
    ),
>;

const LOGO_MOST: usize = 256;
const LOGO_IDLE: u64 = 600;

/// 局ロゴのテクスチャ。キーは `Arc` の指す先。デコードできない PNG は無し。
pub fn logo_texture(
    ctx: &egui::Context,
    logo: &std::sync::Arc<zwcore::guide::Logo>,
) -> Option<egui::TextureHandle> {
    let place = egui::Id::new("局ロゴ");
    let key = std::sync::Arc::as_ptr(logo) as usize;
    let pass = ctx.cumulative_pass_nr();
    // キーはアドレスなので、アドレスが別のロゴに使い回されていないか確かめる。
    if let Some(held) = ctx.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<LogoKept>(place);
        let (same, held, used) = kept.get_mut(&key)?;
        std::sync::Arc::ptr_eq(same, logo).then(|| {
            *used = pass;
            held.clone()
        })
    }) {
        return held;
    }
    let made = logo.rgba().map(|picture| {
        ctx.load_texture(
            format!("局ロゴ {:03x}/{}", logo.logo_id, logo.logo_type),
            rgba_image(&picture),
            egui::TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<LogoKept>(place);
        if kept.len() >= LOGO_MOST {
            kept.retain(|_, (_, _, used)| pass.saturating_sub(*used) < LOGO_IDLE);
        }
        kept.insert(key, (std::sync::Arc::clone(logo), made.clone(), pass));
    });
    made
}

pub fn draw_logo(
    painter: &egui::Painter,
    rect: egui::Rect,
    logo: Option<&std::sync::Arc<zwcore::guide::Logo>>,
    tint: egui::Color32,
) -> bool {
    let Some(texture) = logo.and_then(|logo| logo_texture(painter.ctx(), logo)) else {
        return false;
    };
    let size = texture.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let shown = egui::Rect::from_center_size(rect.center(), size * scale);
    painter.image(
        texture.id(),
        shown,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );
    true
}

pub fn logo(painter: &egui::Painter, rect: egui::Rect, channel: &guide::epg::Channel, size: f32) {
    if draw_logo(painter, rect, channel.logo.as_ref(), egui::Color32::WHITE) {
        return;
    }
    let (ground, text) = badge_look(painter, guide::epg::badge_colour(&channel.name));
    painter.rect_filled(rect, 2.0, ground);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &channel.badge,
        egui::FontId::proportional(size),
        text,
    );
}

/// 暗いときは局の色に白い文字、明るいときは薄めた背景に局の色の文字（明るい画面で浮かないように）。
pub fn badge_look(
    painter: &egui::Painter,
    colour: egui::Color32,
) -> (egui::Color32, egui::Color32) {
    if painter.ctx().theme() == egui::Theme::Dark {
        (colour, egui::Color32::WHITE)
    } else {
        (light_badge(colour), colour)
    }
}

pub fn light_badge(colour: egui::Color32) -> egui::Color32 {
    let mix = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * 0.8).round() as u8;
    egui::Color32::from_rgb(mix(colour.r()), mix(colour.g()), mix(colour.b()))
}

/// 文字の幅に合わせた横長のバッジ。`at` は左端と縦の中心。表示した矩形を返す。
#[allow(clippy::too_many_arguments)]
pub fn logo_pill(
    painter: &egui::Painter,
    at: egui::Pos2,
    logo: Option<&std::sync::Arc<zwcore::guide::Logo>>,
    badge: &str,
    colour: egui::Color32,
    text: egui::Color32,
    size: f32,
    height: f32,
    pad_x: f32,
    least_width: f32,
) -> egui::Rect {
    if logo.is_some() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(at.x, at.y - height / 2.0),
            egui::vec2(height * 16.0 / 9.0, height),
        );
        let tint = egui::Color32::from_white_alpha(text.a());
        if draw_logo(painter, rect, logo, tint) {
            return rect;
        }
    }
    let galley = painter.layout_no_wrap(badge.to_owned(), egui::FontId::proportional(size), text);
    let rect = egui::Rect::from_min_size(
        egui::pos2(at.x, at.y - height / 2.0),
        egui::vec2((galley.size().x + pad_x * 2.0).max(least_width), height),
    );
    painter.rect_filled(rect, 2.0 * (height / 19.0).max(1.0), colour);
    painter.galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            at.y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );
    rect
}

#[allow(clippy::too_many_arguments)]
pub fn key_circle(
    painter: &egui::Painter,
    centre: egui::Pos2,
    radius: f32,
    fill: egui::Color32,
    stroke: Option<egui::Stroke>,
    key: u8,
    size: f32,
    text: egui::Color32,
) {
    painter.circle_filled(centre, radius, fill);
    if let Some(stroke) = stroke {
        painter.circle_stroke(centre, radius, stroke);
    }
    painter.text(
        centre,
        egui::Align2::CENTER_CENTER,
        key,
        egui::FontId::proportional(size),
        text,
    );
}

/// 漢字の字面枠の中でベースラインがある高さ（枠の上 880、下 120 が日本語フォントの慣例）。
pub const IDEOGRAPHIC_ASCENT: f32 = 0.88;

/// フォントの lineGap に左右されないよう、ベースラインから置く。
pub fn seat_em(galley: &egui::Galley, em_top: f32, em: f32) -> f32 {
    let baseline = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
        .unwrap_or(galley.size().y * IDEOGRAPHIC_ASCENT);
    em_top + em * IDEOGRAPHIC_ASCENT - baseline
}

const ALPHA_MOST: usize = 512;

/// テクスチャの番号は egui のコンテキストの中でしか通じないので、キャッシュもコンテキストごとに持つ。
type AlphaKept = std::collections::HashMap<u64, egui::TextureHandle>;

pub fn alpha_texture(
    ctx: &egui::Context,
    width: usize,
    height: usize,
    levels: u8,
    bitmap: &[u8],
) -> egui::TextureId {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (width, height, levels, bitmap).hash(&mut hasher);
    let key = hasher.finish();
    let place = egui::Id::new("階調の画像");
    if let Some(id) = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<AlphaKept>(place)
            .get(&key)
            .map(egui::TextureHandle::id)
    }) {
        return id;
    }
    let top = f32::from(levels.max(2)) - 1.0;
    let pixels: Vec<egui::Color32> = bitmap
        .iter()
        .map(|&value| {
            let alpha = (f32::from(value) / top).clamp(0.0, 1.0);
            egui::Color32::from_white_alpha((alpha * 255.0 + 0.5) as u8)
        })
        .collect();
    let image = egui::ColorImage::new([width, height], pixels);
    let options = if levels > 2 {
        egui::TextureOptions::LINEAR
    } else {
        egui::TextureOptions::NEAREST
    };
    let handle = ctx.load_texture(format!("alpha-{key:016x}"), image, options);
    let id = handle.id();
    ctx.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<AlphaKept>(place);
        // 持ち過ぎない。捨てても、このフレームで渡した番号は egui がレンダリングし終わるまで生きている。
        if kept.len() > ALPHA_MOST {
            kept.clear();
        }
        kept.insert(key, handle);
    });
    id
}

pub fn graded(
    painter: &egui::Painter,
    rect: egui::Rect,
    top: egui::Color32,
    bottom: egui::Color32,
) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

pub fn style(context: &egui::Context, mode: theme::Mode) {
    let skin = theme::status::skin(mode);
    let mut visuals = match mode {
        theme::Mode::Light => egui::Visuals::light(),
        theme::Mode::Dark => egui::Visuals::dark(),
    };
    visuals.panel_fill = skin.background;
    visuals.window_fill = skin.background;
    visuals.override_text_color = Some(skin.text);
    dress(context, mode, visuals);
}

pub fn paper(context: &egui::Context, mode: theme::Mode) {
    let skin = theme::epg::skin(mode);
    let mut visuals = match mode {
        theme::Mode::Light => egui::Visuals::light(),
        theme::Mode::Dark => egui::Visuals::dark(),
    };
    visuals.panel_fill = skin.background;
    visuals.window_fill = skin.background;
    visuals.override_text_color = Some(theme::detail::skin(mode).text);
    dress(context, mode, visuals);
}

/// `set_visuals` は現在の明暗の側に書くので、明暗が変わった直後に崩れないよう明暗を指定して書く。
fn dress(context: &egui::Context, mode: theme::Mode, visuals: egui::Visuals) {
    let theme = match mode {
        theme::Mode::Light => egui::Theme::Light,
        theme::Mode::Dark => egui::Theme::Dark,
    };
    // egui に任せると macOS で「OS に従う」へ戻せなくなる（外観は `Shell::wear` が設定する）。
    context.options_mut(|options| options.sync_window_theme = false);
    context.set_visuals_of(theme, visuals);
    context.set_theme(theme);
    context.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
    });
}
