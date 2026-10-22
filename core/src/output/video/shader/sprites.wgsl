// プラグインのレイヤーの絵（アルファを掛けた RGBA）を、画面（egui）の描画の中で重ねる。
// 位置はビューポートの正規化座標で来る。

struct Out {
    @builtin(position) at: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
}

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;

@vertex
fn vs(@location(0) at: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) alpha: f32) -> Out {
    return Out(vec4<f32>(at, 0.0, 1.0), uv, alpha);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, in.uv) * in.alpha;
}
