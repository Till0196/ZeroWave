// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GLES 2.0 を直接使う出力で、映像の上に画面（egui）を重ねる（[`zwcore::output::present::Layer`]）。
//! 既定は `egui_glow` が GL でレンダリングする。`ZW_SOFT_UI=1` なら別のスレッド（[`Worker`]）が CPU でラスタライズし
//! （[`super::soft`]）、表示する直前に変わった行だけを GL へアップロードする。

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use egui::epaint::textures::TexturesDelta;
use zwcore::output::present::{Layer, LayerTarget};

use super::overlay::Drawn;
use super::soft::{Soft, Textures};

const SOFT_TEXTURE: egui::TextureId = egui::TextureId::User(u64::MAX);

#[derive(Debug)]
pub struct GlesLayer {
    inner: RefCell<Inner>,
}

struct Inner {
    textures: Textures,
    pending: TexturesDelta,
    jobs: Vec<egui::ClippedPrimitive>,
    pixels_per_point: f32,
    size: (u32, u32),
    soft: bool,
    painted: Vec<u8>,
    before: Vec<u8>,
    uploaded: Option<(u64, (usize, usize))>,
    worker: Option<Worker>,
    arrived: Option<Painted>,
    stale: bool,
    painter: Option<(u64, egui_glow::Painter)>,
    /// 画面をレンダリングしておくシート。バッチが変わったときだけレンダリングし直し、フレームごとには画面のある行の範囲だけを貼る。
    sheet: Option<Sheet>,
    sheet_failed: Option<(u64, (u32, u32))>,
    painter_failed: Option<u64>,
    warned: bool,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Inner")
            .field("jobs", &self.jobs.len())
            .field("size", &self.size)
            .field("soft", &self.soft)
            .finish_non_exhaustive()
    }
}

impl Default for GlesLayer {
    fn default() -> Self {
        let layer = Self::synchronous();
        {
            let mut inner = layer.inner.borrow_mut();
            if inner.soft {
                tracing::info!("画面を CPU でレンダリングして合成する（ZW_SOFT_UI）");
                inner.worker = Worker::spawn();
            }
        }
        layer
    }
}

impl GlesLayer {
    pub fn synchronous() -> Self {
        let soft = std::env::var_os("ZW_SOFT_UI").is_some_and(|value| value != "0");
        Self {
            inner: RefCell::new(Inner {
                textures: Textures::default(),
                pending: TexturesDelta::default(),
                jobs: Vec::new(),
                pixels_per_point: 1.0,
                size: (0, 0),
                soft,
                painted: Vec::new(),
                before: Vec::new(),
                uploaded: None,
                worker: None,
                arrived: None,
                stale: false,
                painter: None,
                sheet: None,
                sheet_failed: None,
                painter_failed: None,
                warned: false,
            }),
        }
    }

    /// ビルドした画面を受け取る。差分はすぐコピーへ適用し、GL へはまだ渡していない分として保持する。
    pub fn hand(&mut self, drawn: Drawn) {
        let inner = self.inner.get_mut();
        let (jobs, mut delta, pixels_per_point, size) = drawn.into_parts();
        if let Some(worker) = inner.worker.as_ref() {
            let job = Job {
                jobs,
                delta: std::mem::take(&mut delta),
                pixels_per_point,
                size,
            };
            if let Err(error) = worker.to.send(job) {
                // スレッドが落ちた。差分は捨てる（片付けておかないと epaint が警告する）。
                let mut job = error.0;
                job.delta.clear();
                tracing::warn!("画面のレンダリングスレッドが終了している");
                inner.worker = None;
            }
            inner.size = size;
            return;
        }
        // コピーは CPU でラスタライズするときだけ。GL でレンダリングするときに持つと、フォントのテクスチャ（16 MB）に文字が足されるたびに
        // 丸ごと複製することになり、新しい文字が続くとメモリが膨らむ。
        if inner.soft {
            inner.textures.set(&delta);
            inner.textures.free(&delta);
        }
        inner.pending.append(std::mem::take(&mut delta));
        inner.jobs = jobs;
        inner.pixels_per_point = pixels_per_point;
        inner.size = size;
        inner.stale = true;
    }

    pub fn has(&self) -> bool {
        self.inner.borrow().size != (0, 0)
    }
}

