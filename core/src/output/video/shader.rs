// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `shader/*.wgsl` の断片を選び、`{name}` を埋めて WGSL を組み立てる。
//! 計算は [`super::filter`] と一行ずつ同じ（ゴールデンテスト）。両方を見て直すこと。

use std::sync::LazyLock;

use super::Deinterlace;
use super::colour::Mapping;
use crate::output::template::fill_planes;

/// サンプルがテクスチャにどう並ぶか。色差を sampler で補間させない（インターレースの 4:2:0 は
/// フィールドごとに色差の位置が違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Planar,
    Biplanar,
}

impl Shape {
    pub fn textures(self) -> u32 {
        match self {
            Self::Planar => 3,
            Self::Biplanar => 2,
        }
    }

    /// テクスチャのバインディングの数（三フレーム分）。uniform はその次。
    pub fn bindings(self) -> u32 {
        3 * self.textures()
    }
}

/// WGSL と突き合わせやすいよう手でバイトに書く。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub first_row: u32,
    /// 無ければ [`super::filter::NO_MATE`]。
    pub mate_row: u32,
    pub luma_offset: f32,
    pub luma_gain: f32,
    pub chroma_offset: f32,
    pub chroma_gain: f32,
    pub kr: f32,
    pub kb: f32,
    /// フレームそのものの輝度の大きさ。ハードウェアのサーフェスは 1080 行が 1088 で、D3D12 の
    /// `textureDimensions` は 1088 を返すので渡す必要がある。
    pub width: u32,
    pub height: u32,
    /// 縞を見るしきい値（隣の行との差）と動きのしきい値（前のフレームとの差）。
    pub comb: f32,
    pub motion: f32,
    pub luma_lift: f32,
    pub chroma_cos: f32,
    pub chroma_sin: f32,
}

impl Params {
    pub const SIZE: usize = 64;

