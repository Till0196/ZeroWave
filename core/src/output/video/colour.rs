// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Y'CbCr から表示が求める値へ。HDR（BT.2020 の HLG・PQ、ARIB STD-B32 第1部 表4-6）の
//! マッピングは [`Mapping`] と `shader/fit.wgsl` に二度書き、`tests/video.rs` で突き合わせる。

use mediadecoder::es::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Matrix {
    #[default]
    Bt709,
    Bt2020,
}

impl Matrix {
    /// STD-B32：4K は 9、ほかは 1。無ければ幅で決める。
    pub fn of_format(format: Format) -> Self {
        match format.colour.matrix {
            1 | 5 | 6 => Self::Bt709,
            9 | 10 => Self::Bt2020,
            _ => {
                if is_uhd(format) {
                    Self::Bt2020
                } else {
                    Self::Bt709
                }
            }
        }
    }

    pub fn weights(self) -> (f32, f32) {
        match self {
            Self::Bt709 => (0.2126, 0.0722),
            Self::Bt2020 => (0.2627, 0.0593),
        }
    }
}

fn is_uhd(format: Format) -> bool {
    format.width > 1920 || format.height > 1080
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Primaries {
    /// sRGB と同じ原色なので、SDR の放送はそのまま sRGB のサーフェスに渡せる。
    #[default]
    Bt709,
    Bt2020,
}

impl Primaries {
    pub fn of_format(format: Format) -> Self {
        match format.colour.primaries {
            1 => Self::Bt709,
            9 => Self::Bt2020,
            _ => {
                if is_uhd(format) {
                    Self::Bt2020
                } else {
                    Self::Bt709
                }
            }
        }
    }

    /// BT.2020 では HLG の OOTF もこれを使う。
    pub fn luma(self) -> [f32; 3] {
        match self {
            Self::Bt709 => [0.212_639, 0.715_169, 0.072_192],
            Self::Bt2020 => [0.262_700, 0.678_000, 0.059_300],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transfer {
    /// BT.709（BT.2020 の 10 ビットも同じ曲線）。
    #[default]
    Sdr,
    /// ARIB STD-B67、`transfer_characteristics` 18。1 が公称ピーク。
    Hlg,
    /// SMPTE ST 2084、16。1 が 10 000 cd/m²。
    Pq,
    /// egui の UI レイヤー。egui はガンマ空間でブレンドする前提で文字の縁を作るので、UI はガンマのままレンダリングし、
    /// 重ねるときに伝達関数を外す。
    Ui,
}

impl Transfer {
    /// 記述が無ければ解像度に依らず SDR（STD-B32）。
    pub fn of_format(format: Format) -> Self {
        match format.colour.transfer {
            16 => Self::Pq,
            18 => Self::Hlg,
            _ => Self::Sdr,
        }
    }

    pub fn is_hdr(self) -> bool {
        matches!(self, Self::Hlg | Self::Pq)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Space {
    #[default]
    Srgb,
    /// BT.709 原色、リニア。1 は Windows と Vulkan では 80 cd/m²、Apple の EDR では SDR の白。
    ScRgb,
    Pq,
    Hlg,
}

impl Space {
    pub fn is_hdr(self) -> bool {
        self != Self::Srgb
    }
}

/// BT.2408 4 節。
pub const SDR_WHITE: f32 = 203.0;

/// BT.2100 の基準 OOTF（システムガンマ 1.2）。
pub const HLG_PEAK: f32 = 1000.0;

pub const SCRGB_WHITE: f32 = 80.0;

/// 数はシェーダーの uniform で、[`Mapping::apply`] が同じ計算の Rust 版。
/// 恒等は計算せずに通す（SDR は一ビットも変えない）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub source: Transfer,
    pub target: Space,
    /// 原色が同じなら 0、BT.2020 → BT.709 は 1、逆は 2。
    pub gamut: u32,
    /// SDR のサーフェスでは HLG の公称 1000。
    pub peak: f32,
    pub white: f32,
    /// 映像の上の UI はこの明るさで書く（Apple の EDR で他のウィンドウと同じ明るさにするため）。
    pub one: f32,
}

/// 両方を一台で試せるよう、`cfg` ではなく引数で受ける。
pub fn scrgb_one(apple: bool, white: f32) -> f32 {
    if apple { white } else { SCRGB_WHITE }
}

const APPLE: bool = cfg!(target_vendor = "apple");

impl Mapping {
    pub fn new(source: Transfer, primaries: Primaries, target: Space) -> Self {
        let wants = match target {
            Space::Srgb | Space::ScRgb => Primaries::Bt709,
            Space::Pq | Space::Hlg => Primaries::Bt2020,
        };
        let gamut = match (primaries, wants) {
            (Primaries::Bt2020, Primaries::Bt709) => 1,
            (Primaries::Bt709, Primaries::Bt2020) => 2,
            _ => 0,
        };
        Self {
            source,
            target,
            gamut,
            peak: HLG_PEAK,
            white: SDR_WHITE,
            one: scrgb_one(APPLE, SDR_WHITE),
        }
    }

    /// 白は規格ではなくディスプレイが決める。SDR のサーフェスは変えない。
    pub fn with_white(mut self, white: f32) -> Self {
        if self.target.is_hdr() && white.is_finite() && white > 0.0 {
            self.white = white;
            self.one = scrgb_one(APPLE, white);
        }
        self
    }

    /// DXGI の `MaxLuminance`。HDR のサーフェスでは上限で、越える HLG は縮める。
    pub fn with_peak(mut self, peak: f32) -> Self {
        if self.target.is_hdr() && peak.is_finite() && peak > 0.0 {
            self.peak = peak.clamp(self.white.max(100.0), 10_000.0);
        }
        self
    }

    /// HDR のサーフェスではディスプレイの SDR の白に合わせる（whatwg/html#9112）：75% の信号を白に置く。
    /// SDR のサーフェスではピーク 1000 から白へ縮め、HLG のサーフェスではそのまま。
    pub fn hlg_top(&self) -> f32 {
        match self.target {
            Space::ScRgb | Space::Pq => HLG_PEAK * self.white / SDR_WHITE,
            Space::Srgb | Space::Hlg => self.peak,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.source == Transfer::Sdr && self.target == Space::Srgb && self.gamut == 0
    }

    pub fn source_code(&self) -> u32 {
        match self.source {
            Transfer::Sdr => 0,
            Transfer::Hlg => 1,
            Transfer::Pq => 2,
            Transfer::Ui => 3,
        }
    }

    pub fn target_code(&self) -> u32 {
        match self.target {
            Space::Srgb => 0,
            Space::ScRgb => 1,
            Space::Pq => 2,
            Space::Hlg => 3,
        }
    }

    /// `encode` はサーフェスが `…Srgb` か。`shader/fit.wgsl` と一行ずつ同じ。
    pub fn apply(&self, rgb: [f32; 3], encode: bool) -> [f32; 3] {
        if self.is_identity() {
            return if encode { rgb.map(to_linear) } else { rgb };
        }
        self.encode(self.to_nits(rgb), encode)
    }

    pub fn to_nits(&self, rgb: [f32; 3]) -> [f32; 3] {
        let nits = match self.source {
            Transfer::Sdr => rgb.map(|c| to_linear(c) * self.white),
            Transfer::Hlg => match self.target {
                Space::ScRgb | Space::Pq => {
                    hlg_to_nits(rgb, HLG_PEAK).map(|c| c * self.white / SDR_WHITE)
                }
                Space::Srgb | Space::Hlg => hlg_to_nits(rgb, self.peak),
            },
            Transfer::Pq => rgb.map(pq_to_nits),
            Transfer::Ui => rgb.map(|c| to_linear(c) * self.white),
        };
        match self.gamut {
            1 => multiply(&BT2020_TO_BT709, nits),
            2 => multiply(&BT709_TO_BT2020, nits),
            _ => nits,
        }
    }

    pub fn encode(&self, mut nits: [f32; 3], encode: bool) -> [f32; 3] {
        match self.target {
            Space::Srgb => {
                if self.source.is_hdr() {
                    nits = squeeze(nits, self.peak, self.white);
                }
                let linear = nits.map(|c| (c / self.white).clamp(0.0, 1.0));
                if encode {
                    linear
                } else {
                    linear.map(to_encoded)
                }
            }
            Space::ScRgb | Space::Pq => {
                // ディスプレイの上限を越える HLG は縮める（後段でクリップされないように）。
                let top = self.hlg_top();
                if self.source == Transfer::Hlg && top > self.peak {
                    nits = squeeze(nits, top, self.peak);
                }
                if self.target == Space::ScRgb {
                    nits.map(|c| (c / self.one).max(0.0))
                } else {
                    nits.map(|c| nits_to_pq(c.max(0.0)))
                }
            }
            Space::Hlg => nits_to_hlg(nits.map(|c| c.max(0.0)), self.peak),
        }
    }

    /// sRGB のサーフェスでは符号値を取る。
    pub fn undo(&self, encoded: [f32; 3]) -> [f32; 3] {
        match self.target {
            Space::Srgb => encoded.map(|c| to_linear(c) * self.white),
            Space::ScRgb => encoded.map(|c| c * self.one),
            Space::Pq => encoded.map(pq_to_nits),
            Space::Hlg => hlg_to_nits(encoded, self.peak),
        }
    }

    /// `shader/over.wgsl` と一行ずつ同じ。PQ・HLG は符号値を混ぜると暗くなりすぎる。
    /// `paper` はストレート、`below` と戻り値はサーフェスの符号値。
    pub fn lay(&self, paper: [f32; 3], alpha: f32, below: [f32; 3], encode: bool) -> [f32; 3] {
        let over = self.to_nits(paper);
        let under = self.undo(below);
        let mut mixed = [0.0; 3];
        for (at, out) in mixed.iter_mut().enumerate() {
            *out = over[at] * alpha + under[at] * (1.0 - alpha);
        }
        self.encode(mixed, encode)
    }
}

/// リニア、D65。
pub const BT2020_TO_BT709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641, -0.072_850],
    [-0.124_55, 1.132_9, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_73],
];

pub const BT709_TO_BT2020: [[f32; 3]; 3] = [
    [0.627_404, 0.329_283, 0.043_313],
    [0.069_097, 0.919_540, 0.011_362],
    [0.016_391, 0.088_013, 0.895_595],
];

fn multiply(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// HLG, BT.2100 Table 5.
pub(crate) const HLG_A: f32 = 0.178_832_77;
pub(crate) const HLG_B: f32 = 0.284_668_92;
pub(crate) const HLG_C: f32 = 0.559_910_7;

pub fn hlg_to_scene(signal: f32) -> f32 {
    let signal = signal.clamp(0.0, 1.0);
    if signal <= 0.5 {
        signal * signal / 3.0
    } else {
        (((signal - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
    }
}

pub fn scene_to_hlg(scene: f32) -> f32 {
    let scene = scene.clamp(0.0, 1.0);
    if scene <= 1.0 / 12.0 {
        (3.0 * scene).sqrt()
    } else {
        HLG_A * (12.0 * scene - HLG_B).ln() + HLG_C
    }
}

/// BT.2100 注 5f。
pub fn hlg_gamma(peak: f32) -> f32 {
    1.2 + 0.42 * (peak / 1000.0).log10()
}

/// BT.2100 の EOTF：逆 OETF のあと OOTF。
pub fn hlg_to_nits(signal: [f32; 3], peak: f32) -> [f32; 3] {
    let scene = signal.map(hlg_to_scene);
    let luma = Primaries::Bt2020.luma();
    let y = luma[0] * scene[0] + luma[1] * scene[1] + luma[2] * scene[2];
    let gain = peak * y.max(1e-6).powf(hlg_gamma(peak) - 1.0);
    scene.map(|c| c * gain)
}

/// OOTF は表示の輝度だけから戻す（`Yd = Ys^γ`）。
pub fn nits_to_hlg(nits: [f32; 3], peak: f32) -> [f32; 3] {
    let luma = Primaries::Bt2020.luma();
    let yd = (luma[0] * nits[0] + luma[1] * nits[1] + luma[2] * nits[2]) / peak;
    if yd <= 0.0 {
        return [0.0; 3];
    }
    let gamma = hlg_gamma(peak);
    let gain = peak * yd.powf((gamma - 1.0) / gamma);
    nits.map(|c| scene_to_hlg(c / gain))
}

// PQ, SMPTE ST 2084.
pub(crate) const PQ_M1: f32 = 2610.0 / 16384.0;
pub(crate) const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
pub(crate) const PQ_C1: f32 = 3424.0 / 4096.0;
pub(crate) const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
pub(crate) const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;

pub fn pq_to_nits(signal: f32) -> f32 {
    let p = signal.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    let num = (p - PQ_C1).max(0.0);
    let den = PQ_C2 - PQ_C3 * p;
    10_000.0 * (num / den).powf(1.0 / PQ_M1)
}

pub fn nits_to_pq(nits: f32) -> f32 {
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

/// BT.2390 5.4.1。最大の成分で行い、ほかは同じ比で縮めて色相を保つ。
pub fn squeeze(nits: [f32; 3], peak: f32, white: f32) -> [f32; 3] {
    let most = nits[0].max(nits[1]).max(nits[2]);
    if most <= 0.0 {
        return nits;
    }
    let brought = squeeze_one(most, peak, white);
    nits.map(|c| c * brought / most)
}

pub fn squeeze_one(nits: f32, peak: f32, white: f32) -> f32 {
    let top = nits_to_pq(peak);
    let e1 = nits_to_pq(nits) / top;
    let max_lum = nits_to_pq(white) / top;
    let ks = 1.5 * max_lum - 0.5;
    let e2 = if e1 < ks {
        e1
    } else {
        let t = (e1 - ks) / (1.0 - ks);
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * ks
            + (t3 - 2.0 * t2 + t) * (1.0 - ks)
            + (-2.0 * t3 + 3.0 * t2) * max_lum
    };
    pq_to_nits(e2 * top)
}

/// 明るさ・コントラスト・色合い・彩度
/// 色合いは度で −180〜180、ほかは −100〜100。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Adjust {
    pub brightness: i32,
    pub contrast: i32,
    pub hue: i32,
    pub saturation: i32,
}

impl Adjust {
    pub fn clamped(self) -> Self {
        Self {
            brightness: self.brightness.clamp(-100, 100),
            contrast: self.contrast.clamp(-100, 100),
            hue: self.hue.clamp(-180, 180),
            saturation: self.saturation.clamp(-100, 100),
        }
    }

    pub fn is_identity(self) -> bool {
        self.clamped() == Self::default()
    }
}

/// オフセットと利得は保存幅（255、1023）で正規化した値に対して出す。
/// `luma_lift` と `chroma_cos`・`chroma_sin` は [`Adjust`] の分（掛けなければ 0・1・0）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conversion {
    pub luma_offset: f32,
    pub luma_gain: f32,
    pub chroma_offset: f32,
    pub chroma_gain: f32,
    pub kr: f32,
    pub kb: f32,
    pub luma_lift: f32,
    pub chroma_cos: f32,
    pub chroma_sin: f32,
}

impl Conversion {
    pub fn new(matrix: Matrix, depth: u8, limited: bool) -> Self {
        let depth = depth.clamp(8, 16);
        let full = ((1u32 << depth) - 1) as f32;
        // 10 ビットの黒 64・白 940 は 8 ビットの 16・235 をずらした物。
        let step = (1u32 << (depth - 8)) as f32;
        let (kr, kb) = matrix.weights();
        if limited {
            Self {
                luma_offset: 16.0 * step / full,
                luma_gain: full / (219.0 * step),
                chroma_offset: 128.0 * step / full,
                chroma_gain: full / (224.0 * step),
                kr,
                kb,
                luma_lift: 0.0,
                chroma_cos: 1.0,
                chroma_sin: 0.0,
            }
        } else {
            Self {
                luma_offset: 0.0,
                luma_gain: 1.0,
                chroma_offset: 128.0 * step / full,
                chroma_gain: full / (255.0 * step),
                kr,
                kb,
                luma_lift: 0.0,
                chroma_cos: 1.0,
                chroma_sin: 0.0,
            }
        }
    }

    /// 明るさは 8 ビットの符号値で `brightness × 255 / 100` を足し、コントラストは黒からの幅を
    /// `1 + contrast / 100` 倍にする。色差は色合いの角度だけ回して `1 + saturation / 100` 倍にする。
    pub fn adjusted(self, adjust: Adjust, depth: u8) -> Self {
        let adjust = adjust.clamped();
        let depth = depth.clamp(8, 16);
        let full = ((1u32 << depth) - 1) as f32;
        let step = (1u32 << (depth - 8)) as f32;
        let contrast = 1.0 + adjust.contrast as f32 / 100.0;
        let saturation = 1.0 + adjust.saturation as f32 / 100.0;
        let (sin, cos) = (adjust.hue as f32).to_radians().sin_cos();
        Self {
            luma_lift: adjust.brightness as f32 / 100.0 * 255.0 * step / full * self.luma_gain,
            luma_gain: self.luma_gain * contrast,
            chroma_cos: saturation * cos,
            chroma_sin: saturation * sin,
            ..self
        }
    }

    /// 調整を RGB に掛ける形（行ごとに 3 つの係数と足す値）。ドライバーが RGB にして返すフレーム用。
    pub fn rgb_rows(&self, base: &Self) -> [[f32; 4]; 3] {
        let kg = 1.0 - self.kr - self.kb;
        let to_rgb = [
            [1.0, 0.0, 2.0 * (1.0 - self.kr)],
            [
                1.0,
                -2.0 * (1.0 - self.kb) * self.kb / kg,
                -2.0 * (1.0 - self.kr) * self.kr / kg,
            ],
            [1.0, 2.0 * (1.0 - self.kb), 0.0],
        ];
        let contrast = self.luma_gain / base.luma_gain;
        let adjust = [
            [contrast, 0.0, 0.0],
            [0.0, self.chroma_cos, self.chroma_sin],
            [0.0, -self.chroma_sin, self.chroma_cos],
        ];
        let whole = product(&product(&to_rgb, &adjust), &inverse(&to_rgb));
        whole.map(|row| [row[0], row[1], row[2], self.luma_lift])
    }

    /// フラグメントシェーダーと同じ計算。
    pub fn to_rgb(&self, y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let luma = (y - self.luma_offset) * self.luma_gain + self.luma_lift;
        let cb = (cb - self.chroma_offset) * self.chroma_gain;
        let cr = (cr - self.chroma_offset) * self.chroma_gain;
        let u = cb * self.chroma_cos + cr * self.chroma_sin;
        let v = cr * self.chroma_cos - cb * self.chroma_sin;
        let kg = 1.0 - self.kr - self.kb;
        let rgb = [
            luma + 2.0 * (1.0 - self.kr) * v,
            luma - (2.0 * (1.0 - self.kb) * self.kb / kg) * u
                - (2.0 * (1.0 - self.kr) * self.kr / kg) * v,
            luma + 2.0 * (1.0 - self.kb) * u,
        ];
        rgb.map(|c| c.clamp(0.0, 1.0))
    }
}

fn product(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn inverse(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let cofactor = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let determinant: f32 = (0..3).map(|j| m[0][j] * cofactor(0, j)).sum();
    std::array::from_fn(|i| std::array::from_fn(|j| cofactor(j, i) / determinant))
}

pub fn quantise(value: f32) -> u8 {
    (value * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// テスト用（GPU は `shader/srgb.wgsl`）。
pub fn to_linear(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

pub fn to_encoded(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_the_gamma_off_and_putting_it_back_gives_the_same_code() {
        for code in 0..=255u8 {
            let encoded = f32::from(code) / 255.0;
            assert_eq!(quantise(to_encoded(to_linear(encoded))), code);
        }
    }

    /// 八ビットにリニアを入れると暗部が潰れる。中間のサーフェスをリニアにする「単純化」を防ぐ。
    #[test]
    fn eight_bits_of_linear_light_would_lose_the_shadows() {
        let levels = |from: u8, until: u8| {
            let mut seen: Vec<u8> = (from..until)
                .map(|code| quantise(to_linear(f32::from(code) / 255.0)))
                .collect();
            seen.dedup();
            seen.len()
        };
        assert_eq!(levels(16, 64), 13);
        let mut all: Vec<u8> = (0..=255u8)
            .map(|code| quantise(to_linear(f32::from(code) / 255.0)))
            .collect();
        all.dedup();
        assert_eq!(all.len(), 183);
    }

    #[test]
    fn limited_range_black_and_white_come_out_black_and_white() {
        for depth in [8, 10] {
            let step = (1u32 << (depth - 8)) as f32;
            let full = ((1u32 << depth) - 1) as f32;
            let conversion = Conversion::new(Matrix::Bt709, depth, true);
            let black = conversion.to_rgb(16.0 * step / full, 0.5, 0.5);
            let white = conversion.to_rgb(
                235.0 * step / full,
                128.0 * step / full,
                128.0 * step / full,
            );
            assert_eq!(black.map(quantise), [0, 0, 0], "depth {depth}");
            assert_eq!(white.map(quantise), [255, 255, 255], "depth {depth}");
        }
    }

    #[test]
    fn the_weights_give_the_usual_bt709_coefficients() {
        let conversion = Conversion::new(Matrix::Bt709, 8, true);
        let kg = 1.0 - conversion.kr - conversion.kb;
        let near = |a: f32, b: f32| (a - b).abs() < 5e-5;
        assert!(near(2.0 * (1.0 - conversion.kr), 1.5748));
        assert!(near(2.0 * (1.0 - conversion.kb), 1.8556));
        assert!(near(
            2.0 * (1.0 - conversion.kb) * conversion.kb / kg,
            0.1873
        ));
        assert!(near(
            2.0 * (1.0 - conversion.kr) * conversion.kr / kg,
            0.4681
        ));
    }

    #[test]
    fn four_k_is_read_with_bt2020_and_high_definition_is_not() {
        let hd = Format {
            width: 1920,
            height: 1080,
            ..Format::default()
        };
        let uhd = Format {
            width: 3840,
            height: 2160,
            ..Format::default()
        };
        assert_eq!(Matrix::of_format(hd), Matrix::Bt709);
        assert_eq!(Matrix::of_format(uhd), Matrix::Bt2020);
        assert_eq!(Primaries::of_format(hd), Primaries::Bt709);
        assert_eq!(Primaries::of_format(uhd), Primaries::Bt2020);
        assert_eq!(Transfer::of_format(uhd), Transfer::Sdr);
    }

    #[test]
    fn what_the_stream_declares_is_what_it_is_read_as() {
        let bs4k = Format {
            width: 3840,
            height: 2160,
            depth: 10,
            colour: mediadecoder::es::Colour {
                primaries: 9,
                transfer: 18,
                matrix: 9,
                full_range: false,
            },
            ..Format::default()
        };
        assert_eq!(Transfer::of_format(bs4k), Transfer::Hlg);
        assert_eq!(Primaries::of_format(bs4k), Primaries::Bt2020);
        assert_eq!(Matrix::of_format(bs4k), Matrix::Bt2020);
        let odd = Format {
            colour: mediadecoder::es::Colour {
                primaries: 1,
                transfer: 1,
                matrix: 1,
                full_range: false,
            },
            ..bs4k
        };
        assert_eq!(Matrix::of_format(odd), Matrix::Bt709);
        assert_eq!(Primaries::of_format(odd), Primaries::Bt709);
        let pq = Format {
            colour: mediadecoder::es::Colour {
                transfer: 16,
                ..bs4k.colour
            },
            ..bs4k
        };
        assert_eq!(Transfer::of_format(pq), Transfer::Pq);
    }

    fn near(a: f32, b: f32, within: f32) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn the_hdr_curves_are_inverses_of_each_other() {
        for code in 0..=1023u32 {
            let signal = code as f32 / 1023.0;
            assert!(
                near(scene_to_hlg(hlg_to_scene(signal)), signal, 1e-5),
                "HLG at {signal}"
            );
            assert!(
                near(nits_to_pq(pq_to_nits(signal)), signal, 1e-4),
                "PQ at {signal}"
            );
        }
        let pixel = [0.7, 0.4, 0.2];
        let back = nits_to_hlg(hlg_to_nits(pixel, HLG_PEAK), HLG_PEAK);
        for (a, b) in back.iter().zip(&pixel) {
            assert!(near(*a, *b, 1e-4), "{back:?} against {pixel:?}");
        }
    }

    #[test]
    fn the_curves_land_where_the_recommendations_say() {
        let grey = hlg_to_nits([0.75; 3], HLG_PEAK);
        assert!(near(grey[1], SDR_WHITE, 1.0), "{grey:?}");
        assert!(near(nits_to_pq(SDR_WHITE), 0.5807, 0.001));
        assert!(near(pq_to_nits(1.0), 10_000.0, 1.0));
        assert_eq!(pq_to_nits(0.0), 0.0);
        assert!(near(hlg_gamma(1000.0), 1.2, 1e-6));
        assert!(near(
            hlg_gamma(2000.0),
            1.2 + 0.42 * std::f32::consts::LOG10_2,
            1e-5
        ));
    }

    #[test]
    fn the_gamut_matrices_are_inverses_and_keep_white() {
        let white = [1.0, 1.0, 1.0];
        let across = multiply(&BT709_TO_BT2020, white);
        for c in across {
            assert!(near(c, 1.0, 1e-4), "{across:?}");
        }
        let there_and_back = multiply(
            &BT2020_TO_BT709,
            multiply(&BT709_TO_BT2020, [0.8, 0.3, 0.1]),
        );
        for (a, b) in there_and_back.iter().zip(&[0.8, 0.3, 0.1]) {
            assert!(near(*a, *b, 1e-4), "{there_and_back:?}");
        }
        let red = multiply(&BT2020_TO_BT709, [1.0, 0.0, 0.0]);
        assert!(red[0] > 1.0 && red[1] < 0.0 && red[2] < 0.0, "{red:?}");
    }

    #[test]
    fn sdr_white_follows_the_display_on_an_hdr_surface() {
        let sheet = Mapping::new(Transfer::Ui, Primaries::Bt709, Space::Pq);
        let at_203 = sheet.apply([1.0, 1.0, 1.0], false)[0];
        let at_100 = sheet.with_white(100.0).apply([1.0, 1.0, 1.0], false)[0];
        assert!(near(at_203, nits_to_pq(SDR_WHITE), 1e-5));
        assert!(near(at_100, nits_to_pq(100.0), 1e-5));
        assert!(at_100 < at_203, "a lower white is a darker screen");
        let sdr = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Srgb);
        assert_eq!(sdr.with_white(100.0).white, sdr.white);
        let hlg = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Pq);
        assert!(hlg.with_white(100.0).apply([0.75; 3], false)[0] < hlg.apply([0.75; 3], false)[0]);
        assert_eq!(sheet.with_white(0.0).white, SDR_WHITE);
        assert_eq!(sheet.with_white(f32::NAN).white, SDR_WHITE);
    }

    #[test]
    fn hlg_white_sits_on_sdr_white() {
        let grey = [0.75; 3];
        for space in [Space::Pq, Space::ScRgb] {
            let reference = Mapping::new(Transfer::Hlg, Primaries::Bt2020, space);
            assert!(near(reference.hlg_top(), HLG_PEAK, 1e-3));
            let windows = reference.with_white(80.0);
            assert!(near(windows.hlg_top(), 80.0 * HLG_PEAK / SDR_WHITE, 1e-3));
            let nits = |mapping: &Mapping| -> f32 {
                let out = mapping.apply(grey, false)[0];
                match space {
                    Space::Pq => pq_to_nits(out),
                    _ => out * mapping.one,
                }
            };
            assert!(near(nits(&reference), SDR_WHITE, 1.0), "{space:?}");
            assert!(near(nits(&windows), 80.0, 0.5), "{space:?}");
            let top = |mapping: &Mapping| -> f32 {
                let out = mapping.apply([1.0; 3], false)[0];
                match space {
                    Space::Pq => pq_to_nits(out),
                    _ => out * mapping.one,
                }
            };
            assert!(near(top(&windows), windows.hlg_top(), 2.0), "{space:?}");
        }
        let sdr = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Srgb);
        assert_eq!(sdr.with_white(80.0).hlg_top(), HLG_PEAK);
        let hlg = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Hlg).with_white(80.0);
        let pixel = [0.7, 0.4, 0.2];
        for (a, b) in hlg.apply(pixel, false).iter().zip(&pixel) {
            assert!(near(*a, *b, 1e-4));
        }
    }

    #[test]
    fn the_display_peak_caps_an_hlg_picture() {
        let low = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Pq).with_peak(400.0);
        let out = pq_to_nits(low.apply([1.0; 3], false)[0]);
        assert!(near(out, 400.0, 1.0), "{out}");
        let dim = pq_to_nits(low.apply([0.5; 3], false)[0]);
        let free = pq_to_nits(
            Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Pq).apply([0.5; 3], false)[0],
        );
        assert!(near(dim, free, 0.5), "{dim} against {free}");
        let fits = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Pq)
            .with_white(80.0)
            .with_peak(400.0);
        let out = pq_to_nits(fits.apply([1.0; 3], false)[0]);
        assert!(near(out, fits.hlg_top(), 2.0), "{out}");
    }

    #[test]
    fn a_sheet_is_laid_in_light_not_in_code() {
        let pq = Mapping::new(Transfer::Ui, Primaries::Bt709, Space::Pq);
        let picture = [nits_to_pq(200.0); 3];
        let mixed = pq.lay([0.0; 3], 0.6, picture, false);
        assert!(near(pq_to_nits(mixed[0]), 80.0, 0.5), "{mixed:?}");
        let in_code = 0.4 * nits_to_pq(200.0);
        assert!(pq_to_nits(in_code) < 30.0, "mixed in code it is far darker");
        assert_eq!(
            pq.lay([1.0; 3], 1.0, picture, false),
            pq.apply([1.0; 3], false)
        );
        for (a, b) in pq.lay([1.0; 3], 0.0, picture, false).iter().zip(&picture) {
            assert!(near(*a, *b, 1e-4));
        }
        for space in [Space::Srgb, Space::ScRgb, Space::Pq, Space::Hlg] {
            let mapping = Mapping::new(Transfer::Ui, Primaries::Bt709, space);
            let nits = [150.0, 90.0, 30.0];
            let back = mapping.undo(mapping.encode(nits, false));
            for (a, b) in back.iter().zip(&nits) {
                assert!(near(*a, *b, 0.2), "{space:?}: {back:?}");
            }
        }
        let hlg = Mapping::new(Transfer::Ui, Primaries::Bt709, Space::Hlg);
        let picture = nits_to_hlg([200.0; 3], HLG_PEAK);
        let mixed = hlg.lay([0.0; 3], 0.6, picture, false);
        assert!(
            near(hlg_to_nits(mixed, HLG_PEAK)[0], 80.0, 0.5),
            "{mixed:?}"
        );
    }

    #[test]
    fn scrgb_one_is_sdr_white_on_apple_and_eighty_elsewhere() {
        assert_eq!(scrgb_one(true, 203.0), 203.0);
        assert_eq!(scrgb_one(true, 100.0), 100.0);
        assert_eq!(scrgb_one(false, 203.0), SCRGB_WHITE);
        let mut apple = Mapping::new(Transfer::Ui, Primaries::Bt709, Space::ScRgb);
        apple.one = scrgb_one(true, apple.white);
        assert!(near(apple.apply([1.0; 3], false)[0], 1.0, 1e-6));
        let mut windows = apple;
        windows.one = scrgb_one(false, windows.white);
        assert!(near(
            windows.apply([1.0; 3], false)[0],
            SDR_WHITE / SCRGB_WHITE,
            1e-4
        ));
    }

    #[test]
    fn the_squeeze_leaves_the_low_end_alone_and_brings_the_peak_to_white() {
        assert!(near(squeeze_one(50.0, HLG_PEAK, SDR_WHITE), 50.0, 0.5));
        assert!(near(
            squeeze_one(HLG_PEAK, HLG_PEAK, SDR_WHITE),
            SDR_WHITE,
            0.5
        ));
        let mut last = 0.0;
        for step in 0..=1000 {
            let out = squeeze_one(step as f32, HLG_PEAK, SDR_WHITE);
            assert!(out >= last - 1e-3, "went backwards at {step}");
            assert!(out <= SDR_WHITE + 0.5, "over white at {step}");
            last = out;
        }
        let pixel = [900.0, 450.0, 90.0];
        let brought = squeeze(pixel, HLG_PEAK, SDR_WHITE);
        assert!(near(brought[1] / brought[0], 0.5, 1e-3), "{brought:?}");
        assert!(near(brought[2] / brought[0], 0.1, 1e-3), "{brought:?}");
    }

    #[test]
    fn an_sdr_picture_on_an_sdr_surface_is_left_alone() {
        let plain = Mapping::new(Transfer::Sdr, Primaries::Bt709, Space::Srgb);
        assert!(plain.is_identity());
        let pixel = [0.3, 0.6, 0.9];
        assert_eq!(plain.apply(pixel, false), pixel);
        assert_eq!(plain.apply(pixel, true), pixel.map(to_linear));
        let wide = Mapping::new(Transfer::Sdr, Primaries::Bt2020, Space::Srgb);
        assert!(!wide.is_identity());
        assert_eq!(wide.gamut, 1);
        assert!(!Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Srgb).is_identity());
        let same = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Hlg);
        assert_eq!(same.gamut, 0);
        let back = same.apply(pixel, false);
        for (a, b) in back.iter().zip(&pixel) {
            assert!(near(*a, *b, 1e-4), "{back:?}");
        }
    }

    #[test]
    fn hlg_white_on_an_sdr_display_is_nearly_white() {
        let onto_sdr = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Srgb);
        let white = onto_sdr.apply([0.75; 3], false);
        assert!(near(white[0], 0.898, 0.005), "{white:?}");
        assert!(near(white[0], white[1], 1e-4) && near(white[1], white[2], 1e-4));
        assert_eq!(onto_sdr.apply([1.0; 3], false).map(quantise), [255; 3]);
        assert_eq!(onto_sdr.apply([0.0; 3], false).map(quantise), [0; 3]);
        let linear = onto_sdr.apply([0.75; 3], true);
        assert!(near(to_encoded(linear[0]), white[0], 1e-4));
    }

    #[test]
    fn sdr_white_on_an_hdr_surface_sits_at_the_reference_white() {
        let onto_pq = Mapping::new(Transfer::Sdr, Primaries::Bt709, Space::Pq);
        assert_eq!(onto_pq.gamut, 2);
        let white = onto_pq.apply([1.0; 3], false);
        assert!(near(white[0], 0.5807, 0.002), "{white:?}");
        let onto_scrgb = Mapping::new(Transfer::Sdr, Primaries::Bt709, Space::ScRgb);
        let white = onto_scrgb.apply([1.0; 3], false);
        assert!(
            near(white[0], SDR_WHITE / scrgb_one(APPLE, SDR_WHITE), 1e-3),
            "{white:?}"
        );
        let hlg_onto_pq = Mapping::new(Transfer::Hlg, Primaries::Bt2020, Space::Pq);
        let white = hlg_onto_pq.apply([0.75; 3], false);
        assert!(near(white[0], 0.5807, 0.003), "{white:?}");
    }

    #[test]
    fn no_adjustment_leaves_the_conversion_as_it_was() {
        for depth in [8, 10] {
            let plain = Conversion::new(Matrix::Bt709, depth, true);
            assert_eq!(plain.adjusted(Adjust::default(), depth), plain);
        }
    }

    #[test]
    fn brightness_and_contrast_follow_the_formula() {
        // y = ((c × (Y − 16)) >> 9) + b + 16、c = contrast × 512 / 100 + 512、b = brightness × 255 / 100。
        for depth in [8u8, 10] {
            let step = (1u32 << (depth - 8)) as f32;
            let full = ((1u32 << depth) - 1) as f32;
            let code = |value: f32| value * step / full;
            let base = Conversion::new(Matrix::Bt709, depth, true);
            let adjust = Adjust {
                brightness: 20,
                contrast: -50,
                ..Adjust::default()
            };
            let adjusted = base.adjusted(adjust, depth);
            let y = 126.0;
            let expected = 0.5 * (y - 16.0) + 20.0 * 255.0 / 100.0 + 16.0;
            let grey = adjusted.to_rgb(code(y), code(128.0), code(128.0));
            let want = base.to_rgb(code(expected), code(128.0), code(128.0));
            for (got, want) in grey.iter().zip(want) {
                assert!((got - want).abs() < 1e-4, "depth {depth}: {grey:?} {want}");
            }
        }
    }

    #[test]
    fn half_a_turn_of_hue_swaps_the_colour_for_its_opposite() {
        let base = Conversion::new(Matrix::Bt709, 8, true);
        let turned = base.adjusted(
            Adjust {
                hue: 180,
                ..Adjust::default()
            },
            8,
        );
        let code = |value: f32| value / 255.0;
        let red = turned.to_rgb(code(81.0), code(90.0), code(240.0));
        let opposite = base.to_rgb(code(81.0), code(166.0), code(16.0));
        for (got, want) in red.iter().zip(opposite) {
            assert!((got - want).abs() < 2e-3, "{red:?} {opposite:?}");
        }
    }

    #[test]
    fn no_saturation_leaves_only_grey() {
        let base = Conversion::new(Matrix::Bt709, 8, true);
        let grey = base.adjusted(
            Adjust {
                saturation: -100,
                ..Adjust::default()
            },
            8,
        );
        let rgb = grey.to_rgb(81.0 / 255.0, 90.0 / 255.0, 240.0 / 255.0);
        assert!(
            (rgb[0] - rgb[1]).abs() < 1e-5 && (rgb[1] - rgb[2]).abs() < 1e-5,
            "{rgb:?}"
        );
    }

    #[test]
    fn the_rows_for_rgb_give_what_the_adjustment_gives_on_ycbcr() {
        let base = Conversion::new(Matrix::Bt2020, 10, true);
        let adjust = Adjust {
            brightness: -15,
            contrast: 30,
            hue: 40,
            saturation: 25,
        };
        let adjusted = base.adjusted(adjust, 10);
        let rows = adjusted.rgb_rows(&base);
        for (y, cb, cr) in [(0.3, 0.45, 0.6), (0.5, 0.5, 0.5), (0.7, 0.55, 0.42)] {
            let before = base.to_rgb(y, cb, cr);
            let after = adjusted.to_rgb(y, cb, cr);
            let through = rows.map(|row| {
                (row[0] * before[0] + row[1] * before[1] + row[2] * before[2] + row[3])
                    .clamp(0.0, 1.0)
            });
            for (got, want) in through.iter().zip(after) {
                assert!((got - want).abs() < 1e-4, "{through:?} {after:?}");
            }
        }
    }

    #[test]
    fn values_past_the_ends_are_held_to_them() {
        let wild = Adjust {
            brightness: 300,
            contrast: -300,
            hue: 400,
            saturation: 101,
        };
        assert_eq!(
            wild.clamped(),
            Adjust {
                brightness: 100,
                contrast: -100,
                hue: 180,
                saturation: 100,
            }
        );
    }
}
