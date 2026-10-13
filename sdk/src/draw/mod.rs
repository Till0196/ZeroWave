// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! canvas 2D の API でレンダリングして画像にする。ピクセルの読み書きは持たない。

pub mod fonts;

use anyrender::{PaintScene, Scene};
use kurbo::{Affine, BezPath, Point, Rect, Shape, Stroke, Vec2};
pub use parley::FontContext;
use parley::{FontFamily, FontWeight, LayoutContext, PositionedLayoutItem, StyleProperty};
use peniko::{BlendMode, Color, Compose, Fill, Mix};

pub struct Fonts {
    fcx: FontContext,
    lcx: LayoutContext<()>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Fonts")
    }
}

impl Fonts {
    pub fn new(fcx: FontContext) -> Self {
        Self {
            fcx,
            lcx: LayoutContext::new(),
        }
    }

    fn layout(&mut self, text: &str, font: &str) -> Option<(parley::Layout<()>, Line)> {
        let (size, weight, family) = parse_font(font);
        let Fonts { fcx, lcx } = self;
        let mut builder = lcx.ranged_builder(fcx, text, 1.0, false);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontWeight(weight));
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            std::borrow::Cow::Owned(family),
        )));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        let line = layout.lines().next()?;
        let metrics = line.metrics();
        let line = Line {
            ascent: metrics.baseline - metrics.content_block_min_coord,
            descent: metrics.content_block_max_coord - metrics.baseline,
            baseline: metrics.baseline,
            width: layout.width(),
        };
        Some((layout, line))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub ascent: f32,
    pub descent: f32,
    pub baseline: f32,
    pub width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

impl Align {
    pub fn parse(text: &str) -> Self {
        match text {
            "center" => Self::Center,
            "right" | "end" => Self::End,
            _ => Self::Start,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Baseline {
    #[default]
    Alphabetic,
    Top,
    Middle,
    Bottom,
}

impl Baseline {
    pub fn parse(text: &str) -> Self {
        match text {
            "top" | "hanging" => Self::Top,
            "middle" => Self::Middle,
            "bottom" | "ideographic" => Self::Bottom,
            _ => Self::Alphabetic,
        }
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub fill: Color,
    pub stroke: Color,
    pub line_width: f64,
    pub alpha: f32,
    pub font: String,
    pub align: Align,
    pub baseline: Baseline,
    pub transform: Affine,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fill: Color::BLACK,
            stroke: Color::BLACK,
            line_width: 1.0,
            alpha: 1.0,
            font: String::from("10px sans-serif"),
            align: Align::Start,
            baseline: Baseline::Alphabetic,
            transform: Affine::IDENTITY,
        }
    }
}

#[derive(Debug)]
pub struct Canvas {
    scene: Scene,
    width: f64,
    height: f64,
    pub state: State,
    stack: Vec<(State, usize)>,
    clips: usize,
    open_layers: usize,
    path: BezPath,
    /// 命令の数の見積もり。canvas を互いにコピーし合うと倍々に増えるので [`WEIGHT_MOST`] で止める。
    weight: usize,
}

const WEIGHT_MOST: usize = 1 << 20;

const PATH_MOST: usize = 1 << 16;

const STACK_MOST: usize = 4096;

impl Canvas {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            scene: Scene::new(),
            width: width.max(0.0),
            height: height.max(0.0),
            state: State::default(),
            stack: Vec::new(),
            clips: 0,
            open_layers: 0,
            path: BezPath::new(),
            weight: 0,
        }
    }

    fn spend(&mut self, n: usize) -> bool {
        match self.weight.checked_add(n) {
            Some(weight) if weight <= WEIGHT_MOST => {
                self.weight = weight;
                true
            }
            _ => false,
        }
    }

    fn path_room(&self) -> usize {
        PATH_MOST.saturating_sub(self.path.elements().len())
    }

    pub fn width(&self) -> f64 {
        self.width
    }

    pub fn height(&self) -> f64 {
        self.height
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        self.width = width.max(0.0).floor();
        self.height = height.max(0.0).floor();
        self.reset();
        self.state = State::default();
        self.stack.clear();
    }

