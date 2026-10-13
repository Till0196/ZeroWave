// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GLSL ES 1.00 を `glsl/*.glsl` の断片から組み立てる（埋め込む位置は `{name}`）。
//! [`crate::output::video::shader`] の WGSL と同じ理屈の二重実装で、一致は filter に対するゴールデンが担保する。
//! ESSL1 には `texelFetch`・ビット演算・整数テクスチャ・uniform ブロックが無い（§8.7、§5.9、§4.1）。
//! ES 2.0 で保証されるテクスチャユニットは 8 なので、プレーンは輝度と色差の2つ（ES 2.0.25 §6.2 テーブル 6.20）。

use crate::output::template::fill_planes;
use crate::output::video::colour::{self, Mapping, Space, Transfer};
use crate::output::video::{Deinterlace, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taps {
    Computed,
    Varying,
}

impl Taps {
    pub fn carries(self, quality: Deinterlace) -> bool {
        match self {
            Self::Computed => true,
            Self::Varying => matches!(quality, Deinterlace::Weave | Deinterlace::Bob),
        }
    }
}

/// ES 2.0 に uniform ブロックが無いので個別に渡す。中身と順は [`crate::output::video::shader::Params`] と同じ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub first_row: u32,
    pub mate_row: u32,
    pub luma_offset: f32,
    pub luma_gain: f32,
    pub chroma_offset: f32,
    pub chroma_gain: f32,
    pub kr: f32,
    pub kb: f32,
}

pub const UNIFORMS: [&str; 8] = [
    "first_row",
    "mate_row",
    "luma_offset",
    "luma_gain",
    "chroma_offset",
    "chroma_gain",
    "kr",
    "kb",
];

impl Params {
    pub fn values(&self) -> [f32; 8] {
        [
            self.first_row as f32,
            self.mate_row as f32,
            self.luma_offset,
            self.luma_gain,
            self.chroma_offset,
            self.chroma_gain,
            self.kr,
            self.kb,
        ]
    }
}

/// 時刻3つ × プレーン2つ。
pub const SAMPLERS: [&str; 6] = [
    "y_before_t",
    "y_cur_t",
    "y_after_t",
    "c_before_t",
    "c_cur_t",
    "c_after_t",
];

/// `highp` があれば使い、無ければ `mediump` に落ちる（GLSL ES 1.00 §4.5.4）。
const PRECISION: &str = include_str!("glsl/precision.glsl");

/// `highp` を持つ GPU で、持たない GPU（Mali-450）の丸めを見るための宣言。テストと probe だけ。
const MEDIUMP: &str = include_str!("glsl/mediump.glsl");

fn precision(mediump: bool) -> &'static str {
    if mediump { MEDIUMP } else { PRECISION }
}

const CORNERS: &str = include_str!("glsl/corners.vert.glsl");

/// 頂点側・fragment 側・probe が同じ6本を見るので一箇所に置く。
const VARYINGS: &str = include_str!("glsl/varyings.glsl");

/// 一つのプレーンの本文。[`crate::output::video::shader`] の同じ名前の関数と同じ式・同じ順。
fn body(quality: Deinterlace, plane: &str) -> String {
    let text = match quality {
        // 逆テレシネで復元したフレームは片方のパリティの行を前のフレームから取る（ほかは `mate_row` が2で当たらない）。
        Deinterlace::Weave => include_str!("glsl/body_weave.glsl"),
        Deinterlace::Bob => include_str!("glsl/body_bob.glsl"),
        Deinterlace::Yadif => include_str!("glsl/body_linear.glsl"),
        Deinterlace::Bwdif => include_str!("glsl/body_cubic.glsl"),
    };
    fill_planes(text, plane)
}

/// 動きの量と前後のフィールドの平均（[`crate::output::video::filter`] の `motion` と同じ）。
fn motion(plane: &str) -> String {
    fill_planes(include_str!("glsl/motion.glsl"), plane)
}

