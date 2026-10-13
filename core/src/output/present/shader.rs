// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GL helpers: buffers, targets, compiling and linking.

use super::*;

pub(super) fn uniform(gl: &Gl, program: gl::GLuint, name: &str) -> gl::GLint {
    let Ok(text) = std::ffi::CString::new(name) else {
        return -1;
    };
    // SAFETY: リンク済みの一式と、終端された名前。
    unsafe { (gl.GetUniformLocation)(program, text.as_ptr()) }
}

/// # Safety
/// コンテキストが有効で、`buffer` がこのコンテキストの頂点バッファであること。
pub(super) unsafe fn draw_triangle(gl: &Gl, buffer: gl::GLuint, points: gl::GLsizei) {
    // SAFETY: 呼び出し側の保証のとおり。属性0は `corner` に結んである。
    unsafe {
        (gl.BindBuffer)(gl::GL_ARRAY_BUFFER, buffer);
        (gl.EnableVertexAttribArray)(0);
        (gl.VertexAttribPointer)(0, 2, gl::GL_FLOAT, 0, 0, std::ptr::null());
        (gl.DrawArrays)(gl::GL_TRIANGLES, 0, points);
    }
}

/// # Safety
/// コンテキストを有効にしてあること。
pub(super) unsafe fn buffer(gl: &Gl, points: &[f32]) -> gl::GLuint {
    let mut name = 0;
    // SAFETY: 名前は1つ受け取り、渡す長さは `points` のバイト数。
    unsafe {
        (gl.GenBuffers)(1, &mut name);
        (gl.BindBuffer)(gl::GL_ARRAY_BUFFER, name);
        (gl.BufferData)(
            gl::GL_ARRAY_BUFFER,
            std::mem::size_of_val(points) as gl::GLsizeiptr,
            points.as_ptr().cast(),
            gl::GL_STATIC_DRAW,
        );
    }
    name
}

/// フィルター後のフレームが入るテクスチャと、そこへレンダリングするためのフレームバッファ。
/// # Safety
/// コンテキストを有効にしてあること。
pub(super) unsafe fn target_of(
    gl: &Gl,
    size: (u32, u32),
) -> Result<(gl::GLuint, gl::GLuint), Error> {
    // SAFETY: 呼び出し側の保証のとおり。
    unsafe { target_as(gl, size, false) }
}

/// `half` なら半精度の浮動小数（`OES_texture_half_float`）。HDR の画面の符号値を 8 ビットに丸めると縞が出るため。
/// 使えるかは [`Gl::half_float_target`] で確かめてから。
/// # Safety
/// コンテキストを有効にしてあること。
pub(super) unsafe fn target_as(
    gl: &Gl,
    (width, height): (u32, u32),
    half: bool,
) -> Result<(gl::GLuint, gl::GLuint), Error> {
    let kind = if half {
        gl::GL_HALF_FLOAT_OES
    } else {
        gl::GL_UNSIGNED_BYTE
    };
    let mut picture = 0;
    let mut frame_buffer = 0;
    // SAFETY: 名前は1つずつ受け取り、`RGBA` と `UNSIGNED_BYTE` は ES 2.0 の中核。
    let status = unsafe {
        (gl.GenTextures)(1, &mut picture);
        (gl.BindTexture)(gl::GL_TEXTURE_2D, picture);
        (gl.TexImage2D)(
            gl::GL_TEXTURE_2D,
            0,
            gl::GL_RGBA as gl::GLint,
            width as gl::GLsizei,
            height as gl::GLsizei,
            0,
            gl::GL_RGBA,
            kind,
            std::ptr::null(),
        );
        (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MIN_FILTER, gl::GL_NEAREST);
        (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MAG_FILTER, gl::GL_NEAREST);
        (gl.TexParameteri)(
            gl::GL_TEXTURE_2D,
            gl::GL_TEXTURE_WRAP_S,
            gl::GL_CLAMP_TO_EDGE,
        );
        (gl.TexParameteri)(
            gl::GL_TEXTURE_2D,
            gl::GL_TEXTURE_WRAP_T,
            gl::GL_CLAMP_TO_EDGE,
        );
        (gl.GenFramebuffers)(1, &mut frame_buffer);
        (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, frame_buffer);
        (gl.FramebufferTexture2D)(
            gl::GL_FRAMEBUFFER,
            gl::GL_COLOR_ATTACHMENT0,
            gl::GL_TEXTURE_2D,
            picture,
            0,
        );
        // デプステストもステンシルテストも要らないので無効にする。
        (gl.Disable)(gl::GL_DEPTH_TEST);
        (gl.Disable)(gl::GL_STENCIL_TEST);
        (gl.Disable)(gl::GL_CULL_FACE);
        (gl.Disable)(gl::GL_BLEND);
        (gl.Disable)(gl::GL_SCISSOR_TEST);
        (gl.Disable)(gl::GL_DITHER);
        (gl.CheckFramebufferStatus)(gl::GL_FRAMEBUFFER)
    };
    if status != gl::GL_FRAMEBUFFER_COMPLETE {
        // SAFETY: どちらもすぐ上で作った名前で、ほかに誰も持っていない。
        unsafe {
            (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, 0);
            (gl.DeleteFramebuffers)(1, &frame_buffer);
            (gl.DeleteTextures)(1, &picture);
        }
        return Err(Error::NoDevice(format!(
            "{}4成分のフレームバッファを作れません {status:#x}",
            if half { "半精度" } else { "8ビット" }
        )));
    }
    Ok((picture, frame_buffer))
}

