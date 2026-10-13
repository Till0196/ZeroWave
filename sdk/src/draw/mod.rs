// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! canvas 2D の API でレンダリングして画像にする。ピクセルの読み書きは持たない。
//! 命令を記録しておき、vello_cpu で画像にする。

use std::sync::Arc;

use parlance::Script;
use parley::fontique::{
    Blob, Collection, CollectionOptions, FamilyId, FontInfoOverride, GenericFamily, SourceCache,
};

pub use parley::FontContext;
use parley::{FontFamily, FontWeight, LayoutContext, PositionedLayoutItem, StyleProperty};
use vello_cpu::kurbo::{self, Affine, BezPath, Diagonal2, Point, Rect, Shape, Stroke, Vec2};
use vello_cpu::peniko::{self, BlendMode, Color, Compose, Fill, FontData, Mix};

/// 図形をパスにするときの許容誤差。
const TOLERANCE: f64 = 0.1;

/// 記録した描画の命令。
#[derive(Debug, Clone)]
enum Op {
    Fill {
        transform: Affine,
        colour: Color,
        path: BezPath,
    },
    Stroke {
        transform: Affine,
        colour: Color,
        stroke: Stroke,
        path: BezPath,
    },
    Layer {
        transform: Affine,
        clip: BezPath,
        blend: BlendMode,
        alpha: f32,
    },
    Clip {
        transform: Affine,
        clip: BezPath,
    },
    Pop,
    Glyphs(Box<Glyphs>),
    Scene {
        transform: Affine,
        scene: Scene,
    },
}

#[derive(Debug, Clone)]
struct Glyphs {
    font: FontData,
    size: f32,
    coords: Vec<i16>,
    embolden: Vec2,
    stroke: Option<Stroke>,
    colour: Color,
    transform: Affine,
    glyph_transform: Option<Affine>,
    glyphs: Vec<vello_cpu::Glyph>,
}

/// 記録した命令の並び。
#[derive(Debug, Clone, Default)]
pub struct Scene {
    ops: Arc<Vec<Op>>,
}

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
    ops: Vec<Op>,
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
            ops: Vec::new(),
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
        self.ops.clear();
        self.weight = 0;
        self.open_layers = 0;
        self.clips = 0;
        for (_, clips) in &mut self.stack {
            *clips = 0;
        }
    }

    pub fn scene_for(&self, width: f64, height: f64) -> Scene {
        if self.width <= 0.0 || self.height <= 0.0 {
            return Scene::default();
        }
        let transform = Affine::scale_non_uniform(width / self.width, height / self.height);
        let mut ops = vec![Op::Scene {
            transform,
            scene: Scene {
                ops: Arc::new(self.ops.clone()),
            },
        }];
        // コピー先で対になるように開いたままのレイヤーは閉じる。
        ops.extend(std::iter::repeat_n(Op::Pop, self.open_layers));
        Scene { ops: Arc::new(ops) }
    }

    fn paint_of(&self, colour: Color) -> Color {
        colour.multiply_alpha(self.state.alpha)
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if !self.spend(1) {
            return;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        self.ops.push(Op::Fill {
            transform: self.state.transform,
            colour: self.paint_of(self.state.fill),
            path: rect.into_path(TOLERANCE),
        });
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if !self.spend(1) {
            return;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        self.ops.push(Op::Stroke {
            transform: self.state.transform,
            colour: self.paint_of(self.state.stroke),
            stroke: Stroke::new(self.state.line_width),
            path: rect.into_path(TOLERANCE),
        });
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
        let rect = Rect::new(x, y, x + w, y + h).into_path(TOLERANCE);
        let transform = self.state.transform;
        self.ops.push(Op::Layer {
            transform,
            clip: rect.clone(),
            blend: BlendMode::new(Mix::Normal, Compose::Clear),
            alpha: 1.0,
        });
        self.ops.push(Op::Fill {
            transform,
            colour: Color::BLACK,
            path: rect,
        });
        self.ops.push(Op::Pop);
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
        let colour = colour.multiply_alpha(self.state.alpha);
        let stroke = stroke.then(|| Stroke::new(self.state.line_width));
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
                let glyphs = glyph_run
                    .positioned_glyphs()
                    .map(|glyph| vello_cpu::Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    })
                    .collect();
                self.ops.push(Op::Glyphs(Box::new(Glyphs {
                    font: font.clone(),
                    size,
                    coords: bytemuck::cast_slice(run.normalized_coords()).to_vec(),
                    embolden,
                    stroke: stroke.clone(),
                    colour,
                    transform,
                    glyph_transform,
                    glyphs,
                })));
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
        self.ops.push(Op::Layer {
            transform,
            clip: clip.into_path(TOLERANCE),
            blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
            alpha: self.state.alpha,
        });
        self.ops.push(Op::Scene {
            transform: placed,
            scene,
        });
        self.ops.push(Op::Pop);
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
        self.ops.push(Op::Fill {
            transform: self.state.transform,
            colour: self.paint_of(self.state.fill),
            path: self.path.clone(),
        });
    }

    pub fn stroke(&mut self) {
        if !self.spend(self.path.elements().len() + 1) {
            return;
        }
        self.ops.push(Op::Stroke {
            transform: self.state.transform,
            colour: self.paint_of(self.state.stroke),
            stroke: Stroke::new(self.state.line_width),
            path: self.path.clone(),
        });
    }

    pub fn clip(&mut self) {
        if !self.spend(self.path.elements().len() + 1) {
            return;
        }
        self.ops.push(Op::Clip {
            transform: self.state.transform,
            clip: self.path.clone(),
        });
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
            self.ops.push(Op::Pop);
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
    inner: Option<(u16, u16, vello_cpu::RenderContext, vello_cpu::Resources)>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Renderer")
    }
}

