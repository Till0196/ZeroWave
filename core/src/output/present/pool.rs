// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フレームを置くテクスチャと、そこへのアップロード。ES 2.0 の保証するテクスチャ単位は 8 なので、色差は
//! CPU で 1 つのテクスチャにインターリーブする。`UNPACK_ROW_LENGTH` が無いので `stride` が幅と違うプレーンは行を再パックする。

use mediadecoder::es::Format;
use mediadecoder::video::{Frame, Plane};

use super::gl::{self, Gl};
use crate::output::video::Error;

#[derive(Debug)]
struct Slot {
    luma: gl::GLuint,
    chroma: gl::GLuint,
    /// 書いていないプレーンをバインドしてレンダリングすると、何が出るか決まっていない。
    written: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Cost {
    pub pack: std::time::Duration,
    pub upload: std::time::Duration,
    /// 非同期にキューへ入れる実装では転送の待ちがここに現れる。
    pub check: std::time::Duration,
    /// Amlogic・MediaTek は `stride` が幅と同じ。
    pub luma_passed_through: bool,
}

#[derive(Debug)]
pub struct Pool {
    slots: Vec<Slot>,
    sizes: [(u32, u32); 2],
    bytes: u32,
    scratch: Vec<u8>,
    /// 輝度と別に持つのは、輝度が素通しのときにテクスチャを取り合わないため。
    woven: Vec<u8>,
    check_errors: bool,
    renderable: bool,
}

impl Pool {
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn new(gl: &Gl, format: Format, slots: usize) -> Result<Self, Error> {
        // SAFETY: 呼び出し側の約束のとおり。
        unsafe { Self::make(gl, format, slots, false) }
    }

    /// 輝度は `(y,y,y,y)`、色差は `(cb,cb,cb,cr)` で書く。
    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn renderable(gl: &Gl, format: Format, slots: usize) -> Result<Self, Error> {
        if format.depth > 8 {
            return Err(Error::Unsupported("10 ビットの DMA-BUF はインポートしない"));
        }
        // SAFETY: 呼び出し側の約束のとおり。
        unsafe { Self::make(gl, format, slots, true) }
    }

