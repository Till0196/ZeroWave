// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! OpenGL ES 2.0 を自前の `extern "C"` 宣言で呼ぶ。
//! 2.0 を要求しても「2.0 以上」が返るので、3.x の関数を呼んでも開発環境では通ってしまう。
//! テーブルの名前が全部 ES 2.0 の命令であることをテストで確かめる。

#![allow(non_snake_case)]
// `GLES2/gl2.h` との対応が取れるよう、いま呼んでいない定数と型も残す。
#![allow(dead_code)]

use std::ffi::{CStr, c_char, c_void};

use crate::dl::Object;
use crate::output::video::Error;

pub type GLenum = u32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLuint = u32;
pub type GLfloat = f32;
pub type GLintptr = isize;
pub type GLsizeiptr = isize;

pub const GL_NO_ERROR: GLenum = 0;
pub const GL_FALSE: GLint = 0;
pub const GL_TRUE: GLint = 1;
pub const GL_TRIANGLES: GLenum = 0x0004;
pub const GL_UNSIGNED_BYTE: GLenum = 0x1401;
pub const GL_FLOAT: GLenum = 0x1406;
pub const GL_RGB: GLenum = 0x1907;
pub const GL_RGBA: GLenum = 0x1908;
/// `OES_texture_half_float`。
pub const GL_HALF_FLOAT_OES: GLenum = 0x8D61;
pub const GL_LUMINANCE: GLenum = 0x1909;
pub const GL_LUMINANCE_ALPHA: GLenum = 0x190A;
pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
/// `GL_OES_EGL_image_external`（拡張の名前なので ES 2.0 の関数テーブルには入らない）。
pub const GL_TEXTURE_EXTERNAL_OES: GLenum = 0x8D65;
pub const GL_TEXTURE0: GLenum = 0x84C0;
pub const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
pub const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
pub const GL_NEAREST: GLint = 0x2600;
pub const GL_LINEAR: GLint = 0x2601;
pub const GL_CLAMP_TO_EDGE: GLint = 0x812F;
pub const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
pub const GL_PACK_ALIGNMENT: GLenum = 0x0D05;
pub const GL_FRAMEBUFFER: GLenum = 0x8D40;
pub const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const GL_COLOR_BUFFER_BIT: GLbitfield = 0x0000_4000;
pub const GL_ARRAY_BUFFER: GLenum = 0x8892;
pub const GL_ELEMENT_ARRAY_BUFFER: GLenum = 0x8893;
pub const GL_STATIC_DRAW: GLenum = 0x88E4;
pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const GL_VENDOR: GLenum = 0x1F00;
pub const GL_RENDERER: GLenum = 0x1F01;
pub const GL_VERSION: GLenum = 0x1F02;
pub const GL_EXTENSIONS: GLenum = 0x1F03;
pub const GL_SHADING_LANGUAGE_VERSION: GLenum = 0x8B8C;
pub const GL_MAX_TEXTURE_SIZE: GLenum = 0x0D33;
pub const GL_MAX_TEXTURE_IMAGE_UNITS: GLenum = 0x8872;
pub const GL_MAX_VARYING_VECTORS: GLenum = 0x8DFC;
pub const GL_MAX_FRAGMENT_UNIFORM_VECTORS: GLenum = 0x8DFD;
pub const GL_HIGH_FLOAT: GLenum = 0x8DF2;
pub const GL_DEPTH_TEST: GLenum = 0x0B71;
pub const GL_CULL_FACE: GLenum = 0x0B44;
pub const GL_BLEND: GLenum = 0x0BE2;
pub const GL_SCISSOR_TEST: GLenum = 0x0C11;
pub const GL_DITHER: GLenum = 0x0BD0;
pub const GL_STENCIL_TEST: GLenum = 0x0B90;

