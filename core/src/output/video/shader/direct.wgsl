// デコーダーの 2 プレーン（NV12・P010）を直に読み、変換と拡大縮小を一回で済ませて画面へ書く。
// デインターレースも逆テレシネも要らないフレームだけが通る（`gpu::Video::direct`）。
// 変換は一次式なので、色差を画素の位置で補間してから変換しても、変換してから補間しても同じ。
// 拡大縮小はバイリニアだけ（ほかのカーネルは RGB の中間テクスチャを通る）。

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var display: sampler;
@group(0) @binding(3) var chroma: texture_2d<f32>;

// 2 プレーンの 10 ビットは 16 ビットの上に寄っているので、`code / 1023` に戻す倍率（8 ビットは 1）。
override WIDE: f32 = 1.0;

// `uv`（フレームの 0..1）を、揃えのぶん大きいテクスチャの座標へ。端の半画素の内側に収め、揃えの行を読まない。
fn inside(uv: vec2<f32>, picture: vec2<f32>, texture: vec2<f32>) -> vec2<f32> {
    let at = clamp(uv * picture, vec2<f32>(0.5), picture - vec2<f32>(0.5));
    return at / texture;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(f32(params.width), f32(params.height));
    let half = vec2<f32>(f32((params.width + 1u) / 2u), f32((params.height + 1u) / 2u));
    let y = textureSampleLevel(luma, display, inside(input.uv, size, vec2<f32>(textureDimensions(luma))), 0.0).r;
    let c = textureSampleLevel(chroma, display, inside(input.uv, half, vec2<f32>(textureDimensions(chroma))), 0.0).rg;
    return finish(vec4<f32>(to_rgb(y * WIDE, c.x * WIDE, c.y * WIDE), 1.0));
}