    /// GL のコンテキストがこのスレッドで有効であること。
    unsafe fn make(gl: &Gl, format: Format, slots: usize, renderable: bool) -> Result<Self, Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("放送で使うのは 4:2:0 だけ"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Unsupported("大きさの無いフレーム"));
        }
        let bytes = if format.depth > 8 { 2 } else { 1 };
        let sizes = [
            (format.width, format.height),
            (format.width.div_ceil(2), format.height.div_ceil(2)),
        ];
        let (luma_format, chroma_format) = if renderable {
            (gl::GL_RGBA, gl::GL_RGBA)
        } else if bytes == 2 {
            (gl::GL_LUMINANCE_ALPHA, gl::GL_RGBA)
        } else {
            (gl::GL_LUMINANCE, gl::GL_LUMINANCE_ALPHA)
        };
        let mut made = Vec::with_capacity(slots);
        for _ in 0..slots {
            // SAFETY: 呼び出し側がコンテキストを約束している。
            let (luma, chroma) = unsafe {
                (
                    new_texture(gl, luma_format, sizes[0]),
                    new_texture(gl, chroma_format, sizes[1]),
                )
            };
            made.push(Slot {
                luma,
                chroma,
                written: false,
            });
        }
        if let Some(code) = gl.error() {
            return Err(Error::NoDevice(format!("プレーンを作れません {code:#x}")));
        }
        Ok(Self {
            slots: made,
            sizes,
            bytes,
            scratch: Vec::new(),
            woven: Vec::new(),
            check_errors: true,
            renderable,
        })
    }

    pub fn is_renderable(&self) -> bool {
        self.renderable
    }

    pub fn targets(&self, slot: usize) -> Option<(gl::GLuint, gl::GLuint)> {
        let slot = self.slots.get(slot).filter(|_| self.renderable)?;
        Some((slot.luma, slot.chroma))
    }

    pub fn mark_written(&mut self, slot: usize) {
        if let Some(slot) = self.slots.get_mut(slot) {
            slot.written = true;
        }
    }

    pub fn sizes(&self) -> [(u32, u32); 2] {
        self.sizes
    }

    /// 予定が空きを探す範囲と一致していなければならない。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn textures(&self, slot: usize) -> Option<(gl::GLuint, gl::GLuint)> {
        let slot = self.slots.get(slot)?;
        slot.written.then_some((slot.luma, slot.chroma))
    }

    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn write(&mut self, gl: &Gl, slot: usize, frame: &Frame) -> Result<Cost, Error> {
        if self.renderable {
            return Err(Error::Unsupported(
                "レンダリングでコピーする先のテクスチャにはアップロードしない",
            ));
        }
        if slot >= self.slots.len() {
            return Err(Error::Unsupported("無いスロット"));
        }
        // SAFETY: 呼び出し側がコンテキストを約束している。
        unsafe {
            (gl.PixelStorei)(gl::GL_UNPACK_ALIGNMENT, 1);
        }
        let (luma_texture, chroma_texture) = (self.slots[slot].luma, self.slots[slot].chroma);
        let mut cost = Cost::default();

        let (width, height) = self.sizes[0];
        let began = std::time::Instant::now();
        let row = (width * self.bytes) as usize;
        let want = row * height as usize;
        let straight = frame.planes[0].stride == row
            && frame.planes[0].height >= height as usize
            && frame.planes[0].data.len() >= want;
        cost.luma_passed_through = straight;
        let luma: &[u8] = if straight {
            &frame.planes[0].data[..want]
        } else {
            repack(&mut self.scratch, &frame.planes[0], row, height as usize);
            &self.scratch
        };
        cost.pack += began.elapsed();
        let began = std::time::Instant::now();
        // SAFETY: `luma` はちょうど `width * height * bytes` の長さがある。
        unsafe {
            upload(gl, luma_texture, width, height, luma);
        }
        cost.upload += began.elapsed();

        let (width, height) = self.sizes[1];
        let began = std::time::Instant::now();
        weave(
            &mut self.woven,
            &frame.planes[1],
            &frame.planes[2],
            width as usize,
            height as usize,
            self.bytes as usize,
        );
        cost.pack += began.elapsed();
        let began = std::time::Instant::now();
        // SAFETY: `woven` はちょうど `width * height * bytes * 2` の長さ。
        unsafe {
            upload(gl, chroma_texture, width, height, &self.woven);
        }
        cost.upload += began.elapsed();

        self.slots[slot].written = true;

        // 呼ぶこと自体のコストが高い環境があるので、呼び出し側が決める。
        if !self.check_errors {
            return Ok(cost);
        }
        let began = std::time::Instant::now();
        let told = gl.error();
        cost.check = began.elapsed();
        match told {
            Some(code) => Err(Error::NoDevice(format!("プレーンを書けません {code:#x}"))),
            None => Ok(cost),
        }
    }

    /// 切ってよいのは、切っても結果が変わらないと分かっているときだけ。
    pub fn set_error_checks(&mut self, on: bool) {
        self.check_errors = on;
    }

    /// # Safety
    /// GL のコンテキストがこのスレッドで有効になっていること。
    pub unsafe fn shut(&mut self, gl: &Gl) {
        let names: Vec<gl::GLuint> = self
            .slots
            .iter()
            .flat_map(|slot| [slot.luma, slot.chroma])
            .collect();
        if !names.is_empty() {
            // SAFETY: どれもこのコンテキストで作った名前で、消すのは一度きり。
            unsafe {
                (gl.DeleteTextures)(names.len() as gl::GLsizei, names.as_ptr());
            }
        }
        self.slots.clear();
    }
}

/// `stride` は 1 行が占める長さ。足りない行は最後の行で埋める。
fn repack(into: &mut Vec<u8>, plane: &Plane, row: usize, height: usize) {
    into.clear();
    into.resize(row * height, 0);
    for y in 0..height {
        let source = plane.row(y.min(plane.height.saturating_sub(1)));
        let take = row.min(source.len());
        let at = y * row;
        into[at..at + take].copy_from_slice(&source[..take]);
    }
}