    pub fn bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let words: [[u8; 4]; 15] = [
            self.first_row.to_le_bytes(),
            self.mate_row.to_le_bytes(),
            self.luma_offset.to_le_bytes(),
            self.luma_gain.to_le_bytes(),
            self.chroma_offset.to_le_bytes(),
            self.chroma_gain.to_le_bytes(),
            self.kr.to_le_bytes(),
            self.kb.to_le_bytes(),
            self.width.to_le_bytes(),
            self.height.to_le_bytes(),
            self.comb.to_le_bytes(),
            self.motion.to_le_bytes(),
            self.luma_lift.to_le_bytes(),
            self.chroma_cos.to_le_bytes(),
            self.chroma_sin.to_le_bytes(),
        ];
        for (at, word) in words.iter().enumerate() {
            out[at * 4..at * 4 + 4].copy_from_slice(word);
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Deinterlace(Deinterlace),
    /// `comb.wgsl` のコーミングで c/p を選び、残ったコーミングの画素は動き適応（三次）で補間する。
    Match,
}

fn body(pass: Pass, plane: &str) -> String {
    let quality = match pass {
        Pass::Deinterlace(quality) => quality,
        Pass::Match => {
            let text = concat!(
                include_str!("shader/body_match.wgsl"),
                include_str!("shader/body_cubic.wgsl")
            );
            return fill_planes(text, plane);
        }
    };
    let text = match quality {
        // 逆テレシネのフレームは片方の行を前のフレームから取る。ほかは `mate_row` が 2（どの行でもない）。
        Deinterlace::Weave => include_str!("shader/body_weave.wgsl"),
        Deinterlace::Bob => include_str!("shader/body_bob.wgsl"),
        Deinterlace::Yadif => include_str!("shader/body_linear.wgsl"),
        Deinterlace::Bwdif => include_str!("shader/body_cubic.wgsl"),
    };
    fill_planes(text, plane)
}

fn motion(plane: &str) -> String {
    fill_planes(include_str!("shader/motion.wgsl"), plane)
}

fn edge(plane: &str) -> String {
    let before = format!("{plane}_before");
    let cur = format!("{plane}_cur");
    let after = format!("{plane}_after");
    crate::output::template::fill(
        include_str!("shader/edge.wgsl"),
        &[
            ("plane", plane),
            ("before", &before),
            ("cur", &cur),
            ("after", &after),
            ("slant", &format!("{:?}", super::filter::SLANT_BIAS)),
        ],
    )
}

fn filter(plane: &str, pass: Pass) -> String {
    let mut text = String::new();
    if matches!(
        pass,
        Pass::Deinterlace(Deinterlace::Yadif | Deinterlace::Bwdif) | Pass::Match
    ) {
        text.push_str(&motion(plane));
        text.push_str(&edge(plane));
    }
    let body = body(pass, plane);
    // weave には自分のフィールドが無いので守る行も無い。
    let guard = if pass == Pass::Deinterlace(Deinterlace::Weave) {
        String::new()
    } else {
        crate::output::template::fill(include_str!("shader/guard.wgsl"), &[("plane", plane)])
    };
    text.push_str(&crate::output::template::fill(
        include_str!("shader/deint.wgsl"),
        &[("plane", plane), ("guard", &guard), ("body", &body)],
    ));
    text
}

/// 10 ビットは `R16Unorm` が使えない GPU があるので `R16Uint` で読み、1023 で割る。
pub fn deinterlace(quality: Deinterlace, depth: u8, shape: Shape) -> String {
    assemble(Pass::Deinterlace(quality), depth, shape)
}

/// コーミングの集計（storage）を uniform の次の番号に足す。
pub fn matched(depth: u8, shape: Shape) -> String {
    assemble(Pass::Match, depth, shape)
}

pub fn comb(depth: u8, shape: Shape) -> String {
    let (kind, luma_load, _) = loads(depth, shape);
    crate::output::template::fill(
        include_str!("shader/comb.wgsl"),
        &[("kind", kind), ("luma_load", &luma_load)],
    )
}

fn loads(depth: u8, shape: Shape) -> (&'static str, String, String) {
    let wide = depth > 8;
    // 2 プレーンの 10 ビットは 16 ビットの上に寄っているので、`code / 1023` に戻す。
    let (kind, luma_load, chroma_load) = match (shape, wide) {
        (Shape::Planar, true) => (
            "texture_2d<u32>",
            format!(
                "f32(textureLoad(t, q, 0).r) / {}.0",
                (1u32 << depth.min(16)) - 1
            ),
            String::new(),
        ),
        (Shape::Planar, false) => (
            "texture_2d<f32>",
            String::from("textureLoad(t, q, 0).r"),
            String::new(),
        ),
        (Shape::Biplanar, true) => (
            "texture_2d<f32>",
            String::from("textureLoad(t, q, 0).r * (65535.0 / 65472.0)"),
            String::from("textureLoad(t, q, 0).rg * (65535.0 / 65472.0)"),
        ),
        (Shape::Biplanar, false) => (
            "texture_2d<f32>",
            String::from("textureLoad(t, q, 0).r"),
            String::from("textureLoad(t, q, 0).rg"),
        ),
    };
    (kind, luma_load, chroma_load)
}

fn assemble(pass: Pass, depth: u8, shape: Shape) -> String {
    let (kind, luma_load, chroma_load) = loads(depth, shape);
    let mut text = include_str!("shader/params.wgsl").to_string();
    let names: &[&str] = match shape {
        Shape::Planar => &["y", "cb", "cr"],
        Shape::Biplanar => &["y", "c"],
    };
    let mut binding = 0;
    for name in names {
        for which in ["before", "cur", "after"] {
            text.push_str(&format!(
                "@group(0) @binding({binding}) var {name}_{which}_t: {kind};\n"
            ));
            binding += 1;
        }
    }
    text.push_str(&crate::output::template::fill(
        include_str!("shader/prelude.wgsl"),
        &[
            ("group", "0"),
            ("binding", &binding.to_string()),
            ("kind", kind),
            ("luma_load", &luma_load),
        ],
    ));
    if pass == Pass::Match {
        text.push_str(&format!(
            "@group(0) @binding({}) var<storage, read> combed: array<u32, 8>;\n",
            binding + 1
        ));
    }
    if shape == Shape::Biplanar {
        text.push_str(&crate::output::template::fill(
            include_str!("shader/pair.wgsl"),
            &[("kind", kind), ("chroma_load", &chroma_load)],
        ));
    }
    for which in ["before", "cur", "after"] {
        text.push_str(&format!(
            "fn y_{which}(x: i32, y: i32) -> f32 {{ return at(y_{which}_t, x, y, luma_size()); }}\n"
        ));
        match shape {
            Shape::Planar => {
                for plane in ["cb", "cr"] {
                    text.push_str(&format!(
                        "fn {plane}_{which}(x: i32, y: i32) -> f32 {{ return at({plane}_{which}_t, x, y, chroma_size()); }}\n"
                    ));
                }
            }
            Shape::Biplanar => {
                text.push_str(&format!(
                    "fn cb_{which}(x: i32, y: i32) -> f32 {{ return at_pair(c_{which}_t, x, y).x; }}\n"
                ));
                text.push_str(&format!(
                    "fn cr_{which}(x: i32, y: i32) -> f32 {{ return at_pair(c_{which}_t, x, y).y; }}\n"
                ));
            }
        }
    }
    text.push_str(include_str!("shader/vertex.wgsl"));
    for plane in ["y", "cb", "cr"] {
        text.push_str(&filter(plane, pass));
    }
    text.push_str(include_str!("shader/rgb.wgsl"));
    text.push_str(include_str!("shader/fragment.wgsl"));
    text
}

/// プラグインのデインタレースの前置き（`zwsdk::video::DEINTERLACE_PRELUDE`）。
/// バインディングの番号は `プレーン × フレーム数 + (フレーム - lo)`、その次が [`Params`]。
pub fn frames_prelude(depth: u8, shape: Shape, [lo, hi]: [i32; 2]) -> String {
    let (kind, luma_load, chroma_load) = loads(depth, shape);
    let mut text = include_str!("shader/params.wgsl").to_string();
    let names: &[&str] = match shape {
        Shape::Planar => &["y", "cb", "cr"],
        Shape::Biplanar => &["y", "c"],
    };
    let tag = |frame: i32| match frame {
        f if f < 0 => format!("m{}", -f),
        f => format!("p{f}"),
    };
    let mut binding = 0;
    for name in names {
        for frame in lo..=hi {
            text.push_str(&format!(
                "@group(1) @binding({binding}) var zw_{name}_{}_t: {kind};\n",
                tag(frame)
            ));
            binding += 1;
        }
    }
    text.push_str(&crate::output::template::fill(
        include_str!("shader/prelude.wgsl"),
        &[
            ("group", "1"),
            ("binding", &binding.to_string()),
            ("kind", kind),
            ("luma_load", &luma_load),
        ],
    ));
    if shape == Shape::Biplanar {
        text.push_str(&crate::output::template::fill(
            include_str!("shader/pair.wgsl"),
            &[("kind", kind), ("chroma_load", &chroma_load)],
        ));
    }
    text.push_str(include_str!("shader/rgb.wgsl"));
    text.push_str(&format!(
        "fn zw_frames() -> vec2<i32> {{ return vec2<i32>({lo}, {hi}); }}\n\
         fn zw_size() -> vec2<i32> {{ return luma_size(); }}\n\
         fn zw_chroma_size() -> vec2<i32> {{ return chroma_size(); }}\n\
         fn zw_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32> {{ return to_rgb(y, cb, cr); }}\n"
    ));
    // WGSL はテクスチャを番号で選べないので、if を並べる。
    let reader = |function: &str, read: &dyn Fn(&str) -> String| {
        let mut body = format!(
            "fn {function}(frame: i32, p: vec2<i32>) -> f32 {{\n    let f = clamp(frame, {lo}, {hi});\n"
        );
        for frame in lo..hi {
            body.push_str(&format!(
                "    if f == {frame} {{ return {}; }}\n",
                read(&tag(frame))
            ));
        }
        body.push_str(&format!("    return {};\n}}\n", read(&tag(hi))));
        body
    };
    text.push_str(&reader("zw_luma", &|t| {
        format!("at(zw_y_{t}_t, p.x, p.y, luma_size())")
    }));
    match shape {
        Shape::Planar => {
            for plane in ["cb", "cr"] {
                text.push_str(&reader(&format!("zw_{plane}"), &|t| {
                    format!("at(zw_{plane}_{t}_t, p.x, p.y, chroma_size())")
                }));
            }
        }
        Shape::Biplanar => {
            text.push_str(&reader("zw_cb", &|t| {
                format!("at_pair(zw_c_{t}_t, p.x, p.y).x")
            }));
            text.push_str(&reader("zw_cr", &|t| {
                format!("at_pair(zw_c_{t}_t, p.x, p.y).y")
            }));
        }
    }
    text
}

/// フィルターはストリームの解像度で動くので別のパス。ガンマはここで外す（8 ビットのテクスチャに
/// リニアを入れると暗部が潰れる）。
pub fn blit() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| {
        [
            include_str!("shader/srgb.wgsl"),
            include_str!("shader/fit.wgsl"),
            include_str!("shader/blit.wgsl"),
            &ewa_table(),
        ]
        .concat()
    });
    &TEXT
}