/// このパスが使う ES 2.0 の関数。下の一覧と突き合わせる。
pub const USED: [&str; 52] = [
    "glActiveTexture",
    "glAttachShader",
    "glBindAttribLocation",
    "glBindBuffer",
    "glBindFramebuffer",
    "glBindTexture",
    "glBufferData",
    "glCheckFramebufferStatus",
    "glClear",
    "glClearColor",
    "glCompileShader",
    "glCopyTexSubImage2D",
    "glCreateProgram",
    "glCreateShader",
    "glDeleteBuffers",
    "glDeleteFramebuffers",
    "glDeleteProgram",
    "glDeleteShader",
    "glDeleteTextures",
    "glDisable",
    "glDrawArrays",
    "glEnable",
    "glEnableVertexAttribArray",
    "glFinish",
    "glFramebufferTexture2D",
    "glGenBuffers",
    "glGenFramebuffers",
    "glGenTextures",
    "glGetError",
    "glGetIntegerv",
    "glGetProgramInfoLog",
    "glGetProgramiv",
    "glGetShaderInfoLog",
    "glGetShaderPrecisionFormat",
    "glGetShaderiv",
    "glGetString",
    "glGetUniformLocation",
    "glLinkProgram",
    "glPixelStorei",
    "glReadPixels",
    "glScissor",
    "glShaderSource",
    "glTexImage2D",
    "glTexParameteri",
    "glTexSubImage2D",
    "glUniform1f",
    "glUniform1i",
    "glUniform2f",
    "glUniform4f",
    "glUseProgram",
    "glVertexAttribPointer",
    "glViewport",
];

/// 解決したエントリーポイント。追加したら [`USED`] にも足すこと（テストが見ている）。
#[derive(Debug)]
pub struct Gl {
    library: Object,
    pub ActiveTexture: unsafe extern "C" fn(GLenum),
    pub AttachShader: unsafe extern "C" fn(GLuint, GLuint),
    pub BindAttribLocation: unsafe extern "C" fn(GLuint, GLuint, *const c_char),
    pub BindBuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub BindFramebuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub BindTexture: unsafe extern "C" fn(GLenum, GLuint),
    pub BufferData: unsafe extern "C" fn(GLenum, GLsizeiptr, *const c_void, GLenum),
    pub CheckFramebufferStatus: unsafe extern "C" fn(GLenum) -> GLenum,
    pub Clear: unsafe extern "C" fn(GLbitfield),
    pub ClearColor: unsafe extern "C" fn(GLfloat, GLfloat, GLfloat, GLfloat),
    pub CompileShader: unsafe extern "C" fn(GLuint),
    pub CopyTexSubImage2D:
        unsafe extern "C" fn(GLenum, GLint, GLint, GLint, GLint, GLint, GLsizei, GLsizei),
    pub CreateProgram: unsafe extern "C" fn() -> GLuint,
    pub CreateShader: unsafe extern "C" fn(GLenum) -> GLuint,
    pub DeleteBuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub DeleteFramebuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub DeleteProgram: unsafe extern "C" fn(GLuint),
    pub DeleteShader: unsafe extern "C" fn(GLuint),
    pub DeleteTextures: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub Disable: unsafe extern "C" fn(GLenum),
    pub Enable: unsafe extern "C" fn(GLenum),
    pub DrawArrays: unsafe extern "C" fn(GLenum, GLint, GLsizei),
    pub EnableVertexAttribArray: unsafe extern "C" fn(GLuint),
    pub Finish: unsafe extern "C" fn(),
    pub FramebufferTexture2D: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    pub GenBuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub GenFramebuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub GenTextures: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub GetError: unsafe extern "C" fn() -> GLenum,
    pub GetIntegerv: unsafe extern "C" fn(GLenum, *mut GLint),
    pub GetProgramInfoLog: unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut c_char),
    pub GetProgramiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    pub GetShaderInfoLog: unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut c_char),
    pub GetShaderPrecisionFormat: unsafe extern "C" fn(GLenum, GLenum, *mut GLint, *mut GLint),
    pub GetShaderiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    pub GetString: unsafe extern "C" fn(GLenum) -> *const c_char,
    pub GetUniformLocation: unsafe extern "C" fn(GLuint, *const c_char) -> GLint,
    pub LinkProgram: unsafe extern "C" fn(GLuint),
    pub PixelStorei: unsafe extern "C" fn(GLenum, GLint),
    pub Scissor: unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei),
    pub ReadPixels:
        unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void),
    pub ShaderSource: unsafe extern "C" fn(GLuint, GLsizei, *const *const c_char, *const GLint),
    pub TexImage2D: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLint,
        GLenum,
        GLenum,
        *const c_void,
    ),
    pub TexParameteri: unsafe extern "C" fn(GLenum, GLenum, GLint),
    pub TexSubImage2D: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLenum,
        GLenum,
        *const c_void,
    ),
    pub Uniform1f: unsafe extern "C" fn(GLint, GLfloat),
    pub Uniform1i: unsafe extern "C" fn(GLint, GLint),
    pub Uniform2f: unsafe extern "C" fn(GLint, GLfloat, GLfloat),
    pub Uniform4f: unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat, GLfloat),
    pub UseProgram: unsafe extern "C" fn(GLuint),
    pub VertexAttribPointer:
        unsafe extern "C" fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const c_void),
    pub Viewport: unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei),
}