    pub fn reset(&mut self) {
        self.scene.reset();
        self.weight = 0;
        self.open_layers = 0;
        self.clips = 0;
        for (_, clips) in &mut self.stack {
            *clips = 0;
        }
    }

    pub fn scene_for(&self, width: f64, height: f64) -> Scene {
        let mut out = Scene::new();
        if self.width <= 0.0 || self.height <= 0.0 {
            return out;
        }
        let transform = Affine::scale_non_uniform(width / self.width, height / self.height);
        out.append_scene(self.scene.clone(), transform);
        // コピー先で対になるように開いたままのレイヤーは閉じる。
        for _ in 0..self.open_layers {
            out.pop_layer();
        }
        out
    }

    fn paint_of(&self, colour: Color) -> anyrender::Paint {
        anyrender::Paint::Solid(colour.multiply_alpha(self.state.alpha))
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if !self.spend(1) {
            return;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        let paint = self.paint_of(self.state.fill);
        let transform = self.state.transform;
        self.scene
            .fill(Fill::NonZero, transform, &paint, None, &rect);
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if !self.spend(1) {
            return;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        let paint = self.paint_of(self.state.stroke);
        let transform = self.state.transform;
        let stroke = Stroke::new(self.state.line_width);
        self.scene.stroke(&stroke, transform, &paint, None, &rect);
    }

    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let whole = self.state.transform == Affine::IDENTITY
            && x <= 0.0
            && y <= 0.0
            && x + w >= self.width
            && y + h >= self.height;
        if whole {
            self.reset();
            return;
        }
        if !self.spend(1) {
            return;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        let transform = self.state.transform;
        self.scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::Clear),
            1.0,
            transform,
            &rect,
            None,
            None,
        );
        self.scene.fill(
            Fill::NonZero,
            transform,
            &anyrender::Paint::Solid(Color::BLACK),
            None,
            &rect,
        );
        self.scene.pop_layer();
    }

    pub fn measure_text(&self, fonts: &mut Fonts, text: &str) -> Line {
        if let Some((_, line)) = fonts.layout(text, &self.state.font) {
            return line;
        }
        let (size, _, _) = parse_font(&self.state.font);
        Line {
            ascent: size * 0.88,
            descent: size * 0.12,
            baseline: size * 0.88,
            width: text
                .chars()
                .map(|c| if c.is_ascii() { size / 2.0 } else { size })
                .sum(),
        }
    }

    pub fn fill_text(&mut self, fonts: &mut Fonts, text: &str, x: f64, y: f64) {
        self.draw_text(fonts, text, x, y, false);
    }

    pub fn stroke_text(&mut self, fonts: &mut Fonts, text: &str, x: f64, y: f64) {
        self.draw_text(fonts, text, x, y, true);
    }

