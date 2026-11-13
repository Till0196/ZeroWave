// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ZeroWave のレイヤーを JavaScript で書くための、QuickJS の wasm。
//!
//! 本体（`zwsdk::host`）が提供する `zw_draw_v1`（canvas 2D）と `zw_layer_v1`（レイヤーの
//! 大きさと時計）、`zw_net_v1` の待たない通信を、JS の `globalThis.__zw` に定義する。canvas 2D の
//! インターフェース（`getContext("2d")` や `requestAnimationFrame`）と `fetch`・`WebSocket` は
//! TypeScript の `zwsdk` がこの上に作る。
//!
//! レイヤーの関数（`zw_layer_new`／`message`／`tick`／`frame`／`events`／`drop`）は
//! Rust のプラグインと同じで、それぞれ JS の `__zwNew`／`__zwMessage`／
//! `__zwTick`／`__zwFrame`／`__zwEvents`（`zwsdk` が置く）を呼ぶ。フレームは
//! 番号 1 の canvas にレンダリングし、`frame` は「canvas にレンダリングした」と返す。
//! スクリプトは本体が `zw_layer_v1` の `script` で渡す（wasm のカスタムセクション `zw_script`。
//! `zw-stamp` が書き込む）。`_start` で一度だけ評価し、あとは残った runtime をここから直接呼ぶ。
//!
//! ビルドは `sdk/javy/build.sh`。

use std::cell::RefCell;

use javy_plugin_api::javy::quickjs::{
    prelude::Func, prelude::Opt, prelude::Rest, Context, Ctx, Function, Object, TypedArray, Value,
};
use javy_plugin_api::{javy::Runtime, Config};