impl Gl {
    /// `libGLESv2` を開いてエントリーポイントを解決し、無いものは `eglGetProcAddress` で拾う。
    pub fn open(egl: &super::egl::Egl) -> Result<Self, Error> {
        let library = Object::open_any(&["libGLESv2.so.2", "libGLESv2.so"])
            .map_err(|missing| Error::NoDevice(missing.0))?;
        let get = |name: &str| -> Result<*mut c_void, Error> {
            // SAFETY: 解決するのはアドレスだけで、型に読み替えるのは下の `entry!`。
            unsafe { library.find::<*mut c_void>(name) }
                .or_else(|| egl.proc_address::<*mut c_void>(name))
                .filter(|pointer| !pointer.is_null())
                .ok_or_else(|| Error::NoDevice(format!("{name} がありません")))
        };
        // SAFETY: 直上で空でないことを確かめた関数ポインタを、`GLES2/gl2.h` の型に読み替える。
        macro_rules! entry {
            ($name:literal) => {
                unsafe { std::mem::transmute_copy(&get($name)?) }
            };
        }
        Ok(Self {
            ActiveTexture: entry!("glActiveTexture"),
            AttachShader: entry!("glAttachShader"),
            BindAttribLocation: entry!("glBindAttribLocation"),
            BindBuffer: entry!("glBindBuffer"),
            BindFramebuffer: entry!("glBindFramebuffer"),
            BindTexture: entry!("glBindTexture"),
            BufferData: entry!("glBufferData"),
            CheckFramebufferStatus: entry!("glCheckFramebufferStatus"),
            Clear: entry!("glClear"),
            ClearColor: entry!("glClearColor"),
            CompileShader: entry!("glCompileShader"),
            CopyTexSubImage2D: entry!("glCopyTexSubImage2D"),
            CreateProgram: entry!("glCreateProgram"),
            CreateShader: entry!("glCreateShader"),
            DeleteBuffers: entry!("glDeleteBuffers"),
            DeleteFramebuffers: entry!("glDeleteFramebuffers"),
            DeleteProgram: entry!("glDeleteProgram"),
            DeleteShader: entry!("glDeleteShader"),
            DeleteTextures: entry!("glDeleteTextures"),
            Disable: entry!("glDisable"),
            Enable: entry!("glEnable"),
            DrawArrays: entry!("glDrawArrays"),
            EnableVertexAttribArray: entry!("glEnableVertexAttribArray"),
            Finish: entry!("glFinish"),
            FramebufferTexture2D: entry!("glFramebufferTexture2D"),
            GenBuffers: entry!("glGenBuffers"),
            GenFramebuffers: entry!("glGenFramebuffers"),
            GenTextures: entry!("glGenTextures"),
            GetError: entry!("glGetError"),
            GetIntegerv: entry!("glGetIntegerv"),
            GetProgramInfoLog: entry!("glGetProgramInfoLog"),
            GetProgramiv: entry!("glGetProgramiv"),
            GetShaderInfoLog: entry!("glGetShaderInfoLog"),
            GetShaderPrecisionFormat: entry!("glGetShaderPrecisionFormat"),
            GetShaderiv: entry!("glGetShaderiv"),
            GetString: entry!("glGetString"),
            GetUniformLocation: entry!("glGetUniformLocation"),
            LinkProgram: entry!("glLinkProgram"),
            PixelStorei: entry!("glPixelStorei"),
            ReadPixels: entry!("glReadPixels"),
            Scissor: entry!("glScissor"),
            ShaderSource: entry!("glShaderSource"),
            TexImage2D: entry!("glTexImage2D"),
            TexParameteri: entry!("glTexParameteri"),
            TexSubImage2D: entry!("glTexSubImage2D"),
            Uniform1f: entry!("glUniform1f"),
            Uniform1i: entry!("glUniform1i"),
            Uniform2f: entry!("glUniform2f"),
            Uniform4f: entry!("glUniform4f"),
            UseProgram: entry!("glUseProgram"),
            VertexAttribPointer: entry!("glVertexAttribPointer"),
            Viewport: entry!("glViewport"),
            library,
        })
    }