/// 辺の向き（[`crate::output::video::filter::edge`] と同じ）。
fn edge(plane: &str) -> String {
    let before = format!("{plane}_before");
    let cur = format!("{plane}_cur");
    let after = format!("{plane}_after");
    crate::output::template::fill(
        include_str!("glsl/edge.glsl"),
        &[
            ("plane", plane),
            ("before", &before),
            ("cur", &cur),
            ("after", &after),
            (
                "slant",
                &format!("{:?}", crate::output::video::filter::SLANT_BIAS),
            ),
        ],
    )
}

fn filter(plane: &str, quality: Deinterlace) -> String {
    let mut text = String::new();
    if matches!(quality, Deinterlace::Yadif | Deinterlace::Bwdif) {
        text.push_str(&motion(plane));
        text.push_str(&edge(plane));
    }
    // weave には自分のフィールドが無いのでガードは要らない（`filter::sample` も同じ場所で先に返る）。
    let guard = if quality == Deinterlace::Weave {
        String::new()
    } else {
        crate::output::template::fill(include_str!("glsl/guard.glsl"), &[("plane", plane)])
    };
    let body = body(quality, plane);
    text.push_str(&crate::output::template::fill(
        include_str!("glsl/deint.glsl"),
        &[("plane", plane), ("guard", &guard), ("body", &body)],
    ));
    text
}

/// WGSL の `fs` の後半と同じ式。最後に [`Tone`] の `tone` を通す。
const TO_RGB: &str = include_str!("glsl/to_rgb.glsl");

/// 映像の伝達特性と原色を GLES の画面の符号値へ変換する方法。画面が SDR（sRGB）なら HDR は BT.2390 で
/// トーンマッピングし、BT.2020 は BT.709 へ変換する。画面が PQ・HLG なら BT.2020 のまま符号化する。
/// [`Mapping`] と同じ式と数値。SDR の BT.709 を SDR の画面へ出すときは何もしない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    mapping: Mapping,
}

impl Tone {
    /// SDR の画面へ。
    pub fn of(settings: &Settings) -> Self {
        Self::onto(settings, Space::Srgb)
    }

    pub fn onto(settings: &Settings, space: Space) -> Self {
        Self::of_mapping(Mapping::new(settings.transfer, settings.primaries, space))
    }

    pub fn of_mapping(mapping: Mapping) -> Self {
        Self { mapping }
    }

    pub fn none() -> Self {
        Self::of(&Settings::default())
    }

    pub fn is_identity(&self) -> bool {
        self.mapping.is_identity()
    }

    pub fn mapping(&self) -> Mapping {
        self.mapping
    }

