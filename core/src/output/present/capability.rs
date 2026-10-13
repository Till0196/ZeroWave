// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What this machine can actually present, measured rather than assumed.

use super::*;

/// この環境が実際に何を出せるか。GLES 2.0 ではフィルターのシェーダーがビルドできるかと texel を正しくサンプリングできるかも環境で違う。
#[derive(Debug, Clone)]
pub struct Capability {
    pub renderer: String,
    pub version: String,
    pub glsl: String,
    pub taps: Taps,
    /// fragment に highp があると報告したか（実際にサンプリングできたかではない）。
    pub declared_highp: bool,
    pub addresses: bool,
    /// フィルターごとの、1フレームあたりのミリ秒。ビルドできなかったフィルターは入らない。
    pub costs: Vec<(Deinterlace, f64)>,
    /// 逆テレシネの検出器が1フレームに要るミリ秒。CPU で輝度の四分の一を読む。
    pub telecine_ms: f64,
    pub refused: Vec<(Deinterlace, String)>,
    pub texture_units: i32,
    pub varyings: i32,
    pub biggest_texture: i32,
}

impl Capability {
    pub fn measure(format: Format) -> Result<Self, Error> {
        let mut present = Present::offscreen(format, Settings::of_format(format))?;
        Ok(Self::measure_with(&mut present, format))
    }

    pub fn measure_with(present: &mut Present, format: Format) -> Self {
        const ROUNDS: usize = 20;
        const WARMUP: usize = 4;
        let mut capability = Self {
            renderer: present.gl.string(gl::GL_RENDERER),
            version: present.gl.string(gl::GL_VERSION),
            glsl: present.gl.string(gl::GL_SHADING_LANGUAGE_VERSION),
            taps: present.taps,
            declared_highp: present.gl.fragment_highp(),
            addresses: false,
            costs: Vec::new(),
            telecine_ms: 0.0,
            refused: Vec::new(),
            texture_units: present.gl.integer(gl::GL_MAX_TEXTURE_IMAGE_UNITS),
            varyings: present.gl.integer(gl::GL_MAX_VARYING_VECTORS),
            biggest_texture: present.gl.integer(gl::GL_MAX_TEXTURE_SIZE),
        };
        capability.addresses = addresses(present, format);
        // サンプリングできなかったら、報告に関わらず fragment で座標を作る方式は外す。
        if !capability.addresses && capability.taps == Taps::Computed {
            present.taps = Taps::Varying;
            capability.taps = Taps::Varying;
            capability.addresses = addresses(present, format);
        }
        if !capability.addresses {
            return capability;
        }

        let frame = measured(format);
        capability.telecine_ms = telecine_cost(&frame);
        // 軽いものから測る。bob すら持ち時間に収まらない環境では yadif と bwdif は測らない。
        // panfrost は 500 ms を超えたジョブで GPU を立て直すので、測るだけで再生が止まる。
        for quality in [
            Deinterlace::Weave,
            Deinterlace::Bob,
            Deinterlace::Yadif,
            Deinterlace::Bwdif,
        ] {
            let bob_over = capability
                .costs
                .iter()
                .any(|(step, cost)| *step == Deinterlace::Bob && *cost > FIELD_BUDGET_MS);
            if bob_over && matches!(quality, Deinterlace::Yadif | Deinterlace::Bwdif) {
                capability.refused.push((
                    quality,
                    String::from("bob が持ち時間に収まらないので測らない"),
                ));
                continue;
            }
            if !capability.taps.carries(quality) {
                capability.refused.push((
                    quality,
                    String::from("このサンプリング方式では使えない（varying が足りない）"),
                ));
                continue;
            }
            let mut settings = present.settings();
            settings.deinterlace = quality;
            settings.telecine = false;
            // ビルドできるかは実際にビルドして確かめる（命令数の上限は公表されていない）。
            if let Err(why) = present.set_settings(settings) {
                capability.refused.push((quality, format!("{why}")));
                continue;
            }
            present.reset();
            for _ in 0..3 {
                if present.push(&Decoded::Planes(frame.clone())).is_err() {
                    break;
                }
            }
            if !present.ready() {
                continue;
            }
            let mut taken = Vec::new();
            let mut slow = 0;
            for round in 0..ROUNDS + WARMUP {
                let began = Instant::now();
                present.draw(None, false);
                present.gl.finish();
                let spent = began.elapsed().as_secs_f64() * 1e3;
                if round >= WARMUP {
                    taken.push(spent);
                }
                // 持ち時間の 4 倍を続けて超えるフィルターは選ばれないので、そこで切り上げる（測り切ると選局のたびに止まる）。
                // 1 回目だけで切ると、シェーダーを遅れてビルドするドライバーで軽いフィルターまで落としてしまう。
                slow = if spent > 4.0 * FIELD_BUDGET_MS {
                    slow + 1
                } else {
                    0
                };
                if slow == 2 {
                    taken.push(spent);
                    break;
                }
            }
            taken.sort_by(f64::total_cmp);
            capability.costs.push((quality, taken[taken.len() / 2]));
        }
        present.reset();
        capability
    }

