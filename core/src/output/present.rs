// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! OpenGL ES 2.0 でレンダリングする（wgpu の GLES は 3.0 が下限）。`#version 100` で 2.0 に留める
//! （Mesa は 2.0 を要求しても 3.2 を返す）。

#[cfg(all(target_os = "linux", feature = "wayland"))]
mod described;
mod egl;
mod fence;
mod gl;
pub mod glsl;
mod import;
mod pool;
pub mod probe;
#[cfg(all(target_os = "linux", feature = "wayland"))]
mod subsurface;
mod wayland;
mod window;

pub use egl::context_lost;
pub use window::{Readback, Window};

use std::time::Instant;

use mediadecoder::es::Format;
use mediadecoder::video::Frame;

use crate::decode::{Decoded, Held};
use crate::output::video::colour::Conversion;
use crate::output::video::schedule::Schedule;
use crate::output::video::{
    AFFORDABLE, Cadence, Deinterlace, Error, FIELD_BUDGET_MS, Settings, Space, Transfer, field,
    filter, telecine,
};

use egl::Egl;
use gl::Gl;
use glsl::{Params, Taps, Tone};
use import::Import;
use pool::Pool;

mod capability;
mod draw;
mod shader;

pub use capability::Capability;
use capability::slots;
use shader::{buffer, draw_triangle, link, target_as, target_of, uniform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub field: Option<field::Parity>,
    pub at: u8,
    pub of: u8,
}

/// 並べ直しと転送のどちらを最適化するか決めるために分けて測る。
#[derive(Debug, Default, Clone, Copy)]
pub struct Spent {
    pub upload: std::time::Duration,
    pub pack: std::time::Duration,
    pub transfer: std::time::Duration,
    /// 非同期にコマンドを発行する実装では転送の待ち時間がここに現れる。
    pub check: std::time::Duration,
    pub cadence: std::time::Duration,
    pub passed_through: u64,
    pub draw: std::time::Duration,
    pub show: std::time::Duration,
    pub pushed: u64,
    pub shown: u64,
    pub woven: u64,
    pub locked: bool,
}

#[derive(Debug)]
struct ExternalProgram {
    name: gl::GLuint,
    rect: gl::GLint,
    picture: gl::GLint,
    /// 明るさなどの調整を RGB に掛ける 3 行（[`crate::output::video::colour::Conversion::rgb_rows`]）。
    rows: [gl::GLint; 3],
}

#[derive(Debug)]
struct Program {
    name: gl::GLuint,
    samplers: Vec<gl::GLint>,
    params: Vec<gl::GLint>,
    luma_size: gl::GLint,
    chroma_size: gl::GLint,
    luma_step: gl::GLint,
    chroma_step: gl::GLint,
}

impl Program {
    /// GL のコンテキストがこのスレッドで有効であること。
    unsafe fn build(gl: &Gl, vertex: &str, fragment: &str) -> Result<Self, Error> {
        // SAFETY: 呼び出し側がコンテキストを保証している。
        let name = unsafe { link(gl, vertex, fragment)? };
        let find = |text: &str| uniform(gl, name, text);
        Ok(Self {
            samplers: glsl::SAMPLERS.iter().map(|text| find(text)).collect(),
            params: glsl::UNIFORMS.iter().map(|text| find(text)).collect(),
            luma_size: find("luma_size"),
            chroma_size: find("chroma_size"),
            luma_step: find("luma_step"),
            chroma_step: find("chroma_step"),
            name,
        })
    }
}

pub use wayland::Size as WindowSize;

/// GL の状態（ブレンド、切り抜き、バッファ、テクスチャの単位）は自分で戻す。
pub trait Layer: std::fmt::Debug {
    fn paint(&self, on: &LayerTarget<'_>);
    /// 変わっていなければ、新しいフレームの無い回にレンダリングし直さない。
    fn fresh(&self) -> bool;
}

pub struct LayerTarget<'a> {
    /// 変わったら GL の物は作り直す。
    pub context: u64,
    pub size: (u32, u32),
    pub proc_address: &'a dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void,
}

impl std::fmt::Debug for LayerTarget<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("LayerTarget")
            .field("context", &self.context)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum Native {
    /// EGL は自分で X の接続を開く（winit の `Display *` を共有するとレンダリングのスレッドが
    /// winit のイベント待ちに待たされる）。
    X11 {
        window: *mut std::ffi::c_void,
    },
    /// 大きさは `size` から `wl_egl_window` へ伝える。
    Wayland {
        surface: *mut std::ffi::c_void,
        display: *mut std::ffi::c_void,
        size: WindowSize,
    },
    Android {
        window: *mut std::ffi::c_void,
    },
}

// SAFETY: 識別子を運ぶだけで、指す先に触るのはレンダリングのスレッドで開いた `Window` だけ。
unsafe impl Send for Native {}