/// 8 ビットは Cb, Cr、深いと Cb 下位・上位, Cr 下位・上位。
fn weave(into: &mut Vec<u8>, blue: &Plane, red: &Plane, width: usize, height: usize, bytes: usize) {
    let pair = bytes * 2;
    into.clear();
    into.resize(width * height * pair, 0);
    for y in 0..height {
        let blue_row = blue.row(y.min(blue.height.saturating_sub(1)));
        let red_row = red.row(y.min(red.height.saturating_sub(1)));
        let out = &mut into[y * width * pair..(y + 1) * width * pair];
        if bytes == 1 {
            let take = width.min(blue_row.len()).min(red_row.len());
            for ((out, blue), red) in out
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(&blue_row[..take])
                .zip(&red_row[..take])
            {
                out[0] = *blue;
                out[1] = *red;
            }
            continue;
        }
        for x in 0..width {
            let from = x * bytes;
            let to = x * pair;
            for byte in 0..bytes {
                out[to + byte] = blue_row.get(from + byte).copied().unwrap_or(0);
                out[to + bytes + byte] = red_row.get(from + byte).copied().unwrap_or(0);
            }
        }
    }
}

/// `GL_NEAREST` と `GL_CLAMP_TO_EDGE` なら 2 の冪でなくてよい（ES 2.0 §3.8.2）。
/// # Safety
/// GL のコンテキストがこのスレッドで有効になっていること。
unsafe fn new_texture(gl: &Gl, kind: gl::GLenum, (width, height): (u32, u32)) -> gl::GLuint {
    let mut name = 0;
    // SAFETY: 呼び出し側がコンテキストを約束している。名前は1つ受け取る。
    unsafe {
        (gl.GenTextures)(1, &mut name);
        (gl.BindTexture)(gl::GL_TEXTURE_2D, name);
        (gl.TexImage2D)(
            gl::GL_TEXTURE_2D,
            0,
            kind as gl::GLint,
            width as gl::GLsizei,
            height as gl::GLsizei,
            0,
            kind,
            gl::GL_UNSIGNED_BYTE,
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
    }
    name
}

/// # Safety
/// `data` はちょうどテクスチャ1つ分の長さで、コンテキストは有効で、`GL_UNPACK_ALIGNMENT` は1であること。
unsafe fn upload(gl: &Gl, name: gl::GLuint, width: u32, height: u32, data: &[u8]) {
    // `glTexImage2D` を毎回呼ぶと確保をやり直すので、中身だけ差し替える。
    // SAFETY: 呼び出し側の約束のとおり。
    unsafe {
        (gl.BindTexture)(gl::GL_TEXTURE_2D, name);
        (gl.TexSubImage2D)(
            gl::GL_TEXTURE_2D,
            0,
            0,
            0,
            width as gl::GLsizei,
            height as gl::GLsizei,
            format_of(data.len(), width, height),
            gl::GL_UNSIGNED_BYTE,
            data.as_ptr().cast(),
        );
    }
}

/// 確保したときと同じ並びでなければならない。
fn format_of(len: usize, width: u32, height: u32) -> gl::GLenum {
    let texels = (width as usize * height as usize).max(1);
    match len / texels {
        1 => gl::GL_LUMINANCE,
        2 => gl::GL_LUMINANCE_ALPHA,
        _ => gl::GL_RGBA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediadecoder::es::Structure;

    fn frame(depth: u8) -> Frame {
        let bytes = if depth > 8 { 2 } else { 1 };
        let plane = |width: usize, height: usize, stride: usize, seed: u8| Plane {
            data: (0..stride * height)
                .map(|at| (at as u8).wrapping_add(seed))
                .collect(),
            stride,
            width,
            height,
        };
        Frame {
            tag: 0,
            format: Format {
                width: 8,
                height: 4,
                depth,
                ..Default::default()
            },
            planes: [
                plane(8, 4, 8 * bytes, 0),
                plane(4, 2, 4 * bytes, 100),
                plane(4, 2, 4 * bytes, 200),
            ],
            structure: Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
        }
    }

    #[test]
    fn the_two_chroma_planes_are_woven_blue_first() {
        let frame = frame(8);
        let mut woven = Vec::new();
        weave(&mut woven, &frame.planes[1], &frame.planes[2], 4, 2, 1);
        assert_eq!(woven.len(), 4 * 2 * 2);
        for y in 0..2usize {
            for x in 0..4usize {
                let at = (y * 4 + x) * 2;
                assert_eq!(woven[at], frame.planes[1].row(y)[x], "Cb {x},{y}");
                assert_eq!(woven[at + 1], frame.planes[2].row(y)[x], "Cr {x},{y}");
            }
        }
    }

    /// `mediadecoder` がプレーンに置く順そのもの。
    #[test]
    fn a_wider_sample_takes_two_bytes_low_first() {
        let frame = frame(10);
        let mut woven = Vec::new();
        weave(&mut woven, &frame.planes[1], &frame.planes[2], 4, 2, 2);
        assert_eq!(woven.len(), 4 * 2 * 4);
        let blue = frame.planes[1].row(0);
        let red = frame.planes[2].row(0);
        assert_eq!(&woven[0..2], &blue[0..2], "Cb の2バイト");
        assert_eq!(&woven[2..4], &red[0..2], "Cr の2バイト");
    }

    #[test]
    fn the_padding_a_decoder_leaves_never_reaches_the_texture() {
        let mut plane = Plane {
            data: vec![0u8; 12 * 4],
            stride: 12,
            width: 8,
            height: 4,
        };
        for y in 0..4 {
            for x in 0..12 {
                plane.data[y * 12 + x] = if x < 8 { (y * 8 + x) as u8 } else { 0xff };
            }
        }
        let mut packed = Vec::new();
        repack(&mut packed, &plane, 8, 4);
        assert_eq!(packed.len(), 32);
        assert!(!packed.contains(&0xff), "パディングがフレームに混ざった");
        for y in 0..4usize {
            for x in 0..8usize {
                assert_eq!(packed[y * 8 + x], (y * 8 + x) as u8);
            }
        }
    }

    #[test]
    fn weaving_a_row_at_a_time_beats_checking_every_byte() {
        let (width, height) = (720usize, 540usize);
        let plane = |seed: u8| Plane {
            data: (0..width * height)
                .map(|at| (at as u8).wrapping_add(seed))
                .collect(),
            stride: width,
            width,
            height,
        };
        let (blue, red) = (plane(0), plane(97));

        let naive = |into: &mut Vec<u8>| {
            into.clear();
            into.resize(width * height * 2, 0);
            for y in 0..height {
                let blue_row = blue.row(y);
                let red_row = red.row(y);
                let at = y * width * 2;
                for x in 0..width {
                    into[at + x * 2] = blue_row.get(x).copied().unwrap_or(0);
                    into[at + x * 2 + 1] = red_row.get(x).copied().unwrap_or(0);
                }
            }
        };

        type Way<'a> = Box<dyn FnMut(&mut Vec<u8>) + 'a>;

        let time = |mut run: Way<'_>| {
            let mut into = Vec::new();
            run(&mut into);
            let mut taken = Vec::new();
            for _ in 0..9 {
                let began = std::time::Instant::now();
                run(&mut into);
                taken.push(began.elapsed().as_secs_f64() * 1e3);
            }
            taken.sort_by(f64::total_cmp);
            (taken[taken.len() / 2], into)
        };

        let (slow, from_naive) = time(Box::new(naive));
        let (fast, from_weave) = time(Box::new(|into: &mut Vec<u8>| {
            weave(into, &blue, &red, width, height, 1)
        }));
        tracing::info!(
            "色差をインターリーブする 720x540  1サンプルずつ {slow:.3} ms  行ごと {fast:.3} ms"
        );
        assert_eq!(from_naive, from_weave, "速い実装が違うものを出している");
        assert!(fast <= slow * 2.0, "行ごとのほうが遅い: {fast} と {slow}");
    }

    /// 壊れた入力ではプレーンがテクスチャより短いことがある。
    #[test]
    fn a_plane_shorter_than_the_texture_repeats_its_last_row() {
        let plane = Plane {
            data: (0..8u8 * 2).collect(),
            stride: 8,
            width: 8,
            height: 2,
        };
        let mut packed = Vec::new();
        repack(&mut packed, &plane, 8, 4);
        assert_eq!(&packed[16..24], &packed[8..16], "3行目は2行目のコピー");
        assert_eq!(&packed[24..32], &packed[8..16], "4行目も");
    }
}
