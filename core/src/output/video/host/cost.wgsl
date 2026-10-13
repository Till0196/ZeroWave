// オプティカルフローのバックエンドのコストを、信頼度 (前→今, 今→前)（0〜1）へ（[`super::HOST_FLOW_CONFIDENCE`]）。
// 出力先のグリッドがバックエンドのグリッドと違えば、最近傍でサンプリングする（flow.wgsl と同じ）。
//
// fs：NVOFA のコスト（`R8Uint`、0〜255、小さいほど確か）。

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

@group(0) @binding(0) var forward_cost: texture_2d<u32>;
@group(0) @binding(1) var backward_cost: texture_2d<u32>;

// コスト COST_HALF で信頼度が 0.5 になる。まだ合わせていない値で、実機の
// コストの幅を見て決め直す。
const COST_HALF: f32 = 24.0;

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(forward_cost));
    let p = clamp(vec2<i32>(floor(in.uv * vec2<f32>(dims))), vec2<i32>(0), dims - 1);
    let f = f32(textureLoad(forward_cost, p, 0).x);
    let b = f32(textureLoad(backward_cost, p, 0).x);
    let k = 0.69314718 / COST_HALF;
    return vec4<f32>(exp(-f * k), exp(-b * k), 0.0, 1.0);
}
