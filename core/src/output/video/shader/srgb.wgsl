// sRGB の伝達関数の逆（エンコードされた値をリニアへ）。`Bgra8UnormSrgb` のような
// サーフェスが書き込みに掛ける変換の裏返しで、掛けてから書けばサーフェスが掛け直して
// 元の符号値に戻る。
fn to_linear(c: f32) -> f32 {
    if (c <= 0.04045) { return c / 12.92; }
    return pow((c + 0.055) / 1.055, 2.4);
}

// その順方向の変換。`…Unorm` のサーフェスへ、リニアから作った値を書くときに掛ける。
fn to_encoded(c: f32) -> f32 {
    if (c <= 0.0031308) { return c * 12.92; }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// ---------------------------------------------------------------------------
// BT.2100。Rust の `colour.rs` と一行ずつ同じで、`tests/video.rs` が二つを突き合わせる。
// 数を変えるなら両方。
// ---------------------------------------------------------------------------

// ハイブリッドログガンマ（ARIB STD-B67 / BT.2100 Table 5）。
const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;

// BT.2020 の輝度の重み。HLG の OOTF はこれで明るさを取ります。
const BT2020_LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

// 信号からシーン光（0..1）へ。OETF の逆。
fn hlg_to_scene(signal: f32) -> f32 {
    let s = clamp(signal, 0.0, 1.0);
    if (s <= 0.5) { return s * s / 3.0; }
    return (exp((s - HLG_C) / HLG_A) + HLG_B) / 12.0;
}

// シーン光（0..1）から信号へ。OETF。
fn scene_to_hlg(scene: f32) -> f32 {
    let e = clamp(scene, 0.0, 1.0);
    if (e <= 1.0 / 12.0) { return sqrt(3.0 * e); }
    return HLG_A * log(12.0 * e - HLG_B) + HLG_C;
}

// 画面の最大輝度に対するシステムガンマ。1000 cd/m² で 1.2。
fn hlg_gamma(peak: f32) -> f32 {
    return 1.2 + 0.42 * log(peak / 1000.0) / log(10.0);
}

// 信号から表示光（cd/m²）へ。OETF の逆に OOTF を掛けたもので、三つの
// 成分をシーンの輝度の（γ-1）乗でまとめて伸ばします。
fn hlg_to_nits(signal: vec3<f32>, peak: f32) -> vec3<f32> {
    let scene = vec3<f32>(hlg_to_scene(signal.r), hlg_to_scene(signal.g), hlg_to_scene(signal.b));
    let y = dot(BT2020_LUMA, scene);
    let gain = peak * pow(max(y, 1e-6), hlg_gamma(peak) - 1.0);
    return scene * gain;
}

// 表示光（cd/m²）から信号へ。`hlg_to_nits` の逆で、OOTF は表示の輝度
// だけから外せます（Yd = Ys^γ）。
fn nits_to_hlg(nits: vec3<f32>, peak: f32) -> vec3<f32> {
    let yd = dot(BT2020_LUMA, nits) / peak;
    if (yd <= 0.0) { return vec3<f32>(0.0); }
    let gamma = hlg_gamma(peak);
    let gain = peak * pow(yd, (gamma - 1.0) / gamma);
    let scene = nits / gain;
    return vec3<f32>(scene_to_hlg(scene.r), scene_to_hlg(scene.g), scene_to_hlg(scene.b));
}

// 知覚量子化（SMPTE ST 2084）。定数は規格の分数のまま。
const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;

fn pq_to_nits(signal: f32) -> f32 {
    let p = pow(clamp(signal, 0.0, 1.0), 1.0 / PQ_M2);
    let num = max(p - PQ_C1, 0.0);
    let den = PQ_C2 - PQ_C3 * p;
    return 10000.0 * pow(num / den, 1.0 / PQ_M1);
}

fn nits_to_pq(nits: f32) -> f32 {
    let y = pow(clamp(nits / 10000.0, 0.0, 1.0), PQ_M1);
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
}

// BT.2390 5.4.1 の EETF。0..peak を PQ の領域で 0..white へ、膝から上を
// エルミート曲線で押し込みます。一つの輝度に対して。
fn squeeze_one(nits: f32, peak: f32, white: f32) -> f32 {
    let top = nits_to_pq(peak);
    let e1 = nits_to_pq(nits) / top;
    let max_lum = nits_to_pq(white) / top;
    let ks = 1.5 * max_lum - 0.5;
    var e2 = e1;
    if (e1 >= ks) {
        let t = (e1 - ks) / (1.0 - ks);
        let t2 = t * t;
        let t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * ks
           + (t3 - 2.0 * t2 + t) * (1.0 - ks)
           + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    return pq_to_nits(e2 * top);
}

// 三成分のうち一番大きいものに掛けて、残りをそれに合わせて縮めます。
// 色相は保たれ、範囲に収まっていた成分が押し出されることもありません。
fn squeeze(nits: vec3<f32>, peak: f32, white: f32) -> vec3<f32> {
    let most = max(nits.r, max(nits.g, nits.b));
    if (most <= 0.0) { return nits; }
    return nits * (squeeze_one(most, peak, white) / most);
}

// 原色の変換（リニア光、どちらも D65）。`colour.rs` と同じ数。
const BT2020_TO_BT709: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(1.660491, -0.124550, -0.018151),
    vec3<f32>(-0.587641, 1.132900, -0.100579),
    vec3<f32>(-0.072850, -0.008349, 1.118730),
);
const BT709_TO_BT2020: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.627404, 0.069097, 0.016391),
    vec3<f32>(0.329283, 0.919540, 0.088013),
    vec3<f32>(0.043313, 0.011362, 0.895595),
);
