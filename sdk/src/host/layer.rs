// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `layer` hook：レイヤーのプラグイン一つを読み込む。番号 1 の canvas はレイヤー自体。
//! Javy のモジュール向けに WASI p1 の最小限を提供する。

use std::time::Instant;

use wasmtime::{Caller, Linker, TypedFunc};

use crate::abi;
use crate::draw::Canvas;
use crate::host::{
    HostState, Level, Made, guest_bytes, guest_memory, guest_text, since_epoch, write_guest,
};
use crate::slots::Slots;

#[derive(Debug)]
pub(crate) struct Draw {
    canvases: Slots<Canvas>,
    fonts: crate::draw::Fonts,
    began: Instant,
}

const ROOT: u32 = 1;

impl Draw {
    pub(crate) fn new(fonts: crate::draw::FontContext, width: f64, height: f64) -> Self {
        let mut canvases = Slots::with_most(256);
        canvases.put(Canvas::new(width, height));
        Self {
            canvases,
            fonts: crate::draw::Fonts::new(fonts),
            began: Instant::now(),
        }
    }

    fn get(&mut self, id: u32) -> Option<&mut Canvas> {
        self.canvases.get_mut(id)
    }

    fn root(&self) -> &Canvas {
        self.canvases.get(ROOT).expect("番号 1 は本体が作る")
    }
}

fn draw<'a>(caller: &'a mut Caller<'_, HostState>) -> Option<&'a mut Draw> {
    caller.data_mut().draw.as_mut()
}

fn write_f64s(caller: &mut Caller<'_, HostState>, ptr: u32, values: &[f64]) {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    write_guest(caller, ptr, &bytes);
}

fn write_u32(caller: &mut Caller<'_, HostState>, ptr: u32, v: u32) {
    write_guest(caller, ptr, &v.to_le_bytes());
}

fn write_u64(caller: &mut Caller<'_, HostState>, ptr: u32, v: u64) {
    write_guest(caller, ptr, &v.to_le_bytes());
}

macro_rules! op {
    ($linker:ident, $name:literal, |$c:ident $(, $a:ident)*| $body:expr) => {
        $linker.func_wrap(
            abi::import::DRAW,
            $name,
            |mut caller: Caller<'_, HostState>, id: u32 $(, $a: f64)*| {
                if let Some($c) = draw(&mut caller).and_then(|d| d.get(id)) {
                    $body;
                }
            },
        )?;
    };
}