impl Inner {
    fn take_painted(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        loop {
            match worker.from.try_recv() {
                Ok(mut painted) => {
                    if let Some(older) = self.arrived.take() {
                        painted.whole |= older.whole;
                        let mut rows = older.rows;
                        rows.append(&mut painted.rows);
                        painted.rows = rows;
                    }
                    self.arrived = Some(painted);
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("画面のレンダリングスレッドが停止した");
                    self.worker = None;
                    return;
                }
            }
        }
    }
}

impl Layer for GlesLayer {
    fn fresh(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.take_painted();
        inner.stale || inner.arrived.is_some()
    }

    fn paint(&self, on: &LayerTarget<'_>) {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        if inner.size == (0, 0) {
            return;
        }
        inner.take_painted();
        if inner.painter_failed == Some(on.context) {
            inner.pending.clear();
            return;
        }
        if inner
            .painter
            .as_ref()
            .is_none_or(|(context, _)| *context != on.context)
        {
            // 前のレンダラーの GL のオブジェクトはコンテキストと一緒に消えている。消しに行かない。
            if let Some((_, old)) = inner.painter.take() {
                std::mem::forget(old);
            }
            // SAFETY: コンテキストは有効で、エントリポイントはそのコンテキストの物（`LayerTarget::proc_address`）。
            let gl =
                unsafe { glow::Context::from_loader_function_cstr(|name| (on.proc_address)(name)) };
            match egui_glow::Painter::new(Arc::new(gl), "", None, false) {
                Ok(painter) => {
                    tracing::info!("画面を GL で合成する（egui_glow）");
                    let again = inner.painter.is_some();
                    inner.painter = Some((on.context, painter));
                    if inner.soft {
                        inner.pending.clear();
                        inner.pending = inner.textures.whole();
                    } else if again {
                        tracing::warn!("GL コンテキストが変わった。文字が欠けることがある");
                    }
                    inner.stale = true;
                }
                Err(error) => {
                    if !inner.warned {
                        inner.warned = true;
                        tracing::warn!("画面のレンダラーを作成できない: {error}");
                    }
                    inner.painter_failed = Some(on.context);
                    inner.pending.clear();
                    return;
                }
            }
        }
        let Some((_, painter)) = inner.painter.as_mut() else {
            return;
        };
        let screen = [on.size.0, on.size.1];
        if inner.worker.is_some() {
            let mut delta = std::mem::take(&mut inner.pending);
            delta.clear();
            let lost = inner
                .uploaded
                .is_some_and(|(context, _)| context != on.context);
            if lost && inner.arrived.is_none() && !inner.painted.is_empty() {
                let (_, size) = inner.uploaded.unwrap_or_default();
                inner.arrived = Some(Painted {
                    pixels: std::mem::take(&mut inner.painted),
                    size,
                    rows: Vec::new(),
                    whole: true,
                });
            }
            if let Some(painted) = inner.arrived.take() {
                let (width, height) = painted.size;
                let whole = painted.whole || inner.uploaded != Some((on.context, (width, height)));
                if whole {
                    let image =
                        egui::ColorImage::from_rgba_premultiplied([width, height], &painted.pixels);
                    painter.set_texture(
                        SOFT_TEXTURE,
                        &egui::epaint::ImageDelta::full(image, egui::TextureOptions::NEAREST),
                    );
                } else {
                    let row = width * 4;
                    for (from, to) in painted.rows {
                        let image = egui::ColorImage::from_rgba_premultiplied(
                            [width, to - from],
                            &painted.pixels[from * row..to * row],
                        );
                        painter.set_texture(
                            SOFT_TEXTURE,
                            &egui::epaint::ImageDelta::partial(
                                [0, from],
                                image,
                                egui::TextureOptions::NEAREST,
                            ),
                        );
                    }
                }
                inner.uploaded = Some((on.context, (width, height)));
                inner.painted = painted.pixels;
            }
            inner.stale = false;
            if inner
                .uploaded
                .is_some_and(|(context, _)| context == on.context)
            {
                painter.paint_primitives(screen, 1.0, &[whole_screen(on.size)]);
            }
            return;
        }
        if inner.soft {
            let (width, height) = (on.size.0 as usize, on.size.1 as usize);
            let mut delta = std::mem::take(&mut inner.pending);
            let whole = inner.uploaded != Some((on.context, (width, height)));
            if inner.stale || whole {
                std::mem::swap(&mut inner.painted, &mut inner.before);
                Soft::paint(
                    &inner.textures,
                    &inner.jobs,
                    inner.pixels_per_point,
                    width,
                    height,
                    &mut inner.painted,
                );
                if whole || inner.before.len() != inner.painted.len() {
                    let image =
                        egui::ColorImage::from_rgba_premultiplied([width, height], &inner.painted);
                    painter.set_texture(
                        SOFT_TEXTURE,
                        &egui::epaint::ImageDelta::full(image, egui::TextureOptions::NEAREST),
                    );
                } else {
                    let row = width * 4;
                    let mut y = 0;
                    while y < height {
                        let same = |y: usize| {
                            inner.painted[y * row..(y + 1) * row]
                                == inner.before[y * row..(y + 1) * row]
                        };
                        if same(y) {
                            y += 1;
                            continue;
                        }
                        let from = y;
                        while y < height && !same(y) {
                            y += 1;
                        }
                        let image = egui::ColorImage::from_rgba_premultiplied(
                            [width, y - from],
                            &inner.painted[from * row..y * row],
                        );
                        painter.set_texture(
                            SOFT_TEXTURE,
                            &egui::epaint::ImageDelta::partial(
                                [0, from],
                                image,
                                egui::TextureOptions::NEAREST,
                            ),
                        );
                    }
                }
                inner.uploaded = Some((on.context, (width, height)));
                inner.stale = false;
            }
            delta.clear();
            painter.paint_primitives(screen, 1.0, &[whole_screen(on.size)]);
            return;
        }
        let gl = Arc::clone(painter.gl());
        // シートはコンテキストとサイズごと。コンテキストが替わったシートの GL のオブジェクトはコンテキストと一緒に消えている。
        if inner
            .sheet
            .as_ref()
            .is_none_or(|sheet| sheet.context != on.context || sheet.size != on.size)
            && inner.sheet_failed != Some((on.context, on.size))
        {
            if let Some(old) = inner.sheet.take()
                && old.context == on.context
            {
                // SAFETY: コンテキストは有効で、シートはこのコンテキストで作った物。
                unsafe { old.delete(&gl) };
            }
            // SAFETY: コンテキストは有効にしてある。
            match unsafe { Sheet::new(&gl, on.context, on.size) } {
                Ok(sheet) => inner.sheet = Some(sheet),
                Err(error) => {
                    inner.sheet_failed = Some((on.context, on.size));
                    if !inner.warned {
                        inner.warned = true;
                        tracing::warn!(
                            "画面のオフスクリーンバッファーを作成できないため、毎フレームレンダリングする: {error}"
                        );
                    }
                }
            }
            inner.stale = true;
        }
        let Some(sheet) = inner.sheet.as_mut() else {
            let mut delta = std::mem::take(&mut inner.pending);
            painter.paint_and_update_textures(
                screen,
                inner.pixels_per_point,
                &inner.jobs,
                &mut delta,
            );
            delta.clear();
            inner.stale = false;
            return;
        };
        if inner.stale {
            // SAFETY: コンテキストは有効で、シートはこのコンテキストの物。
            unsafe {
                use glow::HasContext as _;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sheet.framebuffer));
                gl.viewport(0, 0, on.size.0 as i32, on.size.1 as i32);
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
            let mut delta = std::mem::take(&mut inner.pending);
            painter.paint_and_update_textures(
                screen,
                inner.pixels_per_point,
                &inner.jobs,
                &mut delta,
            );
            delta.clear();
            // SAFETY: 画面へ戻すだけ。
            unsafe {
                use glow::HasContext as _;
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            sheet.bands = bands(&inner.jobs, inner.pixels_per_point, on.size);
            inner.stale = false;
        }
        // SAFETY: コンテキストは有効で、出力先は画面。
        unsafe { sheet.lay(&gl) };
    }
}

