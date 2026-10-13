// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ウィンドウ上の EGL のコンテキストとサーフェス。[`Present`](super::Present) は映像のフォーマットが変わるたび作り直すが、
//! こちらはウィンドウと同じだけ生き、映像の無い間も画面（[`Layer`]）を出せ、オーバーレイの GL リソースも消えない。
//! レンダリングするのは一つのスレッドだけ（EGL のコンテキストはスレッドに結びつく）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::egl::{self, Egl};
use super::gl::{self, Gl};
use super::{Error, Layer, LayerTarget, Native, wayland};
use crate::output::video::{Mapping, Primaries, Space, Transfer};

static CONTEXTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct Window {
    pub(super) egl: Rc<Egl>,
    pub(super) gl: Rc<Gl>,
    pub(super) display: egl::EGLDisplay,
    pub(super) context: egl::EGLContext,
    /// HDR と SDR を切り替えるとき作り直す（[`Window::set_space`]）。
    surface: Cell<egl::EGLSurface>,
    /// SDR の画面の設定（8 ビット）。
    config: egl::EGLConfig,
    /// HDR の画面を出せるときだけ。
    hdr: Option<Hdr>,
    /// いまの画面の符号値の色空間。
    space: Cell<Space>,
    /// HDR の画面に画面を重ねるための GL の物。
    ui: RefCell<Option<Ui>>,
    vsync: Cell<bool>,
    /// 対象のウィンドウ。ウィンドウが無ければ（計測・テスト）1x1 の pbuffer。
    pub(super) native: Option<Native>,
    /// Wayland のウィンドウのときの `wl_egl_window`。EGL のサーフェスを破棄した後に破棄する（`Drop`）。
    wayland: RefCell<Option<wayland::EglWindow>>,
    pub(super) generation: u64,
    size: Cell<(u32, u32)>,
    blank: Cell<bool>,
    swapped: Cell<std::time::Instant>,
}

impl std::fmt::Debug for Window {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Window")
            .field("context", &self.context)
            .field("generation", &self.generation)
            .field("size", &self.size.get())
            .finish_non_exhaustive()
    }
}

/// HDR の画面の符号値を合成側へどう伝えるか。どちらも 10 ビットのサーフェスに符号化した値を書く。
enum Route {
    /// EGL がサーフェスの色空間として伝える（`EGL_EXT_gl_colorspace_bt2020_*`。Android）。
    Egl { pq: bool, hlg: bool },
    /// Mesa の EGL は Wayland でも sRGB とリニアしか受けないので、`wp_color_management_v1` で伝える。
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    Wayland(RefCell<super::described::Described>),
}

struct Hdr {
    route: Route,
    ten: egl::EGLConfig,
}

impl std::fmt::Debug for Hdr {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self.route {
            Route::Egl { .. } => "Hdr(EGL)",
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            Route::Wayland(_) => "Hdr(Wayland)",
        })
    }
}

impl Hdr {
    fn offers(&self, space: Space) -> bool {
        match (&self.route, space) {
            (_, Space::Srgb) => true,
            (Route::Egl { pq, .. }, Space::Pq) => *pq,
            (Route::Egl { hlg, .. }, Space::Hlg) => *hlg,
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            (Route::Wayland(described), space) => described.borrow().offers(space),
            _ => false,
        }
    }

    /// コンテキストを設定に縛らずに作れ（サーフェスだけ作り直す）、10 ビットの設定があり、伝える手段があるときだけ。
    fn of(egl: &Egl, display: egl::EGLDisplay, native: &Native) -> Option<Self> {
        let said = |why: &str| {
            tracing::info!("GLES 2.0 の画面は SDR のまま（{why}）");
            None
        };
        if !egl.offers(display, "EGL_KHR_no_config_context") {
            return said("EGL_KHR_no_config_context がありません");
        }
        let Some(ten) = egl.ten_bit_config(display) else {
            return said("10 ビットのウィンドウの EGLConfig がありません");
        };
        let route = match native {
            Native::Android { .. } => {
                if !egl.offers(display, "EGL_KHR_gl_colorspace") {
                    return said("EGL_KHR_gl_colorspace がありません");
                }
                let pq = egl.offers(display, "EGL_EXT_gl_colorspace_bt2020_pq");
                let hlg = egl.offers(display, "EGL_EXT_gl_colorspace_bt2020_hlg");
                if !pq && !hlg {
                    return said("EGL_EXT_gl_colorspace_bt2020_pq も _hlg もありません");
                }
                Route::Egl { pq, hlg }
            }
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            Native::Wayland {
                surface,
                display: connection,
                ..
            } => {
                // SAFETY: どちらも winit が開いたウィンドウの物で、`Window` より長く生きる（`on_window`）。
                match unsafe { super::described::Described::new(*connection, *surface) } {
                    Ok(described) => Route::Wayland(RefCell::new(described)),
                    Err(why) => return said(&why),
                }
            }
            _ => return said("このウィンドウの種類には伝える方法がありません"),
        };
        let hdr = Self { route, ten };
        tracing::info!(
            "GLES 2.0 の画面を HDR で出せる（{hdr:?}、PQ {}、HLG {}）",
            hdr.offers(Space::Pq),
            hdr.offers(Space::Hlg)
        );
        Some(hdr)
    }
}