thread_local! {
    /// `modify_runtime` で受け取った context。
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

// 本体は渡されたアドレスと長さを確かめてコピーするだけで、guest のメモリーへ書くのは
// `get_transform`・`measure_text`・`buf_read` だけ。ほかはどんな引数で呼んでも壊れない。
#[link(wasm_import_module = "zw_draw_v1")]
unsafe extern "C" {
    safe fn canvas_new(w: f64, h: f64) -> u32;
    safe fn canvas_drop(id: u32);
    safe fn canvas_resize(id: u32, w: f64, h: f64);
    safe fn canvas_width(id: u32) -> f64;
    safe fn canvas_height(id: u32) -> f64;
    safe fn fill_style(id: u32, ptr: *const u8, len: u32);
    safe fn stroke_style(id: u32, ptr: *const u8, len: u32);
    safe fn font(id: u32, ptr: *const u8, len: u32);
    safe fn text_align(id: u32, ptr: *const u8, len: u32);
    safe fn text_baseline(id: u32, ptr: *const u8, len: u32);
    safe fn line_width(id: u32, w: f64);
    safe fn global_alpha(id: u32, a: f64);
    safe fn save(id: u32);
    safe fn restore(id: u32);
    safe fn translate(id: u32, x: f64, y: f64);
    safe fn scale(id: u32, x: f64, y: f64);
    safe fn rotate(id: u32, a: f64);
    safe fn transform(id: u32, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64);
    safe fn set_transform(id: u32, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64);
    fn get_transform(id: u32, out: *mut f64);
    safe fn fill_rect(id: u32, x: f64, y: f64, w: f64, h: f64);
    safe fn stroke_rect(id: u32, x: f64, y: f64, w: f64, h: f64);
    safe fn clear_rect(id: u32, x: f64, y: f64, w: f64, h: f64);
    safe fn begin_path(id: u32);
    safe fn close_path(id: u32);
    safe fn move_to(id: u32, x: f64, y: f64);
    safe fn line_to(id: u32, x: f64, y: f64);
    safe fn quad_to(id: u32, cx: f64, cy: f64, x: f64, y: f64);
    safe fn bezier_to(id: u32, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);
    safe fn rect(id: u32, x: f64, y: f64, w: f64, h: f64);
    safe fn arc(id: u32, x: f64, y: f64, r: f64, start: f64, end: f64, anticlockwise: u32);
    safe fn fill(id: u32);
    safe fn stroke(id: u32);
    safe fn clip(id: u32);
    safe fn fill_text(id: u32, ptr: *const u8, len: u32, x: f64, y: f64);
    safe fn stroke_text(id: u32, ptr: *const u8, len: u32, x: f64, y: f64);
    fn measure_text(id: u32, ptr: *const u8, len: u32, out: *mut f64);
    #[allow(clippy::too_many_arguments)]
    safe fn draw_canvas(
        id: u32,
        from: u32,
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    );
}

#[link(wasm_import_module = "zw_layer_v1")]
unsafe extern "C" {
    safe fn width() -> f64;
    safe fn height() -> f64;
    safe fn now_ms() -> f64;
    safe fn unix_ms() -> f64;
    /// 評価するスクリプト（buf の番号）。無ければ 0。
    safe fn script() -> u32;
}

#[link(wasm_import_module = "zw_buf_v1")]
unsafe extern "C" {
    #[link_name = "len"]
    safe fn buf_len(buf: u32) -> u32;
    #[link_name = "read"]
    fn buf_read(buf: u32, from: u32, ptr: *mut u8, len: u32) -> u32;
    #[link_name = "create"]
    safe fn buf_create(ptr: *const u8, len: u32) -> u32;
    #[link_name = "drop"]
    safe fn buf_drop(buf: u32);
}

// 待たない通信。`net` を宣言して許されたときだけ繋がり、そうでなければ `open`・`ws` は 0。
#[link(wasm_import_module = "zw_net_v1")]
unsafe extern "C" {
    safe fn open(
        method_ptr: *const u8,
        method_len: u32,
        url_ptr: *const u8,
        url_len: u32,
        headers: u32,
        body: u32,
    ) -> u32;
    safe fn ws(
        url_ptr: *const u8,
        url_len: u32,
        protocols_ptr: *const u8,
        protocols_len: u32,
        headers: u32,
    ) -> u32;
    safe fn poll(handle: u32) -> u32;
    safe fn send(handle: u32, kind: u32, ptr: *const u8, len: u32) -> u32;
    safe fn close(handle: u32);
}

/// 本体のバイト列をコピーする。0 なら空。
fn bytes_of(buf: u32) -> Vec<u8> {
    if buf == 0 {
        return Vec::new();
    }
    let len = buf_len(buf) as usize;
    let mut out = vec![0u8; len];
    if len > 0 {
        // SAFETY: 本体が書くのは `out` の長さの中だけ。
        unsafe { buf_read(buf, 0, out.as_mut_ptr(), len as u32) };
    }
    out
}

/// バイト列を本体に渡す（buf の番号）。
fn buf_of(bytes: &[u8]) -> u32 {
    buf_create(bytes.as_ptr(), bytes.len() as u32)
}

/// 呼び出しの間だけ本体に渡すバイト列。0 は無し。
struct Lent(u32);

impl Lent {
    fn of(bytes: Option<&[u8]>) -> Self {
        Self(bytes.map_or(0, buf_of))
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if self.0 != 0 {
            buf_drop(self.0);
        }
    }
}

fn config() -> Config {
    let mut c = Config::default();
    c.text_encoding(true);
    c
}

/// `__zw` に、import を一つずつ JS の関数として。
fn modify_runtime(runtime: Runtime) -> Runtime {
    runtime.context().with(|ctx| {
        let zw = Object::new(ctx.clone()).expect("object");
        macro_rules! put {
            ($name:literal, $f:expr) => {
                zw.set($name, Func::from($f)).expect($name);
            };
        }
        put!("canvasNew", |w: f64, h: f64| canvas_new(w, h));
        put!("canvasDrop", |id: u32| canvas_drop(id));
        put!("canvasResize", |id: u32, w: f64, h: f64| {
            canvas_resize(id, w, h)
        });
        put!("canvasWidth", |id: u32| canvas_width(id));
        put!("canvasHeight", |id: u32| canvas_height(id));
        put!("fillStyle", |id: u32, s: String| {
            fill_style(id, s.as_ptr(), s.len() as u32)
        });
        put!("strokeStyle", |id: u32, s: String| {
            stroke_style(id, s.as_ptr(), s.len() as u32)
        });
        put!("font", |id: u32, s: String| {
            font(id, s.as_ptr(), s.len() as u32)
        });
        put!("textAlign", |id: u32, s: String| {
            text_align(id, s.as_ptr(), s.len() as u32)
        });
        put!("textBaseline", |id: u32, s: String| {
            text_baseline(id, s.as_ptr(), s.len() as u32)
        });
        put!("lineWidth", |id: u32, w: f64| line_width(id, w));
        put!("globalAlpha", |id: u32, a: f64| global_alpha(id, a));
        put!("save", |id: u32| save(id));
        put!("restore", |id: u32| restore(id));
        put!("translate", |id: u32, x: f64, y: f64| {
            translate(id, x, y)
        });
        put!("scale", |id: u32, x: f64, y: f64| scale(id, x, y));
        put!("rotate", |id: u32, a: f64| rotate(id, a));
        put!("transform", |id: u32,
                           a: f64,
                           b: f64,
                           c: f64,
                           d: f64,
                           e: f64,
                           f: f64| {
            transform(id, a, b, c, d, e, f)
        });
        put!("setTransform", |id: u32,
                              a: f64,
                              b: f64,
                              c: f64,
                              d: f64,
                              e: f64,
                              f: f64| {
            set_transform(id, a, b, c, d, e, f)
        });
        put!("getTransform", |id: u32| -> Vec<f64> {
            let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            // SAFETY: 本体が書くのは `m` の 6 個の f64 だけ。
            unsafe { get_transform(id, m.as_mut_ptr()) };
            m.to_vec()
        });
        put!("fillRect", |id: u32, x: f64, y: f64, w: f64, h: f64| {
            fill_rect(id, x, y, w, h)
        });
        put!("strokeRect", |id: u32, x: f64, y: f64, w: f64, h: f64| {
            stroke_rect(id, x, y, w, h)
        });
        put!("clearRect", |id: u32, x: f64, y: f64, w: f64, h: f64| {
            clear_rect(id, x, y, w, h)
        });
        put!("beginPath", |id: u32| begin_path(id));
        put!("closePath", |id: u32| close_path(id));
        put!("moveTo", |id: u32, x: f64, y: f64| move_to(id, x, y));
        put!("lineTo", |id: u32, x: f64, y: f64| line_to(id, x, y));
        put!(
            "quadraticCurveTo",
            |id: u32, cx: f64, cy: f64, x: f64, y: f64| quad_to(id, cx, cy, x, y)
        );
        put!("bezierCurveTo", |id: u32,
                               c1x: f64,
                               c1y: f64,
                               c2x: f64,
                               c2y: f64,
                               x: f64,
                               y: f64| {
            bezier_to(id, c1x, c1y, c2x, c2y, x, y)
        });
        put!("rect", |id: u32, x: f64, y: f64, w: f64, h: f64| {
            rect(id, x, y, w, h)
        });
        put!("arc", |id: u32,
                     x: f64,
                     y: f64,
                     r: f64,
                     start: f64,
                     end: f64,
                     anticlockwise: bool| {
            arc(id, x, y, r, start, end, u32::from(anticlockwise))
        });
        put!("fill", |id: u32| fill(id));
        put!("stroke", |id: u32| stroke(id));
        put!("clip", |id: u32| clip(id));
        put!("fillText", |id: u32, s: String, x: f64, y: f64| {
            fill_text(id, s.as_ptr(), s.len() as u32, x, y)
        });
        put!("strokeText", |id: u32, s: String, x: f64, y: f64| {
            stroke_text(id, s.as_ptr(), s.len() as u32, x, y)
        });
        put!("measureText", |id: u32, s: String| -> Vec<f64> {
            let mut out = [0.0; 3];
            // SAFETY: 本体が書くのは `out` の 3 個の f64 だけ。
            unsafe { measure_text(id, s.as_ptr(), s.len() as u32, out.as_mut_ptr()) };
            out.to_vec()
        });
        // 引数が多いので、残りは配列で受け取る（sx, sy, sw, sh, dx, dy, dw, dh）。
        put!("drawCanvas", |id: u32, from: u32, r: Rest<f64>| {
            let a: Vec<f64> = r.0;
            if a.len() == 8 {
                draw_canvas(id, from, a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7])
            }
        });
        put!("layerWidth", || width());
        put!("layerHeight", || height());
        put!("nowMs", || now_ms());
        put!("unixMs", || unix_ms());
        // ヘッダーは `名前: 値` を改行で並べたもの。本文が無ければ null。
        put!(
            "netOpen",
            |method: String, url: String, headers: String, body: Opt<TypedArray<'_, u8>>| {
                let headers = Lent::of((!headers.is_empty()).then_some(headers.as_bytes()));
                let body = Lent::of(body.0.as_ref().map(|body| body.as_bytes().unwrap_or(&[])));
                open(
                    method.as_ptr(),
                    method.len() as u32,
                    url.as_ptr(),
                    url.len() as u32,
                    headers.0,
                    body.0,
                )
            }
        );
        put!(
            "netWebSocket",
            |url: String, protocols: String, headers: String| {
                let headers = Lent::of((!headers.is_empty()).then_some(headers.as_bytes()));
                ws(
                    url.as_ptr(),
                    url.len() as u32,
                    protocols.as_ptr(),
                    protocols.len() as u32,
                    headers.0,
                )
            }
        );
        // 届いた記録（`zwsdk::net::conn`）をそのまま。無ければ null。
        put!("netPoll", |ctx: Ctx<'_>, handle: u32| {
            let out = Lent(poll(handle));
            if out.0 == 0 {
                return None;
            }
            TypedArray::<u8>::new(ctx, bytes_of(out.0)).ok()
        });
        put!(
            "netSend",
            |handle: u32, kind: u32, bytes: TypedArray<'_, u8>| {
                let bytes = bytes.as_bytes().unwrap_or(&[]);
                send(handle, kind, bytes.as_ptr(), bytes.len() as u32) != 0
            }
        );
        put!("netClose", |handle: u32| close(handle));
        ctx.globals().set("__zw", zw).expect("__zw");
    });
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(runtime.context().clone()));
    runtime
}