#[derive(Default)]
struct Made {
    texture: Option<glow::Texture>,
    framebuffer: Option<glow::Framebuffer>,
    program: Option<glow::Program>,
    buffer: Option<glow::Buffer>,
}

impl Made {
    /// # Safety
    ///
    /// コンテキストが有効で、どれもこのコンテキストで作った物であること。
    unsafe fn delete(self, gl: &glow::Context) {
        use glow::HasContext as _;
        unsafe {
            if let Some(framebuffer) = self.framebuffer {
                gl.delete_framebuffer(framebuffer);
            }
            if let Some(texture) = self.texture {
                gl.delete_texture(texture);
            }
            if let Some(program) = self.program {
                gl.delete_program(program);
            }
            if let Some(buffer) = self.buffer {
                gl.delete_buffer(buffer);
            }
        }
    }
}

struct Sheet {
    context: u64,
    size: (u32, u32),
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
    program: glow::Program,
    buffer: glow::Buffer,
    bands: Vec<(u32, u32)>,
}

impl Sheet {
    /// # Safety
    ///
    /// コンテキストを有効にしてあること。
    unsafe fn new(gl: &glow::Context, context: u64, size: (u32, u32)) -> Result<Self, String> {
        // 途中で失敗したら作った物を消してから返す（残すと作り直すたびに VRAM が漏れる）。
        let mut made = Made::default();
        // SAFETY: コンテキストは有効にしてある（呼び出し側の前提）。
        let built = unsafe { Self::build(gl, &mut made, size) };
        match (built, made) {
            (
                Ok(()),
                Made {
                    texture: Some(texture),
                    framebuffer: Some(framebuffer),
                    program: Some(program),
                    buffer: Some(buffer),
                },
            ) => Ok(Self {
                context,
                size,
                texture,
                framebuffer,
                program,
                buffer,
                bands: Vec::new(),
            }),
            (built, made) => {
                // SAFETY: コンテキストは有効で、どれもこのコンテキストで作った物。
                unsafe { made.delete(gl) };
                Err(built.err().unwrap_or_default())
            }
        }
    }

