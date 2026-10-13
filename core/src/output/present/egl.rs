// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! EGL を自前で宣言した `extern "C"` で呼び、`libEGL` は `dlopen` で開く（EGL の無い環境でもビルドできるように）。

#![allow(non_snake_case)]
// `EGL/egl.h` から取った定数は、いま呼んでいないものも残す。
#![allow(dead_code)]

use std::ffi::{CStr, CString, c_char, c_void};

use crate::dl::Object;
use crate::output::video::Error;

pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLNativeWindow = *mut c_void;
pub type EGLint = i32;
pub type EGLBoolean = u32;
pub type EGLenum = u32;

pub const EGL_FALSE: EGLBoolean = 0;
pub const EGL_NO_DISPLAY: EGLDisplay = std::ptr::null_mut();
pub const EGL_NO_SURFACE: EGLSurface = std::ptr::null_mut();
pub const EGL_NO_CONTEXT: EGLContext = std::ptr::null_mut();
pub const EGL_DEFAULT_DISPLAY: *mut c_void = std::ptr::null_mut();

pub const EGL_NONE: EGLint = 0x3038;
pub const EGL_ALPHA_SIZE: EGLint = 0x3021;
pub const EGL_BLUE_SIZE: EGLint = 0x3022;
pub const EGL_GREEN_SIZE: EGLint = 0x3023;
pub const EGL_RED_SIZE: EGLint = 0x3024;
pub const EGL_SURFACE_TYPE: EGLint = 0x3033;
pub const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
pub const EGL_PBUFFER_BIT: EGLint = 0x0001;
pub const EGL_WINDOW_BIT: EGLint = 0x0004;
pub const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
pub const EGL_WIDTH: EGLint = 0x3057;
pub const EGL_HEIGHT: EGLint = 0x3056;
pub const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
pub const EGL_CONTEXT_LOST: EGLint = 0x300E;
pub const EGL_VENDOR: EGLint = 0x3053;
pub const EGL_VERSION: EGLint = 0x3054;
pub const EGL_EXTENSIONS: EGLint = 0x3055;
pub const EGL_OPENGL_ES_API: EGLenum = 0x30A0;

/// `EGL_KHR_no_config_context`：コンテキストを特定の設定に縛らない（設定の違うサーフェスへ付け替えられる）。
pub const EGL_NO_CONFIG_KHR: EGLConfig = std::ptr::null_mut();
/// `EGL_KHR_gl_colorspace` と `EGL_EXT_gl_colorspace_bt2020_*`。PQ・HLG は書き込む側が符号化する。
pub const EGL_GL_COLORSPACE_KHR: EGLint = 0x309D;
pub const EGL_GL_COLORSPACE_BT2020_PQ_EXT: EGLint = 0x3340;
pub const EGL_GL_COLORSPACE_BT2020_HLG_EXT: EGLint = 0x3540;

/// 開発機でウィンドウもディスプレイも無しにコンテキストを作るためだけに使う（Android には無い）。
pub const EGL_PLATFORM_SURFACELESS_MESA: EGLenum = 0x31DD;
pub const EGL_PLATFORM_WAYLAND_KHR: EGLenum = 0x31D8;

type ProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;

#[derive(Debug)]
pub struct Egl {
    library: Object,
    get_proc_address: ProcAddress,
    get_display: unsafe extern "C" fn(*mut c_void) -> EGLDisplay,
    initialize: unsafe extern "C" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean,
    terminate: unsafe extern "C" fn(EGLDisplay) -> EGLBoolean,
    query_string: unsafe extern "C" fn(EGLDisplay, EGLint) -> *const c_char,
    choose_config: unsafe extern "C" fn(
        EGLDisplay,
        *const EGLint,
        *mut EGLConfig,
        EGLint,
        *mut EGLint,
    ) -> EGLBoolean,
    bind_api: unsafe extern "C" fn(EGLenum) -> EGLBoolean,
    create_context:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext,
    destroy_context: unsafe extern "C" fn(EGLDisplay, EGLContext) -> EGLBoolean,
    create_pbuffer_surface:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, *const EGLint) -> EGLSurface,
    create_window_surface:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLNativeWindow, *const EGLint) -> EGLSurface,
    destroy_surface: unsafe extern "C" fn(EGLDisplay, EGLSurface) -> EGLBoolean,
    make_current:
        unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean,
    swap_buffers: unsafe extern "C" fn(EGLDisplay, EGLSurface) -> EGLBoolean,
    swap_interval: unsafe extern "C" fn(EGLDisplay, EGLint) -> EGLBoolean,
    query_surface: unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLint, *mut EGLint) -> EGLBoolean,
    get_config_attrib:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLint, *mut EGLint) -> EGLBoolean,
    get_error: unsafe extern "C" fn() -> EGLint,
}