    /// `vec3 tone(vec3 c)` と、その部品（`tone_to_nits`・`tone_encode`・`tone_undo`）の定義。
    /// `c` はガンマの掛かった RGB（0〜1）。
    pub fn function(&self) -> String {
        if self.is_identity() {
            return String::from("\nvec3 tone(vec3 c) { return c; }\n");
        }
        let m = &self.mapping;
        let hdr_surface = matches!(m.target, Space::Pq | Space::ScRgb);
        // `Mapping::to_nits` と同じ：PQ・scRGB の画面では HLG を公称 1000 で戻して SDR の白に合わせる。
        let (hlg_peak, hlg_scale) = if hdr_surface {
            (
                colour::HLG_PEAK,
                colour::HLG_PEAK * m.white / colour::SDR_WHITE,
            )
        } else {
            (m.peak, m.peak)
        };
        let source = match m.source {
            Transfer::Sdr | Transfer::Ui => String::from(
                "    n = vec3(tone_srgb_linear(c.r), tone_srgb_linear(c.g), tone_srgb_linear(c.b)) * {white};",
            ),
            Transfer::Hlg => format!(
                "    vec3 scene = vec3(tone_hlg_scene(c.r), tone_hlg_scene(c.g), tone_hlg_scene(c.b));\n    \
                 float y = dot(vec3({}), scene);\n    \
                 n = scene * ({} * pow(max(y, 0.000001), {}));",
                list(&colour::Primaries::Bt2020.luma()),
                lit(hlg_scale),
                lit(colour::hlg_gamma(hlg_peak) - 1.0),
            ),
            Transfer::Pq => String::from(
                "    n = vec3(tone_pq_nits(c.r), tone_pq_nits(c.g), tone_pq_nits(c.b));",
            ),
        };
        let gamut = match m.gamut {
            1 => matrix(&colour::BT2020_TO_BT709),
            2 => matrix(&colour::BT709_TO_BT2020),
            _ => String::new(),
        };
        // 縮めるのは SDR の画面への HDR と、PQ・scRGB の画面で上限を越える HLG（`Mapping::encode`）。
        let squeezed = match m.target {
            Space::Srgb if m.source.is_hdr() => Some((m.peak, m.white)),
            Space::Pq | Space::ScRgb if m.source == Transfer::Hlg && m.hlg_top() > m.peak => {
                Some((m.hlg_top(), m.peak))
            }
            _ => None,
        };
        let (top_nits, knee_nits) = squeezed.unwrap_or((m.peak, m.white));
        let luma = list(&colour::Primaries::Bt2020.luma());
        let encode = match m.target {
            Space::Srgb => String::from(
                "    vec3 l = clamp(n / {white}, 0.0, 1.0);\n    \
                 vec3 low = l * 12.92;\n    \
                 vec3 high = 1.055 * pow(l, vec3(1.0 / 2.4)) - 0.055;\n    \
                 return mix(low, high, step(vec3(0.0031308), l));",
            ),
            Space::ScRgb => format!("    return max(n, 0.0) / {};", lit(m.one)),
            Space::Pq => String::from(
                "    n = max(n, 0.0);\n    \
                 return vec3(tone_nits_pq(n.r), tone_nits_pq(n.g), tone_nits_pq(n.b));",
            ),
            // `nits_to_hlg`：OOTF は表示の輝度だけから戻す。
            Space::Hlg => format!(
                "    n = max(n, 0.0);\n    \
                 float yd = dot(vec3({luma}), n) / {peak};\n    \
                 if (yd <= 0.0) {{\n        return vec3(0.0);\n    }}\n    \
                 float gain = {peak} * pow(yd, {inverse});\n    \
                 return vec3(tone_scene_hlg(n.r / gain), tone_scene_hlg(n.g / gain), tone_scene_hlg(n.b / gain));",
                peak = lit(m.peak),
                inverse = lit((colour::hlg_gamma(m.peak) - 1.0) / colour::hlg_gamma(m.peak)),
            ),
        };
        let undo = match m.target {
            Space::Srgb => String::from(
                "    return vec3(tone_srgb_linear(e.r), tone_srgb_linear(e.g), tone_srgb_linear(e.b)) * {white};",
            ),
            Space::ScRgb => format!("    return e * {};", lit(m.one)),
            Space::Pq => String::from(
                "    return vec3(tone_pq_nits(e.r), tone_pq_nits(e.g), tone_pq_nits(e.b));",
            ),
            Space::Hlg => format!(
                "    vec3 scene = vec3(tone_hlg_scene(e.r), tone_hlg_scene(e.g), tone_hlg_scene(e.b));\n    \
                 float y = dot(vec3({luma}), scene);\n    \
                 return scene * ({peak} * pow(max(y, 0.000001), {gamma}));",
                peak = lit(m.peak),
                gamma = lit(colour::hlg_gamma(m.peak) - 1.0),
            ),
        };
        let squeeze = if squeezed.is_some() {
            "    n = tone_squeeze(n);"
        } else {
            ""
        };
        let top = colour::nits_to_pq(top_nits);
        let max_lum = colour::nits_to_pq(knee_nits) / top;
        let white = lit(m.white);
        let fill = |text: &str| crate::output::template::fill(text, &[("white", &white)]);
        crate::output::template::fill(
            include_str!("glsl/tone.glsl"),
            &[
                ("source", &fill(&source)),
                ("gamut", &gamut),
                ("squeeze", squeeze),
                ("encode", &fill(&encode)),
                ("undo", &fill(&undo)),
                ("hlg_a", &lit(colour::HLG_A)),
                ("hlg_b", &lit(colour::HLG_B)),
                ("hlg_c", &lit(colour::HLG_C)),
                ("pq_m1", &lit(colour::PQ_M1)),
                ("pq_m2", &lit(colour::PQ_M2)),
                ("pq_c1", &lit(colour::PQ_C1)),
                ("pq_c2", &lit(colour::PQ_C2)),
                ("pq_c3", &lit(colour::PQ_C3)),
                ("top", &lit(top)),
                ("ks", &lit(1.5 * max_lum - 0.5)),
                ("max_lum", &lit(max_lum)),
            ],
        )
    }
}

