// Intel VME の入力（`opencl.rs`）：チェーンのフレームの輝度（BT.709 の重み、符号値のまま）を、
// OpenCL のイメージと同じ並び（一画素 8 ビット、行の長さ = 幅）でバッファへ直接書く。
// テクスチャをバッファへコピーする方法は遅い。
//
// 一つの呼び出しが横に並んだ 4 画素を 1 ワードへ詰める。幅（16 の倍数）がフレームより広い
// 余りは、フレームの端の値を伸ばす（端で探索しやすく）。

struct Params {
    // OpenCL のイメージの大きさ（画素）と、そのレベルのイメージの大きさ（画素）。
    size: vec2<u32>,
    picture: vec2<u32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(16, 4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let words = params.size.x / 4u;
    if (id.x >= words || id.y >= params.size.y) {
        return;
    }
    let y = min(id.y, params.picture.y - 1u);
    var word = 0u;
    for (var i = 0u; i < 4u; i++) {
        let x = min(id.x * 4u + i, params.picture.x - 1u);
        let uv = (vec2<f32>(f32(x), f32(y)) + 0.5) / vec2<f32>(params.picture);
        let c = textureSampleLevel(src, smp, uv, 0.0).rgb;
        let luma = clamp(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
        word |= u32(round(luma * 255.0)) << (8u * i);
    }
    dst[id.y * words + id.x] = word;
}