/// # Safety
/// コンテキストを有効にしてあること。
pub(super) unsafe fn link(gl: &Gl, vertex: &str, fragment: &str) -> Result<gl::GLuint, Error> {
    // SAFETY: 呼び出し側がコンテキストを保証している。消すのはここでビルドしたシェーダーだけ。
    unsafe {
        let vertex = compile(gl, gl::GL_VERTEX_SHADER, vertex)?;
        let fragment = match compile(gl, gl::GL_FRAGMENT_SHADER, fragment) {
            Ok(fragment) => fragment,
            Err(error) => {
                (gl.DeleteShader)(vertex);
                return Err(error);
            }
        };
        let program = (gl.CreateProgram)();
        (gl.AttachShader)(program, vertex);
        (gl.AttachShader)(program, fragment);
        let corner = std::ffi::CString::new("corner").expect("ascii");
        (gl.BindAttribLocation)(program, 0, corner.as_ptr());
        (gl.LinkProgram)(program);
        (gl.DeleteShader)(vertex);
        (gl.DeleteShader)(fragment);
        let mut ok = 0;
        (gl.GetProgramiv)(program, gl::GL_LINK_STATUS, &mut ok);
        if ok == gl::GL_FALSE {
            let why = info_log(program, gl.GetProgramInfoLog);
            (gl.DeleteProgram)(program);
            return Err(Error::NoDevice(format!("リンクできません: {why}")));
        }
        Ok(program)
    }
}

/// # Safety
/// コンテキストを有効にしてあること。
pub(super) unsafe fn compile(gl: &Gl, kind: gl::GLenum, source: &str) -> Result<gl::GLuint, Error> {
    let text = std::ffi::CString::new(source)
        .map_err(|_| Error::NoDevice(String::from("シェーダーに NUL")))?;
    // SAFETY: 呼び出し側がコンテキストを保証し、`text` は終端されている。
    unsafe {
        let shader = (gl.CreateShader)(kind);
        let pointer = text.as_ptr();
        (gl.ShaderSource)(shader, 1, &pointer, std::ptr::null());
        (gl.CompileShader)(shader);
        let mut ok = 0;
        (gl.GetShaderiv)(shader, gl::GL_COMPILE_STATUS, &mut ok);
        if ok == gl::GL_FALSE {
            let why = info_log(shader, gl.GetShaderInfoLog);
            (gl.DeleteShader)(shader);
            return Err(Error::NoDevice(why));
        }
        Ok(shader)
    }
}

/// プログラムかシェーダーのログを読む。`read` は `glGetProgramInfoLog` か `glGetShaderInfoLog`。
fn info_log(
    name: gl::GLuint,
    read: unsafe extern "C" fn(gl::GLuint, gl::GLsizei, *mut gl::GLsizei, *mut std::ffi::c_char),
) -> String {
    let mut buffer = vec![0i8; 4096];
    let mut wrote = 0;
    // SAFETY: `buffer` は 4096 バイトあり、GL はそれ以上書かない。
    unsafe {
        read(
            name,
            buffer.len() as gl::GLsizei,
            &mut wrote,
            buffer.as_mut_ptr().cast(),
        );
    }
    text_of(&buffer, wrote)
}

pub(super) fn text_of(buffer: &[i8], wrote: gl::GLsizei) -> String {
    let taken = (wrote.max(0) as usize).min(buffer.len());
    let bytes: Vec<u8> = buffer[..taken].iter().map(|value| *value as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}