/// 原色の変換（リニア）。
fn matrix(rows: &[[f32; 3]; 3]) -> String {
    let rows = rows.map(|row| list(&row));
    format!(
        "    n = vec3(dot(vec3({}), n), dot(vec3({}), n), dot(vec3({}), n));",
        rows[0], rows[1], rows[2]
    )
}

/// GLSL の浮動小数の定数（小数点を必ず持つ）。
fn lit(value: f32) -> String {
    format!("{value:.9}")
}

fn list(values: &[f32; 3]) -> String {
    values.map(lit).join(", ")
}

fn to_rgb(tone: &Tone) -> String {
    let mut text = tone.function();
    text.push_str(TO_RGB);
    text
}

/// サンプルの読み方。8ビットは `LUMINANCE` がそのまま 0..1 を返す（ES 2.0.25 テーブル 3.12）。
/// 深いものは2バイトを2成分に入れ、`lo + hi*256` でコード値へ戻す（`code/255` なので丸めが要る）。
fn accessors(depth: u8) -> String {
    if depth > 8 {
        crate::output::template::fill(
            include_str!("glsl/accessors_wide.glsl"),
            &[("full", &full_scale(depth))],
        )
    } else {
        String::from(include_str!("glsl/accessors_eight.glsl"))
    }
}

fn full_scale(depth: u8) -> String {
    format!("{:.1}", f64::from((1u32 << depth.min(16)) - 1))
}

fn names() -> String {
    let mut text = String::new();
    for which in ["before", "cur", "after"] {
        text.push_str(&format!(
            "float y_{which}(float x, float y) {{ return at_y(y_{which}_t, x, y); }}\n"
        ));
        text.push_str(&format!(
            "float cb_{which}(float x, float y) {{ return at_c(c_{which}_t, x, y).x; }}\n"
        ));
        text.push_str(&format!(
            "float cr_{which}(float x, float y) {{ return at_c(c_{which}_t, x, y).y; }}\n"
        ));
    }
    text
}

pub fn vertex(taps: Taps) -> String {
    match taps {
        Taps::Computed => CORNERS.to_string(),
        // vertex の highp は必須なので、座標はここで fp32（GLSL ES 1.00 §4.5.1）。
        Taps::Varying => String::from(include_str!("glsl/varying.vert.glsl")),
    }
}

pub fn deinterlace(quality: Deinterlace, depth: u8, taps: Taps) -> String {
    deinterlace_as(quality, depth, taps, false, &Tone::none())
}

/// `mediump` が true だと `highp` を持たない GPU と同じ宣言になる。通常は false。
pub fn deinterlace_as(
    quality: Deinterlace,
    depth: u8,
    taps: Taps,
    mediump: bool,
    tone: &Tone,
) -> String {
    assert!(taps.carries(quality), "{taps:?} は {quality:?} を持てない");
    match taps {
        Taps::Computed => computed(quality, depth, mediump, tone),
        Taps::Varying => varying(quality, depth, mediump, tone),
    }
}