/// GL のコンテキストはスレッドに結びつくので `Send` にしない。
#[derive(Debug)]
pub struct Present {
    window: std::rc::Rc<Window>,
    egl: std::rc::Rc<Egl>,
    gl: std::rc::Rc<Gl>,
    display: egl::EGLDisplay,
    viewport: (u32, u32),
    area: Option<(u32, u32, u32, u32)>,
    inset: Option<[f32; 5]>,
    pan_scan: crate::output::video::pan_scan::PanScan,
    format: Format,
    settings: Settings,
    adjust: crate::output::video::Adjust,
    /// 映像のトーンの変換先（ウィンドウの画面の色空間、[`Window::space`]）。
    space: Space,
    taps: Taps,
    pool: Pool,
    import: Option<Import>,
    externals: Vec<Vec<import::External>>,
    /// スロットの外部テクスチャが一枚が一フィールドのピクチャーなら、どちらのフィールドか（描くとき半行ずらす）。
    lone: Vec<Option<field::Parity>>,
    /// 作れなかったら `Some(None)`（通常のパスでレンダリングする）。
    external_program: Option<Option<ExternalProgram>>,
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    video_surface: Option<subsurface::VideoSurface>,
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    surfaced: Vec<Option<crate::decode::DmaBuf>>,
    /// スロットが空くまで保持し、その間はデコーダーがバッファを再利用しない。
    leases: Vec<Option<Lease>>,
    /// スロットが空いて手放すリース。外部テクスチャは描くときに DMA-BUF を直接読み、GPU は数フレーム先まで
    /// 命令をためるので、次に描いた後ろに置くフェンスを GPU が越えるまでデコーダーへ返さない。
    released: Vec<Lease>,
    retiring: Vec<(fence::Fence, Vec<Lease>)>,
    /// 一度だけ用意する。作れなければ `Some(None)`（そのときは空いたらすぐ返す）。
    fences: Option<Option<fence::Fences>>,
    schedule: Schedule,
    lost: usize,
    telecine: telecine::Detector,
    /// `Settings::of_format` は検出器を既定で有効にするので必ずここを通す。
    judged: Option<bool>,
    judging: Vec<f64>,
    picture: gl::GLuint,
    frame_buffer: gl::GLuint,
    corners: gl::GLuint,
    quad: gl::GLuint,
    filter_program: Program,
    blit_program: Program,
    blit_rect: gl::GLint,
    blit_picture: gl::GLint,
    direct_program: Option<Program>,
    direct_rect: gl::GLint,
    spent: Spent,
}

/// 映像の形に依る GL のリソース（コンテキストを作り直さずに選局で入れ替える）。
struct Parts {
    slots: usize,
    pool: Pool,
    settings: Settings,
    picture: gl::GLuint,
    frame_buffer: gl::GLuint,
    corners: gl::GLuint,
    quad: gl::GLuint,
    filter_program: Program,
    blit_program: Program,
    blit_rect: gl::GLint,
    blit_picture: gl::GLint,
    direct_program: Option<Program>,
    direct_rect: gl::GLint,
}

impl Parts {
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効であること。
    unsafe fn build(
        gl: &Gl,
        format: Format,
        settings: Settings,
        taps: Taps,
        space: Space,
    ) -> Result<Self, Error> {
        let slots = slots(format);
        let mut settings = settings;
        if !taps.carries(settings.deinterlace) {
            settings.deinterlace = Deinterlace::Bob;
        }
        // SAFETY: 呼び出し側がコンテキストを保証している。
        unsafe {
            let pool = Pool::new(gl, format, slots)?;
            let (picture, frame_buffer) =
                target_as(gl, (format.width, format.height), half_for(gl, space))?;
            let corners = buffer(gl, &[-1.0, -1.0, 3.0, -1.0, -1.0, 3.0]);
            let quad = buffer(
                gl,
                &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
            );
            let filter_program = Program::build(
                gl,
                &glsl::vertex(taps),
                &filter_source(&settings, format.depth, taps, false, space),
            )?;
            let blit_program = Program::build(gl, glsl::BLIT_VERTEX, glsl::BLIT_FRAGMENT)?;
            // 作れなくても困らない。
            let direct_program = (format.depth <= 8)
                .then(|| {
                    Program::build(
                        gl,
                        glsl::BLIT_VERTEX,
                        &glsl::direct(&Tone::onto(&settings, space)),
                    )
                    .ok()
                })
                .flatten();
            Ok(Self {
                slots,
                pool,
                settings,
                picture,
                frame_buffer,
                corners,
                quad,
                blit_rect: uniform(gl, blit_program.name, "rect"),
                blit_picture: uniform(gl, blit_program.name, "picture"),
                direct_rect: direct_program
                    .as_ref()
                    .map_or(-1, |program| uniform(gl, program.name, "rect")),
                filter_program,
                blit_program,
                direct_program,
            })
        }
    }
}

fn filter_source(
    settings: &Settings,
    depth: u8,
    taps: Taps,
    mediump: bool,
    space: Space,
) -> String {
    glsl::deinterlace_as(
        settings.deinterlace,
        depth,
        taps,
        mediump,
        &Tone::onto(settings, space),
    )
}

/// HDR の画面の符号値は 8 ビットに丸めると縞が出るので、中間は半精度にする（使えれば）。
fn half_for(gl: &Gl, space: Space) -> bool {
    space.is_hdr() && gl.half_float_target()
}

/// 上限は持ち時間（フィールド 2 つ分）の 1 割。
pub fn affordable_telecine(ms: f64) -> bool {
    ms <= 2.0 * FIELD_BUDGET_MS * 0.1
}

/// 24p の 1 フレームより長く。
const REPAINT_AFTER: std::time::Duration = std::time::Duration::from_millis(60);

const TO_JUDGE: usize = 3;

/// ビルドできない環境でも理由を残すため。返るのは (レンダラーの 1 行, 拡張, 上限, highp の対応)。
pub fn describe() -> Result<(String, String, [i32; 3], bool), Error> {
    let egl = Egl::open()?;
    let display = egl.display(true)?;
    egl.initialize(display)?;
    let config = egl.config(display, false)?;
    let context = egl.context(display, config)?;
    let surface = egl.pbuffer(display, config, 1, 1)?;
    let told = (|| {
        egl.make_current(display, surface, context)?;
        let gl = Gl::open(&egl)?;
        gl.drain();
        Ok((
            format!(
                "{} / {} / {}",
                gl.string(gl::GL_RENDERER),
                gl.string(gl::GL_VERSION),
                gl.string(gl::GL_SHADING_LANGUAGE_VERSION),
            ),
            gl.string(gl::GL_EXTENSIONS),
            [
                gl.integer(gl::GL_MAX_TEXTURE_IMAGE_UNITS),
                gl.integer(gl::GL_MAX_VARYING_VECTORS),
                gl.integer(gl::GL_MAX_TEXTURE_SIZE),
            ],
            gl.fragment_highp(),
        ))
    })();
    // SAFETY: 開いたものを開いた順の逆に閉じる。以後どれも使わない。
    unsafe {
        egl.destroy_surface(display, surface);
        egl.shut(display, context);
    }
    told
}

