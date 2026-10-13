// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコーダーが DMA-BUF で返したフレームをコピーせずにインポートする。10 ビットと修飾子付き（タイル配置など）のフレームは外部テクスチャだけ。
//! 線形の 8 ビット NV12 は RGBA8 のテクスチャへレンダリングする（外部テクスチャは両フィールドの色差を混ぜる）。
//! panfrost は線形の行に 64 バイトの揃えが要るので、k 行を横に並べた 1 つの画像としてインポートする。

use std::ffi::c_void;
use std::os::fd::AsRawFd;

use super::egl::{self, Egl};
use super::gl::{self, Gl};
use super::shader::{draw_triangle, link, uniform};
use crate::decode::DmaBuf;
use crate::output::video::Error;
use crate::output::video::field::Parity;

type EGLImage = *mut c_void;

const EGL_LINUX_DMA_BUF_EXT: egl::EGLenum = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: egl::EGLint = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: egl::EGLint = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: egl::EGLint = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: egl::EGLint = 0x3274;
const EGL_DMA_BUF_PLANE1_FD_EXT: egl::EGLint = 0x3275;
const EGL_DMA_BUF_PLANE1_OFFSET_EXT: egl::EGLint = 0x3276;
const EGL_DMA_BUF_PLANE1_PITCH_EXT: egl::EGLint = 0x3277;
const EGL_YUV_COLOR_SPACE_HINT_EXT: egl::EGLint = 0x327B;
const EGL_SAMPLE_RANGE_HINT_EXT: egl::EGLint = 0x327C;
const EGL_ITU_REC709_EXT: egl::EGLint = 0x3280;
const EGL_ITU_REC2020_EXT: egl::EGLint = 0x3281;
const EGL_YUV_FULL_RANGE_EXT: egl::EGLint = 0x3282;
const EGL_YUV_NARROW_RANGE_EXT: egl::EGLint = 0x3283;

const DRM_FORMAT_R8: u32 = u32::from_le_bytes(*b"R8  ");
const DRM_FORMAT_GR88: u32 = u32::from_le_bytes(*b"GR88");
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: egl::EGLint = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: egl::EGLint = 0x3444;
const EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT: egl::EGLint = 0x3445;
const EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT: egl::EGLint = 0x3446;

const WHOLE: [u32; 4] = [DmaBuf::NV12, DmaBuf::P010, DmaBuf::P030, DmaBuf::NV15];

type QueryFormats = unsafe extern "C" fn(
    egl::EGLDisplay,
    egl::EGLint,
    *mut egl::EGLint,
    *mut egl::EGLint,
) -> egl::EGLBoolean;
type QueryModifiers = unsafe extern "C" fn(
    egl::EGLDisplay,
    egl::EGLint,
    egl::EGLint,
    *mut u64,
    *mut egl::EGLBoolean,
    *mut egl::EGLint,
) -> egl::EGLBoolean;