fn symbol<T: Copy>(library: &Object, name: &str) -> Result<T, Error> {
    // SAFETY: 名前は `EGL/egl.h` のエントリーポイントで、型はそこで宣言された署名。
    unsafe { library.symbol_named(name) }.map_err(|missing| Error::NoDevice(missing.0))
}

impl Egl {
    /// Linux の実行時パッケージは `libEGL.so.1`、Android は `libEGL.so` だけを持つので両方試す。
    pub fn open() -> Result<Self, Error> {
        let library = Object::open_any(&["libEGL.so.1", "libEGL.so"])
            .map_err(|missing| Error::NoDevice(missing.0))?;
        let get_proc_address: ProcAddress = symbol(&library, "eglGetProcAddress")?;
        Ok(Self {
            get_display: symbol(&library, "eglGetDisplay")?,
            initialize: symbol(&library, "eglInitialize")?,
            terminate: symbol(&library, "eglTerminate")?,
            query_string: symbol(&library, "eglQueryString")?,
            choose_config: symbol(&library, "eglChooseConfig")?,
            bind_api: symbol(&library, "eglBindAPI")?,
            create_context: symbol(&library, "eglCreateContext")?,
            destroy_context: symbol(&library, "eglDestroyContext")?,
            create_pbuffer_surface: symbol(&library, "eglCreatePbufferSurface")?,
            create_window_surface: symbol(&library, "eglCreateWindowSurface")?,
            destroy_surface: symbol(&library, "eglDestroySurface")?,
            make_current: symbol(&library, "eglMakeCurrent")?,
            swap_buffers: symbol(&library, "eglSwapBuffers")?,
            swap_interval: symbol(&library, "eglSwapInterval")?,
            query_surface: symbol(&library, "eglQuerySurface")?,
            get_config_attrib: symbol(&library, "eglGetConfigAttrib")?,
            get_error: symbol(&library, "eglGetError")?,
            get_proc_address,
            library,
        })
    }

    fn extension<T: Copy>(&self, name: &str) -> Option<T> {
        let name = CString::new(name).ok()?;
        // SAFETY: `name` は終端されており、返るのは関数ポインタか空。
        let pointer = unsafe { (self.get_proc_address)(name.as_ptr()) };
        if pointer.is_null() {
            return None;
        }
        debug_assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: `dlsym` と同じ変換。呼び出し側が型を宣言している。
        Some(unsafe { std::mem::transmute_copy(&pointer) })
    }

    pub fn client_extensions(&self) -> String {
        self.query(EGL_NO_DISPLAY, EGL_EXTENSIONS)
    }