fn preamble(depth: u8, mediump: bool, tone: &Tone) -> String {
    let mut text = String::from(precision(mediump));
    for name in SAMPLERS {
        text.push_str(&format!("uniform sampler2D {name};\n"));
    }
    text.push_str("uniform vec2 luma_size;\nuniform vec2 chroma_size;\n");
    for name in UNIFORMS {
        text.push_str(&format!("uniform float {name};\n"));
    }
    text.push_str(include_str!("glsl/spot.glsl"));
    text.push_str(&accessors(depth));
    text.push_str(&names());
    text.push_str(&to_rgb(tone));
    text
}

fn computed(quality: Deinterlace, depth: u8, mediump: bool, tone: &Tone) -> String {
    let mut text = preamble(depth, mediump, tone);
    for plane in ["y", "cb", "cr"] {
        text.push_str(&filter(plane, quality));
    }
    text.push_str(include_str!("glsl/main_computed.glsl"));
    text
}

/// 座標を varying で受け取るサンプリング方式。fragment で座標の算術をしない。`p` は行の偶奇にだけ使い、
/// 整数は fp16 でも 2048 まで正確。
fn varying(quality: Deinterlace, depth: u8, mediump: bool, tone: &Tone) -> String {
    let mut text = String::from(precision(mediump));
    for name in SAMPLERS {
        text.push_str(&format!("uniform sampler2D {name};\n"));
    }
    for name in UNIFORMS {
        text.push_str(&format!("uniform float {name};\n"));
    }
    text.push_str(VARYINGS);
    text.push_str(&if depth > 8 {
        crate::output::template::fill(
            include_str!("glsl/varying_accessors_wide.glsl"),
            &[("full", &full_scale(depth))],
        )
    } else {
        String::from(include_str!("glsl/varying_accessors_eight.glsl"))
    });
    text.push_str(&to_rgb(tone));
    text.push_str(&main_of(quality));
    text
}

fn main_of(quality: Deinterlace) -> String {
    match quality {
        Deinterlace::Weave => String::from(include_str!("glsl/main_varying_weave.glsl")),
        Deinterlace::Bob => String::from(include_str!("glsl/main_varying_bob.glsl")),
        other => unreachable!("{other:?} は varying のサンプリング方式では使えない"),
    }
}

/// texel を正しく読めるかを確かめる一式。`GL_FRAGMENT_PRECISION_HIGH` だけでは足りない：
/// Mali-400 系は fragment の一時変数が fp16（ARM ECM-0442477 §3.5）で、1440 幅に足りない。
pub fn addressing_probe(taps: Taps) -> String {
    let mut text = String::from(PRECISION);
    text.push_str("uniform sampler2D y_cur_t;\n");
    match taps {
        Taps::Computed => text.push_str(include_str!("glsl/probe_computed.glsl")),
        Taps::Varying => {
            text.push_str(VARYINGS);
            text.push_str(include_str!("glsl/probe_varying.glsl"));
        }
    }
    text
}

/// 完成したフレームを表示へ移すパス。フィルターはコード化された解像度で動くので別にある。
pub const BLIT_VERTEX: &str = include_str!("glsl/blit.vert.glsl");

pub const BLIT_FRAGMENT: &str = include_str!("glsl/blit.frag.glsl");

/// 画面全体に重ねるパスの頂点。レンダーターゲットも既定のフレームバッファも行0が下なので、裏返さない。
pub const WHOLE_VERTEX: &str = "#version 100
attribute vec2 corner;
varying vec2 uv;
void main() {
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
    uv = corner;
}
";

/// HDR の画面に、画面（egui が乗算済みのアルファで描いた sRGB の符号値）を重ねる。符号値のまま混ぜると
/// 暗くなりすぎるので、下の映像を cd/m² へ戻してから混ぜる（`shader/over.wgsl`・[`Mapping::lay`] と同じ）。
pub fn over(space: Space) -> String {
    let tone = Tone::of_mapping(Mapping::new(Transfer::Ui, colour::Primaries::Bt709, space));
    let mut text = String::from(PRECISION);
    text.push_str("uniform sampler2D paper;\nuniform sampler2D below;\nvarying vec2 uv;\n");
    text.push_str(&tone.function());
    text.push_str(
        "void main() {
    vec4 p = texture2D(paper, uv);
    if (p.a <= 0.0) {
        discard;
    }
    vec3 over = tone_to_nits(clamp(p.rgb / p.a, 0.0, 1.0));
    vec3 under = tone_undo(texture2D(below, uv).rgb);
    gl_FragColor = vec4(tone_encode(over * p.a + under * (1.0 - p.a)), 1.0);
}
",
    );
    text
}