/// [`crate::decode::takes`] へ渡す物。修飾子を並べない形は線形だけと見る。
/// # Safety
/// GL のコンテキストがこのスレッドで有効になっていること。
pub unsafe fn taken(egl: &Egl, gl: &Gl, display: egl::EGLDisplay) -> Vec<(u32, u64)> {
    let mut out = vec![(DmaBuf::NV12, DRM_FORMAT_MOD_LINEAR)];
    let egl_extensions = egl.query(display, egl::EGL_EXTENSIONS);
    let has = |name: &str| {
        egl_extensions
            .split_whitespace()
            .any(|offered| offered == name)
    };
    let external = gl
        .string(gl::GL_EXTENSIONS)
        .split_whitespace()
        .any(|name| name == "GL_OES_EGL_image_external");
    if !has("EGL_EXT_image_dma_buf_import") || !external {
        return out;
    }
    if !has("EGL_EXT_image_dma_buf_import_modifiers") {
        return out;
    }
    let (Some(formats), Some(modifiers)) = (
        egl.proc_address::<QueryFormats>("eglQueryDmaBufFormatsEXT"),
        egl.proc_address::<QueryModifiers>("eglQueryDmaBufModifiersEXT"),
    ) else {
        return out;
    };
    let mut count = 0;
    // SAFETY: 数だけを問い合わせる（配列は渡さない）。
    if unsafe { formats(display, 0, std::ptr::null_mut(), &mut count) } == 0 || count <= 0 {
        return out;
    }
    let mut listed = vec![0; count as usize];
    // SAFETY: `listed` は `count` 個分ある。
    if unsafe { formats(display, count, listed.as_mut_ptr(), &mut count) } == 0 {
        return out;
    }
    listed.truncate(count.max(0) as usize);
    for fourcc in WHOLE {
        if !listed.contains(&(fourcc as egl::EGLint)) {
            continue;
        }
        let mut many = 0;
        // SAFETY: 数だけを問い合わせる。
        let asked = unsafe {
            modifiers(
                display,
                fourcc as egl::EGLint,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut many,
            )
        };
        if asked == 0 {
            continue;
        }
        if many <= 0 {
            out.push((fourcc, DRM_FORMAT_MOD_LINEAR));
            continue;
        }
        let mut values = vec![0u64; many as usize];
        let mut external_only = vec![0; many as usize];
        // SAFETY: どちらの配列も `many` 個分ある。
        let asked = unsafe {
            modifiers(
                display,
                fourcc as egl::EGLint,
                many,
                values.as_mut_ptr(),
                external_only.as_mut_ptr(),
                &mut many,
            )
        };
        if asked == 0 {
            continue;
        }
        values.truncate(many.max(0) as usize);
        for modifier in values {
            if !out.contains(&(fourcc, modifier)) {
                out.push((fourcc, modifier));
            }
        }
    }
    out
}

/// 8 行を超えて揃わない行の幅は、放送で使う解像度には無い。
const ROWS: [u32; 4] = [1, 2, 4, 8];

/// `gl_FragCoord` は mediump で、1024 を超えると奇数の列を取り違えるので `place` を使う。
const VERTEX: &str = include_str!("glsl/corners.vert.glsl");

/// 行 0 はどちらもメモリーの先頭なので上下は裏返らない。
const FRAGMENT: &str = "#version 100
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D plane;
uniform vec2 source;
uniform float rows;
uniform float pitch;
uniform float chroma;
uniform vec2 target;
varying vec2 place;
void main() {
    vec2 at = floor(place * target);
    float row = mod(at.y, rows);
    vec2 from = vec2(at.x + row * pitch, floor(at.y / rows));
    vec4 s = texture2D(plane, (from + 0.5) / source);
    gl_FragColor = chroma > 0.5 ? vec4(s.rrr, s.g) : vec4(s.r);
}
";

#[derive(Debug)]
pub struct Import {
    create_image: unsafe extern "C" fn(
        egl::EGLDisplay,
        egl::EGLContext,
        egl::EGLenum,
        *mut c_void,
        *const egl::EGLint,
    ) -> EGLImage,
    destroy_image: unsafe extern "C" fn(egl::EGLDisplay, EGLImage) -> egl::EGLBoolean,
    target_texture: unsafe extern "C" fn(gl::GLenum, EGLImage),
    program: gl::GLuint,
    source: gl::GLint,
    rows: gl::GLint,
    pitch: gl::GLint,
    chroma: gl::GLint,
    target: gl::GLint,
    plane: gl::GLint,
    textures: [gl::GLuint; 2],
    frame_buffer: gl::GLuint,
    /// 拒否される並べ方を毎回試すと Mesa が毎回警告を出す。
    rows_for: Vec<(u32, u32)>,
    /// fragment の highp が無いと、並べた画像の座標（2880 など）が表せない。
    highp: bool,
    modifiers: bool,
}