/// JS の関数を一つ呼び、例外は stderr（本体のログ）へ。無ければ `None`。
fn call<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    args: impl javy_plugin_api::javy::quickjs::function::IntoArgs<'js>,
) -> Option<Value<'js>> {
    let function: Result<Function, _> = ctx.globals().get(name);
    let Ok(function) = function else {
        eprintln!("zwjavy: {name} が無い");
        return None;
    };
    let out = match function.call::<_, Value>(args) {
        Ok(out) => Some(out),
        Err(error) => {
            let text = ctx
                .catch()
                .as_exception()
                .and_then(|e| e.message())
                .unwrap_or_else(|| error.to_string());
            eprintln!("zwjavy: {name}: {text}");
            None
        }
    };
    while ctx.execute_pending_job() {}
    out
}

/// runtime の上で一つの処理を実行する。
fn with_context<T>(what: impl FnOnce(&Ctx<'_>) -> T) -> Option<T> {
    CONTEXT.with(|slot| {
        let slot = slot.borrow();
        let Some(context) = slot.as_ref() else {
            eprintln!("zwjavy: runtime が無い");
            return None;
        };
        Some(context.with(|ctx| what(&ctx)))
    })
}

/// 設定（JSON）を `__zwNew` へ。インスタンスは一つなので番号は 1。
#[export_name = "zw_layer_new"]
extern "C" fn zw_layer_new(config: u32) -> u32 {
    let config = String::from_utf8_lossy(&bytes_of(config)).into_owned();
    with_context(|ctx| {
        call(ctx, "__zwNew", (config,));
    });
    1
}

/// 本体のメッセージ（JSON）と添えたバイト列を `__zwMessage(json, bytes)` へ。
#[export_name = "zw_layer_message"]
extern "C" fn zw_layer_message(_handle: u32, json: u32, bytes: u32) {
    let json = String::from_utf8_lossy(&bytes_of(json)).into_owned();
    let bytes = bytes_of(bytes);
    with_context(|ctx| {
        let array = TypedArray::<u8>::new(ctx.clone(), bytes).ok();
        call(ctx, "__zwMessage", (json, array));
    });
}

/// 時刻を進める：`__zwTick()`（タイマー、requestAnimationFrame）。
#[export_name = "zw_layer_tick"]
extern "C" fn zw_layer_tick(_handle: u32) -> u32 {
    with_context(|ctx| {
        call(ctx, "__zwTick", ());
    });
    0
}

/// 一回レンダリングする：`__zwFrame()` を呼ぶ。偽を返したら「変わっていない」、
/// そうでなければ番号 1 の canvas にレンダリングしたと本体に返す。
#[export_name = "zw_layer_frame"]
extern "C" fn zw_layer_frame(_handle: u32) -> u32 {
    let drew = with_context(|ctx| {
        call(ctx, "__zwFrame", ())
            .map(|out| !matches!(out.as_bool(), Some(false)))
            .unwrap_or(false)
    })
    .unwrap_or(false);
    if !drew {
        return 0;
    }
    buf_of(b"{\"source\":\"canvas\"}\n")
}

/// キューに入った要求：`__zwEvents()` が JSON の配列の文字列を返す。
#[export_name = "zw_layer_events"]
extern "C" fn zw_layer_events(_handle: u32) -> u32 {
    let json = with_context(|ctx| {
        call(ctx, "__zwEvents", ())
            .and_then(|out| out.as_string().and_then(|s| s.to_string().ok()))
            .unwrap_or_default()
    })
    .unwrap_or_default();
    if json.is_empty() || json == "[]" {
        return 0;
    }
    let mut out = json.into_bytes();
    out.push(b'\n');
    buf_of(&out)
}

#[export_name = "zw_layer_drop"]
extern "C" fn zw_layer_drop(_handle: u32) {}

#[export_name = "_start"]
extern "C" fn start() {
    let source = bytes_of(script());
    if source.is_empty() {
        eprintln!("zwjavy: スクリプトが無い（zw-stamp で書き込む）");
        return;
    }
    let ran = javy_plugin_api::initialize_runtime(config, modify_runtime)
        .and_then(|()| javy_plugin_api::compile_src(&source))
        .and_then(|bytecode| javy_plugin_api::invoke(&bytecode, None));
    if let Err(error) = ran {
        eprintln!("zwjavy: {error}");
    }
}