    fn draw_text(&mut self, fonts: &mut Fonts, text: &str, x: f64, y: f64, stroke: bool) {
        if text.is_empty() || !self.spend(text.chars().count()) {
            return;
        }
        let Some((layout, line)) = fonts.layout(text, &self.state.font) else {
            return;
        };
        let dx = match self.state.align {
            Align::Start => 0.0,
            Align::Center => -f64::from(line.width) / 2.0,
            Align::End => -f64::from(line.width),
        };
        let dy = match self.state.baseline {
            Baseline::Top => 0.0,
            Baseline::Middle => {
                -f64::from(line.baseline) + f64::from(line.ascent - line.descent) / 2.0
            }
            Baseline::Bottom => -f64::from(line.baseline + line.descent),
            Baseline::Alphabetic => -f64::from(line.baseline),
        };
        let transform = self
            .state
            .transform
            .then_translate(Vec2::new(x + dx, y + dy));
        let colour = if stroke {
            self.state.stroke
        } else {
            self.state.fill
        };
        let paint = anyrender::Paint::Solid(colour);
        let alpha = self.state.alpha;
        let stroke_style = Stroke::new(self.state.line_width);
        let style: peniko::StyleRef<'_> = if stroke {
            (&stroke_style).into()
        } else {
            Fill::NonZero.into()
        };
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font = run.font();
                let size = run.font_size();
                // 太字が無いフォントの太字は、輪郭を太らせてレンダリングする（片側 1/48 em）。
                let embolden = if run.synthesis().embolden() {
                    let amount = f64::from(size) / 48.0;
                    Vec2::new(amount, amount)
                } else {
                    Vec2::ZERO
                };
                let glyph_transform = run
                    .synthesis()
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
                let glyphs = glyph_run.positioned_glyphs().map(|glyph| anyrender::Glyph {
                    id: glyph.id as _,
                    x: glyph.x,
                    y: glyph.y,
                });
                self.scene.draw_glyphs(
                    font,
                    size,
                    true,
                    bytemuck::cast_slice(run.normalized_coords()),
                    embolden,
                    style,
                    &paint,
                    alpha,
                    transform,
                    glyph_transform,
                    glyphs,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_canvas(
        &mut self,
        from: &Canvas,
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    ) {
        if sw <= 0.0 || sh <= 0.0 || dw <= 0.0 || dh <= 0.0 || !self.spend(from.weight + 1) {
            return;
        }
        let scene = from.scene_for(from.width, from.height);
        let transform = self.state.transform;
        let placed = transform
            * Affine::translate((dx, dy))
            * Affine::scale_non_uniform(dw / sw, dh / sh)
            * Affine::translate((-sx, -sy));
        let clip = Rect::new(dx, dy, dx + dw, dy + dh);
        self.scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            self.state.alpha,
            transform,
            &clip,
            None,
            None,
        );
        self.scene.append_scene(scene, placed);
        self.scene.pop_layer();
    }

    pub fn draw_canvas_at(&mut self, from: &Canvas, dx: f64, dy: f64, size: Option<(f64, f64)>) {
        let (dw, dh) = size.unwrap_or((from.width, from.height));
        self.draw_canvas(from, 0.0, 0.0, from.width, from.height, dx, dy, dw, dh);
    }

    pub fn begin_path(&mut self) {
        self.path = BezPath::new();
    }

    pub fn close_path(&mut self) {
        self.path.close_path();
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        if self.path_room() > 0 {
            self.path.move_to(Point::new(x, y));
        }
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        self.join_at(Point::new(x, y));
    }

    fn join_at(&mut self, p: Point) {
        if self.path_room() == 0 {
            return;
        }
        if self.path.elements().is_empty() {
            self.path.move_to(p);
        } else {
            self.path.line_to(p);
        }
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let room = self.path_room();
        self.path
            .extend(Rect::new(x, y, x + w, y + h).path_elements(0.1).take(room));
    }

    pub fn arc(&mut self, x: f64, y: f64, r: f64, start: f64, end: f64, anticlockwise: bool) {
        let mut sweep = end - start;
        if anticlockwise {
            if sweep > 0.0 {
                sweep -= std::f64::consts::TAU;
            }
        } else if sweep < 0.0 {
            sweep += std::f64::consts::TAU;
        }
        let arc = kurbo::Arc::new(Point::new(x, y), Vec2::new(r, r), start, sweep, 0.0);
        self.join_at(Point::new(x + r * start.cos(), y + r * start.sin()));
        let mut elements = arc.path_elements(0.1);
        let _ = elements.next();
        // 半径が大きいと要素がいくらでも増えるので、上限までで切る。
        let room = self.path_room();
        self.path.extend(elements.take(room));
    }