/// フィールドは行の幅を 2 倍にし、下は 1 行ずらす（4:2:0 のインターレースは色差の行も交互）。
#[derive(Debug, Clone, Copy)]
pub struct FieldView {
    pub height: u32,
    rows: u32,
    skip: u32,
}

impl FieldView {
    pub fn of(frame: &DmaBuf, field: Option<Parity>) -> Self {
        match field {
            None => Self {
                height: frame.height,
                rows: 1,
                skip: 0,
            },
            Some(parity) => Self {
                height: frame.height / 2,
                rows: 2,
                skip: parity.first_row(),
            },
        }
    }

    pub fn offset(&self, plane: &crate::decode::DmaBufPlane) -> u32 {
        plane.offset + self.skip * plane.stride
    }

    pub fn pitch(&self, plane: &crate::decode::DmaBufPlane) -> u32 {
        plane.stride * self.rows
    }
}

/// サンプリングすると RGB が返る。
#[derive(Debug)]
pub struct External {
    pub texture: gl::GLuint,
    image: EGLImage,
}

impl Import {
    /// インポートできない環境では `None`。
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn open(egl: &Egl, gl: &Gl, display: egl::EGLDisplay) -> Result<Self, Error> {
        let egl_extensions = egl.query(display, egl::EGL_EXTENSIONS);
        if !egl_extensions
            .split_whitespace()
            .any(|name| name == "EGL_EXT_image_dma_buf_import")
        {
            return Err(Error::Unsupported(
                "EGL_EXT_image_dma_buf_import が無い（DMA-BUF のフレームをインポートできない）",
            ));
        }
        if !gl
            .string(gl::GL_EXTENSIONS)
            .split_whitespace()
            .any(|name| name == "GL_OES_EGL_image")
        {
            return Err(Error::Unsupported("GL_OES_EGL_image が無い"));
        }
        let missing = |name: &str| Error::NoDevice(format!("{name} がありません"));
        let create_image = egl
            .proc_address("eglCreateImageKHR")
            .ok_or_else(|| missing("eglCreateImageKHR"))?;
        let destroy_image = egl
            .proc_address("eglDestroyImageKHR")
            .ok_or_else(|| missing("eglDestroyImageKHR"))?;
        let target_texture = egl
            .proc_address("glEGLImageTargetTexture2DOES")
            .ok_or_else(|| missing("glEGLImageTargetTexture2DOES"))?;
        let mut textures = [0; 2];
        let mut frame_buffer = 0;
        // SAFETY: 呼び出し側がコンテキストを約束している。名前は受け取る場所がある。
        let program = unsafe {
            let program = link(gl, VERTEX, FRAGMENT)?;
            (gl.GenTextures)(2, textures.as_mut_ptr());
            for name in textures {
                (gl.BindTexture)(gl::GL_TEXTURE_2D, name);
                for (what, value) in [
                    (gl::GL_TEXTURE_MIN_FILTER, gl::GL_NEAREST),
                    (gl::GL_TEXTURE_MAG_FILTER, gl::GL_NEAREST),
                    (gl::GL_TEXTURE_WRAP_S, gl::GL_CLAMP_TO_EDGE),
                    (gl::GL_TEXTURE_WRAP_T, gl::GL_CLAMP_TO_EDGE),
                ] {
                    (gl.TexParameteri)(gl::GL_TEXTURE_2D, what, value);
                }
            }
            (gl.GenFramebuffers)(1, &mut frame_buffer);
            program
        };
        Ok(Self {
            create_image,
            destroy_image,
            target_texture,
            source: uniform(gl, program, "source"),
            rows: uniform(gl, program, "rows"),
            pitch: uniform(gl, program, "pitch"),
            chroma: uniform(gl, program, "chroma"),
            target: uniform(gl, program, "target"),
            plane: uniform(gl, program, "plane"),
            program,
            textures,
            frame_buffer,
            rows_for: Vec::new(),
            highp: gl.fragment_highp(),
            modifiers: egl_extensions
                .split_whitespace()
                .any(|name| name == "EGL_EXT_image_dma_buf_import_modifiers"),
        })
    }

