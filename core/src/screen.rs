// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レンダリング先。wgpu と GLES 2.0（unix、Vulkan の無い環境向け）のどちらでも上の層は変わらない。
//! サーフェスは呼び出し側（deck の `Stage`）がフレームごとに [`Canvas`] で渡す。

use std::time::Duration;

use mediadecoder::es::Format;

use crate::decode::Decoded;
#[cfg(unix)]
use crate::output::present::{Capability as GlesCapability, Present};
use crate::output::video::Error as VideoError;
use crate::output::video::Settings;
#[cfg(feature = "wgpu")]
use crate::output::video::Space;
#[cfg(feature = "wgpu")]
use crate::output::video::gpu::{Capability, Gpu, Onto, Upload, Video};

#[derive(Debug, Default, Clone, Copy)]
pub struct Spent {
    pub upload: Duration,
    pub acquire: Duration,
    pub draw: Duration,
    pub show: Duration,
    pub pushed: u64,
    pub shown: u64,
    pub woven: u64,
    pub locked: bool,
    pub whole_frames: bool,
    pub soft: bool,
    pub matched: u64,
    pub film: bool,
    /// film だが流れるテロップなどがあるので 60i でレンダリングしている（wgpu だけ）。
    pub overlay: bool,
    pub film_changes: u64,
    pub combed: u64,
    pub stripes: u32,
}

impl Spent {
    pub fn add(&mut self, more: &Spent) {
        self.upload += more.upload;
        self.acquire += more.acquire;
        self.draw += more.draw;
        self.show += more.show;
        self.pushed += more.pushed;
        self.shown += more.shown;
        self.woven += more.woven;
        self.matched += more.matched;
        self.film_changes += more.film_changes;
        self.combed += more.combed;
        self.locked = more.locked;
        self.whole_frames = more.whole_frames;
        self.soft = more.soft;
        self.film = more.film;
        self.overlay = more.overlay;
        self.stripes = more.stripes;
    }
}

#[derive(Debug)]
pub enum Painter {
    #[cfg(feature = "wgpu")]
    Wgpu {
        gpu: Gpu,
        target: wgpu::TextureFormat,
        space: Space,
        white: f32,
        /// 画面のピーク、cd/m²。HLG はこれを基準にマッピングする（BT.2100 の OOTF）。分からなければ 1000。
        peak: f32,
    },
    #[cfg(unix)]
    Gles {
        native: crate::output::present::Native,
        size: (u32, u32),
        vsync: bool,
        /// レンダリングスレッドで最初に必要になったときに開き、ウィンドウと同じだけ生きる（レンダラーは選局のたびに作り直す）。
        window: std::cell::OnceCell<Bound<std::rc::Rc<crate::output::present::Window>>>,
    },
    #[cfg(not(any(feature = "wgpu", unix)))]
    #[doc(hidden)]
    Never(std::convert::Infallible),
}

/// 作ったスレッドでだけ中身に触れるラッパー。別のスレッドで触れば panic、落ちても中身は手放さない。
#[cfg(unix)]
pub struct Bound<T> {
    value: Option<T>,
    owner: std::thread::ThreadId,
}

#[cfg(unix)]
impl<T> Bound<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: Some(value),
            owner: std::thread::current().id(),
        }
    }

    fn owned(&self) -> bool {
        self.owner == std::thread::current().id()
    }
}

#[cfg(unix)]
impl<T> std::ops::Deref for Bound<T> {
    type Target = T;

    fn deref(&self) -> &T {
        assert!(self.owned(), "作ったのと別のスレッドから触った");
        self.value.as_ref().expect("落とすまで中身はある")
    }
}

#[cfg(unix)]
impl<T> std::ops::DerefMut for Bound<T> {
    fn deref_mut(&mut self) -> &mut T {
        assert!(self.owned(), "作ったのと別のスレッドから触った");
        self.value.as_mut().expect("落とすまで中身はある")
    }
}

#[cfg(unix)]
impl<T> Drop for Bound<T> {
    fn drop(&mut self) {
        if !self.owned() {
            std::mem::forget(self.value.take());
        }
    }
}