    /// 予算に収まる一番良いフィルター。品質の高い順に見る（品質とコストは同じ順ではない）。
    pub fn affordable(&self) -> Deinterlace {
        if !self.addresses {
            // texel を正しくサンプリングできない環境では、どのフィルターも違う映像を出す。
            return Deinterlace::Weave;
        }
        let budget = FIELD_BUDGET_MS * AFFORDABLE;
        let cost_of = |quality: Deinterlace| {
            self.costs
                .iter()
                .find(|(step, _)| *step == quality)
                .map(|(_, cost)| *cost)
        };
        for quality in Deinterlace::LADDER {
            if cost_of(quality).is_some_and(|cost| cost <= budget) {
                return quality;
            }
        }
        // bob はフィールドごと、weave はフレームごとに 1 回出力する。bob が 1 フィールドの持ち時間に収まらない環境では
        // フレームが落ちるので、フレームの持ち時間に収まる weave にする（コーミングは出るがフレームは落ちない）。
        if cost_of(Deinterlace::Bob).is_some_and(|cost| cost > FIELD_BUDGET_MS)
            && cost_of(Deinterlace::Weave).is_some_and(|cost| cost <= 2.0 * FIELD_BUDGET_MS * 0.9)
        {
            return Deinterlace::Weave;
        }
        Deinterlace::Bob
    }

    /// 逆テレシネの検出器が間に合うか。上限はフレームの持ち時間（2 × 16.68 ms）の1割。
    /// 同じフレームで省けるのは検出器だけなので。
    pub fn can_afford_telecine(&self) -> bool {
        affordable_telecine(self.telecine_ms)
    }

    pub fn settings(&self, format: Format) -> Settings {
        let mut settings = Settings::of_format(format);
        if settings.deinterlace != Deinterlace::Weave {
            settings.deinterlace = self.affordable();
        }
        if settings.telecine && !self.can_afford_telecine() {
            settings.telecine = false;
        }
        settings
    }

    pub fn report(&self) -> String {
        let mut text = format!(
            "GL       {}\nGLSL     {}\nrenderer {}\n\
             テクスチャ単位 {}（保証 8）  varying {}（保証 8）  最大テクスチャ {}\n",
            self.version,
            self.glsl,
            self.renderer,
            self.texture_units,
            self.varyings,
            self.biggest_texture
        );
        text.push_str(&self.ladder_report());
        text
    }