    /// # Safety
    ///
    /// コンテキストを有効にしてあること。作った物は `made` に置く。
    unsafe fn build(gl: &glow::Context, made: &mut Made, size: (u32, u32)) -> Result<(), String> {
        use glow::HasContext as _;
        unsafe {
            let texture = gl.create_texture()?;
            made.texture = Some(texture);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                size.0 as i32,
                size.1 as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            let framebuffer = gl.create_framebuffer()?;
            made.framebuffer = Some(framebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if complete != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!(
                    "シートのフレームバッファが揃わない（{complete:#x}）"
                ));
            }
            let program = gl.create_program()?;
            made.program = Some(program);
            for (kind, text) in [
                (glow::VERTEX_SHADER, SHEET_VERTEX),
                (glow::FRAGMENT_SHADER, SHEET_FRAGMENT),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, text);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    return Err(log);
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.bind_attrib_location(program, 0, "place");
            gl.bind_attrib_location(program, 1, "at");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            made.buffer = Some(gl.create_buffer()?);
            Ok(())
        }
    }

    /// 範囲を画面へ貼る。シートは乗算済みアルファなので、egui と同じ混ぜ方で。
    /// # Safety
    ///
    /// コンテキストが有効で、画面（フレームバッファ 0）がバインドされていること。
    unsafe fn lay(&self, gl: &glow::Context) {
        use glow::HasContext as _;
        if self.bands.is_empty() {
            return;
        }
        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        let _ = width;
        let mut corners: Vec<f32> = Vec::with_capacity(self.bands.len() * 24);
        for &(top, bottom) in &self.bands {
            let (y0, y1) = (
                1.0 - 2.0 * top as f32 / height,
                1.0 - 2.0 * bottom as f32 / height,
            );
            // シートの行 0 は画面の下（GL の座標系）。
            let (v0, v1) = (1.0 - top as f32 / height, 1.0 - bottom as f32 / height);
            corners.extend_from_slice(&[
                -1.0, y0, 0.0, v0, 1.0, y0, 1.0, v0, -1.0, y1, 0.0, v1, //
                1.0, y0, 1.0, v0, 1.0, y1, 1.0, v1, -1.0, y1, 0.0, v1,
            ]);
        }
        let bytes: Vec<u8> = corners.iter().flat_map(|v| v.to_ne_bytes()).collect();
        unsafe {
            gl.viewport(0, 0, self.size.0 as i32, self.size.1 as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if let Some(at) = gl.get_uniform_location(self.program, "sheet") {
                gl.uniform_1_i32(Some(&at), 0);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STREAM_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
            gl.draw_arrays(glow::TRIANGLES, 0, (corners.len() / 4) as i32);
            gl.disable_vertex_attrib_array(0);
            gl.disable_vertex_attrib_array(1);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }

    /// # Safety
    ///
    /// コンテキストが有効で、シートはこのコンテキストで作った物であること。
    unsafe fn delete(self, gl: &glow::Context) {
        use glow::HasContext as _;
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
            gl.delete_program(self.program);
            gl.delete_buffer(self.buffer);
        }
    }
}

const SHEET_VERTEX: &str = "#version 100
attribute vec2 place;
attribute vec2 at;
varying vec2 texel;
void main() {
    texel = at;
    gl_Position = vec4(place, 0.0, 1.0);
}
";

const SHEET_FRAGMENT: &str = "#version 100
precision mediump float;
uniform sampler2D sheet;
varying vec2 texel;
void main() {
    gl_FragColor = texture2D(sheet, texel);
}
";

fn bands(
    jobs: &[egui::ClippedPrimitive],
    pixels_per_point: f32,
    size: (u32, u32),
) -> Vec<(u32, u32)> {
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for job in jobs {
        let egui::epaint::Primitive::Mesh(mesh) = &job.primitive else {
            continue;
        };
        if mesh.vertices.is_empty() {
            continue;
        }
        let (low, high) = mesh
            .vertices
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), v| {
                (low.min(v.pos.y), high.max(v.pos.y))
            });
        let top = low.max(job.clip_rect.min.y) * pixels_per_point;
        let bottom = high.min(job.clip_rect.max.y) * pixels_per_point;
        let top = top.floor().max(0.0) as u32;
        let bottom = (bottom.ceil().max(0.0) as u32).min(size.1);
        if top < bottom {
            spans.push((top, bottom));
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (top, bottom) in spans {
        match merged.last_mut() {
            Some(last) if top <= last.1 + 8 => last.1 = last.1.max(bottom),
            _ => merged.push((top, bottom)),
        }
    }
    merged
}

fn whole_screen((width, height): (u32, u32)) -> egui::ClippedPrimitive {
    let mut mesh = egui::Mesh::with_texture(SOFT_TEXTURE);
    mesh.add_rect_with_uv(
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32)),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    egui::ClippedPrimitive {
        clip_rect: egui::Rect::EVERYTHING,
        primitive: egui::epaint::Primitive::Mesh(mesh),
    }
}

