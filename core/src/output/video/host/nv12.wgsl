// NV12 の二つのプレーン（Y は `R8Unorm`、UV は半分の `Rg8Unorm`、BT.709 のリミテッドレンジ）を
// RGB へ（oneVPL が作ったフレームをチェーンへ戻す）。出力先の大きさに合わせて読む。

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Out {
    let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: Out;
    out.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let y = (textureSampleLevel(luma, smp, in.uv, 0.0).r * 255.0 - 16.0) / 219.0;
    let c = (textureSampleLevel(chroma, smp, in.uv, 0.0).rg * 255.0 - 128.0) / 224.0;
    let r = y + 1.5748 * c.y;
    let g = y - 0.1873 * c.x - 0.4681 * c.y;
    let b = y + 1.8556 * c.x;
    return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