/// テクスチャの符号値を `tone` で変換して写す（HDR の画面で描いたキャプチャを SDR に戻す）。
pub fn convert(tone: &Tone) -> String {
    let mut text = String::from(PRECISION);
    text.push_str("uniform sampler2D picture;\nvarying vec2 uv;\n");
    text.push_str(&tone.function());
    text.push_str(
        "void main() { gl_FragColor = vec4(tone(clamp(texture2D(picture, uv).rgb, 0.0, 1.0)), 1.0); }\n",
    );
    text
}

/// コピーせずに持つフレーム（外部テクスチャ）を画面へ 1 パスで。RGB はドライバーが返し、伝達特性と原色は
/// [`Tone`] がこちらで変換する（HDR のトーンマッピングには `highp` があるほうがよい）。
pub fn external(tone: &Tone) -> String {
    let mut text = if tone.is_identity() {
        String::from("#version 100\nprecision mediump float;\n")
    } else {
        String::from(PRECISION)
    };
    text.insert_str(
        text.find('\n').map_or(0, |at| at + 1),
        "#extension GL_OES_EGL_image_external : require\n",
    );
    text.push_str("uniform samplerExternalOES picture;\nvarying vec2 uv;\n");
    text.push_str(&tone.function());
    text.push_str(
        "void main() { gl_FragColor = vec4(tone(clamp(texture2D(picture, uv).rgb, 0.0, 1.0)), 1.0); }\n",
    );
    text
}