impl Renderer {
    pub fn render_into(&mut self, scene: &Scene, width: u32, height: u32, out: &mut Vec<u8>) {
        out.clear();
        let (Ok(width), Ok(height)) = (u16::try_from(width), u16::try_from(height)) else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        let (ctx, resources) = match &mut self.inner {
            Some((w, h, ctx, resources)) if *w == width && *h == height => (ctx, resources),
            slot => {
                let made = slot.insert((
                    width,
                    height,
                    vello_cpu::RenderContext::new(width, height),
                    vello_cpu::Resources::new(),
                ));
                (&mut made.2, &mut made.3)
            }
        };
        ctx.reset();
        play(ctx, resources, &scene.ops, Affine::IDENTITY);
        ctx.flush();
        out.resize(usize::from(width) * usize::from(height) * 4, 0);
        let Some(pixmap) = vello_cpu::PixmapMut::new(width, height, out) else {
            out.clear();
            return;
        };
        ctx.render(pixmap, resources);
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

/// 記録した命令を vello_cpu に流す。`outer` は重ねた記録の外側の変換。
fn play(
    ctx: &mut vello_cpu::RenderContext,
    resources: &mut vello_cpu::Resources,
    ops: &[Op],
    outer: Affine,
) {
    for op in ops {
        match op {
            Op::Fill {
                transform,
                colour,
                path,
            } => {
                ctx.set_transform(outer * *transform);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.set_paint(*colour);
                ctx.reset_paint_transform();
                ctx.fill_path(path);
            }
            Op::Stroke {
                transform,
                colour,
                stroke,
                path,
            } => {
                ctx.set_transform(outer * *transform);
                ctx.set_stroke(stroke.clone());
                ctx.set_paint(*colour);
                ctx.reset_paint_transform();
                ctx.stroke_path(path);
            }
            Op::Layer {
                transform,
                clip,
                blend,
                alpha,
            } => {
                ctx.set_transform(outer * *transform);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.push_layer(Some(clip), Some(*blend), Some(*alpha), None, None);
            }
            Op::Clip { transform, clip } => {
                ctx.set_transform(outer * *transform);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.push_clip_layer(clip);
            }
            Op::Pop => ctx.pop_layer(),
            Op::Glyphs(run) => {
                ctx.set_transform(outer * run.transform);
                ctx.set_paint(run.colour);
                ctx.reset_paint_transform();
                if let Some(stroke) = &run.stroke {
                    ctx.set_stroke(stroke.clone());
                }
                let glyphs = run.glyphs.iter().copied();
                let builder = ctx
                    .glyph_run(resources, &run.font)
                    .font_size(run.size)
                    .hint(true)
                    .normalized_coords(&run.coords)
                    .glyph_transform(run.glyph_transform.unwrap_or_default());
                let _ = match &run.stroke {
                    Some(_) => builder.stroke_glyphs(glyphs),
                    None => builder
                        .font_embolden(glifo::FontEmbolden::new(Diagonal2::new(
                            run.embolden.x,
                            run.embolden.y,
                        )))
                        .fill_glyphs(glyphs),
                };
            }
            Op::Scene { transform, scene } => {
                play(ctx, resources, &scene.ops, outer * *transform);
            }
        }
    }
}

/// [`crate::fonts::Faces`] のフォントを登録したフォントのコンテキスト。
/// 順序は固定する（登録順に依るとレンダリングのたびにグリフが変わる）。
pub fn font_context(faces: &crate::fonts::Faces) -> FontContext {
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    let mut families = Vec::new();
    let register = |collection: &mut Collection, face: &Vec<u8>, alias: Option<&str>| {
        let blob = Blob::new(Arc::new(face.clone()) as Arc<dyn AsRef<[u8]> + Send + Sync>);
        let over = alias.map(|name| FontInfoOverride {
            family_name: Some(name),
            ..Default::default()
        });
        let mut found: Vec<(String, FamilyId)> = collection
            .register_fonts(blob, over)
            .into_iter()
            .map(|(id, _)| (collection.family_name(id).unwrap_or("").to_owned(), id))
            .collect();
        found.sort();
        found.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
    };
    if let Some(sans) = &faces.sans {
        families.extend(register(&mut collection, sans, None));
        // データ放送の CSS は `font-family:"丸ゴシック"` と書く。
        for alias in ["丸ゴシック", "角ゴシック", "太丸ゴシック"] {
            let _ = register(&mut collection, sans, Some(alias));
        }
    }
    if let Some(mono) = &faces.mono {
        let named = register(&mut collection, mono, None);
        let _ = register(&mut collection, mono, Some(crate::fonts::MONO_ALIAS));
        families.extend(named);
    }
    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::Serif,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
        GenericFamily::UiSansSerif,
        GenericFamily::Cursive,
        GenericFamily::Fantasy,
    ] {
        collection.set_generic_families(generic, families.iter().copied());
    }
    for script in [
        *b"Hira", *b"Kana", *b"Hani", *b"Latn", *b"Zyyy", *b"Zinh", *b"Zzzz",
    ] {
        collection.set_fallbacks(Script::from_bytes(script), families.iter().copied());
    }
    FontContext {
        source_cache: SourceCache::new_shared(),
        collection,
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
    fn a_clip_limits_what_is_painted_until_restore() {
        let mut canvas = Canvas::new(100.0, 100.0);
        canvas.save();
        canvas.begin_path();
        canvas.rect(0.0, 0.0, 50.0, 50.0);
        canvas.clip();
        canvas.state.fill = colour("#00ff00");
        canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
        assert!(canvas.restore());
        canvas.state.fill = colour("#0000ff");
        canvas.fill_rect(60.0, 60.0, 40.0, 40.0);
        let rgba = render(&canvas.scene_for(100.0, 100.0), 100, 100);
        assert_eq!(pixel(&rgba, 100, 25, 25), [0, 255, 0, 255]);
        assert_eq!(pixel(&rgba, 100, 75, 25)[3], 0, "切り抜きの外");
        assert_eq!(
            pixel(&rgba, 100, 80, 80),
            [0, 0, 255, 255],
            "restore の後は切り抜かない"
        );
    }

    #[test]
    fn clear_rect_and_global_alpha() {
        let mut canvas = Canvas::new(100.0, 100.0);
        canvas.state.fill = colour("#ff0000");
        canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
        canvas.clear_rect(0.0, 0.0, 50.0, 100.0);
        canvas.state.alpha = 0.5;
        canvas.state.fill = colour("#0000ff");
        canvas.fill_rect(0.0, 50.0, 50.0, 50.0);
        let rgba = render(&canvas.scene_for(100.0, 100.0), 100, 100);
        assert_eq!(pixel(&rgba, 100, 25, 25)[3], 0, "消した所");
        assert_eq!(pixel(&rgba, 100, 75, 25), [255, 0, 0, 255]);
        let half = pixel(&rgba, 100, 25, 75);
        assert_eq!(half[..3], [0, 0, 255]);
        assert!((126..=129).contains(&half[3]), "{half:?}");
    }

    #[test]
    fn text_is_painted_with_the_global_alpha() {
        let faces = crate::fonts::Faces::find();
        if faces.sans.is_none() {
            return;
        }
        let mut fonts = Fonts::new(font_context(&faces));
        let mut canvas = Canvas::new(100.0, 40.0);
        canvas.state.font = String::from("bold 32px sans-serif");
        canvas.state.baseline = Baseline::Top;
        canvas.fill_text(&mut fonts, "■", 0.0, 0.0);
        canvas.state.alpha = 0.5;
        canvas.fill_text(&mut fonts, "■", 60.0, 0.0);
        let rgba = render(&canvas.scene_for(100.0, 40.0), 100, 40);
        let most = |from: u32| {
            (0..40)
                .flat_map(|y| (from..from + 40).map(move |x| (x, y)))
                .map(|(x, y)| pixel(&rgba, 100, x, y)[3])
                .max()
                .unwrap_or(0)
        };
        assert_eq!(most(0), 255, "文字が描かれる");
        let half = most(60);
        assert!((120..=135).contains(&half), "半分の不透明度: {half}");
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
