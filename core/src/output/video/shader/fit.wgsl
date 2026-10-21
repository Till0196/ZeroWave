struct Fit {
    // Where the picture sits in the target, in clip coordinates.
    rect: vec4<f32>,
    // 書いた値をこのサーフェスが sRGB エンコードするか（`…Srgb` のサーフェス）。するなら
    // 映像に掛かっているガンマをここで外す。外した直後にサーフェスが掛け直すので往復は
    // ぴったり戻り、8 ビットに入るのはエンコードされた値のまま。しないサーフェス（`…Unorm`）
    // へはガンマを掛けたまま素通しする。
    srgb: u32,
    // フレームに掛かっている伝達特性。0 が SDR、1 が HLG、2 が PQ、3 がリニア
    // （`colour::Transfer`）。
    source: u32,
    // サーフェスが受け取る色空間。0 が sRGB、1 が scRGB、2 が PQ、3 が HLG（`colour::Space`）。
    space: u32,
    // 原色の乗り換え。0 は無し、1 は BT.2020 から BT.709、2 はその逆。
    gamut: u32,
    // 画面のピーク（cd/m²）。HDR のサーフェスではディスプレイのもの、SDR のサーフェスでは
    // HLG のフレームの公称値 1000。
    peak: f32,
    // SDR の白の輝度（cd/m²）。BT.2408 は 203。
    white: f32,
    // scRGB のサーフェスで 1.0 が何 cd/m² か。Windows は 80、Apple は SDR の白。
    one: f32,
    // 拡大した値を、まわりの元の 2×2 の画素の幅に収めるか（リンギングの抑制、
    // `Scaling::antiring`）。拡大と縮小のしかたはパイプラインの定数
    // （`UP`・`DOWN`）。
    antiring: u32,
};

@group(0) @binding(2) var<uniform> fit: Fit;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let corner = corners[index];
    var out: VsOut;
    out.position = vec4<f32>(
        mix(fit.rect.x, fit.rect.z, corner.x),
        mix(fit.rect.y, fit.rect.w, corner.y),
        0.0,
        1.0,
    );
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

// HLG のフレームのピークが何 cd/m² か（`Mapping::hlg_top`）。scRGB と PQ のサーフェスでは
// SDR の白に相対（whatwg/html#9112）で、1000 の参照ディスプレイで外して 75% の信号（203）を白の高さへ寄せるので、
// ピークは white × 1000/203。sRGB と HLG のサーフェスでは peak そのもの。
fn hlg_top() -> f32 {
    if (fit.space == 1u || fit.space == 2u) { return 1000.0 * fit.white / 203.0; }
    return fit.peak;
}

// HLG のフレームを cd/m² へ。
fn hlg_placed(taken: vec3<f32>) -> vec3<f32> {
    if (fit.space == 1u || fit.space == 2u) {
        return hlg_to_nits(taken, 1000.0) * (fit.white / 203.0);
    }
    return hlg_to_nits(taken, fit.peak);
}

// フレームの伝達特性と原色から、サーフェスの伝達特性と原色へ。
//
// `colour::Mapping::apply` と一行ずつ同じ。処理はいつも三段で、曲線を外して cd/m² へ、
// 原色が違えば変換し（`to_nits`）、サーフェスの曲線を掛ける（`encode`）。SDR のフレームを
// SDR のサーフェスへ出すときだけ何もしない（`fs` の先頭）。
fn mapped(taken: vec3<f32>) -> vec3<f32> {
    return encode(to_nits(taken));
}

// 一段目と二段目: フレームの値を、サーフェスの原色の cd/m² へ（`Mapping::to_nits`）。
fn to_nits(taken: vec3<f32>) -> vec3<f32> {
    // 1. cd/m² へ、フレームの原色のまま。
    var nits: vec3<f32>;
    switch (fit.source) {
        case 1u: { nits = hlg_placed(taken); }
        case 2u: { nits = vec3<f32>(pq_to_nits(taken.r), pq_to_nits(taken.g), pq_to_nits(taken.b)); }
        // オーバーレイ（`gpu::Sheet`）は sRGB の曲線のままレンダリングしてある。
        case 3u: { nits = vec3<f32>(to_linear(taken.r), to_linear(taken.g), to_linear(taken.b)) * fit.white; }
        default: { nits = vec3<f32>(to_linear(taken.r), to_linear(taken.g), to_linear(taken.b)) * fit.white; }
    }
    // 2. 原色の変換。
    if (fit.gamut == 1u) { nits = BT2020_TO_BT709 * nits; }
    if (fit.gamut == 2u) { nits = BT709_TO_BT2020 * nits; }
    return nits;
}

// 三段目: サーフェスの原色の cd/m² を、サーフェスの曲線へ（`Mapping::encode`）。
fn encode(given: vec3<f32>) -> vec3<f32> {
    var nits = given;
    switch (fit.space) {
        case 1u, 2u: {
            // 上限はディスプレイのピーク。HLG のフレームのピークがその上なら寄せる。
            let top = hlg_top();
            if (fit.source == 1u && top > fit.peak) { nits = squeeze(nits, top, fit.peak); }
            if (fit.space == 1u) { return max(nits / fit.one, vec3<f32>(0.0)); }
            let n = max(nits, vec3<f32>(0.0));
            return vec3<f32>(nits_to_pq(n.r), nits_to_pq(n.g), nits_to_pq(n.b));
        }
        case 3u: { return nits_to_hlg(max(nits, vec3<f32>(0.0)), fit.peak); }
        default: {
            if (fit.source != 0u) { nits = squeeze(nits, fit.peak, fit.white); }
            let linear = clamp(nits / fit.white, vec3<f32>(0.0), vec3<f32>(1.0));
            if (fit.srgb != 0u) { return linear; }
            return vec3<f32>(to_encoded(linear.r), to_encoded(linear.g), to_encoded(linear.b));
        }
    }
}

// サーフェスが持っている符号値を、サーフェスの原色の cd/m² へ戻す（`Mapping::undo`）。
fn undo(encoded: vec3<f32>) -> vec3<f32> {
    switch (fit.space) {
        case 1u: { return encoded * fit.one; }
        case 2u: { return vec3<f32>(pq_to_nits(encoded.r), pq_to_nits(encoded.g), pq_to_nits(encoded.b)); }
        case 3u: { return hlg_to_nits(encoded, fit.peak); }
        default: { return vec3<f32>(to_linear(encoded.r), to_linear(encoded.g), to_linear(encoded.b)) * fit.white; }
    }
}

// 求めた色をサーフェスへ書く値にする。
fn finish(taken: vec4<f32>) -> vec4<f32> {
    // SDR のフレームを SDR のサーフェスへ。書く値を一バイトも変えないパス。
    if (fit.source == 0u && fit.space == 0u && fit.gamut == 0u) {
        if (fit.srgb == 0u) {
            return taken;
        }
        return vec4<f32>(to_linear(taken.r), to_linear(taken.g), to_linear(taken.b), taken.a);
    }
    // 映像のアルファは常に 1。1 でないのは重ねるオーバーレイ（`gpu::Sheet`）で、
    // そちらは乗算済みなので、曲線を掛ける前に割り戻す。
    var rgb = taken.rgb;
    if (taken.a > 0.0 && taken.a < 1.0) {
        rgb = rgb / taken.a;
    }
    return vec4<f32>(mapped(rgb), taken.a);
}