/// デコーダーの 2 プレーンを直に読んで画面へ書くパス（`direct.wgsl`）。
pub fn direct() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| {
        [
            include_str!("shader/srgb.wgsl"),
            include_str!("shader/fit.wgsl"),
            include_str!("shader/params.wgsl"),
            "@group(0) @binding(4) var<uniform> params: Params;\n",
            include_str!("shader/rgb.wgsl"),
            include_str!("shader/direct.wgsl"),
        ]
        .concat()
    });
    &TEXT
}

/// プラグインのレイヤーの絵を画面の描画の中で重ねるパス（`vs`・`fs`）。
pub fn sprites() -> &'static str {
    include_str!("shader/sprites.wgsl")
}

/// PQ・HLG のサーフェスへリニア光で重ねるパス（`fs_over`）を足した物。
pub fn over() -> &'static str {
    static TEXT: LazyLock<String> =
        LazyLock::new(|| [blit(), include_str!("shader/over.wgsl")].concat());
    &TEXT
}

const EWA_STEPS: usize = 1024;
const EWA_RADIUS: f64 = 3.238_315_484_166_236;
const JINC_ZERO: f64 = 1.2196698912665045;

/// 点ごとの Jinc の計算が重いため、距離の二乗を添字にして参照するテーブル。
fn ewa_table() -> String {
    let r2 = EWA_RADIUS * EWA_RADIUS;
    let values: Vec<String> = (0..=EWA_STEPS)
        .map(|k| {
            let r = (k as f64 / EWA_STEPS as f64 * r2).sqrt();
            let w = if k == EWA_STEPS {
                0.0
            } else {
                jinc(r) * jinc(r * JINC_ZERO / EWA_RADIUS)
            };
            format!("{w:e}")
        })
        .collect();
    format!(
        "\nconst EWA_RADIUS: f32 = {EWA_RADIUS};\nconst EWA_STEPS: f32 = {EWA_STEPS}.0;\nconst EWA_TABLE = array<f32, {}>({});\n",
        EWA_STEPS + 1,
        values.join(", ")
    )
}

