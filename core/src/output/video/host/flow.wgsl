// オプティカルフローのバックエンドが出した物を、プラグインの決まった形式へ直す。出力先のグリッドが
// バックエンドのグリッドと違えば、出力先の画素の中心に当たるバックエンドのグリッドをサンプリングする
// （最近傍。フローの値はフレームの画素単位なので、グリッドが変わっても値は変えない）。
//
// fs：NVOFA のオプティカルフロー（S10.5 の固定小数点、前方 = 前のフレーム → 今のフレーム、後方 = 今 → 前）を、
//     ピクセル単位の浮動小数点の (前方 x, y, 後方 x, y) へ。

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

@group(0) @binding(0) var forward: texture_2d<i32>;
@group(0) @binding(1) var backward: texture_2d<i32>;

// 出力先の画素の中心に当たる、バックエンドのグリッドの位置。
fn cell(uv: vec2<f32>, dims: vec2<i32>) -> vec2<i32> {
    return clamp(vec2<i32>(floor(uv * vec2<f32>(dims))), vec2<i32>(0), dims - 1);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let p = cell(in.uv, vec2<i32>(textureDimensions(forward)));
    let f = vec2<f32>(textureLoad(forward, p, 0).xy) / 32.0;
    let b = vec2<f32>(textureLoad(backward, p, 0).xy) / 32.0;
    return vec4<f32>(f, b);
}