    /// 終わるのは待たない（デコーダーはバッファを数フレームあとまで使い回さない）。
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効で、`corners` が [`super::Present`] の画面全体を覆う三角形であること。
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn unpack(
        &mut self,
        egl: &Egl,
        gl: &Gl,
        display: egl::EGLDisplay,
        corners: gl::GLuint,
        frame: &DmaBuf,
        (width, height): (u32, u32),
        targets: (gl::GLuint, gl::GLuint),
    ) -> Result<(), Error> {
        if frame.modifier != DRM_FORMAT_MOD_LINEAR {
            return Err(Error::Unsupported(
                "タイル配置の DMA-BUF はインポートしない",
            ));
        }
        let [luma, chroma] = frame.planes.as_slice() else {
            return Err(Error::Unsupported("輝度と色差の二つでない DMA-BUF"));
        };
        if luma.format != DRM_FORMAT_R8 || chroma.format != DRM_FORMAT_GR88 {
            return Err(Error::Unsupported("8 ビットの NV12 でない DMA-BUF"));
        }
        let planes = [
            (luma, 1, (width, height), targets.0),
            (
                chroma,
                2,
                (width.div_ceil(2), height.div_ceil(2)),
                targets.1,
            ),
        ];
        let rows = self.rows_of(egl, display, &planes)?;
        let mut images = [std::ptr::null_mut(); 2];
        for (at, (plane, bytes, size, _)) in planes.iter().enumerate() {
            images[at] = self.image(display, plane, *bytes, *size, rows);
            if images[at].is_null() {
                self.destroy(display, &images);
                return Err(Error::NoDevice(format!(
                    "DMA-BUF を EGLImage にできません {:#x}",
                    egl.error()
                )));
            }
        }
        // SAFETY: 呼び出し側がコンテキストを約束している。どの名前もこのコンテキストで作ったもの。
        unsafe {
            (gl.UseProgram)(self.program);
            (gl.Uniform1i)(self.plane, 0);
            (gl.Uniform1f)(self.rows, rows as gl::GLfloat);
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, self.frame_buffer);
            (gl.ActiveTexture)(gl::GL_TEXTURE0);
            for (at, (plane, bytes, (w, h), target)) in planes.iter().enumerate() {
                (gl.FramebufferTexture2D)(
                    gl::GL_FRAMEBUFFER,
                    gl::GL_COLOR_ATTACHMENT0,
                    gl::GL_TEXTURE_2D,
                    *target,
                    0,
                );
                (gl.BindTexture)(gl::GL_TEXTURE_2D, self.textures[at]);
                (self.target_texture)(gl::GL_TEXTURE_2D, images[at]);
                let pitch = plane.stride / bytes;
                (gl.Uniform2f)(
                    self.source,
                    (pitch * rows) as gl::GLfloat,
                    h.div_ceil(rows) as gl::GLfloat,
                );
                (gl.Uniform1f)(self.pitch, pitch as gl::GLfloat);
                (gl.Uniform1f)(self.chroma, at as gl::GLfloat);
                (gl.Uniform2f)(self.target, *w as gl::GLfloat, *h as gl::GLfloat);
                (gl.Viewport)(0, 0, *w as gl::GLsizei, *h as gl::GLsizei);
                draw_triangle(gl, corners, 3);
            }
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, 0);
        }
        // EGLImage を結び付けたテクスチャが中身を持ち続けるので、名前だけ先に放してよい。
        self.destroy(display, &images);
        match gl.error() {
            Some(code) => Err(Error::NoDevice(format!(
                "DMA-BUF をレンダリングでコピーできません {code:#x}"
            ))),
            None => Ok(()),
        }
    }

    /// 返るのは RGB なのでフィルターの要らないフレームだけに使う。SAND は `field` を指定されてもフレームのまま。
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn external(
        &mut self,
        egl: &Egl,
        gl: &Gl,
        display: egl::EGLDisplay,
        frame: &DmaBuf,
        field: Option<Parity>,
        bt2020: bool,
        limited: bool,
    ) -> Result<External, Error> {
        let Some(fourcc) = frame.fourcc() else {
            return Err(Error::Unsupported(
                "NV12・P010・P030・NV15 のどれでもない DMA-BUF",
            ));
        };
        let tiled = frame.modifier != DRM_FORMAT_MOD_LINEAR;
        if tiled && !self.modifiers {
            return Err(Error::Unsupported(
                "修飾子付きの DMA-BUF をインポートする拡張（EGL_EXT_image_dma_buf_import_modifiers）が無い",
            ));
        }
        let [luma, chroma] = frame.planes.as_slice() else {
            return Err(Error::Unsupported("輝度と色差の二つでない DMA-BUF"));
        };
        let view = FieldView::of(frame, if tiled { None } else { field });
        let mut attributes = vec![
            egl::EGL_WIDTH,
            frame.width as egl::EGLint,
            egl::EGL_HEIGHT,
            view.height as egl::EGLint,
            EGL_LINUX_DRM_FOURCC_EXT,
            fourcc as egl::EGLint,
            EGL_DMA_BUF_PLANE0_FD_EXT,
            luma.fd.as_raw_fd(),
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            view.offset(luma) as egl::EGLint,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            view.pitch(luma) as egl::EGLint,
            EGL_DMA_BUF_PLANE1_FD_EXT,
            chroma.fd.as_raw_fd(),
            EGL_DMA_BUF_PLANE1_OFFSET_EXT,
            view.offset(chroma) as egl::EGLint,
            EGL_DMA_BUF_PLANE1_PITCH_EXT,
            view.pitch(chroma) as egl::EGLint,
            EGL_YUV_COLOR_SPACE_HINT_EXT,
            if bt2020 {
                EGL_ITU_REC2020_EXT
            } else {
                EGL_ITU_REC709_EXT
            },
            EGL_SAMPLE_RANGE_HINT_EXT,
            if limited {
                EGL_YUV_NARROW_RANGE_EXT
            } else {
                EGL_YUV_FULL_RANGE_EXT
            },
        ];
        if tiled {
            let low = frame.modifier as u32 as egl::EGLint;
            let high = (frame.modifier >> 32) as u32 as egl::EGLint;
            attributes.extend_from_slice(&[
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                low,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                high,
                EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                low,
                EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
                high,
            ]);
        }
        attributes.push(egl::EGL_NONE);
        // SAFETY: 属性は `EGL_NONE` で終わり、記述子はこの呼び出しのあいだ生きている。
        let image = unsafe {
            (self.create_image)(
                display,
                std::ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                attributes.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(Error::NoDevice(format!(
                "{} の DMA-BUF（修飾子 {:#x}）を EGLImage にできません {:#x}",
                String::from_utf8_lossy(&fourcc.to_le_bytes()),
                frame.modifier,
                egl.error()
            )));
        }
        let mut texture = 0;
        // SAFETY: 呼び出し側がコンテキストを約束している。名前は一つ受け取る。
        unsafe {
            (gl.GenTextures)(1, &mut texture);
            (gl.BindTexture)(gl::GL_TEXTURE_EXTERNAL_OES, texture);
            for (what, value) in [
                (gl::GL_TEXTURE_MIN_FILTER, gl::GL_LINEAR),
                (gl::GL_TEXTURE_MAG_FILTER, gl::GL_LINEAR),
                (gl::GL_TEXTURE_WRAP_S, gl::GL_CLAMP_TO_EDGE),
                (gl::GL_TEXTURE_WRAP_T, gl::GL_CLAMP_TO_EDGE),
            ] {
                (gl.TexParameteri)(gl::GL_TEXTURE_EXTERNAL_OES, what, value);
            }
            (self.target_texture)(gl::GL_TEXTURE_EXTERNAL_OES, image);
        }
        if let Some(code) = gl.error() {
            // SAFETY: 作ったばかりの名前と EGLImage。
            unsafe { (gl.DeleteTextures)(1, &texture) };
            self.destroy(display, &[image]);
            return Err(Error::NoDevice(format!(
                "外部テクスチャに結び付けられません {code:#x}"
            )));
        }
        Ok(External { texture, image })
    }

    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn drop_external(&self, gl: &Gl, display: egl::EGLDisplay, external: External) {
        // SAFETY: このコンテキストで作った名前で、放すのは一度きり。
        unsafe { (gl.DeleteTextures)(1, &external.texture) };
        self.destroy(display, &[external.image]);
    }

    /// 初めての幅は k=1 から試し、通った値を覚える。
    fn rows_of(
        &mut self,
        egl: &Egl,
        display: egl::EGLDisplay,
        planes: &[(&crate::decode::DmaBufPlane, u32, (u32, u32), gl::GLuint); 2],
    ) -> Result<u32, Error> {
        let stride = planes[0].0.stride;
        if let Some(&(_, rows)) = self.rows_for.iter().find(|(known, _)| *known == stride) {
            return Ok(rows);
        }
        for rows in ROWS {
            // mediump は 2048 までの整数しか正しく持たないので、並べるのは highp があるときだけ。
            if rows > 1 && !self.highp {
                break;
            }
            let images =
                planes.map(|(plane, bytes, size, _)| self.image(display, plane, bytes, size, rows));
            let took = images.iter().all(|image| !image.is_null());
            self.destroy(display, &images);
            if took {
                if rows > 1 {
                    tracing::info!(
                        "DMA-BUF: ストライド {stride} バイトはそのままインポートできないので {rows} 行ずつまとめてインポートする"
                    );
                }
                self.rows_for.push((stride, rows));
                return Ok(rows);
            }
        }
        tracing::warn!(
            "DMA-BUF: ストライド {stride} バイトを EGL がインポートできない（{:#x}）",
            egl.error()
        );
        Err(Error::Unsupported(
            "このストライドの DMA-BUF はインポートできない",
        ))
    }

    /// 拒否されたら空。
    fn image(
        &self,
        display: egl::EGLDisplay,
        plane: &crate::decode::DmaBufPlane,
        bytes: u32,
        (_, height): (u32, u32),
        rows: u32,
    ) -> EGLImage {
        let attributes = [
            egl::EGL_WIDTH,
            (plane.stride / bytes * rows) as egl::EGLint,
            egl::EGL_HEIGHT,
            height.div_ceil(rows) as egl::EGLint,
            EGL_LINUX_DRM_FOURCC_EXT,
            plane.format as egl::EGLint,
            EGL_DMA_BUF_PLANE0_FD_EXT,
            plane.fd.as_raw_fd(),
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            plane.offset as egl::EGLint,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            (plane.stride * rows) as egl::EGLint,
            egl::EGL_NONE,
        ];
        // SAFETY: 属性は `EGL_NONE` で終わり、記述子は呼び出しのあいだ生きている（EGL はコピーして持つ）。
        unsafe {
            (self.create_image)(
                display,
                std::ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                attributes.as_ptr(),
            )
        }
    }

    fn destroy(&self, display: egl::EGLDisplay, images: &[EGLImage]) {
        for &image in images {
            if !image.is_null() {
                // SAFETY: このディスプレイで作った EGLImage で、放すのは一度きり。
                unsafe { (self.destroy_image)(display, image) };
            }
        }
    }

    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn shut(&mut self, gl: &Gl) {
        // SAFETY: 呼び出し側がコンテキストを約束している。どれもこのコンテキストで作った名前。
        unsafe {
            (gl.DeleteTextures)(2, self.textures.as_ptr());
            (gl.DeleteFramebuffers)(1, &self.frame_buffer);
            (gl.DeleteProgram)(self.program);
        }
    }
}
