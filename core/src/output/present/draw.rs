// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Drawing a picture onto the target, and letting go of what was made.

use super::*;

impl Present {
    /// `onto` があればそのフレームバッファへ。`consume` が偽なら予定を減らさない（測定で同じフレームを何度もレンダリングするため）。
    pub(super) fn draw(
        &mut self,
        onto: Option<(gl::GLuint, (u32, u32))>,
        consume: bool,
    ) -> Option<Shown> {
        let ready = self.schedule.front()?;
        let shown = self.draw_entry(ready, onto, false)?;
        if consume {
            self.schedule.pop_shown();
        }
        Some(shown)
    }

    /// 最後に出したフレームをもう一度レンダリングする（画面が変わった回、一時停止中など）。出したフレームが無ければ偽。
    pub(super) fn draw_again(&mut self, onto: (gl::GLuint, (u32, u32))) -> bool {
        let Some(ready) = self.schedule.shown() else {
            return false;
        };
        self.draw_entry(ready, Some(onto), true).is_some()
    }

    /// `ready` をレンダリングする。`again` なら再レンダリング：サブサーフェスは付け直さず、フィルターもやり直さない。
    fn draw_entry(
        &mut self,
        ready: crate::output::video::schedule::Ready,
        onto: Option<(gl::GLuint, (u32, u32))>,
        again: bool,
    ) -> Option<Shown> {
        // Wayland のサブサーフェスに持たせたフレームは、そちらへ付けて出す。EGL のサーフェスは黒く塗り、映像の所だけ
        // 透明に抜く（OSD はこのサーフェスにレンダリングする）。
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        let (rect, visible) = self.placement();
        #[cfg(all(target_os = "linux", feature = "wayland"))]
        // 画面（フレームバッファ 0）へ出すときだけ。キャプチャのレンダーターゲットには GL でレンダリングする。
        if let (Some((target @ 0, (view_width, view_height))), Some(surface)) =
            (onto, self.video_surface.as_mut())
            && surface.holds(ready.cur)
        {
            let width = ((rect[2] - rect[0]) * 0.5 * view_width as f32).round() as i32;
            let height = ((rect[3] - rect[1]) * 0.5 * view_height as f32).round() as i32;
            let left = ((rect[0] + 1.0) * 0.5 * view_width as f32).round() as i32;
            let top = ((1.0 - rect[3]) * 0.5 * view_height as f32).round() as i32;
            if !again
                && let Err(error) =
                    surface.show_slot(ready.cur, ready.output.field, (left, top, width, height))
            {
                tracing::warn!("サブサーフェスに表示できない: {error:?}");
            }
            let gl = &self.gl;
            // SAFETY: 既定のサーフェスにレンダリングするだけ。コンテキストは有効にしてある。
            unsafe {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, target);
                (gl.Viewport)(0, 0, view_width as gl::GLsizei, view_height as gl::GLsizei);
                (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
                (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
                // GL の原点は左下。抜くのは映像の所のうち見せてよい所だけ（比率ではみ出た所は黒で隠す）。
                let bottom = view_height as i32 - top - height;
                let x0 = left.max(visible.0);
                let y0 = bottom.max(visible.1);
                let x1 = (left + width).min(visible.0 + visible.2);
                let y1 = (bottom + height).min(visible.1 + visible.3);
                (gl.Enable)(gl::GL_SCISSOR_TEST);
                (gl.Scissor)(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0));
                (gl.ClearColor)(0.0, 0.0, 0.0, 0.0);
                (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
                (gl.Disable)(gl::GL_SCISSOR_TEST);
            }
            let shown = Shown {
                field: ready.output.field,
                at: ready.output.at,
                of: ready.output.of,
            };
            return Some(shown);
        }
        // コピーせずに持つ NV12 は、画面へ1パスで（サンプリングすると RGB が返る）。
        let held = self.externals.get(ready.cur).map_or(&[][..], Vec::as_slice);
        let external = match (held, ready.output.field) {
            ([one], _) => Some(one),
            ([_, bottom], Some(field::Parity::Bottom)) => Some(bottom),
            ([top, _], _) => Some(top),
            _ => None,
        };
        if let (Some((target, (view_width, view_height))), Some(external), Some(Some(program))) =
            (onto, external, self.external_program.as_ref())
        {
            let mut rect = self.letterbox();
            // フィールドだけの bob：上のフィールドは偶数行、下は奇数行なので、同じ所へ伸ばすと半行ずつ上下して震える。
            // 上は半行上へ、下は半行下へずらす（一行はクリップ座標でフレームの高さ÷行数）。
            if held.len() == 2 {
                let half_line = 0.5 * (rect[3] - rect[1]) / self.format.height.max(1) as f32;
                let shift = match ready.output.field {
                    Some(field::Parity::Bottom) => -half_line,
                    _ => half_line,
                };
                rect[1] += shift;
                rect[3] += shift;
            }
            // 一枚が一フィールドのピクチャーも同じ。高さはフィールドの行数なので、フレームの半行はその 4 分の 1。
            if let Some(parity) = self.lone.get(ready.cur).copied().flatten() {
                let half_line = 0.25 * (rect[3] - rect[1]) / self.format.height.max(1) as f32;
                let shift = match parity {
                    field::Parity::Bottom => -half_line,
                    field::Parity::Top => half_line,
                };
                rect[1] += shift;
                rect[3] += shift;
            }
            let gl = &self.gl;
            // SAFETY: どれもこのコンテキストで作った名前を使う。コンテキストは有効にしてある。
            unsafe {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, target);
                (gl.Viewport)(0, 0, view_width as gl::GLsizei, view_height as gl::GLsizei);
                (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
                (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
                self.clip_to_visible();
                (gl.UseProgram)(program.name);
                (gl.ActiveTexture)(gl::GL_TEXTURE0);
                (gl.BindTexture)(gl::GL_TEXTURE_EXTERNAL_OES, external.texture);
                if program.picture >= 0 {
                    (gl.Uniform1i)(program.picture, 0);
                }
                if program.rect >= 0 {
                    (gl.Uniform4f)(program.rect, rect[0], rect[1], rect[2], rect[3]);
                }
                let base = Conversion::new(
                    self.settings.matrix,
                    self.format.depth,
                    self.settings.limited,
                );
                let rows = base
                    .adjusted(self.adjust, self.format.depth)
                    .rgb_rows(&base);
                for (at, row) in program.rows.iter().zip(rows) {
                    if *at >= 0 {
                        (gl.Uniform4f)(*at, row[0], row[1], row[2], row[3]);
                    }
                }
                draw_triangle(gl, self.quad, 6);
                (gl.Disable)(gl::GL_SCISSOR_TEST);
            }
            let shown = Shown {
                field: ready.output.field,
                at: ready.output.at,
                of: ready.output.of,
            };
            return Some(shown);
        }
        let base = Conversion::new(
            self.settings.matrix,
            self.format.depth,
            self.settings.limited,
        );
        let conversion = base.adjusted(self.adjust, self.format.depth);
        let rows = filter::Rows::of(&ready.output);
        let params = Params {
            first_row: rows.first,
            mate_row: rows.mate,
            luma_offset: conversion.luma_offset,
            luma_gain: conversion.luma_gain,
            chroma_offset: conversion.chroma_offset,
            chroma_gain: conversion.chroma_gain,
            kr: conversion.kr,
            kb: conversion.kb,
            luma_lift: conversion.luma_lift,
            chroma_cos: conversion.chroma_cos,
            chroma_sin: conversion.chroma_sin,
        };
        let slots = [ready.before, ready.cur, ready.after];
        let mut bound = [0u32; 6];
        for (which, slot) in slots.iter().enumerate() {
            // 捨てられたフレームに予定が立つと、スロットにサーフェスが無い。
            let (luma, chroma) = self.pool.textures(*slot)?;
            bound[which] = luma;
            bound[3 + which] = chroma;
        }
        // フィルターの要らないフレーム（weave で前のフレームから行を取らない物）は、中間のレンダーターゲットを省いて画面へ1パスで。
        if let (Some(target), Some(direct)) = (onto, self.direct_program.as_ref())
            && self.settings.deinterlace == Deinterlace::Weave
            && rows.mate == filter::NO_MATE
        {
            let rect = self.letterbox();
            let gl = &self.gl;
            let (target, (view_width, view_height)) = target;
            // SAFETY: どれもこのコンテキストで作った名前を使う。コンテキストは有効にしてある。
            unsafe {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, target);
                (gl.Viewport)(0, 0, view_width as gl::GLsizei, view_height as gl::GLsizei);
                (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
                (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
                self.clip_to_visible();
                (gl.UseProgram)(direct.name);
                // 拡大縮小はサンプリング時のバイリニアで。フィルターのパスは最近傍を前提にするので戻す。
                for (unit, (name, sampler)) in
                    [(bound[1], 1), (bound[4], 4)].into_iter().enumerate()
                {
                    (gl.ActiveTexture)(gl::GL_TEXTURE0 + unit as gl::GLenum);
                    (gl.BindTexture)(gl::GL_TEXTURE_2D, name);
                    (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MIN_FILTER, gl::GL_LINEAR);
                    (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MAG_FILTER, gl::GL_LINEAR);
                    let at = direct.samplers[sampler];
                    if at >= 0 {
                        (gl.Uniform1i)(at, unit as gl::GLint);
                    }
                }
                for (at, value) in direct.params.iter().zip(params.values().iter()) {
                    if *at >= 0 {
                        (gl.Uniform1f)(*at, *value);
                    }
                }
                if self.direct_rect >= 0 {
                    (gl.Uniform4f)(self.direct_rect, rect[0], rect[1], rect[2], rect[3]);
                }
                draw_triangle(gl, self.quad, 6);
                (gl.Disable)(gl::GL_SCISSOR_TEST);
                for name in [bound[1], bound[4]] {
                    (gl.BindTexture)(gl::GL_TEXTURE_2D, name);
                    (gl.TexParameteri)(
                        gl::GL_TEXTURE_2D,
                        gl::GL_TEXTURE_MIN_FILTER,
                        gl::GL_NEAREST,
                    );
                    (gl.TexParameteri)(
                        gl::GL_TEXTURE_2D,
                        gl::GL_TEXTURE_MAG_FILTER,
                        gl::GL_NEAREST,
                    );
                }
            }
            return Some(Shown {
                field: ready.output.field,
                at: ready.output.at,
                of: ready.output.of,
            });
        }
        let gl = &self.gl;
        let (width, height) = (self.format.width, self.format.height);
        // SAFETY: どれもこのコンテキストで作った名前を使う。コンテキストは有効にしてある。
        // 再レンダリングならフィルター済みの結果（`picture`）をコピーするだけ。前後のフレームのテクスチャはもう別のフレームかもしれない。
        if !again {
            unsafe {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, self.frame_buffer);
                (gl.Viewport)(0, 0, width as gl::GLsizei, height as gl::GLsizei);
                (gl.UseProgram)(self.filter_program.name);
                for (unit, name) in bound.iter().enumerate() {
                    (gl.ActiveTexture)(gl::GL_TEXTURE0 + unit as gl::GLenum);
                    (gl.BindTexture)(gl::GL_TEXTURE_2D, *name);
                    let at = self.filter_program.samplers[unit];
                    if at >= 0 {
                        (gl.Uniform1i)(at, unit as gl::GLint);
                    }
                }
                for (at, value) in self
                    .filter_program
                    .params
                    .iter()
                    .zip(params.values().iter())
                {
                    if *at >= 0 {
                        (gl.Uniform1f)(*at, *value);
                    }
                }
                let half = (
                    f32::from(u16::try_from(width.div_ceil(2)).unwrap_or(u16::MAX)),
                    f32::from(u16::try_from(height.div_ceil(2)).unwrap_or(u16::MAX)),
                );
                if self.filter_program.luma_size >= 0 {
                    (gl.Uniform2f)(self.filter_program.luma_size, width as f32, height as f32);
                }
                if self.filter_program.chroma_size >= 0 {
                    (gl.Uniform2f)(self.filter_program.chroma_size, half.0, half.1);
                }
                if self.filter_program.luma_step >= 0 {
                    (gl.Uniform2f)(self.filter_program.luma_step, 0.0, 1.0 / height as f32);
                }
                if self.filter_program.chroma_step >= 0 {
                    (gl.Uniform2f)(self.filter_program.chroma_step, 0.0, 1.0 / half.1);
                }
                draw_triangle(gl, self.corners, 3);
            }
        }
        // SAFETY: 同上。
        unsafe {
            if let Some((target, (view_width, view_height))) = onto {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, target);
                (gl.Viewport)(0, 0, view_width as gl::GLsizei, view_height as gl::GLsizei);
                // 前のフレームが残ると、縦横比が変わったときに尾を引く。
                (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
                (gl.Clear)(gl::GL_COLOR_BUFFER_BIT);
                self.clip_to_visible();
                (gl.UseProgram)(self.blit_program.name);
                (gl.ActiveTexture)(gl::GL_TEXTURE0);
                (gl.BindTexture)(gl::GL_TEXTURE_2D, self.picture);
                (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MIN_FILTER, gl::GL_LINEAR);
                (gl.TexParameteri)(gl::GL_TEXTURE_2D, gl::GL_TEXTURE_MAG_FILTER, gl::GL_LINEAR);
                if self.blit_picture >= 0 {
                    (gl.Uniform1i)(self.blit_picture, 0);
                }
                let rect = self.letterbox();
                if self.blit_rect >= 0 {
                    (gl.Uniform4f)(self.blit_rect, rect[0], rect[1], rect[2], rect[3]);
                }
                draw_triangle(gl, self.quad, 6);
                (gl.Disable)(gl::GL_SCISSOR_TEST);
            }
        }
        Some(Shown {
            field: ready.output.field,
            at: ready.output.at,
            of: ready.output.of,
        })
    }

    /// 映像を置く矩形（クリップ座標、左・下・右・上）。wgpu 側（`gpu::Video::letterbox`）と同じ決め方。
    /// 縦横比はシーケンスヘッダーから取る（1440x1080 は 16:9 で表示するものが多く、幅から出すと横に縮む）。
    pub(super) fn letterbox(&self) -> [f32; 4] {
        self.placement().0
    }

    /// 映像を見せてよい範囲（GL のピクセル、左・下・幅・高さ）。映像の領域か、アスペクト比で切っていればその枠。
    pub(super) fn visible(&self) -> (i32, i32, i32, i32) {
        self.placement().1
    }

    fn placement(&self) -> ([f32; 4], (i32, i32, i32, i32)) {
        let (view_width, view_height) = self.viewport;
        let (view_width, view_height) = (view_width.max(1) as f32, view_height.max(1) as f32);
        let (left, top, width, height) = match self.area {
            Some((left, top, width, height)) => (
                left as f32,
                top as f32,
                width.max(1) as f32,
                height.max(1) as f32,
            ),
            None => (0.0, 0.0, view_width, view_height),
        };
        let (mask, rect) = self.boxes((width, height));
        let to_surface = |[x0, y0, x1, y1]: [f32; 4]| {
            let x = |x: f32| (left + (x + 1.0) * 0.5 * width) / view_width * 2.0 - 1.0;
            let y = |y: f32| 1.0 - (top + (1.0 - y) * 0.5 * height) / view_height * 2.0;
            [x(x0), y(y0), x(x1), y(y1)]
        };
        let visible = to_surface(mask.unwrap_or([-1.0, -1.0, 1.0, 1.0]));
        let px = |clip: f32, size: f32| ((clip + 1.0) * 0.5 * size).round() as i32;
        let (x0, y0) = (px(visible[0], view_width), px(visible[1], view_height));
        let (x1, y1) = (px(visible[2], view_width), px(visible[3], view_height));
        (
            to_surface(rect),
            (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0)),
        )
    }

    /// 映像の領域（`size`、ピクセル）の中での（隠す枠、映像の矩形）。どちらもその領域のクリップ座標。
    fn boxes(&self, (width, height): (f32, f32)) -> (Option<[f32; 4]>, [f32; 4]) {
        let (num, den) = self.format.aspect;
        let source = if num == 0 || den == 0 {
            self.format.width as f32 / self.format.height.max(1) as f32
        } else {
            num as f32 / den as f32
        };
        let have = width / height.max(1.0);
        // データ放送の文書が映像を画面の一部へ縮めているか（画面いっぱいならアスペクト比は効かせる）。
        let shrinks = self.inset.is_some_and(|[left, top, wide, tall, _]| {
            left > 0.001 || top > 0.001 || wide < 0.999 || tall < 0.999
        });
        if !shrinks && !self.pan_scan.is_default() {
            let (mask, rect) = self.pan_scan.fit(source, have);
            return (self.pan_scan.mask.then_some(mask), rect);
        }
        // 文書があれば枠はそのページで、形は文書の指定に従う。
        let wanted = match self.inset {
            Some([_, _, _, _, aspect]) if aspect > 0.0 => aspect,
            _ => source,
        };
        // 枠が映像と同じ比率なら枠いっぱい（丸め誤差の1ピクセルで黒帯を作らない）。
        let (x, y) = if (have - wanted).abs() / wanted < 0.01 {
            (1.0, 1.0)
        } else if have > wanted {
            (wanted / have, 1.0)
        } else {
            (1.0, have / wanted)
        };
        let Some([left, top, wide, tall, _]) = self.inset else {
            return (None, [-x, -y, x, y]);
        };
        // 文書の座標はページの左上から。クリップ座標の y は上向き。
        let l = -x + 2.0 * x * left;
        let t = y - 2.0 * y * top;
        (None, [l, t - 2.0 * y * tall, l + 2.0 * x * wide, t])
    }

    /// 映像を見せてよい範囲の外をレンダリングしないようにする（アスペクト比で切った映像がはみ出さないように）。
    /// # Safety
    /// コンテキストを有効にしてあること。レンダリングしたら [`gl::GL_SCISSOR_TEST`] を切ること。
    unsafe fn clip_to_visible(&self) {
        let (x, y, width, height) = self.visible();
        let gl = &self.gl;
        unsafe {
            (gl.Enable)(gl::GL_SCISSOR_TEST);
            (gl.Scissor)(x, y, width, height);
        }
    }

    /// # Safety
    /// コンテキストを有効にしてあること。呼んだあと、持っていた名前は使えない。
    pub(super) unsafe fn release(&mut self) {
        let gl = &self.gl;
        // SAFETY: 呼び出し側がコンテキストを約束している。消すのはどれもこのコンテキストで作った名前。
        unsafe {
            (gl.DeleteProgram)(self.filter_program.name);
            (gl.DeleteProgram)(self.blit_program.name);
            if let Some(direct) = self.direct_program.take() {
                (gl.DeleteProgram)(direct.name);
            }
            if let Some(Some(external)) = self.external_program.take() {
                (gl.DeleteProgram)(external.name);
            }
            (gl.DeleteFramebuffers)(1, &self.frame_buffer);
            (gl.DeleteTextures)(1, &self.picture);
            (gl.DeleteBuffers)(1, &self.corners);
            (gl.DeleteBuffers)(1, &self.quad);
            self.pool.shut(gl);
            let held: Vec<_> = self.externals.iter_mut().flat_map(std::mem::take).collect();
            if let Some(import) = self.import.as_ref() {
                for external in held {
                    import.drop_external(gl, self.display, external);
                }
            }
            if let Some(import) = self.import.as_mut() {
                import.shut(gl);
            }
        }
        if let Some(Some(fences)) = self.fences.as_ref() {
            for (fence, _) in self.retiring.drain(..) {
                fences.forget(fence);
            }
        }
    }
}