macro_rules! style {
    ($linker:ident, $name:literal, |$c:ident, $s:ident| $body:expr) => {
        $linker.func_wrap(
            abi::import::DRAW,
            $name,
            |mut caller: Caller<'_, HostState>, id: u32, ptr: u32, len: u32| {
                let $s = guest_text(&mut caller, ptr, len);
                if let Some($c) = draw(&mut caller).and_then(|d| d.get(id)) {
                    $body;
                }
            },
        )?;
    };
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::DRAW,
        "canvas_new",
        |mut caller: Caller<'_, HostState>, w: f64, h: f64| -> u32 {
            draw(&mut caller).map_or(0, |d| d.canvases.put(Canvas::new(w.max(0.0), h.max(0.0))))
        },
    )?;
    linker.func_wrap(
        abi::import::DRAW,
        "canvas_drop",
        |mut caller: Caller<'_, HostState>, id: u32| {
            if id != ROOT
                && let Some(d) = draw(&mut caller)
            {
                d.canvases.take(id);
            }
        },
    )?;
    op!(linker, "canvas_resize", |c, w, h| c
        .resize(w.max(0.0), h.max(0.0)));
    linker.func_wrap(
        abi::import::DRAW,
        "canvas_width",
        |mut caller: Caller<'_, HostState>, id: u32| -> f64 {
            draw(&mut caller)
                .and_then(|d| d.get(id))
                .map_or(0.0, |c| c.width())
        },
    )?;
    linker.func_wrap(
        abi::import::DRAW,
        "canvas_height",
        |mut caller: Caller<'_, HostState>, id: u32| -> f64 {
            draw(&mut caller)
                .and_then(|d| d.get(id))
                .map_or(0.0, |c| c.height())
        },
    )?;

    style!(linker, "fill_style", |c, s| c.state.fill =
        crate::draw::colour(&s));
    style!(linker, "stroke_style", |c, s| c.state.stroke =
        crate::draw::colour(&s));
    style!(linker, "font", |c, s| c.state.font = s);
    style!(linker, "text_align", |c, s| c.state.align =
        crate::draw::Align::parse(&s));
    style!(linker, "text_baseline", |c, s| c.state.baseline =
        crate::draw::Baseline::parse(&s));
    op!(linker, "line_width", |c, w| {
        if w.is_finite() && w > 0.0 {
            c.state.line_width = w;
        }
    });
    op!(linker, "global_alpha", |c, a| {
        if a.is_finite() {
            c.state.alpha = a.clamp(0.0, 1.0) as f32;
        }
    });

    op!(linker, "save", |c| c.save());
    op!(linker, "restore", |c| {
        c.restore();
    });
    op!(linker, "translate", |c, x, y| c.translate(x, y));
    op!(linker, "scale", |c, x, y| c.scale(x, y));
    op!(linker, "rotate", |c, a| c.rotate(a));
    op!(linker, "transform", |c, a, b, cc, d, e, f| c
        .transform([a, b, cc, d, e, f]));
    op!(linker, "set_transform", |c, a, b, cc, d, e, f| c
        .set_transform([a, b, cc, d, e, f]));
    linker.func_wrap(
        abi::import::DRAW,
        "get_transform",
        |mut caller: Caller<'_, HostState>, id: u32, out: u32| {
            let m = draw(&mut caller)
                .and_then(|d| d.get(id))
                .map_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], |c| c.get_transform());
            write_f64s(&mut caller, out, &m);
        },
    )?;

    op!(linker, "fill_rect", |c, x, y, w, h| c.fill_rect(x, y, w, h));
    op!(linker, "stroke_rect", |c, x, y, w, h| c
        .stroke_rect(x, y, w, h));
    op!(linker, "clear_rect", |c, x, y, w, h| c
        .clear_rect(x, y, w, h));
    op!(linker, "begin_path", |c| c.begin_path());
    op!(linker, "close_path", |c| c.close_path());
    op!(linker, "move_to", |c, x, y| c.move_to(x, y));
    op!(linker, "line_to", |c, x, y| c.line_to(x, y));
    op!(linker, "quad_to", |c, cx, cy, x, y| c.quad_to(cx, cy, x, y));
    op!(linker, "bezier_to", |c, c1x, c1y, c2x, c2y, x, y| c
        .bezier_to(c1x, c1y, c2x, c2y, x, y));
    op!(linker, "rect", |c, x, y, w, h| c.rect(x, y, w, h));
    linker.func_wrap(
        abi::import::DRAW,
        "arc",
        |mut caller: Caller<'_, HostState>,
         id: u32,
         x: f64,
         y: f64,
         r: f64,
         start: f64,
         end: f64,
         anticlockwise: u32| {
            if let Some(c) = draw(&mut caller).and_then(|d| d.get(id)) {
                c.arc(x, y, r, start, end, anticlockwise != 0);
            }
        },
    )?;
    op!(linker, "fill", |c| c.fill());
    op!(linker, "stroke", |c| c.stroke());
    op!(linker, "clip", |c| c.clip());

    for (name, stroke) in [("fill_text", false), ("stroke_text", true)] {
        linker.func_wrap(
            abi::import::DRAW,
            name,
            move |mut caller: Caller<'_, HostState>,
                  id: u32,
                  ptr: u32,
                  len: u32,
                  x: f64,
                  y: f64| {
                let s = guest_text(&mut caller, ptr, len);
                let Some(Draw {
                    canvases, fonts, ..
                }) = draw(&mut caller)
                else {
                    return;
                };
                let Some(c) = canvases.get_mut(id) else {
                    return;
                };
                if stroke {
                    c.stroke_text(fonts, &s, x, y);
                } else {
                    c.fill_text(fonts, &s, x, y);
                }
            },
        )?;
    }
    linker.func_wrap(
        abi::import::DRAW,
        "measure_text",
        |mut caller: Caller<'_, HostState>, id: u32, ptr: u32, len: u32, out: u32| {
            let s = guest_text(&mut caller, ptr, len);
            let line = draw(&mut caller).and_then(
                |Draw {
                     canvases, fonts, ..
                 }| { canvases.get(id).map(|c| c.measure_text(fonts, &s)) },
            );
            let (w, a, dsc) = line.map_or((0.0, 0.0, 0.0), |l| {
                (
                    f64::from(l.width),
                    f64::from(l.ascent),
                    f64::from(l.descent),
                )
            });
            write_f64s(&mut caller, out, &[w, a, dsc]);
        },
    )?;
    linker.func_wrap(
        abi::import::DRAW,
        "draw_canvas",
        |mut caller: Caller<'_, HostState>,
         id: u32,
         from: u32,
         sx: f64,
         sy: f64,
         sw: f64,
         sh: f64,
         dx: f64,
         dy: f64,
         dw: f64,
         dh: f64| {
            if let Some((c, source)) = draw(&mut caller).and_then(|d| d.canvases.pair(id, from)) {
                c.draw_canvas(source, sx, sy, sw, sh, dx, dy, dw, dh);
            }
        },
    )?;

    linker.func_wrap(
        abi::import::LAYER,
        "width",
        |mut caller: Caller<'_, HostState>| -> f64 {
            draw(&mut caller).map_or(0.0, |d| d.root().width())
        },
    )?;
    linker.func_wrap(
        abi::import::LAYER,
        "height",
        |mut caller: Caller<'_, HostState>| -> f64 {
            draw(&mut caller).map_or(0.0, |d| d.root().height())
        },
    )?;
    linker.func_wrap(
        abi::import::LAYER,
        "now_ms",
        |mut caller: Caller<'_, HostState>| -> f64 {
            draw(&mut caller).map_or(0.0, |d| d.began.elapsed().as_secs_f64() * 1000.0)
        },
    )?;
    linker.func_wrap(abi::import::LAYER, "unix_ms", || -> f64 {
        since_epoch().as_secs_f64() * 1000.0
    })?;
    Ok(())
}