    pub fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        if self.path_room() < 2 {
            return;
        }
        let c = Point::new(cx, cy);
        if self.path.elements().is_empty() {
            self.path.move_to(c);
        }
        self.path.quad_to(c, Point::new(x, y));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn bezier_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        if self.path_room() < 2 {
            return;
        }
        let c1 = Point::new(c1x, c1y);
        if self.path.elements().is_empty() {
            self.path.move_to(c1);
        }
        self.path
            .curve_to(c1, Point::new(c2x, c2y), Point::new(x, y));
    }

    pub fn fill(&mut self) {
        if !self.spend(self.path.elements().len() + 1) {
            return;
        }
        let paint = self.paint_of(self.state.fill);
        let (transform, path) = (self.state.transform, self.path.clone());
        self.scene
            .fill(Fill::NonZero, transform, &paint, None, &path);
    }

    pub fn stroke(&mut self) {
        if !self.spend(self.path.elements().len() + 1) {
            return;
        }
        let paint = self.paint_of(self.state.stroke);
        let (transform, path) = (self.state.transform, self.path.clone());
        let stroke = Stroke::new(self.state.line_width);
        self.scene.stroke(&stroke, transform, &paint, None, &path);
    }

    pub fn clip(&mut self) {
        if !self.spend(self.path.elements().len() + 1) {
            return;
        }
        let (transform, path) = (self.state.transform, self.path.clone());
        self.scene.push_clip_layer(Fill::NonZero, transform, &path);
        self.open_layers += 1;
        self.clips += 1;
    }

    pub fn save(&mut self) {
        if self.stack.len() >= STACK_MOST {
            return;
        }
        self.stack.push((self.state.clone(), self.clips));
        self.clips = 0;
    }

    pub fn restore(&mut self) -> bool {
        let Some((state, clips)) = self.stack.pop() else {
            return false;
        };
        for _ in 0..self.clips {
            self.scene.pop_layer();
            self.open_layers = self.open_layers.saturating_sub(1);
        }
        self.clips = clips;
        self.state = state;
        true
    }

    pub fn translate(&mut self, x: f64, y: f64) {
        self.state.transform *= Affine::translate((x, y));
    }

    pub fn scale(&mut self, x: f64, y: f64) {
        self.state.transform *= Affine::scale_non_uniform(x, y);
    }

    pub fn rotate(&mut self, angle: f64) {
        self.state.transform *= Affine::rotate(angle);
    }

    pub fn transform(&mut self, m: [f64; 6]) {
        self.state.transform *= Affine::new(m);
    }

    pub fn set_transform(&mut self, m: [f64; 6]) {
        self.state.transform = Affine::new(m);
    }

    pub fn get_transform(&self) -> [f64; 6] {
        self.state.transform.as_coeffs()
    }
}

pub fn colour(text: &str) -> Color {
    peniko::color::parse_color(text.trim())
        .map(|c| c.to_alpha_color::<peniko::color::Srgb>())
        .unwrap_or(Color::BLACK)
}

pub fn parse_font(font: &str) -> (f32, FontWeight, String) {
    let mut size = 10.0f32;
    let mut weight = FontWeight::NORMAL;
    let mut family = String::from("sans-serif");
    let mut rest: Vec<&str> = Vec::new();
    let mut seen_size = false;
    for part in font.split_whitespace() {
        if seen_size {
            rest.push(part);
            continue;
        }
        if let Some(px) = part
            .split('/')
            .next()
            .and_then(|p| p.strip_suffix("px"))
            .and_then(|n| n.parse::<f32>().ok())
        {
            size = px;
            seen_size = true;
            continue;
        }
        match part {
            "bold" | "bolder" => weight = FontWeight::BOLD,
            "lighter" => weight = FontWeight::LIGHT,
            "normal" | "italic" | "oblique" | "small-caps" => {}
            _ => {
                if let Ok(n) = part.parse::<f32>() {
                    weight = FontWeight::new(n);
                }
            }
        }
    }
    if !rest.is_empty() {
        family = rest.join(" ");
    }
    (size, weight, family)
}

/// 乗算済みでない RGBA に。vello_cpu は乗算済みでレンダリングするので alpha で割り戻す。
pub fn render(scene: &Scene, width: u32, height: u32) -> Vec<u8> {
    let mut renderer = Renderer::default();
    let mut out = Vec::new();
    renderer.render_into(scene, width, height, &mut out);
    out
}

#[derive(Default)]
pub struct Renderer {
    inner: Option<(u32, u32, anyrender_vello_cpu::VelloCpuImageRenderer)>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Renderer")
    }
}

