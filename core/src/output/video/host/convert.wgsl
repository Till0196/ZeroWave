// ホスト側のバックエンド（`host`）へフレームを渡す・戻すためのコピー。全体を覆う三角形一つで、
// 出力先の大きさに合わせて読む。
//
// fs_copy：色をそのまま（フォーマットと大きさを変えてコピーする）。
// fs_luma：輝度だけ（NVOFA の入力、BT.709 の重み、符号値のまま）。
// fs_y・fs_uv：NV12 の Y プレーンと UV プレーンへ（oneVPL の入力、BT.709 のリミテッドレンジ）。
//          UV は半分の大きさでレンダリングし、出力先の画素の中心を線形補間でサンプリングして
//          元の 2×2 を平均する。
// fs_flow：NVOFA のオプティカルフロー（S10.5 の固定小数点、前方と後方）を、ピクセル単位の浮動小数点の
//          (前方 x, y, 後方 x, y) へ。読むのは出力先と同じグリッド。

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

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn fs_copy(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(src, smp, in.uv, 0.0).rgb, 1.0);
}

@fragment
fn fs_luma(in: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, in.uv, 0.0).rgb;
    let y = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return vec4<f32>(y, y, y, 1.0);
}

// BT.709 のリミテッドレンジ（Y 16〜235、Cb・Cr 16〜240）。
@fragment
fn fs_y(in: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, in.uv, 0.0).rgb;
    let y = (16.0 + 219.0 * dot(c, vec3<f32>(0.2126, 0.7152, 0.0722))) / 255.0;
    return vec4<f32>(y, 0.0, 0.0, 1.0);
}

@fragment
fn fs_uv(in: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, in.uv, 0.0).rgb;
    let cb = (128.0 + 224.0 * dot(c, vec3<f32>(-0.1146, -0.3854, 0.5))) / 255.0;
    let cr = (128.0 + 224.0 * dot(c, vec3<f32>(0.5, -0.4542, -0.0458))) / 255.0;
    return vec4<f32>(cb, cr, 0.0, 1.0);
}