const STDIO_LINE_MOST: usize = 8 << 10;

const FRAME_SIDE_MOST: u32 = 8192;

pub(crate) fn link_wasi(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    const WASI: &str = "wasi_snapshot_preview1";
    const EBADF: i32 = 8;
    const EFAULT: i32 = 21;
    const EIO: i32 = 29;
    const ESPIPE: i32 = 70;
    linker.func_wrap(
        WASI,
        "args_get",
        |_: Caller<'_, HostState>, _: i32, _: i32| -> i32 { 0 },
    )?;
    linker.func_wrap(
        WASI,
        "args_sizes_get",
        |mut caller: Caller<'_, HostState>, count: i32, size: i32| -> i32 {
            write_u32(&mut caller, count as u32, 0);
            write_u32(&mut caller, size as u32, 0);
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "environ_get",
        |_: Caller<'_, HostState>, _: i32, _: i32| -> i32 { 0 },
    )?;
    linker.func_wrap(
        WASI,
        "environ_sizes_get",
        |mut caller: Caller<'_, HostState>, count: i32, size: i32| -> i32 {
            write_u32(&mut caller, count as u32, 0);
            write_u32(&mut caller, size as u32, 0);
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "clock_time_get",
        |mut caller: Caller<'_, HostState>, id: i32, _: i64, out: i32| -> i32 {
            let ns = if id == 0 {
                since_epoch().as_nanos() as u64
            } else {
                // 単調な時計は必ず進める。止まっていると JS のタイマーが二度と発火しない。
                caller.data().began.elapsed().as_nanos() as u64
            };
            write_u64(&mut caller, out as u32, ns);
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "random_get",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
            let Some(memory) = guest_memory(&mut caller) else {
                return EBADF;
            };
            let Some(slot) = (ptr as u32 as usize)
                .checked_add(len as u32 as usize)
                .and_then(|end| {
                    memory
                        .data_mut(&mut caller)
                        .get_mut(ptr as u32 as usize..end)
                })
            else {
                return EFAULT;
            };
            use ring::rand::SecureRandom;
            if ring::rand::SystemRandom::new().fill(slot).is_err() {
                return EIO;
            }
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "fd_close",
        |_: Caller<'_, HostState>, _: i32| -> i32 { EBADF },
    )?;
    // 起動時に標準入出力の flags を設定する言語がある（Go など）。無いと instantiate が失敗する。
    linker.func_wrap(
        WASI,
        "fd_fdstat_set_flags",
        |_: Caller<'_, HostState>, fd: i32, _: i32| -> i32 {
            if (0..=2).contains(&fd) { 0 } else { EBADF }
        },
    )?;
    linker.func_wrap(
        WASI,
        "fd_fdstat_get",
        |mut caller: Caller<'_, HostState>, fd: i32, out: i32| -> i32 {
            if !(0..=2).contains(&fd) {
                return EBADF;
            }
            write_u64(&mut caller, out as u32, 2);
            write_u64(&mut caller, out as u32 + 8, 0);
            write_u64(&mut caller, out as u32 + 16, 0);
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "fd_seek",
        |_: Caller<'_, HostState>, _: i32, _: i64, _: i32, _: i32| -> i32 { ESPIPE },
    )?;
    linker.func_wrap(
        WASI,
        "fd_read",
        |_: Caller<'_, HostState>, _: i32, _: i32, _: i32, _: i32| -> i32 { EBADF },
    )?;
    linker.func_wrap(
        WASI,
        "fd_write",
        |mut caller: Caller<'_, HostState>, fd: i32, iovs: i32, count: i32, written: i32| -> i32 {
            if !(1..=2).contains(&fd) {
                return EBADF;
            }
            let mut bytes = Vec::new();
            for n in 0..count.max(0) as u32 {
                let at = iovs as u32 + n * 8;
                let Some(head) = guest_bytes(&mut caller, at, 8) else {
                    break;
                };
                let ptr = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
                let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
                if let Some(part) = guest_bytes(&mut caller, ptr, len) {
                    bytes.extend_from_slice(&part);
                }
            }
            let total = bytes.len() as u32;
            write_u32(&mut caller, written as u32, total);
            let level = if fd == 2 { Level::Warning } else { Level::Info };
            // `eprintln!` は書式の区切りごとに書くので一行になるまでバッファに貯める。
            let state = caller.data_mut();
            let held = &mut state.stdio[(fd - 1) as usize];
            held.extend_from_slice(&bytes);
            let mut lines = Vec::new();
            while let Some(at) = held.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = held.drain(..=at).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                if !text.is_empty() {
                    lines.push(text);
                }
            }
            if held.len() >= STDIO_LINE_MOST {
                lines.push(String::from_utf8_lossy(held).into_owned());
                held.clear();
            }
            for line in lines {
                caller.data().log(level, &line);
            }
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "fd_prestat_get",
        |_: Caller<'_, HostState>, _: i32, _: i32| -> i32 { EBADF },
    )?;
    linker.func_wrap(
        WASI,
        "fd_prestat_dir_name",
        |_: Caller<'_, HostState>, _: i32, _: i32, _: i32| -> i32 { EBADF },
    )?;
    linker.func_wrap(WASI, "sched_yield", || -> i32 { 0 })?;
    const ENOENT: i32 = 44;
    const ENOTSUP: i32 = 58;
    linker.func_wrap(
        WASI,
        "fd_filestat_get",
        |_: Caller<'_, HostState>, _: i32, _: i32| -> i32 { EBADF },
    )?;
    linker.func_wrap(
        WASI,
        "path_filestat_get",
        |_: Caller<'_, HostState>, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOENT },
    )?;
    linker.func_wrap(
        WASI,
        "path_open",
        |_: Caller<'_, HostState>,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i64,
         _: i64,
         _: i32,
         _: i32|
         -> i32 { ENOENT },
    )?;
    linker.func_wrap(
        WASI,
        "poll_oneoff",
        |mut caller: Caller<'_, HostState>, _: i32, _: i32, count: i32, out: i32| -> i32 {
            write_u32(&mut caller, out as u32, count as u32);
            0
        },
    )?;
    linker.func_wrap(
        WASI,
        "fd_readdir",
        |_: Caller<'_, HostState>, _: i32, _: i32, _: i32, _: i64, _: i32| -> i32 { ENOTSUP },
    )?;
    linker.func_wrap(
        WASI,
        "proc_exit",
        |code: i32| -> Result<(), wasmtime::Error> { Err(Exit(code).into()) },
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct Exit(pub i32);

impl std::fmt::Display for Exit {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "proc_exit({})", self.0)
    }
}