impl Renderer {
    pub fn render_into(&mut self, scene: &Scene, width: u32, height: u32, out: &mut Vec<u8>) {
        use anyrender::ImageRenderer;
        out.clear();
        if width == 0 || height == 0 {
            return;
        }
        let renderer = match &mut self.inner {
            Some((w, h, renderer)) if *w == width && *h == height => renderer,
            slot => {
                *slot = Some((
                    width,
                    height,
                    anyrender_vello_cpu::VelloCpuImageRenderer::new(width, height),
                ));
                &mut slot.as_mut().expect("just set").2
            }
        };
        renderer.reset();
        renderer.render_to_vec(
            |painter| painter.append_scene(scene.clone(), Affine::IDENTITY),
            out,
        );
        for px in out.as_chunks_mut::<4>().0 {
            let alpha = px[3];
            if alpha == 0 {
                *px = [0, 0, 0, 0];
            } else if alpha != 255 {
                for channel in px.iter_mut().take(3) {
                    *channel = ((u16::from(*channel) * 255 + u16::from(alpha) / 2)
                        / u16::from(alpha))
                    .min(255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvases_drawn_into_each_other_stop_growing() {
        let mut a = Canvas::new(10.0, 10.0);
        let mut b = Canvas::new(10.0, 10.0);
        a.fill_rect(0.0, 0.0, 1.0, 1.0);
        for _ in 0..64 {
            b.draw_canvas_at(&a, 0.0, 0.0, None);
            a.draw_canvas_at(&b, 0.0, 0.0, None);
        }
        assert!(a.weight <= WEIGHT_MOST && b.weight <= WEIGHT_MOST);
        a.reset();
        assert_eq!(a.weight, 0);
        let mut path = Canvas::new(10.0, 10.0);
        path.arc(0.0, 0.0, 1e300, 0.0, 6.0, false);
        assert!(path.path.elements().len() <= PATH_MOST);
    }

    #[test]
    fn the_font_shorthand_gives_size_weight_and_family() {
        assert_eq!(
            parse_font("bold 24px \"Noto Sans JP\", sans-serif"),
            (
                24.0,
                FontWeight::BOLD,
                String::from("\"Noto Sans JP\", sans-serif")
            )
        );
        assert_eq!(parse_font("10px sans-serif").0, 10.0);
        assert_eq!(
            parse_font("italic 700 32px/1.2 Arial").1,
            FontWeight::new(700.0)
        );
        assert_eq!(parse_font("nothing").0, 10.0);
    }

    #[test]
    fn a_css_colour_is_read() {
        assert_eq!(colour("#ff0000").to_rgba8().r, 255);
        assert_eq!(colour("rgba(0, 255, 0, 0.5)").to_rgba8().g, 255);
        assert_eq!(colour("nonsense"), Color::BLACK);
    }

    fn pixel(buffer: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        buffer[at..at + 4].try_into().expect("四つ")
    }

    #[test]
    fn a_filled_rect_renders_opaque_on_a_transparent_ground() {
        let mut canvas = Canvas::new(100.0, 100.0);
        canvas.state.fill = colour("#ff0000");
        canvas.fill_rect(0.0, 0.0, 50.0, 100.0);
        let mut off = Canvas::new(10.0, 10.0);
        off.state.fill = colour("rgb(0, 0, 255)");
        off.fill_rect(0.0, 0.0, 10.0, 10.0);
        canvas.draw_canvas_at(&off, 60.0, 10.0, Some((30.0, 30.0)));
        let rgba = render(&canvas.scene_for(100.0, 100.0), 100, 100);
        assert_eq!(pixel(&rgba, 100, 25, 50), [255, 0, 0, 255]);
        assert_eq!(pixel(&rgba, 100, 75, 25), [0, 0, 255, 255]);
        assert_eq!(pixel(&rgba, 100, 75, 75)[3], 0);
    }

    #[test]
    fn restore_brings_the_state_back() {
        let mut canvas = Canvas::new(10.0, 10.0);
        canvas.save();
        canvas.translate(5.0, 5.0);
        canvas.state.alpha = 0.5;
        assert!(canvas.restore());
        assert_eq!(canvas.get_transform(), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(canvas.state.alpha, 1.0);
        assert!(!canvas.restore());
    }
}