    pub fn string(&self, what: GLenum) -> String {
        // SAFETY: GL は未知の列挙値には NULL を返す。
        let text = unsafe { (self.GetString)(what) };
        if text.is_null() {
            return String::new();
        }
        // SAFETY: GL の返す文字列は終端された静的なもの。
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn integer(&self, what: GLenum) -> GLint {
        let mut value = 0;
        // SAFETY: 単一の値を返す問い合わせで、書ける場所を渡している。
        unsafe { (self.GetIntegerv)(what, &mut value) };
        value
    }

    /// fragment で `highp` が使えると報告するか（ES 2.0 §6.1.8）。texel を読めるかはレンダリングして確かめる（[`super::Capability`]）。
    pub fn fragment_highp(&self) -> bool {
        let mut range = [0i32; 2];
        let mut precision = 0i32;
        // SAFETY: 出力は2つと1つの `GLint` で、どちらも書ける場所。
        unsafe {
            (self.GetShaderPrecisionFormat)(
                GL_FRAGMENT_SHADER,
                GL_HIGH_FLOAT,
                range.as_mut_ptr(),
                &mut precision,
            )
        };
        range != [0, 0] && precision != 0
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.string(GL_EXTENSIONS)
            .split_whitespace()
            .any(|offered| offered == name)
    }

    /// 半精度の浮動小数のテクスチャへレンダリングでき、バイリニアで読めるか（HDR の画面の中間に使う）。
    pub fn half_float_target(&self) -> bool {
        self.has_extension("GL_OES_texture_half_float")
            && self.has_extension("GL_OES_texture_half_float_linear")
            && self.has_extension("GL_EXT_color_buffer_half_float")
    }

    /// 名前の妥当性は GL が確かめる（無い名前は GL のエラーになるだけ）。
    pub fn bind_framebuffer(&self, name: GLuint) {
        // SAFETY: 値を渡すだけ。
        unsafe { (self.BindFramebuffer)(GL_FRAMEBUFFER, name) };
    }

    pub fn finish(&self) {
        // SAFETY: 引数なし。
        unsafe { (self.Finish)() };
    }

    /// `frame_buffer` の `y` 行目から `rows` 行を RGBA8 で `into` の先頭へ読む。`into` が足りなければ読まない。
    pub fn read_rgba(&self, frame_buffer: GLuint, y: u32, width: u32, rows: u32, into: &mut [u8]) {
        if into.len() < width as usize * rows as usize * 4 {
            return;
        }
        self.bind_framebuffer(frame_buffer);
        // SAFETY: パディングなしで並べる（PACK_ALIGNMENT 1）ので、書くのは `into` の中の width×rows×4 バイト。
        unsafe {
            (self.PixelStorei)(GL_PACK_ALIGNMENT, 1);
            (self.ReadPixels)(
                0,
                y as GLint,
                width as GLsizei,
                rows as GLsizei,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                into.as_mut_ptr().cast(),
            );
        }
    }

    pub fn error(&self) -> Option<GLenum> {
        // SAFETY: 引数なし。
        let code = unsafe { (self.GetError)() };
        (code != GL_NO_ERROR).then_some(code)
    }

    pub fn drain(&self) {
        for _ in 0..16 {
            if self.error().is_none() {
                return;
            }
        }
    }

    /// オーバーレイ（[`super::Layer`]）が使うエントリーポイントを名前で解決する。[`USED`] の外の物も返る。解決できなければ NULL。
    pub fn proc_address(&self, egl: &super::egl::Egl, name: &std::ffi::CStr) -> *const c_void {
        let Ok(name) = name.to_str() else {
            return std::ptr::null();
        };
        // SAFETY: アドレスを解決するだけ。型に読み替えるのは受け取った側。
        unsafe { self.library.find::<*mut c_void>(name) }
            .or_else(|| egl.proc_address::<*mut c_void>(name))
            .map_or(std::ptr::null(), |pointer| pointer.cast_const())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ES2: &str = "glActiveTexture glAttachShader glBindAttribLocation glBindBuffer \
glBindFramebuffer glBindRenderbuffer glBindTexture glBlendColor glBlendEquation \
glBlendEquationSeparate glBlendFunc glBlendFuncSeparate glBufferData glBufferSubData \
glCheckFramebufferStatus glClear glClearColor glClearDepthf glClearStencil glColorMask \
glCompileShader glCompressedTexImage2D glCompressedTexSubImage2D glCopyTexImage2D \
glCopyTexSubImage2D glCreateProgram glCreateShader glCullFace glDeleteBuffers \
glDeleteFramebuffers glDeleteProgram glDeleteRenderbuffers glDeleteShader glDeleteTextures \
glDepthFunc glDepthMask glDepthRangef glDetachShader glDisable glDisableVertexAttribArray \
glDrawArrays glDrawElements glEnable glEnableVertexAttribArray glFinish glFlush \
glFramebufferRenderbuffer glFramebufferTexture2D glFrontFace glGenBuffers glGenerateMipmap \
glGenFramebuffers glGenRenderbuffers glGenTextures glGetActiveAttrib glGetActiveUniform \
glGetAttachedShaders glGetAttribLocation glGetBooleanv glGetBufferParameteriv glGetError \
glGetFloatv glGetFramebufferAttachmentParameteriv glGetIntegerv glGetProgramInfoLog \
glGetProgramiv glGetRenderbufferParameteriv glGetShaderInfoLog glGetShaderiv \
glGetShaderPrecisionFormat glGetShaderSource glGetString glGetTexParameterfv \
glGetTexParameteriv glGetUniformfv glGetUniformiv glGetUniformLocation glGetVertexAttribfv \
glGetVertexAttribiv glGetVertexAttribPointerv glHint glIsBuffer glIsEnabled glIsFramebuffer \
glIsProgram glIsRenderbuffer glIsShader glIsTexture glLineWidth glLinkProgram glPixelStorei \
glPolygonOffset glReadPixels glReleaseShaderCompiler glRenderbufferStorage glSampleCoverage \
glScissor glShaderBinary glShaderSource glStencilFunc glStencilFuncSeparate glStencilMask \
glStencilMaskSeparate glStencilOp glStencilOpSeparate glTexImage2D glTexParameterf \
glTexParameterfv glTexParameteri glTexParameteriv glTexSubImage2D glUniform1f glUniform1fv \
glUniform1i glUniform1iv glUniform2f glUniform2fv glUniform2i glUniform2iv glUniform3f \
glUniform3fv glUniform3i glUniform3iv glUniform4f glUniform4fv glUniform4i glUniform4iv \
glUniformMatrix2fv glUniformMatrix3fv glUniformMatrix4fv glUseProgram glValidateProgram \
glVertexAttrib1f glVertexAttrib1fv glVertexAttrib2f glVertexAttrib2fv glVertexAttrib3f \
glVertexAttrib3fv glVertexAttrib4f glVertexAttrib4fv glVertexAttribPointer glViewport";

    /// このパスが 2.0 に留まっている保証。コンテキストのバージョンでは保証できないので、呼べる関数を制限する。
    #[test]
    fn the_table_holds_nothing_but_es_two_commands() {
        let known: Vec<&str> = ES2.split_whitespace().collect();
        assert_eq!(known.len(), 142, "gl2.h の関数は 142 個");
        for name in USED {
            assert!(known.contains(&name), "{name} は ES 2.0 の命令ではない");
        }
    }

    /// [`Gl`] のフィールドと見比べられるよう名前順に。
    #[test]
    fn the_list_stays_in_an_order_that_can_be_read_against_the_table() {
        assert!(USED.is_sorted(), "名前順で並べる");
        assert_eq!(USED.len(), 52);
    }
}