struct Job {
    jobs: Vec<egui::ClippedPrimitive>,
    delta: TexturesDelta,
    pixels_per_point: f32,
    size: (u32, u32),
}

struct Painted {
    pixels: Vec<u8>,
    size: (usize, usize),
    rows: Vec<(usize, usize)>,
    whole: bool,
}

struct Worker {
    to: Sender<Job>,
    from: Receiver<Painted>,
}

impl Worker {
    fn spawn() -> Option<Self> {
        let (to, jobs) = std::sync::mpsc::channel::<Job>();
        let (painted, from) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(String::from("soft-ui"))
            .spawn(move || paint_away(&jobs, &painted));
        match spawned {
            Ok(_) => Some(Self { to, from }),
            Err(error) => {
                tracing::warn!(
                    "画面のレンダリングスレッドを起動できないため、再生スレッドでレンダリングする: {error}"
                );
                None
            }
        }
    }
}

fn paint_away(jobs: &Receiver<Job>, out: &Sender<Painted>) {
    let mut textures = Textures::default();
    let mut previous: Vec<u8> = Vec::new();
    let mut previous_size = (0, 0);
    let mut buffer = Vec::new();
    while let Ok(first) = jobs.recv() {
        let mut latest = first;
        textures.set(&latest.delta);
        textures.free(&latest.delta);
        latest.delta.clear();
        while let Ok(mut next) = jobs.try_recv() {
            textures.set(&next.delta);
            textures.free(&next.delta);
            next.delta.clear();
            latest = next;
        }
        let size = (latest.size.0 as usize, latest.size.1 as usize);
        if size.0 == 0 || size.1 == 0 {
            continue;
        }
        Soft::paint(
            &textures,
            &latest.jobs,
            latest.pixels_per_point,
            size.0,
            size.1,
            &mut buffer,
        );
        let whole = size != previous_size || previous.len() != buffer.len();
        let mut rows = Vec::new();
        if !whole {
            let row = size.0 * 4;
            let same =
                |y: usize| buffer[y * row..(y + 1) * row] == previous[y * row..(y + 1) * row];
            let mut y = 0;
            while y < size.1 {
                if same(y) {
                    y += 1;
                    continue;
                }
                let from = y;
                while y < size.1 && !same(y) {
                    y += 1;
                }
                rows.push((from, y));
            }
            if rows.is_empty() {
                continue;
            }
        }
        previous.clone_from(&buffer);
        previous_size = size;
        if out
            .send(Painted {
                pixels: buffer.clone(),
                size,
                rows,
                whole,
            })
            .is_err()
        {
            return;
        }
    }
}