#[cfg(unix)]
impl<T> std::fmt::Debug for Bound<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Bound")
    }
}

// SAFETY: 中身に触れるのは作ったスレッドだけ（`Deref`・`DerefMut` の検査と `Drop`）。
#[cfg(unix)]
unsafe impl<T> Send for Bound<T> {}

#[cfg_attr(not(any(feature = "wgpu", unix)), allow(unused_variables))]
impl Painter {
    pub fn name(&self) -> String {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu { gpu, .. } => gpu.name(),
            #[cfg(unix)]
            Self::Gles { .. } => String::from("GLES 2.0"),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    #[cfg(feature = "plugins")]
    pub fn host_jobs(&self) -> Vec<String> {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu { gpu, .. } => crate::output::video::host::available(&gpu.device),
            #[cfg(unix)]
            Self::Gles { .. } => Vec::new(),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    pub fn settings(&self, format: Format) -> Settings {
        match self {
            #[cfg(feature = "wgpu")]
            // 測り終えるまでは、重い逆テレシネの判定を切っておく。
            Self::Wgpu { gpu, .. } => measured(gpu, format).map_or_else(
                || Settings {
                    telecine: false,
                    ..Settings::of_format(format)
                },
                |(settings, _, _)| settings,
            ),
            #[cfg(unix)]
            Self::Gles { .. } => Settings::of_format(format),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    /// この形式で GPU の性能を測り始める（シェーダーでデインタレースするときだけ要る）。
    pub fn measure(&self, format: Format) {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu { gpu, .. } => start_measure(gpu, format),
            #[cfg(unix)]
            Self::Gles { .. } => {}
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    /// この形式を測り終えているか（[`Painter::settings`] が測った結果を返すか）。
    pub fn settled(&self, format: Format) -> bool {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu { gpu, .. } => {
                let format = Format {
                    progressive: format.progressive && crate::output::video::adapt_progressive(),
                    ..format
                };
                let name = gpu.name();
                MEASURED
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .any(|have| {
                        have.gpu == name
                            && have.width == format.width
                            && have.height == format.height
                            && have.depth == format.depth
                            && have.progressive == format.progressive
                    })
            }
            #[cfg(unix)]
            Self::Gles { .. } => true,
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    /// `tune` が真なら測って品質を決め直す。フラグで品質を指定されたときは偽（指定を上書きしない）。
    pub fn build(
        &self,
        format: Format,
        settings: Settings,
        tune: bool,
    ) -> Result<Screen, VideoError> {
        #[cfg(not(unix))]
        let _ = tune;
        #[cfg(not(feature = "wgpu"))]
        let _ = (format, settings);
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu {
                gpu,
                target,
                space,
                white,
                peak,
            } => Video::new(&gpu.device, &gpu.queue, *target, format, settings).map(|mut video| {
                let measured = measured(gpu, format);
                video.set_upload(measured.map_or_else(Upload::default, |(_, upload, _)| upload));
                video.set_strength(measured.map(|(_, _, strength)| strength));
                video.set_space(*space);
                video.set_sdr_white(*white);
                video.set_hlg_peak(*peak);
                Screen::Wgpu(Box::new(video))
            }),
            #[cfg(unix)]
            Self::Gles { size, vsync, .. } => {
                let window = self.gles_window()?;
                let mut present = Present::on(window, format, settings)?;
                present.resize(size.0, size.1);
                present.set_vsync(*vsync);
                if tune {
                    // 別に開くとコンテキストが二つになるので、このコンテキストの上でビルドしてから測る。
                    let capability = GlesCapability::measure_with(&mut present, format);
                    present.set_settings(capability.settings(format))?;
                }
                Ok(Screen::Gles(Bound::new(Box::new(present))))
            }
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    #[cfg(unix)]
    #[allow(irrefutable_let_patterns)]
    pub fn gles_window(&self) -> Result<std::rc::Rc<crate::output::present::Window>, VideoError> {
        let Self::Gles {
            native,
            vsync,
            window,
            ..
        } = self
        else {
            return Err(VideoError::Unsupported("GLES 2.0 のレンダラーではない"));
        };
        if let Some(window) = window.get() {
            return Ok(std::rc::Rc::clone(&**window));
        }
        // SAFETY: `native` は winit が開いたウィンドウのもので、`Deck` がレンダラーより長く持っている。
        let opened = unsafe { crate::output::present::Window::on_window(native.clone()) }?;
        opened.set_vsync(*vsync);
        let _ = window.set(Bound::new(std::rc::Rc::clone(&opened)));
        Ok(opened)
    }

    #[cfg(unix)]
    pub fn paint_blank(
        &self,
        layer: Option<&dyn crate::output::present::Layer>,
    ) -> Result<bool, VideoError> {
        self.gles_window()?.paint_blank(layer)
    }

    #[cfg(unix)]
    #[cfg_attr(not(feature = "wgpu"), allow(irrefutable_let_patterns))]
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Self::Gles { native, size, .. } = self {
            *size = (width.max(1), height.max(1));
            if let crate::output::present::Native::Wayland { size: shared, .. } = native {
                shared.set(width, height);
            }
        }
    }

    #[cfg(not(unix))]
    pub fn resize(&mut self, _width: u32, _height: u32) {}
}

#[derive(Debug)]
pub enum Canvas<'a> {
    #[cfg(feature = "wgpu")]
    Wgpu {
        view: &'a wgpu::TextureView,
        onto: Onto,
    },
    #[cfg(unix)]
    Gles {
        layer: Option<&'a dyn crate::output::present::Layer>,
        area: Option<(u32, u32, u32, u32)>,
        black: bool,
        /// HDR の画面にしたい映像の伝達特性（`--hdr`）。`None` なら SDR。
        hdr: Option<crate::output::video::Transfer>,
    },
    #[cfg(not(feature = "wgpu"))]
    #[doc(hidden)]
    Never(std::marker::PhantomData<&'a ()>),
}

#[derive(Debug)]
pub enum Screen {
    #[cfg(feature = "wgpu")]
    Wgpu(Box<Video>),
    /// 作るのは `Stage` のスレッドで、GL のコンテキストはそこでだけ使う。
    #[cfg(unix)]
    Gles(Bound<Box<Present>>),
    #[cfg(not(any(feature = "wgpu", unix)))]
    #[doc(hidden)]
    Never(std::convert::Infallible),
}

macro_rules! either {
    ($screen:expr, |$it:ident| $body:expr) => {
        match $screen {
            #[cfg(feature = "wgpu")]
            Screen::Wgpu($it) => $body,
            #[cfg(unix)]
            Screen::Gles($it) => $body,
            #[cfg(not(any(feature = "wgpu", unix)))]
            Screen::Never(never) => match *never {},
        }
    };
}

#[cfg_attr(not(any(feature = "wgpu", unix)), allow(unused_variables))]
impl Screen {
    /// wgpu のときだけ持つ機能のため。GLES ならば `None`（Windows では wgpu だけなので常に `Some`）。
    #[cfg(feature = "wgpu")]
    fn wgpu(&self) -> Option<&Video> {
        match self {
            Self::Wgpu(video) => Some(&**video),
            #[cfg(unix)]
            Self::Gles(_) => None,
        }
    }

    #[cfg(feature = "wgpu")]
    fn wgpu_mut(&mut self) -> Option<&mut Video> {
        match self {
            Self::Wgpu(video) => Some(&mut **video),
            #[cfg(unix)]
            Self::Gles(_) => None,
        }
    }

    /// 出力の数を返す。数え直してはいけない。
    pub fn push(&mut self, picture: &Decoded) -> Result<usize, VideoError> {
        either!(self, |it| it.push(picture))
    }

    pub fn push_at(&mut self, picture: &Decoded, number: u64) -> Result<usize, VideoError> {
        match self {
            #[cfg(feature = "wgpu")]
            Screen::Wgpu(video) => video.push_at(picture, Some(number)),
            #[cfg(unix)]
            Screen::Gles(present) => {
                let _ = number;
                present.push(picture)
            }
            #[cfg(not(any(feature = "wgpu", unix)))]
            Screen::Never(never) => match *never {},
        }
    }

    pub fn set_inset(&mut self, inset: Option<[f32; 5]>) {
        match self {
            #[cfg(feature = "wgpu")]
            Screen::Wgpu(video) => video.set_inset(inset),
            #[cfg(unix)]
            Screen::Gles(present) => present.set_inset(inset),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    /// レンダリングしたなら真。表示するのは呼び出し側（画面を重ねるうえ、サーフェスは 1 フレームに一度しか取れない）。
    pub fn draw(&mut self, canvas: &Canvas<'_>) -> Result<bool, VideoError> {
        match (self, canvas) {
            #[cfg(feature = "wgpu")]
            (Self::Wgpu(video), Canvas::Wgpu { view, onto }) => {
                Ok(video.draw_onto(view, *onto).is_some())
            }
            #[cfg(unix)]
            (
                Self::Gles(present),
                Canvas::Gles {
                    layer, area, hdr, ..
                },
            ) => {
                present.set_area(*area);
                present.set_hdr(*hdr);
                present.present(*layer).map(|shown| shown.is_some())
            }
            #[cfg(unix)]
            _ => Err(VideoError::Unsupported(
                "レンダラーとレンダリング先が一致しない",
            )),
            #[cfg(not(any(feature = "wgpu", unix)))]
            (Self::Never(never), _) => match *never {},
        }
    }

    /// 出力がまだビルドできないとき前のフレームをもう一度レンダリングする（そのまま表示すると数フレーム前のフレームが出る）。
    #[cfg_attr(not(unix), allow(irrefutable_let_patterns))]
    pub fn redraw(&mut self, canvas: &Canvas<'_>) {
        match (self, canvas) {
            #[cfg(feature = "wgpu")]
            (Self::Wgpu(video), Canvas::Wgpu { view, onto }) => {
                video.redraw_onto(view, *onto);
            }
            // GLES 2.0 は画面が変わったときだけレンダリングし直して表示する。
            #[cfg(unix)]
            (
                Self::Gles(present),
                Canvas::Gles {
                    layer,
                    area,
                    black,
                    hdr,
                },
            ) => {
                present.set_area(*area);
                present.set_hdr(*hdr);
                if let Err(error) = present.repaint(*layer, *black) {
                    tracing::warn!("再レンダリングできない: {error:?}");
                }
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    #[cfg(unix)]
    pub fn capture_gles(
        &mut self,
        size: (u32, u32),
        video: Option<(u32, u32, u32, u32)>,
        layer: Option<&dyn crate::output::present::Layer>,
    ) -> Result<crate::output::present::Readback, VideoError> {
        match self {
            Self::Gles(present) => present.capture(size, video, layer),
            #[allow(unreachable_patterns)]
            _ => Err(VideoError::Unsupported("GLES 2.0 のレンダラーではない")),
        }
    }

    pub fn redraw_unfiltered(&mut self, canvas: &Canvas<'_>) {
        #[cfg(feature = "wgpu")]
        if let (Some(video), Canvas::Wgpu { view, onto }) = (self.wgpu_mut(), canvas) {
            video.redraw_unfiltered_onto(view, *onto);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = canvas;
    }

    pub fn set_pan_scan(&mut self, pan_scan: crate::output::video::pan_scan::PanScan) {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(video) => video.set_pan_scan(pan_scan),
            #[cfg(unix)]
            Self::Gles(present) => present.set_pan_scan(pan_scan),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    pub fn ready(&self) -> bool {
        either!(self, |it| it.ready())
    }

    pub fn pending(&self) -> usize {
        either!(self, |it| it.pending())
    }

    pub fn skip(&mut self) -> bool {
        either!(self, |it| it.skip())
    }

    pub fn set_display_rate(&mut self, mhz: u32) {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu_mut() {
            video.set_display_rate(mhz);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = mhz;
    }

    pub fn last_phases(&self) -> Vec<f32> {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu() {
            return video.last_phases().to_vec();
        }
        Vec::new()
    }

    pub fn take_lost(&mut self) -> usize {
        either!(self, |it| it.take_lost())
    }

    pub fn flush(&mut self) {
        either!(self, |it| it.flush())
    }

    pub fn poll(&self) {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(video) => video.poll(),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        either!(self, |it| it.reset())
    }

    pub fn spent(&mut self) -> Spent {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(video) => {
                let spent = video.spent();
                Spent {
                    upload: spent.upload,
                    acquire: spent.acquire,
                    draw: spent.draw,
                    show: spent.show,
                    pushed: spent.pushed,
                    shown: spent.shown,
                    woven: spent.woven,
                    locked: spent.locked,
                    whole_frames: spent.whole_frames,
                    soft: spent.soft,
                    matched: spent.matched,
                    film: spent.film,
                    overlay: spent.overlay,
                    film_changes: spent.film_changes,
                    combed: spent.combed,
                    stripes: spent.stripes,
                }
            }
            #[cfg(unix)]
            Self::Gles(present) => {
                let spent = present.spent();
                Spent {
                    upload: spent.upload,
                    acquire: Duration::ZERO,
                    draw: spent.draw,
                    show: spent.show,
                    pushed: spent.pushed,
                    shown: spent.shown,
                    woven: spent.woven,
                    locked: spent.locked,
                    whole_frames: false,
                    soft: false,
                    matched: 0,
                    film: false,
                    overlay: false,
                    film_changes: 0,
                    combed: 0,
                    stripes: 0,
                }
            }
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    pub fn settings(&self) -> Settings {
        either!(self, |it| it.settings())
    }

    #[cfg(all(feature = "wgpu", feature = "plugins"))]
    pub fn filter_report(&self) -> Option<crate::output::video::hook::Report> {
        self.wgpu().map(|video| video.filter_report())
    }

    #[cfg(feature = "plugins")]
    pub fn filter_held_ms(&self) -> f64 {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu() {
            return video.filter_held_ms();
        }
        0.0
    }

    #[cfg(feature = "plugins")]
    pub fn set_filter_program(&mut self, program: Option<zwsdk::video::Program>) {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu_mut() {
            video.set_filter_program(program);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = program;
    }

    /// 方式を自動のままシェーダーでデインタレースしている（GPU を測って方式を選ぶ）。
    pub fn wants_measure(&self) -> bool {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu() {
            return video.wants_measure();
        }
        false
    }

    pub fn set_scaling(&mut self, scaling: crate::output::video::Scaling) {
        #[cfg(feature = "wgpu")]
        if let Some(video) = self.wgpu_mut() {
            video.set_scaling(scaling);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = scaling;
    }

    pub fn set_adjust(&mut self, adjust: crate::output::video::Adjust) {
        #[cfg(not(any(feature = "wgpu", unix)))]
        let _ = adjust;
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(video) => video.set_adjust(adjust),
            #[cfg(unix)]
            Self::Gles(present) => present.set_adjust(adjust),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    pub fn set_settings(&mut self, settings: Settings) {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(video) => video.set_settings(settings),
            #[cfg(unix)]
            Self::Gles(present) => {
                if let Err(error) = present.set_settings(settings) {
                    tracing::warn!("映像の設定を変更できない: {error}");
                }
            }
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }

    pub fn about(&self) -> String {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Wgpu(_) => String::new(),
            #[cfg(unix)]
            Self::Gles(present) => format!("  サンプリング方式 {:?}", present.taps()),
            #[cfg(not(any(feature = "wgpu", unix)))]
            Self::Never(never) => match *never {},
        }
    }
}

/// 測った結果。GPU（名前にドライバーの版が入る）とフレームの大きさごと。アプリが保存して次の起動で [`recall`] する。
#[cfg(feature = "wgpu")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    pub gpu: String,
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub progressive: bool,
    pub deinterlace: crate::output::video::Deinterlace,
    pub upload: Upload,
}

#[cfg(feature = "wgpu")]
static MEASURED: std::sync::Mutex<Vec<Measure>> = std::sync::Mutex::new(Vec::new());

/// 測っている最中の物（GPU と形式）。
#[cfg(feature = "wgpu")]
static MEASURING: std::sync::Mutex<Vec<(String, Format)>> = std::sync::Mutex::new(Vec::new());

/// 前に測った結果を入れる（起動したとき）。
#[cfg(feature = "wgpu")]
pub fn recall(measures: Vec<Measure>) {
    let mut kept = MEASURED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for measure in measures {
        if !kept.iter().any(|have| same(have, &measure)) {
            kept.push(measure);
        }
    }
}

/// これまでに測った結果（保存する物）。
#[cfg(feature = "wgpu")]
pub fn measures() -> Vec<Measure> {
    MEASURED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(feature = "wgpu")]
pub fn measuring() -> bool {
    !MEASURING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

#[cfg(feature = "wgpu")]
fn same(a: &Measure, b: &Measure) -> bool {
    a.gpu == b.gpu
        && a.width == b.width
        && a.height == b.height
        && a.depth == b.depth
        && a.progressive == b.progressive
}

/// プログレッシブを見ない設定では、インターレースと同じに測って別に覚える。
#[cfg(feature = "wgpu")]
fn measured_shape(format: Format) -> Format {
    Format {
        progressive: format.progressive && crate::output::video::adapt_progressive(),
        ..format
    }
}

#[cfg(feature = "wgpu")]
fn wanted(gpu: &Gpu, format: Format) -> Measure {
    Measure {
        gpu: gpu.name(),
        width: format.width,
        height: format.height,
        depth: format.depth,
        progressive: format.progressive,
        deinterlace: crate::output::video::Deinterlace::Bob,
        upload: Upload::default(),
    }
}

/// 測った設定。まだ測っていなければ `None`（測るのは [`Painter::measure`]、それまでは軽い既定で表示する）。
#[cfg(feature = "wgpu")]
fn measured(
    gpu: &Gpu,
    format: Format,
) -> Option<(Settings, Upload, crate::output::video::Deinterlace)> {
    let format = measured_shape(format);
    let wanted = wanted(gpu, format);
    let found = MEASURED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|have| same(have, &wanted))
        .cloned()?;
    let mut settings = Settings::of_format(format);
    if settings.deinterlace != crate::output::video::Deinterlace::Weave {
        settings.deinterlace = found.deinterlace;
    }
    Some((settings, found.upload, found.deinterlace))
}

/// 別のスレッドで測り始める（もう測ったか測っている最中なら何もしない）。UI のスレッドで測ると、遅い GPU では数十秒止まる。
#[cfg(feature = "wgpu")]
fn start_measure(gpu: &Gpu, format: Format) {
    const MOST: usize = 16;
    if measured(gpu, format).is_some() {
        return;
    }
    let format = measured_shape(format);
    let name = gpu.name();
    let wanted = wanted(gpu, format);
    {
        let mut measuring = MEASURING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if measuring
            .iter()
            .any(|(gpu, shape)| *gpu == name && *shape == format)
        {
            return;
        }
        measuring.push((name.clone(), format));
    }
    let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
    let spawned = std::thread::Builder::new()
        .name(String::from("GPU の計測"))
        .spawn(move || {
            let began = std::time::Instant::now();
            let capability = Capability::measure_with(&device, &queue, format);
            let settings = capability.settings(format);
            tracing::info!(
                "GPU への転送 {:?}（ステージング {:.2} ms、マップ {:.2} ms）、デインターレース {:?}（{}）、計測 {:.1} 秒",
                capability.upload,
                capability.staged_ms,
                capability.mapped_ms,
                settings.deinterlace,
                capability
                    .costs
                    .iter()
                    .map(|(quality, ms)| format!("{} {ms:.2} ms", quality.name()))
                    .collect::<Vec<_>>()
                    .join("、"),
                began.elapsed().as_secs_f64()
            );
            {
                let mut kept = MEASURED
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if kept.len() >= MOST {
                    kept.remove(0);
                }
                kept.push(Measure {
                    deinterlace: capability.affordable(),
                    upload: capability.upload,
                    ..wanted
                });
            }
            MEASURING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(gpu, shape)| !(*gpu == name && *shape == format));
        });
    if spawned.is_err() {
        MEASURING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}