impl Present {
    pub fn offscreen(format: Format, settings: Settings) -> Result<Self, Error> {
        Self::open(format, settings)
    }

    /// # Safety
    /// `native` のウィンドウと接続はこのスレッドで有効で、返った [`Present`] より長く生きること。
    pub unsafe fn on_window(
        native: Native,
        format: Format,
        settings: Settings,
    ) -> Result<Self, Error> {
        // SAFETY: 呼び出し側の保証をそのまま渡す。
        let window = unsafe { Window::on_window(native) }?;
        Self::on(window, format, settings)
    }

    /// 選局でデコーダーを作り直してもコンテキストは残る。
    pub fn on(
        window: std::rc::Rc<Window>,
        format: Format,
        settings: Settings,
    ) -> Result<Self, Error> {
        window.make_current()?;
        let gl = &window.gl;
        gl.drain();
        let taps = if gl.fragment_highp() {
            Taps::Computed
        } else {
            Taps::Varying
        };
        let has_window = window.native.is_some();
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        let native = window.native.clone();
        let mut present = Self::furnish(window, format, settings, taps)?;
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        if let Some(Native::Wayland {
            surface, display, ..
        }) = &native
        {
            // SAFETY: どちらも winit が開いたウィンドウの物で、`Present` より長く生きる（`on_window`）。
            match unsafe { subsurface::VideoSurface::new(*display, *surface) } {
                Ok(mut made) => {
                    made.resize_slots(present.pool.len());
                    tracing::info!("Wayland のサブサーフェスに DMA-BUF をそのまま表示する");
                    present.video_surface = Some(made);
                }
                Err(error) => tracing::info!("Wayland のサブサーフェスを作れない: {error:?}"),
            }
        }
        // ウィンドウの大きさを映像の大きさで代用すると、右と下がレンダリングされないまま残る。
        if has_window {
            present.viewport = present.window.follow();
        }
        Ok(present)
    }

    fn open(format: Format, settings: Settings) -> Result<Self, Error> {
        Self::on(Window::offscreen()?, format, settings)
    }

    #[allow(clippy::too_many_arguments)]
    fn furnish(
        window: std::rc::Rc<Window>,
        format: Format,
        settings: Settings,
        taps: Taps,
    ) -> Result<Self, Error> {
        let egl = std::rc::Rc::clone(&window.egl);
        let gl = std::rc::Rc::clone(&window.gl);
        let display = window.display;
        let space = window.space();
        let made = (|| -> Result<_, Error> {
            let biggest = gl.integer(gl::GL_MAX_TEXTURE_SIZE) as u32;
            if format.width > biggest || format.height > biggest {
                return Err(Error::Unsupported(
                    "この GPU のテクスチャ上限より大きいフレーム",
                ));
            }
            let units = gl.integer(gl::GL_MAX_TEXTURE_IMAGE_UNITS);
            if units < glsl::SAMPLERS.len() as gl::GLint {
                return Err(Error::Unsupported("テクスチャ単位が足りない"));
            }
            // SAFETY: `on` がコンテキストを有効にしてある。
            unsafe { Parts::build(&gl, format, settings, taps, space) }
        })();
        let Parts {
            slots,
            pool,
            settings,
            picture,
            frame_buffer,
            corners,
            quad,
            filter_program,
            blit_program,
            blit_rect,
            blit_picture,
            direct_program,
            direct_rect,
        } = made?;
        debug_assert_eq!(pool.len(), slots, "プールとスケジュールのスロット数が違う");
        Ok(Self {
            window,
            egl,
            gl,
            display,
            viewport: (format.width, format.height),
            area: None,
            inset: None,
            pan_scan: Default::default(),
            format,
            settings,
            adjust: crate::output::video::Adjust::default(),
            space,
            taps,
            pool,
            import: None,
            externals: (0..slots).map(|_| Vec::new()).collect(),
            lone: vec![None; slots],
            external_program: None,
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            video_surface: None,
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            surfaced: Vec::new(),
            leases: Vec::new(),
            released: Vec::new(),
            retiring: Vec::new(),
            fences: None,
            schedule: Schedule::new(slots),
            lost: 0,
            telecine: telecine::Detector::new(),
            judged: None,
            judging: Vec::new(),
            picture,
            frame_buffer,
            corners,
            quad,
            filter_program,
            blit_program,
            blit_rect,
            blit_picture,
            direct_program,
            direct_rect,
            spent: Spent::default(),
        })
    }

    pub fn name(&self) -> String {
        format!(
            "{} / {} / {}",
            self.gl.string(gl::GL_RENDERER),
            self.gl.string(gl::GL_VERSION),
            self.gl.string(gl::GL_SHADING_LANGUAGE_VERSION),
        )
    }

    pub fn extensions(&self) -> String {
        self.gl.string(gl::GL_EXTENSIONS)
    }

    pub fn taps(&self) -> Taps {
        self.taps
    }

    /// テストと probe だけが呼ぶ。
    pub fn force_mediump(&mut self) -> Result<(), Error> {
        self.replace_filter(self.settings, self.taps, true)
    }

    /// 前のフィルターのプログラムを消して入れ替え、作り終えていた出力を捨てる。
    fn replace_filter(
        &mut self,
        settings: Settings,
        taps: Taps,
        mediump: bool,
    ) -> Result<(), Error> {
        // SAFETY: コンテキストは有効にしてある。前のプログラムはもう使わない。
        let built = unsafe {
            let built = Program::build(
                &self.gl,
                &glsl::vertex(taps),
                &filter_source(&settings, self.format.depth, taps, mediump, self.space),
            )?;
            (self.gl.DeleteProgram)(self.filter_program.name);
            built
        };
        self.filter_program = built;
        self.lost += self.schedule.forget_outputs();
        Ok(())
    }