fn jinc(x: f64) -> f64 {
    if x < 1e-9 {
        return 1.0;
    }
    let t = std::f64::consts::PI * x;
    2.0 * bessel_j1(t) / t
}

/// Numerical Recipes の bessj1。
fn bessel_j1(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 8.0 {
        let y = x * x;
        let a = x
            * (72362614232.0
                + y * (-7895059235.0
                    + y * (242396853.1
                        + y * (-2972611.439 + y * (15704.48260 + y * (-30.16036606))))));
        let b = 144725228442.0
            + y * (2300535178.0 + y * (18583304.74 + y * (99447.43394 + y * (376.9991397 + y))));
        return a / b;
    }
    let z = 8.0 / ax;
    let y = z * z;
    let xx = ax - 2.356194491;
    let p = 1.0
        + y * (0.183105e-2
            + y * (-0.3516396496e-4 + y * (0.2457520174e-5 + y * (-0.240337019e-6))));
    let q = 0.04687499995
        + y * (-0.2002690873e-3
            + y * (0.8449199096e-5 + y * (-0.88228987e-6 + y * 0.105787412e-6)));
    let r = (std::f64::consts::FRAC_2_PI / ax).sqrt() * (xx.cos() * p - z * xx.sin() * q);
    if x < 0.0 { -r } else { r }
}

