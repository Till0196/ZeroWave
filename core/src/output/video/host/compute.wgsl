// シェーダーによるオプティカルフローのバックエンド（`compute.rs`）。ハードウェアのオプティカルフローを持たない GPU のため。
//
// fs_search：ピラミッドの一つのレベルで、4×4 のブロックごとに、上のレベルの結果（2 倍して予測に）の
//     周りを ±radius 画素探索する。比べるのはブロックを中心にした 8×8 のウィンドウの差の平均
//     （`step` 画素おきにサンプリングする）。動きの大きさに小さなペナルティを足して、平坦な所で
//     でたらめな動きを選ばないようにする。一番下のレベルでは、差の放物線補間で 1 画素より
//     細かく合わせる（サブピクセル）。出すのは (x, y, 差の平均, 0)、単位はそのレベルの画素。
// fs_combine：前→今と今→前の等倍の結果を、プラグインのオプティカルフローの形式
//     (前→今の x, y, 今→前の x, y) へ（出力先のグリッドへ最近傍でリサンプリングする）。
// fs_confidence：差の平均から信頼度 (前→今, 今→前) へ。

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

struct Params {
    // 一番上のレベルか（予測が無い）、一番下のレベルか（サブピクセルで合わせる）。
    top: u32,
    bottom: u32,
    radius: i32,
    step: i32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_2d<f32>;
@group(0) @binding(2) var coarser: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;

// 動き一画素あたりのペナルティ（差の平均と同じ尺度、輝度 0〜1）。
const MOTION_PENALTY: f32 = 0.0015;

fn luma(t: texture_2d<f32>, q: vec2<i32>, dims: vec2<i32>) -> f32 {
    return textureLoad(t, clamp(q, vec2<i32>(0), dims - 1), 0).r;
}

fn sad(origin: vec2<i32>, d: vec2<i32>, dims: vec2<i32>) -> f32 {
    var sum = 0.0;
    var count = 0.0;
    for (var y = 0; y < 8; y += params.step) {
        for (var x = 0; x < 8; x += params.step) {
            let q = origin + vec2<i32>(x, y);
            sum += abs(luma(src, q, dims) - luma(dst, q + d, dims));
            count += 1.0;
        }
    }
    return sum / count;
}

fn parabola(minus: f32, center: f32, plus: f32) -> f32 {
    let curve = minus - 2.0 * center + plus;
    if (curve <= 1e-6) {
        return 0.0;
    }
    return clamp(0.5 * (minus - plus) / curve, -0.5, 0.5);
}

@fragment
fn fs_search(in: Out) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src));
    let block = vec2<i32>(in.position.xy);
    let origin = block * 4 - 2;
    var predicted = vec2<i32>(0);
    if (params.top == 0u) {
        let cd = vec2<i32>(textureDimensions(coarser));
        let c = textureLoad(coarser, clamp(block / 2, vec2<i32>(0), cd - 1), 0).xy;
        predicted = vec2<i32>(round(c * 2.0));
    }
    var best = predicted;
    var best_cost = 1e9;
    for (var dy = -params.radius; dy <= params.radius; dy++) {
        for (var dx = -params.radius; dx <= params.radius; dx++) {
            let d = predicted + vec2<i32>(dx, dy);
            let length = abs(f32(d.x)) + abs(f32(d.y));
            let cost = sad(origin, d, dims) + MOTION_PENALTY * length;
            if (cost < best_cost) {
                best_cost = cost;
                best = d;
            }
        }
    }
    let center = sad(origin, best, dims);
    var fine = vec2<f32>(best);
    if (params.bottom != 0u) {
        fine.x += parabola(sad(origin, best - vec2<i32>(1, 0), dims), center, sad(origin, best + vec2<i32>(1, 0), dims));
        fine.y += parabola(sad(origin, best - vec2<i32>(0, 1), dims), center, sad(origin, best + vec2<i32>(0, 1), dims));
    }
    return vec4<f32>(fine, center, 0.0);
}

// 出力先の画素の中心に当たる、等倍の結果のグリッド。
fn cell(uv: vec2<f32>, dims: vec2<i32>) -> vec2<i32> {
    return clamp(vec2<i32>(floor(uv * vec2<f32>(dims))), vec2<i32>(0), dims - 1);
}

@fragment
fn fs_combine(in: Out) -> @location(0) vec4<f32> {
    let p = cell(in.uv, vec2<i32>(textureDimensions(src)));
    let f = textureLoad(src, p, 0).xy;
    let b = textureLoad(dst, p, 0).xy;
    return vec4<f32>(f, b);
}

// 差の平均 COST_HALF（輝度 0〜1）で信頼度が 0.5。NVOFA のコストの 24（8 ビット、24/255）に当たる値で、
// 実際の映像ではまだ調整していない。
const COST_HALF: f32 = 0.094;

@fragment
fn fs_confidence(in: Out) -> @location(0) vec4<f32> {
    let p = cell(in.uv, vec2<i32>(textureDimensions(src)));
    let k = 0.69314718 / COST_HALF;
    let f = textureLoad(src, p, 0).z;
    let b = textureLoad(dst, p, 0).z;
    return vec4<f32>(exp(-f * k), exp(-b * k), 0.0, 1.0);
}