    pub fn query(&self, display: EGLDisplay, what: EGLint) -> String {
        // SAFETY: EGL は未知の列挙値には空を返す。
        let text = unsafe { (self.query_string)(display, what) };
        if text.is_null() {
            return String::new();
        }
        // SAFETY: EGL の返す文字列は終端された静的なもの。
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    /// ウィンドウを使うなら surfaceless を選んではいけない（ウィンドウサーフェスを作れない）。
    pub fn display(&self, offscreen: bool) -> Result<EGLDisplay, Error> {
        if offscreen
            && self.client_extensions().split(' ').any(|name| {
                name == "EGL_MESA_platform_surfaceless" || name == "EGL_EXT_platform_base"
            })
            && let Some(get) = self.extension::<unsafe extern "C" fn(
                EGLenum,
                *mut c_void,
                *const EGLint,
            ) -> EGLDisplay>("eglGetPlatformDisplayEXT")
        {
            // SAFETY: 拡張があると報告されていて、引数はどれも規定のもの。
            let display = unsafe {
                get(
                    EGL_PLATFORM_SURFACELESS_MESA,
                    EGL_DEFAULT_DISPLAY,
                    std::ptr::null(),
                )
            };
            if display != EGL_NO_DISPLAY {
                return Ok(display);
            }
        }
        // SAFETY: 既定のディスプレイを取るだけ。
        let display = unsafe { (self.get_display)(EGL_DEFAULT_DISPLAY) };
        if display == EGL_NO_DISPLAY {
            return Err(Error::NoDevice(String::from(
                "EGL のディスプレイを取れません",
            )));
        }
        Ok(display)
    }

    /// ウィンドウ側の接続（`wl_display *`）からディスプレイを開く。既定のディスプレイだと Mesa が別の接続を開いてしまう。
    pub fn display_on(&self, platform: EGLenum, native: *mut c_void) -> Result<EGLDisplay, Error> {
        let named = "EGL_KHR_platform_wayland";
        let offered = self
            .client_extensions()
            .split(' ')
            .any(|name| name == named || name == named.replace("KHR", "EXT"));
        if offered
            && let Some(get) = self.extension::<unsafe extern "C" fn(
                EGLenum,
                *mut c_void,
                *const EGLint,
            ) -> EGLDisplay>("eglGetPlatformDisplayEXT")
        {
            // SAFETY: 拡張があると報告されていて、`native` はウィンドウ側がまだ開いている接続。
            let display = unsafe { get(platform, native, std::ptr::null()) };
            if display != EGL_NO_DISPLAY {
                return Ok(display);
            }
        }
        Err(Error::NoDevice(format!("EGL に {named} がありません")))
    }

    pub fn swap_interval(&self, display: EGLDisplay, interval: EGLint) {
        // SAFETY: ディスプレイは初期化済みで、有効なコンテキストとサーフェスに効く。失敗しても困らない。
        unsafe { (self.swap_interval)(display, interval) };
    }

    pub fn initialize(&self, display: EGLDisplay) -> Result<(EGLint, EGLint), Error> {
        let mut major = 0;
        let mut minor = 0;
        // SAFETY: 出力は2つの `EGLint` で、どちらも書ける場所を渡している。
        let ok = unsafe { (self.initialize)(display, &mut major, &mut minor) };
        if ok == EGL_FALSE {
            return Err(Error::NoDevice(format!(
                "eglInitialize に失敗 {:#x}",
                self.error()
            )));
        }
        Ok((major, minor))
    }

    pub fn config(&self, display: EGLDisplay, window: bool) -> Result<EGLConfig, Error> {
        let surface = if window {
            EGL_WINDOW_BIT
        } else {
            EGL_PBUFFER_BIT
        };
        let wanted = [
            EGL_SURFACE_TYPE,
            surface,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            8,
            EGL_NONE,
        ];
        let mut config: EGLConfig = std::ptr::null_mut();
        let mut count: EGLint = 0;
        // SAFETY: `wanted` は `EGL_NONE` で終わっており、出力は1つ分。
        let ok =
            unsafe { (self.choose_config)(display, wanted.as_ptr(), &mut config, 1, &mut count) };
        if ok == EGL_FALSE || count == 0 {
            return Err(Error::NoDevice(String::from(
                "ES 2.0 でレンダリングできる EGLConfig がありません",
            )));
        }
        Ok(config)
    }

    /// ディスプレイの拡張にあるか。
    pub fn offers(&self, display: EGLDisplay, name: &str) -> bool {
        self.query(display, EGL_EXTENSIONS)
            .split_whitespace()
            .any(|offered| offered == name)
    }

    /// 各成分 10 ビットのウィンドウの設定（HDR の画面の符号値を書くため）。無ければ `None`。
    pub fn ten_bit_config(&self, display: EGLDisplay) -> Option<EGLConfig> {
        let wanted = [
            EGL_SURFACE_TYPE,
            EGL_WINDOW_BIT,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_RED_SIZE,
            10,
            EGL_GREEN_SIZE,
            10,
            EGL_BLUE_SIZE,
            10,
            EGL_NONE,
        ];
        let mut configs: [EGLConfig; 32] = [std::ptr::null_mut(); 32];
        let mut count: EGLint = 0;
        // SAFETY: `wanted` は `EGL_NONE` で終わっており、出力は 32 個分。
        let ok = unsafe {
            (self.choose_config)(
                display,
                wanted.as_ptr(),
                configs.as_mut_ptr(),
                configs.len() as EGLint,
                &mut count,
            )
        };
        if ok == EGL_FALSE {
            return None;
        }
        // 大きいほうから並ぶ（EGL 1.5 §3.4.1.2）ので、ちょうど 10 ビットの物を選ぶ。
        configs[..count.max(0) as usize]
            .iter()
            .copied()
            .find(|&config| {
                [EGL_RED_SIZE, EGL_GREEN_SIZE, EGL_BLUE_SIZE]
                    .iter()
                    .all(|&what| self.config_attrib(display, config, what) == Some(10))
            })
    }

    fn config_attrib(
        &self,
        display: EGLDisplay,
        config: EGLConfig,
        what: EGLint,
    ) -> Option<EGLint> {
        let mut value = 0;
        // SAFETY: 設定はこのディスプレイから得た物で、出力は `EGLint` 一つ。
        let ok = unsafe { (self.get_config_attrib)(display, config, what, &mut value) };
        (ok != EGL_FALSE).then_some(value)
    }

    /// ES 2.0 のコンテキストを要求する。返るのは 2.0 以上なので、2.0 に留める担保は [`super::gl`] と `#version 100`。
    pub fn context(&self, display: EGLDisplay, config: EGLConfig) -> Result<EGLContext, Error> {
        // SAFETY: 定数の並びを渡すだけ。
        unsafe { (self.bind_api)(EGL_OPENGL_ES_API) };
        let wanted = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
        // SAFETY: 共有相手なし、属性は終端されている。
        let context =
            unsafe { (self.create_context)(display, config, EGL_NO_CONTEXT, wanted.as_ptr()) };
        if context == EGL_NO_CONTEXT {
            return Err(Error::NoDevice(format!(
                "ES 2.0 のコンテキストを作れません {:#x}",
                self.error()
            )));
        }
        Ok(context)
    }

    pub fn pbuffer(
        &self,
        display: EGLDisplay,
        config: EGLConfig,
        width: i32,
        height: i32,
    ) -> Result<EGLSurface, Error> {
        let wanted = [EGL_WIDTH, width, EGL_HEIGHT, height, EGL_NONE];
        // SAFETY: 属性は終端されている。
        let surface = unsafe { (self.create_pbuffer_surface)(display, config, wanted.as_ptr()) };
        if surface == EGL_NO_SURFACE {
            return Err(Error::NoDevice(format!(
                "pbuffer を作れません {:#x}",
                self.error()
            )));
        }
        Ok(surface)
    }

    /// 有効なコンテキストとサーフェスを外す（サーフェスを作り直す前に）。
    pub fn release(&self, display: EGLDisplay) {
        // SAFETY: どれも「無し」を渡すだけ。
        unsafe { (self.make_current)(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT) };
    }

    /// 属性付きのウィンドウサーフェス（`attributes` は `EGL_NONE` で終わること）。
    /// # Safety
    /// [`Egl::window`] と同じ。
    pub unsafe fn window_with(
        &self,
        display: EGLDisplay,
        config: EGLConfig,
        window: EGLNativeWindow,
        attributes: &[EGLint],
    ) -> Result<EGLSurface, Error> {
        debug_assert_eq!(attributes.last(), Some(&EGL_NONE));
        // SAFETY: 呼び出し側が `window` の妥当性を約束している。属性は終端されている。
        let surface =
            unsafe { (self.create_window_surface)(display, config, window, attributes.as_ptr()) };
        if surface == EGL_NO_SURFACE {
            return Err(Error::NoDevice(format!(
                "ウィンドウサーフェスを作れません {:#x}",
                self.error()
            )));
        }
        Ok(surface)
    }

    /// ウィンドウサーフェス。Android では `ANativeWindow *` を渡す。
    /// # Safety
    /// `window` はこのディスプレイで有効な native window で、返ったサーフェスより長く生きること。
    pub unsafe fn window(
        &self,
        display: EGLDisplay,
        config: EGLConfig,
        window: EGLNativeWindow,
    ) -> Result<EGLSurface, Error> {
        // SAFETY: 呼び出し側が `window` の妥当性を約束している。
        let surface =
            unsafe { (self.create_window_surface)(display, config, window, std::ptr::null()) };
        if surface == EGL_NO_SURFACE {
            return Err(Error::NoDevice(format!(
                "ウィンドウサーフェスを作れません {:#x}",
                self.error()
            )));
        }
        Ok(surface)
    }

    pub fn make_current(
        &self,
        display: EGLDisplay,
        surface: EGLSurface,
        context: EGLContext,
    ) -> Result<(), Error> {
        // SAFETY: どれもこのディスプレイから得たもの。
        let ok = unsafe { (self.make_current)(display, surface, surface, context) };
        if ok == EGL_FALSE {
            return Err(Error::NoDevice(format!(
                "eglMakeCurrent に失敗 {:#x}",
                self.checked_error()
            )));
        }
        Ok(())
    }

    pub fn swap(&self, display: EGLDisplay, surface: EGLSurface) -> Result<(), Error> {
        // SAFETY: どちらもこのディスプレイから得たもの。
        let ok = unsafe { (self.swap_buffers)(display, surface) };
        if ok == EGL_FALSE {
            return Err(Error::Surface(format!(
                "eglSwapBuffers に失敗 {:#x}",
                self.checked_error()
            )));
        }
        Ok(())
    }

    /// ウィンドウの大きさをフレームの大きさで代用すると、右と下がレンダリングされないまま残る。回転でも変わる。
    pub fn size(&self, display: EGLDisplay, surface: EGLSurface) -> Option<(u32, u32)> {
        let mut width = 0;
        let mut height = 0;
        // SAFETY: どちらもこのディスプレイから得たもので、出力は `EGLint` 1つずつ。
        let ok = unsafe {
            (self.query_surface)(display, surface, EGL_WIDTH, &mut width) != EGL_FALSE
                && (self.query_surface)(display, surface, EGL_HEIGHT, &mut height) != EGL_FALSE
        };
        (ok && width > 0 && height > 0).then_some((width as u32, height as u32))
    }

    /// コンテキストを失っていたらフラグを立てる（作り直さないと以後はレンダリングできない）。
    fn checked_error(&self) -> EGLint {
        let error = self.error();
        if error == EGL_CONTEXT_LOST {
            CONTEXT_LOST.store(true, std::sync::atomic::Ordering::Release);
        }
        error
    }

    pub fn error(&self) -> EGLint {
        // SAFETY: 引数なし。
        unsafe { (self.get_error)() }
    }

    /// # Safety
    /// 呼んだあと、このサーフェスを使ってはならない。
    pub unsafe fn destroy_surface(&self, display: EGLDisplay, surface: EGLSurface) {
        if surface != EGL_NO_SURFACE {
            // SAFETY: 呼び出し側が二度使わないことを約束している。
            unsafe { (self.destroy_surface)(display, surface) };
        }
    }

    /// コンテキストを破棄する。`eglTerminate` は参照カウントを持たず（EGL 1.5 §3.2）同じプロセスの他の利用者まで終了させるので、ディスプレイは終了しない。
    /// # Safety
    /// 呼んだあと、このコンテキストを使ってはならない。
    pub unsafe fn shut(&self, display: EGLDisplay, context: EGLContext) {
        if display == EGL_NO_DISPLAY {
            return;
        }
        // SAFETY: 有効なコンテキストを外してから破棄する。順序はこれが規定。
        unsafe {
            (self.make_current)(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            if context != EGL_NO_CONTEXT {
                (self.destroy_context)(display, context);
            }
        }
    }

    /// `dlsym` で見つからなかった GL の関数の代わりに入れる関数。
    pub fn proc_address<T: Copy>(&self, name: &str) -> Option<T> {
        self.extension(name)
    }
}

static CONTEXT_LOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// EGL のコンテキストを失ったか（電源の切り替えや GPU のリセット）。
pub fn context_lost() -> bool {
    CONTEXT_LOST.load(std::sync::atomic::Ordering::Acquire)
}