/// フィルターの要らない 8 ビットのフレームを、テクスチャから画面へ 1 パスで（`glsl/direct.frag.glsl`）。
/// mediump で足りる：座標は varying のまま読み、色の変換も 8 ビットの幅しか要らない。
pub fn direct(tone: &Tone) -> String {
    let mut text = String::from(if tone.is_identity() {
        precision(true)
    } else {
        PRECISION
    });
    for name in ["y_cur_t", "c_cur_t"] {
        text.push_str(&format!("uniform sampler2D {name};\n"));
    }
    for name in UNIFORMS {
        text.push_str(&format!("uniform float {name};\n"));
    }
    text.push_str(&to_rgb(tone));
    text.push_str(include_str!("glsl/direct.frag.glsl"));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_comments(text: &str) -> String {
        text.lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// ESSL1 に無いものを書いていないか（GL の無い環境でも動くチェック）。
    #[test]
    fn nothing_reaches_for_what_glsl_es_one_does_not_have() {
        for taps in [Taps::Computed, Taps::Varying] {
            for quality in [
                Deinterlace::Weave,
                Deinterlace::Bob,
                Deinterlace::Yadif,
                Deinterlace::Bwdif,
            ] {
                if !taps.carries(quality) {
                    continue;
                }
                for depth in [8, 10] {
                    let text = deinterlace(quality, depth, taps);
                    assert!(text.starts_with("#version 100"), "{taps:?} {quality:?}");
                    // 注釈の中の `%` や `&` で落ちると本物の書き間違いが埋もれる。
                    let text = strip_comments(&text);
                    for forbidden in [
                        "texelFetch",
                        "textureSize",
                        "texture(",
                        "in vec",
                        "out vec",
                        "isampler",
                        "usampler",
                        "&",
                        "|",
                        "<<",
                        ">>",
                        "%",
                    ] {
                        assert!(
                            !text.contains(forbidden),
                            "{taps:?} {quality:?} が {forbidden} を使っている"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_pass_stays_inside_the_eight_texture_units_es_two_guarantees() {
        assert_eq!(SAMPLERS.len(), 6);
        let text = deinterlace(Deinterlace::Bwdif, 8, Taps::Computed);
        for name in SAMPLERS {
            assert!(
                text.contains(&format!("uniform sampler2D {name};")),
                "{name}"
            );
        }
    }

    #[test]
    fn the_varying_route_stays_inside_the_eight_varyings_es_two_guarantees() {
        let text = vertex(Taps::Varying);
        assert_eq!(text.matches("varying vec2").count(), 6);
    }

    /// 時間の検査は時間を見る2つだけ。輪郭の探索は両方が持つ。
    #[test]
    fn only_the_temporal_filters_carry_the_temporal_check() {
        for quality in [Deinterlace::Bwdif, Deinterlace::Yadif] {
            let text = deinterlace(quality, 8, Taps::Computed);
            assert!(text.contains("vec2 motion_y"), "{quality:?}");
            for plane in ["y", "cb", "cr"] {
                assert!(text.contains(&format!("float edge_{plane}")), "{quality:?}");
            }
        }
        for quality in [Deinterlace::Bob, Deinterlace::Weave] {
            let text = deinterlace(quality, 8, Taps::Computed);
            assert!(!text.contains("vec2 motion_y"), "{quality:?}");
            assert!(!text.contains("float edge_y"), "{quality:?}");
        }
    }

    #[test]
    fn the_depth_decides_how_a_sample_is_read() {
        let eight = deinterlace(Deinterlace::Bob, 8, Taps::Computed);
        let ten = deinterlace(Deinterlace::Bob, 10, Taps::Computed);
        assert!(!eight.contains("byte_of"));
        assert!(ten.contains("byte_of"));
        assert!(ten.contains("1023.0"));
    }

    /// HDR のトーンマッピングの式も ESSL1 の中で書き、外部テクスチャの拡張の宣言は先頭の行のすぐ後に置く。
    #[test]
    fn the_tone_stays_inside_glsl_es_one() {
        use crate::output::video::{Matrix, Primaries, Settings, Transfer};
        for transfer in [Transfer::Sdr, Transfer::Hlg, Transfer::Pq] {
            let tone = Tone::of(&Settings {
                matrix: Matrix::Bt2020,
                transfer,
                primaries: Primaries::Bt2020,
                ..Settings::default()
            });
            assert!(!tone.is_identity(), "{transfer:?}");
            let function = strip_comments(&tone.function());
            assert!(function.contains("vec3 tone(vec3 c)"), "{transfer:?}");
            for forbidden in ["&", "|", "<<", ">>", "%", "texture("] {
                assert!(
                    !function.contains(forbidden),
                    "{transfer:?} が {forbidden} を使っている"
                );
            }
            assert_eq!(
                function.contains("tone_squeeze(n);"),
                transfer != Transfer::Sdr,
                "{transfer:?}"
            );
            let external = external(&tone);
            let mut lines = external.lines();
            assert_eq!(lines.next(), Some("#version 100"));
            assert_eq!(
                lines.next(),
                Some("#extension GL_OES_EGL_image_external : require")
            );
            for depth in [8, 10] {
                let text = deinterlace_as(Deinterlace::Bob, depth, Taps::Computed, false, &tone);
                assert!(text.contains("rgb = tone(rgb);"));
                assert!(text.contains("vec3 tone_to_nits"));
            }
        }
        let none = Tone::none();
        assert!(none.is_identity());
        assert!(!none.function().contains("tone_squeeze"));
        assert!(external(&none).contains("precision mediump float;"));
    }

    /// 対応しないものを要求したら、黙って違うフレームを出さずにその場で panic する。
    #[test]
    fn the_varying_route_refuses_what_it_cannot_carry() {
        assert!(Taps::Varying.carries(Deinterlace::Bob));
        assert!(!Taps::Varying.carries(Deinterlace::Bwdif));
        assert!(Taps::Computed.carries(Deinterlace::Bwdif));
    }
}