/// `Fit` の並びそのもの（`vec4<f32>` は十六バイト境界）。
pub fn fit_bytes(rect: [f32; 4], srgb: bool, mapping: &Mapping, antiring: bool) -> [u8; FIT_SIZE] {
    let mut out = [0u8; FIT_SIZE];
    for (at, value) in rect.iter().enumerate() {
        out[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    let words: [[u8; 4]; 8] = [
        u32::from(srgb).to_le_bytes(),
        mapping.source_code().to_le_bytes(),
        mapping.target_code().to_le_bytes(),
        mapping.gamut.to_le_bytes(),
        mapping.peak.to_le_bytes(),
        mapping.white.to_le_bytes(),
        mapping.one.to_le_bytes(),
        u32::from(antiring).to_le_bytes(),
    ];
    for (at, word) in words.iter().enumerate() {
        out[16 + at * 4..20 + at * 4].copy_from_slice(word);
    }
    out
}

pub const FIT_SIZE: usize = 48;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parameters_are_written_in_the_order_the_shader_reads_them() {
        let params = Params {
            first_row: 1,
            mate_row: 0,
            luma_offset: 0.5,
            luma_gain: 2.0,
            chroma_offset: 0.25,
            chroma_gain: 4.0,
            kr: 0.2126,
            kb: 0.0722,
            width: 1440,
            height: 1080,
            comb: 0.001,
            motion: 0.02,
            luma_lift: 0.125,
            chroma_cos: 0.75,
            chroma_sin: -0.5,
        };
        let bytes = params.bytes();
        assert_eq!(&bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0u32.to_le_bytes());
        assert_eq!(&bytes[8..12], &0.5f32.to_le_bytes());
        assert_eq!(&bytes[32..36], &1440u32.to_le_bytes());
        assert_eq!(&bytes[36..40], &1080u32.to_le_bytes());
        assert_eq!(&bytes[48..52], &0.125f32.to_le_bytes());
        assert_eq!(&bytes[56..60], &(-0.5f32).to_le_bytes());
        assert_eq!(bytes.len(), Params::SIZE);
    }

    #[test]
    fn the_blit_is_told_where_to_put_the_picture_and_whether_to_undo_the_gamma() {
        use super::super::colour::{Primaries, Space, Transfer};
        let hlg = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Srgb);
        let bytes = fit_bytes([-1.0, -0.5, 1.0, 0.5], true, &hlg, false);
        assert_eq!(&bytes[0..4], &(-1.0f32).to_le_bytes());
        assert_eq!(&bytes[12..16], &0.5f32.to_le_bytes());
        assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &1u32.to_le_bytes(), "HLG は一");
        assert_eq!(&bytes[24..28], &0u32.to_le_bytes(), "sRGB のサーフェスは零");
        assert_eq!(&bytes[28..32], &1u32.to_le_bytes(), "BT.2020 から BT.709");
        assert_eq!(&bytes[32..36], &1000.0f32.to_le_bytes());
        assert_eq!(&bytes[36..40], &203.0f32.to_le_bytes());
        let sdr = Mapping::new(Transfer::Sdr, Primaries::Bt709, Space::Srgb);
        assert!(sdr.is_identity());
        assert_eq!(fit_bytes([0.0; 4], false, &sdr, false)[16..32], [0; 16]);
    }

    #[test]
    fn every_shader_of_the_ladder_binds_all_of_its_planes() {
        for shape in [Shape::Planar, Shape::Biplanar] {
            for quality in [
                Deinterlace::Weave,
                Deinterlace::Bob,
                Deinterlace::Yadif,
                Deinterlace::Bwdif,
            ] {
                for depth in [8, 10] {
                    let text = deinterlace(quality, depth, shape);
                    for binding in 0..shape.bindings() {
                        assert!(
                            text.contains(&format!("@binding({binding})")),
                            "{shape:?} {quality:?} depth {depth} is missing binding {binding}"
                        );
                    }
                    assert!(
                        !text.contains(&format!("@binding({})", shape.bindings() + 1)),
                        "{shape:?} binds more than it declares"
                    );
                    assert!(text.contains("fn deint_y"));
                    assert!(text.contains("fn deint_cb"));
                    assert!(text.contains("fn deint_cr"));
                }
            }
        }
        assert_eq!(Shape::Planar.bindings(), 9);
        assert_eq!(Shape::Biplanar.bindings(), 6);
    }

    /// GPU が無い環境でも WGSL の書き間違いを検出する。
    fn validates(label: &str, text: &str) {
        let module = match naga::front::wgsl::parse_str(text) {
            Ok(module) => module,
            Err(error) => panic!("{label}: {}", error.emit_to_string(text)),
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        if let Err(error) = validator.validate(&module) {
            panic!("{label}: {}", error.emit_to_string(text));
        }
    }

    #[test]
    fn the_plugin_prelude_parses_and_validates() {
        let user = "
@fragment
fn main(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(at.xy);
    let w = zw_frames();
    let c = p / 2;
    let rgb = zw_rgb(zw_luma(w.x, p) + zw_luma(w.y, p), zw_cb(0, c), zw_cr(1, c));
    return vec4<f32>(rgb, f32(zw_size().x + zw_chroma_size().y));
}
";
        for shape in [Shape::Planar, Shape::Biplanar] {
            for depth in [8, 10] {
                for frames in [[-1, 1], [-2, 2], [0, 0], [-2, 0]] {
                    let text = format!("{}{user}", frames_prelude(depth, shape, frames));
                    validates(&format!("prelude {shape:?} {depth} {frames:?}"), &text);
                    let count = (frames[1] - frames[0] + 1) as u32 * shape.textures();
                    assert!(
                        text.contains(&format!("@group(1) @binding({count}) var<uniform> params"))
                    );
                }
            }
        }
    }

    #[test]
    fn every_shader_parses_and_validates() {
        for shape in [Shape::Planar, Shape::Biplanar] {
            for depth in [8, 10] {
                for quality in [
                    Deinterlace::Weave,
                    Deinterlace::Bob,
                    Deinterlace::Yadif,
                    Deinterlace::Bwdif,
                ] {
                    validates(
                        &format!("{quality:?} {shape:?} {depth}"),
                        &deinterlace(quality, depth, shape),
                    );
                }
                validates(&format!("match {shape:?} {depth}"), &matched(depth, shape));
                validates(&format!("comb {shape:?} {depth}"), &comb(depth, shape));
            }
        }
        validates("blit", blit());
        validates("direct", direct());
        validates("over", over());
        validates("sprites", sprites());
    }

    #[test]
    fn the_match_pass_binds_the_comb_counts_after_the_uniform() {
        for shape in [Shape::Planar, Shape::Biplanar] {
            for depth in [8, 10] {
                let text = matched(depth, shape);
                assert!(text.contains(&format!(
                    "@binding({}) var<storage, read> combed",
                    shape.bindings() + 1
                )));
                assert!(text.contains("combed[1] * 4u < combed[0] * 3u"));
                assert!(text.contains("params.motion"));
                assert!(
                    text.contains("(9.0 * near - far) / 16.0"),
                    "三次の補間が要る"
                );
                assert!(text.contains("params.comb"));
                let comb = comb(depth, shape);
                assert!(comb.contains("@compute"));
                assert!(!comb.contains("{kind}") && !comb.contains("{luma_load}"));
                if depth > 8 && shape == Shape::Planar {
                    assert!(comb.contains("texture_2d<u32>"));
                }
            }
        }
    }

    #[test]
    fn the_filters_do_not_depend_on_the_shape_the_samples_came_in() {
        for quality in [Deinterlace::Bob, Deinterlace::Yadif, Deinterlace::Bwdif] {
            let planar = deinterlace(quality, 8, Shape::Planar);
            let biplanar = deinterlace(quality, 8, Shape::Biplanar);
            for plane in ["y", "cb", "cr"] {
                let of = |text: &str| {
                    let at = text
                        .find(&format!("fn deint_{plane}("))
                        .expect("the filter");
                    let end = text[at..].find("\n}\n").expect("its end");
                    text[at..at + end].to_string()
                };
                assert_eq!(of(&planar), of(&biplanar), "{quality:?} deint_{plane}");
            }
        }
    }

    #[test]
    fn the_depth_decides_how_a_sample_is_normalised() {
        let of = |depth, shape| deinterlace(Deinterlace::Bob, depth, shape);
        assert!(of(10, Shape::Planar).contains("/ 1023.0"));
        assert!(of(10, Shape::Planar).contains("texture_2d<u32>"));
        assert!(!of(8, Shape::Planar).contains("/ 1023.0"));
        assert!(of(8, Shape::Planar).contains("texture_2d<f32>"));
        assert!(of(10, Shape::Biplanar).contains("65535.0 / 65472.0"));
        assert!(of(10, Shape::Biplanar).contains("texture_2d<f32>"));
        assert!(!of(8, Shape::Biplanar).contains("65472"));
    }

    #[test]
    fn only_the_temporal_filters_carry_the_temporal_check() {
        for quality in [Deinterlace::Bwdif, Deinterlace::Yadif] {
            let text = deinterlace(quality, 8, Shape::Planar);
            assert!(text.contains("fn motion_y"), "{quality:?}");
            for plane in ["y", "cb", "cr"] {
                assert!(text.contains(&format!("fn edge_{plane}")), "{quality:?}");
            }
        }
        for quality in [Deinterlace::Bob, Deinterlace::Weave] {
            let text = deinterlace(quality, 8, Shape::Planar);
            assert!(!text.contains("fn motion_y"), "{quality:?}");
            assert!(!text.contains("fn edge_y"), "{quality:?}");
        }
    }
}