    /// GPU の情報を除いたフィルターの部分だけ（[`probe`] が GPU の情報と上限を先に出す）。
    pub fn ladder_report(&self) -> String {
        let mut text = format!(
            "サンプリング方式 {:?}（fragment の highp: {}）\n",
            self.taps,
            if self.declared_highp {
                "対応"
            } else {
                "非対応"
            }
        );
        text.push_str(&format!(
            "texel を実寸でサンプリングできるか  {}\n",
            if self.addresses {
                "できた"
            } else {
                "**できない**"
            }
        ));
        text.push_str("フィルターのコストはフィルターと色変換のパスだけ（アップロードと画面への出力のコストは別）\n");
        let budget = FIELD_BUDGET_MS * AFFORDABLE;
        for (quality, cost) in &self.costs {
            text.push_str(&format!(
                "  {:6} {cost:7.2} ms  予算 {:.2} ms の {:.0}%{}\n",
                quality.name(),
                FIELD_BUDGET_MS,
                cost / FIELD_BUDGET_MS * 100.0,
                if *cost <= budget {
                    ""
                } else {
                    "  収まらない"
                },
            ));
        }
        for (quality, why) in &self.refused {
            text.push_str(&format!("  {:6} ビルドできない: {why}\n", quality.name()));
        }
        text.push_str(&format!(
            "逆テレシネの検出器 {:.2} ms/フレーム（持ち時間 {:.2} ms の {:.0}%）  {}\n",
            self.telecine_ms,
            2.0 * FIELD_BUDGET_MS,
            self.telecine_ms / (2.0 * FIELD_BUDGET_MS) * 100.0,
            if self.can_afford_telecine() {
                "有効"
            } else {
                "**無効**"
            }
        ));
        text.push_str(&format!("選ぶフィルター  {}\n", self.affordable().name()));
        text
    }
}

/// 逆テレシネの検出器が1フレームに要る時間（GPU は要らない）。2フレームを交互に入れるのは、
/// 同じものを続けると比較相手が同一になって速い分岐を通るから。
pub(super) fn telecine_cost(frame: &Frame) -> f64 {
    let mut other = frame.clone();
    for value in &mut other.planes[0].data {
        *value = value.wrapping_add(17);
    }
    let mut detector = telecine::Detector::new();
    detector.push(frame);
    let mut taken = Vec::new();
    for round in 0..8 {
        let picture = if round % 2 == 0 { &other } else { frame };
        let began = Instant::now();
        detector.push(picture);
        taken.push(began.elapsed().as_secs_f64() * 1e3);
    }
    taken.sort_by(f64::total_cmp);
    taken[taken.len() / 2]
}