impl std::error::Error for Exit {}

pub type Painted = Option<(crate::layer::Frame, Vec<u8>)>;

pub struct LayerInstance {
    name: String,
    made: Made,
    handle: u32,
    /// 一度でも trap した。以後は触らない（解放されていないリソースがあり、またクラッシュするだけ）。
    poisoned: bool,
    message: TypedFunc<(u32, u32, u32), ()>,
    tick: TypedFunc<u32, u32>,
    frame: TypedFunc<u32, u32>,
    events: TypedFunc<u32, u32>,
    drop: TypedFunc<u32, ()>,
    renderer: crate::draw::Renderer,
}

impl std::fmt::Debug for LayerInstance {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("LayerInstance")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl LayerInstance {
    pub(super) fn new(
        name: String,
        mut made: Made,
        config: &crate::layer::Config,
    ) -> Result<Self, String> {
        if let Ok(start) = made.func::<(), ()>("_start")
            && let Err(error) = start.call(made.armed(), ())
        {
            match error.downcast_ref::<Exit>() {
                Some(Exit(0)) => {}
                _ => return Err(format!("_start: {error}")),
            }
        }
        let new: TypedFunc<u32, u32> = made.func(abi::export::LAYER_NEW)?;
        let message = made.func(abi::export::LAYER_MESSAGE)?;
        let tick = made.func(abi::export::LAYER_TICK)?;
        let frame = made.func(abi::export::LAYER_FRAME)?;
        let events = made.func(abi::export::LAYER_EVENTS)?;
        let drop = made.func(abi::export::LAYER_DROP)?;
        let config = made.give(crate::layer::to_json(config))?;
        let handle = new
            .call(made.armed(), config)
            .map_err(|error| format!("{}: {error}", abi::export::LAYER_NEW))?;
        made.store.data_mut().bufs.take(config);
        if handle == 0 {
            return Err(format!("{} が 0 を返した", abi::export::LAYER_NEW));
        }
        Ok(Self {
            name,
            made,
            handle,
            poisoned: false,
            message,
            tick,
            frame,
            events,
            drop,
            renderer: crate::draw::Renderer::default(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn check<T>(&mut self, what: &str, result: wasmtime::Result<T>) -> Result<T, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.poisoned = true;
                Err(format!("{what}: {error}"))
            }
        }
    }

    fn took(&mut self, buf: u32) -> Vec<u8> {
        self.made
            .store
            .data_mut()
            .bufs
            .take(buf)
            .unwrap_or_default()
    }

    fn alive(&self) -> Result<(), String> {
        if self.poisoned {
            Err(String::from("クラッシュしたまま"))
        } else {
            Ok(())
        }
    }

    pub fn message(&mut self, message: &crate::layer::Message, bytes: &[u8]) -> Result<(), String> {
        self.alive()?;
        // 渡せないのは guest が解放せずに貯めたから。以後も同じなので止める。
        let json = self
            .made
            .give(crate::layer::to_json(message))
            .inspect_err(|_| self.poisoned = true)?;
        let bytes = if bytes.is_empty() {
            0
        } else {
            match self.made.give(bytes.to_vec()) {
                Ok(handle) => handle,
                Err(why) => {
                    self.made.store.data_mut().bufs.take(json);
                    self.poisoned = true;
                    return Err(why);
                }
            }
        };
        let called = self
            .message
            .call(self.made.armed(), (self.handle, json, bytes));
        let bufs = &mut self.made.store.data_mut().bufs;
        bufs.take(json);
        bufs.take(bytes);
        self.check(abi::export::LAYER_MESSAGE, called)
    }

    pub fn tick(&mut self) -> Result<crate::layer::Ticked, String> {
        self.alive()?;
        let called = self.tick.call(self.made.armed(), self.handle);
        let out = self.check(abi::export::LAYER_TICK, called)?;
        if out == 0 {
            return Ok(crate::layer::Ticked::default());
        }
        let bytes = self.took(out);
        Ok(crate::layer::from_json(&bytes).unwrap_or_default())
    }

    /// `Some(None)` は消えた。canvas のレイヤーは `area` の RGBA にする。
    pub fn frame(&mut self, area: (u32, u32)) -> Result<Option<Painted>, String> {
        self.alive()?;
        let called = self.frame.call(self.made.armed(), self.handle);
        let out = self.check(abi::export::LAYER_FRAME, called)?;
        if out == 0 {
            return Ok(None);
        }
        let bytes = self.took(out);
        let Some((mut frame, rgba)) = crate::layer::decode_frame(&bytes) else {
            return Ok(Some(None));
        };
        match frame.source {
            crate::layer::Source::Rgba => {
                // 幅×高さ×4 と長さが合わない画像は egui が assert で止まる。
                let fits = frame.width <= FRAME_SIDE_MOST
                    && frame.height <= FRAME_SIDE_MOST
                    && (frame.width as usize)
                        .checked_mul(frame.height as usize)
                        .and_then(|n| n.checked_mul(4))
                        == Some(rgba.len());
                if !fits {
                    self.made.store.data().log(
                        Level::Warning,
                        &format!(
                            "{}: 画像のサイズ {}×{} と長さ {} が合わないので破棄する",
                            self.name,
                            frame.width,
                            frame.height,
                            rgba.len()
                        ),
                    );
                    return Ok(Some(None));
                }
                Ok(Some(Some((frame, rgba.to_vec()))))
            }
            crate::layer::Source::Canvas => {
                let Some(draw) = self.made.store.data().draw.as_ref() else {
                    return Ok(Some(None));
                };
                let (width, height) = (area.0.max(1), area.1.max(1));
                let scene = draw.root().scene_for(f64::from(width), f64::from(height));
                let mut rgba = Vec::new();
                self.renderer.render_into(&scene, width, height, &mut rgba);
                frame.width = width;
                frame.height = height;
                if frame.size == (0.0, 0.0) {
                    frame.size = (draw.root().width() as f32, draw.root().height() as f32);
                }
                Ok(Some(Some((frame, rgba))))
            }
        }
    }

    pub fn events(&mut self) -> Result<(Vec<crate::layer::Event>, Vec<Vec<u8>>), String> {
        self.alive()?;
        let called = self.events.call(self.made.armed(), self.handle);
        let out = self.check(abi::export::LAYER_EVENTS, called)?;
        if out == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        Ok(crate::layer::decode_events(&self.took(out)))
    }
}

impl Drop for LayerInstance {
    fn drop(&mut self) {
        if !self.poisoned {
            let _ = self.drop.call(self.made.armed(), self.handle);
        }
    }
}