/// HDR の画面に画面を重ねるための物。画面は一度テクスチャへ描き、下の映像はコピーして読む。
#[derive(Debug)]
struct Ui {
    size: (u32, u32),
    space: Space,
    paper: gl::GLuint,
    paper_buffer: gl::GLuint,
    below: gl::GLuint,
    quad: gl::GLuint,
    program: gl::GLuint,
    paper_at: gl::GLint,
    below_at: gl::GLint,
}

impl Window {
    /// # Safety
    /// `native` のウィンドウと接続はこのスレッドで有効で、返った [`Window`] より長く生きること。
    pub unsafe fn on_window(native: Native) -> Result<Rc<Self>, Error> {
        Self::open(Some(native))
    }

    pub fn offscreen() -> Result<Rc<Self>, Error> {
        Self::open(None)
    }

    fn open(window: Option<Native>) -> Result<Rc<Self>, Error> {
        let egl = Egl::open()?;
        let display = match &window {
            None => egl.display(true)?,
            Some(Native::Wayland { display, .. }) => {
                egl.display_on(egl::EGL_PLATFORM_WAYLAND_KHR, *display)?
            }
            Some(_) => egl.display(false)?,
        };
        egl.initialize(display)?;
        let config = egl.config(display, window.is_some())?;
        let mut hdr = window
            .as_ref()
            .and_then(|native| Hdr::of(&egl, display, native));
        // 設定に縛らないコンテキストにできなければ、HDR の画面は使わない。
        let context = match hdr
            .as_ref()
            .map(|_| egl.context(display, egl::EGL_NO_CONFIG_KHR))
        {
            Some(Ok(context)) => context,
            Some(Err(error)) => {
                tracing::info!("GLES 2.0 の画面は SDR のまま（{error:?}）");
                hdr = None;
                egl.context(display, config)?
            }
            None => egl.context(display, config)?,
        };
        let mut wayland = None;
        let surface = match &window {
            // SAFETY: 呼び出し側が `surface` の妥当性を保証している。
            Some(Native::Wayland { surface, size, .. }) => {
                match unsafe { wayland::EglWindow::new(*surface, size.clone()) } {
                    Ok(made) => {
                        // SAFETY: 作ったばかりの `wl_egl_window` で、`Window` が後まで持つ。
                        let surface = unsafe { egl.window(display, config, made.native()) };
                        wayland = Some(made);
                        surface
                    }
                    Err(error) => Err(error),
                }
            }
            // SAFETY: 呼び出し側が `window` の妥当性を保証している。
            Some(Native::X11 { window, .. } | Native::Android { window }) => unsafe {
                egl.window(display, config, *window)
            },
            // ウィンドウが無くてもコンテキストは有効にしなければならないので、1x1 の pbuffer を使う。
            None => egl.pbuffer(display, config, 1, 1),
        };
        // ここから先で失敗したら、サーフェスとコンテキストを破棄してから抜ける（GL のリソースはコンテキストと一緒に消える）。
        let surface = match surface {
            Ok(surface) => surface,
            Err(error) => {
                // SAFETY: 作ったばかりのコンテキストで、ほかに誰も使っていない。
                unsafe { egl.shut(display, context) };
                return Err(error);
            }
        };
        let gl = match (|| {
            egl.make_current(display, surface, context)?;
            Gl::open(&egl)
        })() {
            Ok(gl) => gl,
            Err(error) => {
                // SAFETY: 作ったばかりのサーフェスとコンテキストで、ほかに誰も使っていない。
                unsafe {
                    egl.destroy_surface(display, surface);
                    egl.shut(display, context);
                }
                return Err(error);
            }
        };
        gl.drain();
        // ウィンドウへのレンダリングはこのパスなので、デコーダーが出力フォーマットを選べるよう受け取れるフォーマットを伝えておく。
        if window.is_some() {
            // SAFETY: 直上でコンテキストを有効にしてある。
            let taken = unsafe { super::import::taken(&egl, &gl, display) };
            tracing::debug!(
                "インポートできる DMA-BUF 形式: {}",
                taken
                    .iter()
                    .map(|(fourcc, modifier)| format!(
                        "{}/{modifier:#x}",
                        String::from_utf8_lossy(&fourcc.to_le_bytes())
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            crate::decode::takes::offer(taken);
            // 一枚が一フィールドの DMA-BUF は、外部テクスチャで一フィールドずつ描く（[`super::Present::push`]）。
            let lone = egl
                .query(display, egl::EGL_EXTENSIONS)
                .split_whitespace()
                .any(|name| name == "EGL_EXT_image_dma_buf_import")
                && gl
                    .string(gl::GL_EXTENSIONS)
                    .split_whitespace()
                    .any(|name| name == "GL_OES_EGL_image_external");
            crate::decode::takes::offer_lone_fields(lone);
        }
        // Wayland のウィンドウは、隠れている間は VSync の通知（frame callback）が来ないので、待つと
        // レンダリングのスレッドごと止まる。タイミングはプレイヤーのクロックが取る。
        if wayland.is_some() {
            egl.swap_interval(display, 0);
        }
        let size = window
            .as_ref()
            .and_then(|_| egl.size(display, surface))
            .unwrap_or((1, 1));
        Ok(Rc::new(Self {
            egl: Rc::new(egl),
            gl: Rc::new(gl),
            display,
            context,
            surface: Cell::new(surface),
            config,
            hdr,
            space: Cell::new(Space::Srgb),
            ui: RefCell::new(None),
            vsync: Cell::new(true),
            native: window,
            wayland: RefCell::new(wayland),
            generation: CONTEXTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            size: Cell::new(size),
            blank: Cell::new(false),
            swapped: Cell::new(std::time::Instant::now()),
        }))
    }

    pub fn make_current(&self) -> Result<(), Error> {
        self.egl
            .make_current(self.display, self.surface.get(), self.context)
    }

    /// ウィンドウの大きさに追従し、サーフェスの大きさを返す。回転でも変わるので、出すたびに問い合わせる。
    pub fn follow(&self) -> (u32, u32) {
        if let Some(wayland) = self.wayland.borrow_mut().as_mut() {
            wayland.follow();
        }
        if self.native.is_some()
            && let Some(size) = self.egl.size(self.display, self.surface.get())
        {
            self.size.set(size);
        }
        self.size.get()
    }

    pub fn size(&self) -> (u32, u32) {
        self.size.get()
    }

    pub fn blank(&self) -> bool {
        self.blank.get()
    }

    pub(super) fn showed_picture(&self) {
        self.blank.set(false);
    }

    /// 映像の無いフレームを、必要なときだけ出す：まだ出ていないか、画面が変わったとき。出したなら true。
    pub fn paint_blank(&self, layer: Option<&dyn Layer>) -> Result<bool, Error> {
        if self.blank() && !layer.is_some_and(|layer| layer.fresh()) {
            return Ok(false);
        }
        self.paint_idle(layer).map(|()| true)
    }

    pub fn is_wayland(&self) -> bool {
        self.wayland.borrow().is_some()
    }

    /// 出すときに VSync を待つか。Wayland のウィンドウはいつも待たない（[`Window::open`]）。
    pub fn set_vsync(&self, on: bool) {
        self.vsync.set(on);
        if !self.is_wayland() {
            self.egl.swap_interval(self.display, i32::from(on));
        }
    }

    /// いまの画面の符号値の色空間。映像のトーンはこれへ変換する。
    pub fn space(&self) -> Space {
        self.space.get()
    }

    /// `transfer` の映像に合う画面：HLG は HLG、無ければ PQ。PQ と SDR（`--hdr on`）は PQ。どちらも無ければ sRGB。
    pub fn space_for(&self, transfer: Transfer) -> Space {
        let Some(hdr) = &self.hdr else {
            return Space::Srgb;
        };
        let order: &[Space] = if transfer == Transfer::Hlg {
            &[Space::Hlg, Space::Pq]
        } else {
            &[Space::Pq]
        };
        order
            .iter()
            .copied()
            .find(|&space| hdr.offers(space))
            .unwrap_or(Space::Srgb)
    }

    /// 画面の色空間を替える（サーフェスを作り直す。コンテキストと GL の物は残る）。替えられなければ前のまま。
    pub fn set_space(&self, wanted: Space) -> Space {
        let now = self.space.get();
        if wanted == now {
            return now;
        }
        let Some(hdr) = &self.hdr else {
            return now;
        };
        if !hdr.offers(wanted) {
            return now;
        }
        match self.remake(hdr, wanted) {
            Ok(()) => {
                tracing::info!("GLES 2.0 の画面を {} にした", name(wanted));
                self.space.set(wanted);
            }
            Err(error) => {
                tracing::warn!("GLES 2.0 の画面を {} にできない: {error:?}", name(wanted));
                // 作り直しに失敗したらサーフェスが無いので、SDR で作り直す。
                if wanted != Space::Srgb {
                    match self.remake(hdr, Space::Srgb) {
                        Ok(()) => self.space.set(Space::Srgb),
                        Err(error) => tracing::warn!("SDR の画面にも戻せない: {error:?}"),
                    }
                }
            }
        }
        self.space.get()
    }

    fn native_window(&self) -> Option<egl::EGLNativeWindow> {
        if let Some(wayland) = self.wayland.borrow().as_ref() {
            return Some(wayland.native());
        }
        match &self.native {
            Some(Native::X11 { window } | Native::Android { window }) => Some(*window),
            _ => None,
        }
    }

    fn remake(&self, hdr: &Hdr, space: Space) -> Result<(), Error> {
        let window = self
            .native_window()
            .ok_or(Error::Unsupported("ウィンドウが無い"))?;
        let colour = match space {
            Space::Pq => egl::EGL_GL_COLORSPACE_BT2020_PQ_EXT,
            Space::Hlg => egl::EGL_GL_COLORSPACE_BT2020_HLG_EXT,
            Space::Srgb | Space::ScRgb => 0,
        };
        let (config, attributes) = match (&hdr.route, space) {
            (_, Space::Srgb | Space::ScRgb) => (self.config, vec![egl::EGL_NONE]),
            (Route::Egl { .. }, _) => (
                hdr.ten,
                vec![egl::EGL_GL_COLORSPACE_KHR, colour, egl::EGL_NONE],
            ),
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            (Route::Wayland(_), _) => (hdr.ten, vec![egl::EGL_NONE]),
        };
        self.egl.release(self.display);
        // SAFETY: 外したので誰もこのサーフェスを使っていない。次の行で置き換える。
        unsafe { self.egl.destroy_surface(self.display, self.surface.get()) };
        self.surface.set(egl::EGL_NO_SURFACE);
        // SAFETY: `window` は `Window` が持つウィンドウ（Wayland は `wl_egl_window`）で、後まで生きる。
        let surface = unsafe {
            self.egl
                .window_with(self.display, config, window, &attributes)
        }?;
        self.surface.set(surface);
        self.make_current()?;
        if self.is_wayland() {
            self.egl.swap_interval(self.display, 0);
        } else {
            self.egl
                .swap_interval(self.display, i32::from(self.vsync.get()));
        }
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        if let Route::Wayland(described) = &hdr.route {
            described
                .borrow_mut()
                .tell(space)
                .map_err(|why| Error::NoDevice(format!("合成側へ伝えられない: {why}")))?;
        }
        if let Some(size) = self.egl.size(self.display, surface) {
            self.size.set(size);
        }
        self.blank.set(false);
        Ok(())
    }

    pub fn swap(&self) -> Result<(), Error> {
        let swapped = self.egl.swap(self.display, self.surface.get());
        self.swapped.set(std::time::Instant::now());
        swapped
    }

    pub fn since_swap(&self) -> std::time::Duration {
        self.swapped.get().elapsed()
    }

    /// GL のエントリーポイントを名前で解決する（[`LayerTarget::proc_address`] と同じ）。
    pub fn proc_address(&self, name: &std::ffi::CStr) -> *const std::ffi::c_void {
        self.gl.proc_address(&self.egl, name)
    }

    /// オーバーレイをレンダリングさせ、映像の側が前提にしている GL の状態へ戻す。
    pub(super) fn paint_layer(&self, layer: &dyn Layer, size: (u32, u32)) {
        let gl = &self.gl;
        let proc_address =
            |name: &std::ffi::CStr| -> *const std::ffi::c_void { gl.proc_address(&self.egl, name) };
        layer.paint(&LayerTarget {
            context: self.generation,
            size,
            proc_address: &proc_address,
        });
        // SAFETY: コンテキストは有効にしてある。戻すのは ES 2.0 の既定の状態。
        unsafe {
            (gl.Disable)(gl::GL_BLEND);
            (gl.Disable)(gl::GL_SCISSOR_TEST);
            (gl.BindBuffer)(gl::GL_ARRAY_BUFFER, 0);
            (gl.BindBuffer)(gl::GL_ELEMENT_ARRAY_BUFFER, 0);
            (gl.ActiveTexture)(gl::GL_TEXTURE0);
        }
    }

    /// 画面（フレームバッファ 0）へ重ねる。HDR の画面では、符号値のまま混ぜると暗くなりすぎるので
    /// 一度テクスチャへ描き、下の映像と cd/m² で混ぜて符号化し直す（[`super::glsl::over`]）。
    pub(super) fn paint_layer_on_screen(&self, layer: &dyn Layer, size: (u32, u32)) {
        let space = self.space.get();
        if !space.is_hdr() {
            self.paint_layer(layer, size);
            return;
        }
        if let Err(error) = self.paint_layer_hdr(layer, size, space, 0) {
            tracing::warn!("HDR の画面に画面を重ねられない: {error:?}");
            self.paint_layer(layer, size);
        }
    }

    /// `screen` は重ねる先のフレームバッファ（画面なら 0）。
    fn paint_layer_hdr(
        &self,
        layer: &dyn Layer,
        size: (u32, u32),
        space: Space,
        screen: gl::GLuint,
    ) -> Result<(), Error> {
        let gl = &self.gl;
        let mut held = self.ui.borrow_mut();
        if held
            .as_ref()
            .is_some_and(|ui| ui.size != size || ui.space != space)
            && let Some(old) = held.take()
        {
            // SAFETY: コンテキストは有効にしてある。このコンテキストで作った名前を一度だけ消す。
            unsafe { forget_ui(gl, &old) };
        }
        if held.is_none() {
            // SAFETY: コンテキストは有効にしてある。
            *held = Some(unsafe { make_ui(gl, size, space) }?);
        }
        let Some(ui) = held.as_ref() else {
            return Ok(());
        };
        let (width, height) = (size.0 as gl::GLsizei, size.1 as gl::GLsizei);
        gl.bind_framebuffer(ui.paper_buffer);
        // SAFETY: コンテキストは有効で、どれもこのコンテキストの名前。
        unsafe {
            (gl.Viewport)(0, 0, width, height);
            (gl.ClearColor)(0.0, 0.0, 0.0, 0.0);
            (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
        }
        self.paint_layer(layer, size);
        gl.bind_framebuffer(screen);
        // SAFETY: 同上。下の映像（`screen`）を `below` へ写してから、その上に描く。
        unsafe {
            (gl.ActiveTexture)(gl::GL_TEXTURE0 + 1);
            (gl.BindTexture)(gl::GL_TEXTURE_2D, ui.below);
            (gl.CopyTexSubImage2D)(gl::GL_TEXTURE_2D, 0, 0, 0, 0, 0, width, height);
            (gl.ActiveTexture)(gl::GL_TEXTURE0);
            (gl.BindTexture)(gl::GL_TEXTURE_2D, ui.paper);
            (gl.Viewport)(0, 0, width, height);
            (gl.Disable)(gl::GL_BLEND);
            (gl.Disable)(gl::GL_SCISSOR_TEST);
            (gl.UseProgram)(ui.program);
            if ui.paper_at >= 0 {
                (gl.Uniform1i)(ui.paper_at, 0);
            }
            if ui.below_at >= 0 {
                (gl.Uniform1i)(ui.below_at, 1);
            }
            super::shader::draw_triangle(gl, ui.quad, 6);
            (gl.BindBuffer)(gl::GL_ARRAY_BUFFER, 0);
        }
        Ok(())
    }

    /// テストだけが呼ぶ。`below`（`space` の符号値の RGBA、行0が下）の上に `layer` を HDR の画面と同じく重ね、
    /// 結果を同じ並びで返す。
    pub fn lay_for_test(
        &self,
        layer: &dyn Layer,
        space: Space,
        below: &[u8],
        size: (u32, u32),
    ) -> Result<Vec<u8>, Error> {
        self.make_current()?;
        let gl = &self.gl;
        // SAFETY: コンテキストは有効にしてある。作った物はこの中で消す。
        unsafe {
            let (picture, frame_buffer) = super::shader::target_of(gl, size)?;
            (gl.BindTexture)(gl::GL_TEXTURE_2D, picture);
            (gl.PixelStorei)(gl::GL_UNPACK_ALIGNMENT, 1);
            (gl.TexSubImage2D)(
                gl::GL_TEXTURE_2D,
                0,
                0,
                0,
                size.0 as gl::GLsizei,
                size.1 as gl::GLsizei,
                gl::GL_RGBA,
                gl::GL_UNSIGNED_BYTE,
                below.as_ptr().cast(),
            );
            let laid = self.paint_layer_hdr(layer, size, space, frame_buffer);
            let mut out = vec![0u8; (size.0 * size.1 * 4) as usize];
            gl.read_rgba(frame_buffer, 0, size.0, size.1, &mut out);
            gl.bind_framebuffer(0);
            (gl.DeleteFramebuffers)(1, &frame_buffer);
            (gl.DeleteTextures)(1, &picture);
            if let Some(old) = self.ui.borrow_mut().take() {
                forget_ui(gl, &old);
            }
            laid.map(|()| out)
        }
    }

    /// `size` の画像へレンダリングする（画像キャプチャ）。黒の上に `video`、その上に `layer` を重ねる。
    /// 読み戻しは [`Window::read_some`] で少しずつ（panfrost は 1080p を一度に読むと 400 ms 止まる）。
    pub fn capture(
        &self,
        size: (u32, u32),
        video: &mut dyn FnMut(gl::GLuint, (u32, u32)),
        layer: Option<&dyn Layer>,
    ) -> Result<Readback, Error> {
        self.make_current()?;
        let (width, height) = (size.0.max(1), size.1.max(1));
        let gl = &self.gl;
        // SAFETY: コンテキストは有効にしてある。
        let (picture, frame_buffer) = unsafe { super::shader::target_of(gl, (width, height)) }?;
        // SAFETY: 作ったばかりのレンダーターゲットにレンダリングするだけ。
        unsafe {
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, frame_buffer);
            (gl.Viewport)(0, 0, width as gl::GLsizei, height as gl::GLsizei);
            (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
            (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
        }
        let space = self.space.get();
        if space.is_hdr() {
            // 映像は HDR の画面の符号値で描かれるので、一度別に描いてから SDR に戻して写す。
            // SAFETY: コンテキストは有効にしてある。
            unsafe { self.capture_hdr(frame_buffer, (width, height), space, video) }?;
        } else {
            video(frame_buffer, (width, height));
        }
        if let Some(layer) = layer {
            // オーバーレイはバインドしてあるレンダーターゲットへレンダリングする。
            gl.bind_framebuffer(frame_buffer);
            self.paint_layer(layer, (width, height));
        }
        gl.bind_framebuffer(0);
        Ok(Readback {
            picture,
            frame_buffer,
            size: (width, height),
            read: 0,
            rows_per_ms: None,
            flipped: vec![0u8; (width * height * 4) as usize],
            generation: self.generation,
        })
    }

    /// # Safety
    /// コンテキストを有効にしてあること。
    unsafe fn capture_hdr(
        &self,
        onto: gl::GLuint,
        (width, height): (u32, u32),
        space: Space,
        video: &mut dyn FnMut(gl::GLuint, (u32, u32)),
    ) -> Result<(), Error> {
        let gl = &self.gl;
        let half = gl.half_float_target();
        // SAFETY: 呼び出し側の保証のとおり。作った物はこの中で消す。
        unsafe {
            let (picture, frame_buffer) = super::shader::target_as(gl, (width, height), half)?;
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, frame_buffer);
            (gl.Viewport)(0, 0, width as gl::GLsizei, height as gl::GLsizei);
            (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
            (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
            video(frame_buffer, (width, height));
            let shown = match space {
                Space::Hlg => Transfer::Hlg,
                _ => Transfer::Pq,
            };
            let tone =
                super::glsl::Tone::of_mapping(Mapping::new(shown, Primaries::Bt2020, Space::Srgb));
            let program =
                super::shader::link(gl, super::glsl::WHOLE_VERTEX, &super::glsl::convert(&tone));
            let quad = super::shader::buffer(gl, &QUAD);
            if let Ok(program) = program {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, onto);
                (gl.Viewport)(0, 0, width as gl::GLsizei, height as gl::GLsizei);
                (gl.UseProgram)(program);
                (gl.ActiveTexture)(gl::GL_TEXTURE0);
                (gl.BindTexture)(gl::GL_TEXTURE_2D, picture);
                let at = super::shader::uniform(gl, program, "picture");
                if at >= 0 {
                    (gl.Uniform1i)(at, 0);
                }
                super::shader::draw_triangle(gl, quad, 6);
                (gl.DeleteProgram)(program);
            }
            (gl.BindBuffer)(gl::GL_ARRAY_BUFFER, 0);
            (gl.DeleteBuffers)(1, &quad);
            (gl.DeleteFramebuffers)(1, &frame_buffer);
            (gl.DeleteTextures)(1, &picture);
            program.map(drop)
        }
    }

    /// だいたい `budget` の間だけ読み進め、読み終えたら RGBA8（上から下）を返して GL のリソースを消す。
    pub fn read_some(
        &self,
        readback: &mut Readback,
        budget: std::time::Duration,
    ) -> Result<Option<Vec<u8>>, Error> {
        if readback.generation != self.generation {
            return Err(Error::Unsupported(
                "キャプチャの途中でコンテキストが替わった",
            ));
        }
        self.make_current()?;
        let (width, height) = readback.size;
        // 一行あたりの時間は一度読んで測る。始めは 8 行。
        let rows = readback
            .rows_per_ms
            .map_or(8, |rate| (rate * budget.as_secs_f64() * 1000.0) as u32)
            .clamp(1, height - readback.read);
        let row = (width * 4) as usize;
        let at = readback.read as usize * row;
        let began = std::time::Instant::now();
        let gl = &self.gl;
        gl.read_rgba(
            readback.frame_buffer,
            readback.read,
            width,
            rows,
            &mut readback.flipped[at..],
        );
        gl.bind_framebuffer(0);
        let spent = began.elapsed().as_secs_f64() * 1000.0;
        readback.rows_per_ms = Some(f64::from(rows) / spent.max(0.05));
        readback.read += rows;
        if let Some(code) = gl.error() {
            tracing::warn!("キャプチャを読み戻せない: GL {code:#x}");
            self.forget(readback);
            return Err(Error::Unsupported("キャプチャを読み戻せない"));
        }
        if readback.read < height {
            return Ok(None);
        }
        self.forget(readback);
        // GL は下から上。レンダーターゲットではフレームの行0が下に来るので、裏返すと上から下になる。
        let mut out = Vec::with_capacity(readback.flipped.len());
        for y in (0..height as usize).rev() {
            out.extend_from_slice(&readback.flipped[y * row..(y + 1) * row]);
        }
        Ok(Some(out))
    }

    pub fn forget(&self, readback: &mut Readback) {
        if readback.frame_buffer == 0 || readback.generation != self.generation {
            return;
        }
        let gl = &self.gl;
        // SAFETY: このコンテキストで作った名前で、消すのは一度きり（消したら 0 にする）。
        unsafe {
            (gl.DeleteFramebuffers)(1, &readback.frame_buffer);
            (gl.DeleteTextures)(1, &readback.picture);
        }
        readback.frame_buffer = 0;
        readback.picture = 0;
    }

    /// 映像の無いフレーム：黒く塗り、画面を重ねて出す（選局の間、音だけの番組、再生を切ったとき）。
    pub fn paint_idle(&self, layer: Option<&dyn Layer>) -> Result<(), Error> {
        self.make_current()?;
        let size = self.follow();
        let gl = &self.gl;
        // SAFETY: 既定のフレームバッファにレンダリングするだけ。コンテキストは有効にしてある。
        unsafe {
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, 0);
            (gl.Viewport)(0, 0, size.0 as gl::GLsizei, size.1 as gl::GLsizei);
            (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
            (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
        }
        if let Some(layer) = layer {
            self.paint_layer_on_screen(layer, size);
        }
        self.blank.set(true);
        self.swap()
    }
}

/// レンダリングしておいて、少しずつ読み戻しているキャプチャ（[`Window::capture`]）。
#[derive(Debug)]
pub struct Readback {
    picture: gl::GLuint,
    frame_buffer: gl::GLuint,
    size: (u32, u32),
    /// 読み終えた行（下から）。
    read: u32,
    rows_per_ms: Option<f64>,
    flipped: Vec<u8>,
    generation: u64,
}

impl Readback {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 破棄する順はサーフェス → コンテキストとディスプレイ → `wl_egl_window`（フィールドの drop 時）。
        unsafe {
            self.egl.destroy_surface(self.display, self.surface.get());
            self.egl.shut(self.display, self.context);
        }
    }
}

/// 画面全体を覆う二つの三角形（`corner` は 0〜1）。
const QUAD: [f32; 12] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];

fn name(space: Space) -> &'static str {
    match space {
        Space::Srgb => "SDR（sRGB）",
        Space::ScRgb => "scRGB",
        Space::Pq => "BT.2100 PQ",
        Space::Hlg => "BT.2100 HLG",
    }
}

/// # Safety
/// コンテキストを有効にしてあること。
unsafe fn make_ui(gl: &Gl, size: (u32, u32), space: Space) -> Result<Ui, Error> {
    // SAFETY: 呼び出し側の保証のとおり。失敗したら作った物を消して返す。
    unsafe {
        let (paper, paper_buffer) = super::shader::target_of(gl, size)?;
        let mut below = 0;
        (gl.GenTextures)(1, &mut below);
        (gl.BindTexture)(gl::GL_TEXTURE_2D, below);
        // 下の映像は RGB だけ写す（10 ビットの画面にはアルファの無い物がある。ES 2.0 §3.7.2）。
        (gl.TexImage2D)(
            gl::GL_TEXTURE_2D,
            0,
            gl::GL_RGB as gl::GLint,
            size.0 as gl::GLsizei,
            size.1 as gl::GLsizei,
            0,
            gl::GL_RGB,
            gl::GL_UNSIGNED_BYTE,
            std::ptr::null(),
        );
        for (what, value) in [
            (gl::GL_TEXTURE_MIN_FILTER, gl::GL_NEAREST),
            (gl::GL_TEXTURE_MAG_FILTER, gl::GL_NEAREST),
            (gl::GL_TEXTURE_WRAP_S, gl::GL_CLAMP_TO_EDGE),
            (gl::GL_TEXTURE_WRAP_T, gl::GL_CLAMP_TO_EDGE),
        ] {
            (gl.TexParameteri)(gl::GL_TEXTURE_2D, what, value);
        }
        let quad = super::shader::buffer(gl, &QUAD);
        let ui = Ui {
            size,
            space,
            paper,
            paper_buffer,
            below,
            quad,
            program: 0,
            paper_at: -1,
            below_at: -1,
        };
        match super::shader::link(gl, super::glsl::WHOLE_VERTEX, &super::glsl::over(space)) {
            Ok(program) => Ok(Ui {
                program,
                paper_at: super::shader::uniform(gl, program, "paper"),
                below_at: super::shader::uniform(gl, program, "below"),
                ..ui
            }),
            Err(error) => {
                forget_ui(gl, &ui);
                Err(error)
            }
        }
    }
}

/// # Safety
/// コンテキストが有効で、`ui` がこのコンテキストで作った物であること。
unsafe fn forget_ui(gl: &Gl, ui: &Ui) {
    // SAFETY: 呼び出し側の保証のとおり。
    unsafe {
        (gl.DeleteFramebuffers)(1, &ui.paper_buffer);
        (gl.DeleteTextures)(1, &ui.paper);
        (gl.DeleteTextures)(1, &ui.below);
        (gl.DeleteBuffers)(1, &ui.quad);
        if ui.program != 0 {
            (gl.DeleteProgram)(ui.program);
        }
    }
}