/// 実寸でレンダリングして texel が全部合うかを見る。`GL_FRAGMENT_PRECISION_HIGH` の報告の代わり。
pub(super) fn addresses(present: &mut Present, format: Format) -> bool {
    let (width, height) = (format.width, format.height);
    if width == 0 || height == 0 {
        return false;
    }
    let want: Vec<u8> = (0..width * height)
        .map(|at| {
            let (x, y) = (at % width, at / width);
            ((x * 7 + y * 13) & 0xff) as u8
        })
        .collect();
    let gl = &present.gl;
    // SAFETY: コンテキストは有効にしてある。どれもこのコンテキストで作った名前を使う。
    let outcome = unsafe {
        let mut texture = 0;
        (gl.GenTextures)(1, &mut texture);
        (gl.BindTexture)(gl::GL_TEXTURE_2D, texture);
        (gl.PixelStorei)(gl::GL_UNPACK_ALIGNMENT, 1);
        (gl.TexImage2D)(
            gl::GL_TEXTURE_2D,
            0,
            gl::GL_LUMINANCE as gl::GLint,
            width as gl::GLsizei,
            height as gl::GLsizei,
            0,
            gl::GL_LUMINANCE,
            gl::GL_UNSIGNED_BYTE,
            want.as_ptr().cast(),
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
        let program = Program::build(
            gl,
            &glsl::vertex(present.taps),
            &glsl::addressing_probe(present.taps),
        );
        let outcome = match program {
            Err(_) => false,
            Ok(program) => {
                (gl.BindFramebuffer)(gl::GL_FRAMEBUFFER, present.frame_buffer);
                (gl.Viewport)(0, 0, width as gl::GLsizei, height as gl::GLsizei);
                (gl.UseProgram)(program.name);
                (gl.ActiveTexture)(gl::GL_TEXTURE0);
                (gl.BindTexture)(gl::GL_TEXTURE_2D, texture);
                if program.samplers[1] >= 0 {
                    (gl.Uniform1i)(program.samplers[1], 0);
                }
                if program.luma_size >= 0 {
                    (gl.Uniform2f)(program.luma_size, width as f32, height as f32);
                }
                if program.luma_step >= 0 {
                    (gl.Uniform2f)(program.luma_step, 0.0, 1.0 / height as f32);
                }
                if program.chroma_step >= 0 {
                    (gl.Uniform2f)(program.chroma_step, 0.0, 1.0 / height as f32);
                }
                draw_triangle(gl, present.corners, 3);
                let mut got = vec![0u8; (width * height * 4) as usize];
                (gl.PixelStorei)(gl::GL_PACK_ALIGNMENT, 1);
                (gl.ReadPixels)(
                    0,
                    0,
                    width as gl::GLsizei,
                    height as gl::GLsizei,
                    gl::GL_RGBA,
                    gl::GL_UNSIGNED_BYTE,
                    got.as_mut_ptr().cast(),
                );
                (gl.DeleteProgram)(program.name);
                exact(&got, &want, width, height)
            }
        };
        (gl.DeleteTextures)(1, &texture);
        outcome
    };
    present.gl.drain();
    outcome
}

pub(super) fn exact(got: &[u8], want: &[u8], width: u32, height: u32) -> bool {
    let at = |x: u32, y: u32| want[(y * width + x) as usize];
    for y in 0..height {
        for x in 0..width {
            let out = &got[((y * width + x) * 4) as usize..];
            let up = y.saturating_sub(1);
            let down = (y + 1).min(height - 1);
            if out[0] != at(x, y) || out[1] != at(x, up) || out[2] != at(x, down) {
                return false;
            }
        }
    }
    true
}

/// 測るのに使うフレーム。平坦なフレームではエッジ方向の探索がどの方向も同じ結果になって軽く出るので、斜めの縞を
/// 細かい縦縞の上に置く。
pub(super) fn measured(format: Format) -> Frame {
    use mediadecoder::video::Plane;
    let plane = |width: usize, height: usize| {
        let mut data = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let band = ((x + y) / 12) % 2;
                data[y * width + x] = (40 + band * 120 + (x % 5) * 6 + (y % 3) * 9).min(235) as u8;
            }
        }
        Plane {
            data,
            stride: width,
            width,
            height,
        }
    };
    let (width, height) = (format.width as usize, format.height as usize);
    Frame {
        tag: 0,
        format,
        planes: [
            plane(width, height),
            plane(width.div_ceil(2), height.div_ceil(2)),
            plane(width.div_ceil(2), height.div_ceil(2)),
        ],
        structure: mediadecoder::es::Structure::Frame,
        top_field_first: true,
        repeat_first_field: false,
        progressive: false,
    }
}

/// メモリの予算で決めるスロットの数。このパスを使うのはメモリの少ない環境なので wgpu 側より控えめ。
pub(super) fn slots(format: Format) -> usize {
    const BUDGET: u64 = 24 << 20;
    let bytes = u64::from(format.width)
        * u64::from(format.height)
        * if format.depth > 8 { 2 } else { 1 }
        * 3
        / 2;
    // スロットはデコーダーの DMA-BUF をそのまま持つことがあるので、デコーダーのバッファの見積もり
    // （[`crate::output::video::HELD_BY_FILTER`]）より多くは持たない。
    (BUDGET / bytes.max(1)).clamp(6, crate::output::video::HELD_BY_FILTER as u64) as usize
}