    /// テストと probe だけが呼ぶ。
    pub fn force_taps(&mut self, taps: Taps) -> Result<(), Error> {
        if taps == self.taps {
            return Ok(());
        }
        self.taps = taps;
        let mut settings = self.settings;
        if !taps.carries(settings.deinterlace) {
            settings.deinterlace = Deinterlace::Bob;
        }
        self.replace_filter(settings, taps, false)?;
        self.settings = settings;
        Ok(())
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn telecine_verdict(&self) -> Option<bool> {
        self.judged
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn set_adjust(&mut self, adjust: crate::output::video::Adjust) {
        self.adjust = adjust;
    }

    pub fn set_settings(&mut self, settings: Settings) -> Result<(), Error> {
        let mut settings = settings;
        if !self.taps.carries(settings.deinterlace) {
            settings.deinterlace = Deinterlace::Bob;
        }
        let toned = Tone::onto(&settings, self.space) != Tone::onto(&self.settings, self.space);
        if settings.deinterlace != self.settings.deinterlace || toned {
            self.replace_filter(settings, self.taps, false)?;
        }
        if toned {
            self.rebuild_direct(&settings);
        }
        if !settings.telecine {
            self.telecine.reset();
        }
        if settings.telecine != self.settings.telecine {
            self.judged = None;
            self.judging.clear();
        }
        self.settings = settings;
        Ok(())
    }

    /// 外部テクスチャと 1 パスのプログラムを、いまのトーンで作り直す（外部テクスチャの物は次に使うとき作る）。
    fn rebuild_direct(&mut self, settings: &Settings) {
        // SAFETY: コンテキストは有効にしてある。前のプログラムはもう使わない。
        let direct_program = unsafe {
            let direct_program = (self.format.depth <= 8)
                .then(|| {
                    Program::build(
                        &self.gl,
                        glsl::BLIT_VERTEX,
                        &glsl::direct(&Tone::onto(settings, self.space)),
                    )
                    .ok()
                })
                .flatten();
            if let Some(direct) = self.direct_program.take() {
                (self.gl.DeleteProgram)(direct.name);
            }
            if let Some(Some(external)) = self.external_program.take() {
                (self.gl.DeleteProgram)(external.name);
            }
            direct_program
        };
        self.direct_rect = direct_program
            .as_ref()
            .map_or(-1, |program| uniform(&self.gl, program.name, "rect"));
        self.direct_program = direct_program;
    }

    /// 映像が HDR なら画面も HDR に（`wanted` は `--hdr` で決まる、HDR にしたい映像の伝達特性）。
    /// 画面の色空間が変わったら、トーンと中間のテクスチャを作り直す。
    pub fn set_hdr(&mut self, wanted: Option<Transfer>) {
        // Wayland のサブサーフェスに出している映像は合成側が変換するので、ウィンドウの画面は SDR のまま。
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        let surfaced = self.video_surface.as_ref().is_some_and(|surface| {
            self.schedule
                .front()
                .or(self.schedule.shown())
                .is_some_and(|ready| surface.holds(ready.cur))
        });
        #[cfg(not(all(target_os = "linux", feature = "wayland")))]
        let surfaced = false;
        let wanted = match wanted {
            Some(transfer) if !surfaced => self.window.space_for(transfer),
            _ => Space::Srgb,
        };
        let space = self.window.set_space(wanted);
        if space != self.space
            && let Err(error) = self.respace(space)
        {
            tracing::warn!("画面の色空間に合わせて作り直せない: {error:?}");
        }
    }

    fn respace(&mut self, space: Space) -> Result<(), Error> {
        let half = half_for(&self.gl, space);
        self.respace_as(space, half)
    }

    /// テストだけが呼ぶ。ウィンドウの無い所で、トーンの変換先だけを替える（`half` が偽なら中間は 8 ビット）。
    pub fn force_space(&mut self, space: Space, half: bool) -> Result<(), Error> {
        self.respace_as(space, half)
    }

    fn respace_as(&mut self, space: Space, half: bool) -> Result<(), Error> {
        self.window.make_current()?;
        self.space = space;
        // SAFETY: コンテキストは有効にしてある。新しい物を作ってから古い物を消す。
        unsafe {
            let (picture, frame_buffer) =
                target_as(&self.gl, (self.format.width, self.format.height), half)?;
            (self.gl.DeleteFramebuffers)(1, &self.frame_buffer);
            (self.gl.DeleteTextures)(1, &self.picture);
            self.picture = picture;
            self.frame_buffer = frame_buffer;
        }
        self.replace_filter(self.settings, self.taps, false)?;
        let settings = self.settings;
        self.rebuild_direct(&settings);
        Ok(())
    }

    /// コンテキストごと作り直すとウィンドウとの結びつきを失うので、映像に依存する物だけ。
    pub fn retune(&mut self, format: Format, settings: Settings) -> Result<(), Error> {
        // 新しい物を揃えてから古い物を捨てる（逆だと失敗時に他人の名前を `Drop` が消す）。
        // SAFETY: コンテキストは有効にしてある。
        let Parts {
            slots,
            pool,
            settings,
            picture,
            frame_buffer,
            corners,
            quad,
            filter_program,
            blit_program,
            blit_rect,
            blit_picture,
            direct_program,
            direct_rect,
        } = unsafe { Parts::build(&self.gl, format, settings, self.taps, self.space) }?;
        // SAFETY: コンテキストは有効にしてある。捨てるのはこのコンテキストで作った名前で、以後どれも参照しない。
        unsafe {
            self.release();
        }
        self.externals = (0..slots).map(|_| Vec::new()).collect();
        self.lone = vec![None; slots];
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        if let Some(surface) = self.video_surface.as_mut() {
            surface.resize_slots(slots);
        }
        self.direct_rect = direct_rect;
        self.direct_program = direct_program;
        self.blit_rect = blit_rect;
        self.blit_picture = blit_picture;
        self.pool = pool;
        self.picture = picture;
        self.frame_buffer = frame_buffer;
        self.corners = corners;
        self.quad = quad;
        self.filter_program = filter_program;
        self.blit_program = blit_program;
        self.format = format;
        self.settings = settings;
        self.schedule = Schedule::new(slots);
        self.telecine.reset();
        Ok(())
    }

    /// 既定はエラーを確かめる（黙って壊れたフレームを出すより遅いほうがまし）。
    pub fn set_error_checks(&mut self, on: bool) {
        self.pool.set_error_checks(on);
    }

    /// 読み戻さない。測るときに GPU を待たせないため。
    pub fn draw_ready(&mut self) -> Option<Shown> {
        self.draw(None, true)
    }

    /// Wayland のウィンドウはいつも待たない。
    pub fn set_vsync(&mut self, on: bool) {
        self.window.set_vsync(on);
    }

    pub fn window(&self) -> &std::rc::Rc<Window> {
        &self.window
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.viewport = (width.max(1), height.max(1));
    }

    /// 出力の数を返す。プールが全部塞がっていれば [`Error::Full`]。
    pub fn push(&mut self, picture: &Decoded) -> Result<usize, Error> {
        let (frame, dma_buf) = match picture {
            Decoded::Planes(frame) => (Some(frame), None),
            Decoded::Surface(surface) => match &surface.held {
                Held::DmaBuf(dma_buf) => (None, Some(dma_buf)),
                #[allow(unreachable_patterns)]
                _ => {
                    return Err(Error::Unsupported(
                        "GLES 2.0 のパスが受け取るのはメインメモリのフレームと DMA-BUF だけ",
                    ));
                }
            },
        };
        let size = match (frame, dma_buf) {
            (Some(frame), _) => (frame.planes[0].width, frame.planes[0].height),
            (_, Some(dma_buf)) => (dma_buf.width as usize, dma_buf.height as usize),
            _ => (0, 0),
        };
        if size != (self.format.width as usize, self.format.height as usize) {
            return Err(Error::Unsupported("フレームの大きさが変わった"));
        }
        for (slot, lease) in self.leases.iter_mut().enumerate() {
            if lease.is_some()
                && self.schedule.is_free(slot)
                && let Some(lease) = lease.take()
            {
                self.released.push(lease);
            }
        }
        let about = picture.about();
        // コピーせずに出すパスの bob はフィールド単位なので、明示の指定が無い weave もこれに替える。
        // HEVC の `field_seq_flag` のフレームは全行そろっているのでフィルターを掛けない。
        let adapt = crate::output::video::adapt_progressive();
        let interlaced = !(adapt && (about.progressive || about.format.progressive))
            && !about.format.field_pictures;
        let field_bob = interlaced
            && (self.settings.deinterlace != Deinterlace::Weave
                || !self.settings.deinterlace_chosen);
        let zero_copy = if field_bob {
            Deinterlace::Bob
        } else {
            Deinterlace::Weave
        };
        // 一枚が一フィールドのピクチャー（HEVC の `field_seq_flag`、どちらのフィールドかを `structure` に持つ）は
        // 組まず、外部テクスチャで縦に伸ばして一フィールドずつ描く（bob。半行ずらすのは描くとき）。
        if let Some(dma_buf) = dma_buf
            && about.format.field_pictures
            && about.structure.field()
            && self.external_ready()
        {
            let parity = field::first_field(&about);
            let slot = self.schedule.free().ok_or(Error::Full)?;
            self.put_lease(slot, dma_buf.lease.clone());
            let began = Instant::now();
            self.hold_external(slot, dma_buf, false)?;
            if let Some(lone) = self.lone.get_mut(slot) {
                *lone = Some(parity);
            }
            self.spent.transfer += began.elapsed();
            self.spent.upload += began.elapsed();
            self.spent.pushed += 1;
            let plan = field::plan_of(&about, Deinterlace::Weave, Cadence::None);
            let outputs = plan.len();
            self.schedule.hold(slot, plan);
            return Ok(outputs);
        }
        // サブサーフェスへ出すのはコンポジターがフォーマットと伝達特性・色域・行列を受け取れるときだけ。
        // タイル配置のフレーム（SAND）は行を飛ばせないので、フレームのまま出すときだけ。
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        if let (Some(dma_buf), Some(surface)) = (dma_buf, self.video_surface.as_mut())
            && (field_bob || self.settings.deinterlace == Deinterlace::Weave)
            && (dma_buf.modifier == 0 || !field_bob)
            && surface.takes(dma_buf, subsurface::Look::of(&self.settings))
        {
            let slot = self.schedule.free().ok_or(Error::Full)?;
            let began = Instant::now();
            surface.hold(slot, dma_buf, field_bob)?;
            self.put_lease(slot, dma_buf.lease.clone());
            if self.surfaced.len() <= slot {
                self.surfaced.resize_with(slot + 1, || None);
            }
            self.surfaced[slot] = duplicate(dma_buf);
            self.spent.transfer += began.elapsed();
            self.spent.upload += began.elapsed();
            self.spent.pushed += 1;
            let plan = field::plan_of(&about, zero_copy, Cadence::None);
            let outputs = plan.len();
            self.schedule.hold(slot, plan);
            return Ok(outputs);
        }
        if let Some(dma_buf) = dma_buf
            && (field_bob || self.settings.deinterlace == Deinterlace::Weave)
            && self.external_ready()
        {
            let slot = self.schedule.free().ok_or(Error::Full)?;
            self.put_lease(slot, dma_buf.lease.clone());
            let began = Instant::now();
            self.hold_external(slot, dma_buf, field_bob)?;
            self.spent.transfer += began.elapsed();
            self.spent.upload += began.elapsed();
            self.spent.pushed += 1;
            let plan = field::plan_of(&about, zero_copy, Cadence::None);
            let outputs = plan.len();
            self.schedule.hold(slot, plan);
            return Ok(outputs);
        }
        // 途中でソフトウェアデコードへ切り替わることもあるので、プールの種類はフレームの来かたに合わせる。
        self.pool_for(dma_buf.is_some())?;
        let slot = self.schedule.free().ok_or(Error::Full)?;
        self.put_lease(slot, dma_buf.and_then(|dma_buf| dma_buf.lease.clone()));
        self.drop_external(slot);
        let began = Instant::now();
        if let Some(frame) = frame {
            // SAFETY: コンテキストは有効にしてある。
            let cost = unsafe { self.pool.write(&self.gl, slot, frame) }?;
            self.spent.pack += cost.pack;
            self.spent.transfer += cost.upload;
            self.spent.check += cost.check;
            self.spent.passed_through += u64::from(cost.luma_passed_through);
        }
        if let Some(dma_buf) = dma_buf {
            self.adopt(slot, dma_buf)?;
            self.spent.transfer += began.elapsed();
        }
        let detecting = Instant::now();
        let cadence = match frame {
            Some(frame)
                if self.settings.telecine && self.settings.deinterlace != Deinterlace::Weave =>
            {
                self.telecine.push(frame)
            }
            _ => Cadence::None,
        };
        let took = detecting.elapsed();
        self.spent.cadence += took;
        if self.settings.telecine && self.judged.is_none() {
            self.judging.push(took.as_secs_f64() * 1e3);
            if self.judging.len() >= TO_JUDGE {
                self.judging.sort_by(f64::total_cmp);
                let middle = self.judging[self.judging.len() / 2];
                let afford = affordable_telecine(middle);
                self.judged = Some(afford);
                if !afford {
                    self.settings.telecine = false;
                    self.telecine.reset();
                }
            }
        }
        self.spent.upload += began.elapsed();
        self.spent.pushed += 1;
        if matches!(cadence, Cadence::Whole | Cadence::Spanning) {
            self.spent.woven += 1;
        }
        let plan = field::plan_of(&about, self.settings.deinterlace, cadence);
        let outputs = plan.len();
        self.schedule.hold(slot, plan);
        Ok(outputs)
    }

    fn put_lease(&mut self, slot: usize, lease: Option<Lease>) {
        if self.leases.len() <= slot {
            self.leases.resize_with(slot + 1, || None);
        }
        if let Some(old) = std::mem::replace(&mut self.leases[slot], lease) {
            self.released.push(old);
        }
    }

    /// 描いた後で呼ぶ。手放したリースの後ろにフェンスを置き、GPU が越えたものから返す。
    fn retire(&mut self) {
        if self.fences.is_none() {
            self.fences = Some(fence::Fences::open(&self.egl, &self.gl, self.display));
        }
        let Some(Some(fences)) = self.fences.as_ref() else {
            // フェンスが使えないので、空いたらすぐ返す。
            self.released.clear();
            return;
        };
        if !self.released.is_empty() {
            match fences.put() {
                Some(fence) => self
                    .retiring
                    .push((fence, std::mem::take(&mut self.released))),
                None => self.released.clear(),
            }
        }
        let mut waiting = Vec::with_capacity(self.retiring.len());
        for (fence, leases) in self.retiring.drain(..) {
            if fences.passed(&fence) {
                fences.forget(fence);
                drop(leases);
            } else {
                waiting.push((fence, leases));
            }
        }
        self.retiring = waiting;
    }

    /// 作り直すとスケジュールに溜まったフレームは捨てる。
    fn pool_for(&mut self, renderable: bool) -> Result<(), Error> {
        if self.pool.is_renderable() == renderable {
            return Ok(());
        }
        let slots = self.pool.len();
        // SAFETY: コンテキストは有効にしてある。古いプールの名前はここで解放し、以後使わない。
        let pool = unsafe {
            let pool = if renderable {
                Pool::renderable(&self.gl, self.format, slots)
            } else {
                Pool::new(&self.gl, self.format, slots)
            }?;
            self.pool.shut(&self.gl);
            pool
        };
        self.pool = pool;
        self.lost += self.schedule.len();
        self.schedule.clear();
        Ok(())
    }

    /// 最初の一回で用意する。
    fn external_ready(&mut self) -> bool {
        if let Some(program) = &self.external_program {
            return program.is_some();
        }
        let offered = self
            .gl
            .string(gl::GL_EXTENSIONS)
            .split_whitespace()
            .any(|name| name == "GL_OES_EGL_image_external");
        if offered && self.import.is_none() {
            // SAFETY: コンテキストは有効にしてある。
            match unsafe { Import::open(&self.egl, &self.gl, self.display) } {
                Ok(import) => self.import = Some(import),
                Err(error) => tracing::info!("DMA-BUF をインポートできない: {error:?}"),
            }
        }
        let program = (offered && self.import.is_some())
            .then(|| {
                // SAFETY: コンテキストは有効にしてある。
                let name = unsafe {
                    link(
                        &self.gl,
                        glsl::BLIT_VERTEX,
                        &glsl::external(&Tone::onto(&self.settings, self.space)),
                    )
                }
                .ok()?;
                Some(ExternalProgram {
                    name,
                    rect: uniform(&self.gl, name, "rect"),
                    picture: uniform(&self.gl, name, "picture"),
                    rows: ["row_r", "row_g", "row_b"].map(|row| uniform(&self.gl, name, row)),
                })
            })
            .flatten();
        if program.is_some() {
            tracing::info!("DMA-BUF はコピーせず外部テクスチャでレンダリングする");
        }
        self.external_program = Some(program);
        self.external_program.as_ref().is_some_and(Option::is_some)
    }

    /// `fields` なら二つのフィールドを別々に。
    fn hold_external(
        &mut self,
        slot: usize,
        dma_buf: &crate::decode::DmaBuf,
        fields: bool,
    ) -> Result<(), Error> {
        self.drop_external(slot);
        let bt2020 = self.settings.matrix == crate::output::video::colour::Matrix::Bt2020;
        let Some(import) = self.import.as_mut() else {
            return Err(Error::Unsupported("DMA-BUF をインポートする方法が無い"));
        };
        let wanted: &[Option<field::Parity>] = if fields && dma_buf.modifier == 0 {
            &[Some(field::Parity::Top), Some(field::Parity::Bottom)]
        } else {
            &[None]
        };
        let mut made = Vec::with_capacity(wanted.len());
        for parity in wanted {
            // SAFETY: コンテキストは有効にしてある。
            let external = unsafe {
                import.external(
                    &self.egl,
                    &self.gl,
                    self.display,
                    dma_buf,
                    *parity,
                    bt2020,
                    self.settings.limited,
                )
            };
            match external {
                Ok(external) => made.push(external),
                Err(error) => {
                    for external in made {
                        // SAFETY: 同上。作ったばかりのもの。
                        unsafe { import.drop_external(&self.gl, self.display, external) };
                    }
                    return Err(error);
                }
            }
        }
        if let Some(place) = self.externals.get_mut(slot) {
            *place = made;
        }
        Ok(())
    }

    fn drop_external(&mut self, slot: usize) {
        if let Some(lone) = self.lone.get_mut(slot) {
            *lone = None;
        }
        let held = self.externals.get_mut(slot).map(std::mem::take);
        if let (Some(held), Some(import)) = (held, self.import.as_ref()) {
            for external in held {
                // SAFETY: コンテキストは有効にしてある。
                unsafe { import.drop_external(&self.gl, self.display, external) };
            }
        }
    }

    fn drop_externals(&mut self) {
        for slot in 0..self.externals.len() {
            self.drop_external(slot);
        }
    }

    fn adopt(&mut self, slot: usize, dma_buf: &crate::decode::DmaBuf) -> Result<(), Error> {
        if self.import.is_none() {
            // SAFETY: コンテキストは有効にしてある。
            let import = unsafe { Import::open(&self.egl, &self.gl, self.display) }?;
            self.import = Some(import);
        }
        let (Some(import), Some(targets)) = (self.import.as_mut(), self.pool.targets(slot)) else {
            return Err(Error::Unsupported("コピー先のテクスチャが無い"));
        };
        let size = self.pool.sizes()[0];
        // SAFETY: コンテキストは有効で、`corners` は画面全体を覆う三角形。
        unsafe {
            import.unpack(
                &self.egl,
                &self.gl,
                self.display,
                self.corners,
                dma_buf,
                size,
                targets,
            )
        }?;
        self.pool.mark_written(slot);
        Ok(())
    }

    pub fn flush(&mut self) {
        self.schedule.expand(true);
    }

    pub fn reset(&mut self) {
        self.schedule.clear();
        self.drop_externals();
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        self.surfaced.clear();
        let held: Vec<Lease> = self.leases.drain(..).flatten().collect();
        self.released.extend(held);
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        if let Some(surface) = self.video_surface.as_mut() {
            surface.resize_slots(self.pool.len());
            surface.clear();
        }
        self.lost = 0;
        self.telecine.reset();
    }

    pub fn pending(&self) -> usize {
        self.schedule.len()
    }

    /// `gpu::Video::take_lost` と同じインターフェース。
    pub fn take_lost(&mut self) -> usize {
        std::mem::take(&mut self.lost)
    }

    pub fn ready(&self) -> bool {
        !self.schedule.is_empty()
    }

    pub fn skip(&mut self) -> bool {
        self.schedule.pop()
    }

    pub fn spent(&mut self) -> Spent {
        let mut spent = std::mem::take(&mut self.spent);
        spent.locked = self.telecine.locked();
        spent
    }

    pub fn set_inset(&mut self, inset: Option<[f32; 5]>) {
        self.inset = inset;
    }

    /// データ放送の文書が表示位置を決めている間は効かない。
    pub fn set_pan_scan(&mut self, pan_scan: crate::output::video::pan_scan::PanScan) {
        self.pan_scan = pan_scan;
    }

    /// 外は黒く塗る。
    pub fn set_area(&mut self, area: Option<(u32, u32, u32, u32)>) {
        self.area = area;
    }

    pub fn present(&mut self, layer: Option<&dyn Layer>) -> Result<Option<Shown>, Error> {
        // 新しいフレームが無い回も、GPU が読み終えたバッファは返す（デコーダーが待っているかもしれない）。
        self.retire();
        if !self.ready() {
            return Ok(None);
        }
        self.viewport = self.window.follow();
        let began = Instant::now();
        let shown = self.draw(Some((0, self.viewport)), true);
        if shown.is_some()
            && let Some(layer) = layer
        {
            self.window.paint_layer_on_screen(layer, self.viewport);
        }
        let drew = Instant::now();
        if shown.is_some() {
            self.window.swap()?;
            self.window.showed_picture();
        }
        self.retire();
        self.spent.draw += drew - began;
        self.spent.show += drew.elapsed();
        self.spent.shown += u64::from(shown.is_some());
        Ok(shown)
    }

    /// 新しいフレームの無い回に、画面が変わっていれば最後のフレームに重ね直して出す。出したなら true。
    pub fn repaint(&mut self, layer: Option<&dyn Layer>, black: bool) -> Result<bool, Error> {
        if black || self.schedule.shown().is_none() {
            return self.window.paint_blank(layer);
        }
        let Some(layer) = layer.filter(|layer| layer.fresh()) else {
            return Ok(false);
        };
        // 映像が流れている間に出すと VSync を一回使い、次のフレームが遅れる。
        if self.window.since_swap() < REPAINT_AFTER {
            return Ok(false);
        }
        self.viewport = self.window.follow();
        if !self.draw_again((0, self.viewport)) {
            return self.window.paint_idle(Some(layer)).map(|()| true);
        }
        self.window.paint_layer_on_screen(layer, self.viewport);
        self.window.swap()?;
        self.window.showed_picture();
        self.retire();
        Ok(true)
    }

    /// ゴールデン用。上から下のラスタ（`glReadPixels` と FB で二度裏返って戻る）。
    pub fn read_back(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        self.draw(None, true)?;
        let (width, height) = (self.format.width, self.format.height);
        let mut out = vec![0u8; (width * height * 4) as usize];
        // `RGBA`/`UNSIGNED_BYTE` は必ず通る（ES 2.0 §4.3.1）。
        self.gl
            .read_rgba(self.frame_buffer, 0, width, height, &mut out);
        self.gl.error().map_or(Some((out, width, height)), |_| None)
    }

    /// 上下の向きを確かめるため。
    pub fn read_screen(&mut self, width: u32, height: u32) -> Option<(Vec<u8>, u32, u32)> {
        let (width, height) = (width.max(1), height.max(1));
        let target = Format {
            width,
            height,
            ..self.format
        };
        // SAFETY: コンテキストは有効にしてある。
        let (screen, frame_buffer) =
            unsafe { target_of(&self.gl, (target.width, target.height)) }.ok()?;
        self.draw(Some((frame_buffer, (width, height))), true)?;
        let mut flipped = vec![0u8; (width * height * 4) as usize];
        self.gl
            .read_rgba(frame_buffer, 0, width, height, &mut flipped);
        // SAFETY: コンテキストは有効にしてある。今作った名前を消す。
        unsafe {
            (self.gl.DeleteFramebuffers)(1, &frame_buffer);
            (self.gl.DeleteTextures)(1, &screen);
        }
        let row = (width * 4) as usize;
        let mut out = Vec::with_capacity(flipped.len());
        for y in (0..height as usize).rev() {
            out.extend_from_slice(&flipped[y * row..(y + 1) * row]);
        }
        Some((out, width, height))
    }
}

impl Present {
    /// 画像キャプチャ。映像は `video` の矩形へ。
    pub fn capture(
        &mut self,
        size: (u32, u32),
        video: Option<(u32, u32, u32, u32)>,
        layer: Option<&dyn Layer>,
    ) -> Result<window::Readback, Error> {
        let window = std::rc::Rc::clone(&self.window);
        let mut draw = |frame_buffer: gl::GLuint, size: (u32, u32)| {
            let Some(area) = video else {
                return;
            };
            let kept = (self.viewport, self.area);
            self.viewport = size;
            self.area = Some(area);
            // サブサーフェスに渡したフレームは GL では持っていないので、その間だけインポートする。
            let borrowed = self.borrow_surfaced();
            if !self.draw_again((frame_buffer, size)) {
                tracing::info!("キャプチャ: もう一度レンダリングできるフレームがない");
            }
            if let Some(slot) = borrowed {
                // 一時的にインポートした EGLImage は GPU の処理が終わってから消す（先に消すと panfrost は黒を出す）。
                self.gl.finish();
                self.drop_external(slot);
            }
            (self.viewport, self.area) = kept;
        };
        window.capture(size, &mut draw, layer)
    }

    /// 最後のフレームがサブサーフェスにだけあれば、保持している DMA-BUF をインポートする。
    fn borrow_surfaced(&mut self) -> Option<usize> {
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        {
            let ready = self.schedule.shown()?;
            let slot = ready.cur;
            // サブサーフェスが持っていないスロットのコピーは古い。
            if self
                .externals
                .get(slot)
                .is_some_and(|held| !held.is_empty())
                || !self
                    .video_surface
                    .as_ref()
                    .is_some_and(|surface| surface.holds(slot))
            {
                return None;
            }
            let Some(dma_buf) = self.surfaced.get_mut(slot).and_then(Option::take) else {
                tracing::warn!(
                    "キャプチャ: サブサーフェスのフレームの控えがない（スロット {slot}）"
                );
                return None;
            };
            if !self.external_ready() {
                tracing::warn!(
                    "キャプチャ: 外部テクスチャを使えないのでサブサーフェスのフレームを読めない"
                );
                self.surfaced[slot] = Some(dma_buf);
                return None;
            }
            let fields = ready.output.field.is_some();
            let held = self.hold_external(slot, &dma_buf, fields);
            self.surfaced[slot] = Some(dma_buf);
            match held {
                Ok(()) => Some(slot),
                Err(error) => {
                    tracing::warn!(
                        "キャプチャ: サブサーフェスのフレームをインポートできない: {error:?}"
                    );
                    None
                }
            }
        }
        #[cfg(not(all(target_os = "linux", feature = "wayland")))]
        None
    }
}

/// デコーダーのバッファを、持っている間は再利用させない印（[`crate::decode::DmaBuf::lease`]）。
type Lease = std::sync::Arc<dyn std::any::Any + Send + Sync>;

#[cfg(all(target_os = "linux", feature = "wayland"))]
fn duplicate(dma_buf: &crate::decode::DmaBuf) -> Option<crate::decode::DmaBuf> {
    let planes = dma_buf
        .planes
        .iter()
        .map(|plane| {
            Some(crate::decode::DmaBufPlane {
                fd: plane.fd.try_clone().ok()?,
                offset: plane.offset,
                stride: plane.stride,
                format: plane.format,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(crate::decode::DmaBuf {
        width: dma_buf.width,
        height: dma_buf.height,
        modifier: dma_buf.modifier,
        planes,
        lease: None,
    })
}

impl Drop for Present {
    fn drop(&mut self) {
        // 先に自分のウィンドウのコンテキストを有効にしてから GL のリソースを消す。
        if let Err(error) = self.window.make_current() {
            tracing::warn!("表示の GL リソースを解放できない: {error:?}");
            return;
        }
        // SAFETY: コンテキストはすぐ上で有効にした。
        unsafe { self.release() };
    }
}
