// 外部プロセスのバックエンド（`process`）とのフレームの受け渡し（GPU 上で）。
// フレームは RGB の三つのプレーンを縦に並べた物：プレーン c の y 行目は (c × 高さ + y) × 行の幅 の位置から。
// 値は f32 か f16（二つで 1 ワード、前の画素が下位 16 ビット）。行の幅は 4 バイトの倍数。
//
// pack：フレームを詰める（一つの呼び出しが 1 ワード、x はワード、y はプレーンと行）。行の余りは 0。
// unpack：詰めたデータをフレームへ（一つの呼び出しが 1 画素）。

struct P {
    w: u32,
    h: u32,
    // 行の幅（ワード）。
    stride: u32,
    // 1 なら f16。
    half: u32,
}

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> planes: array<u32>;
@group(0) @binding(3) var<storage, read> result: array<u32>;
@group(0) @binding(4) var made: texture_storage_2d<rgba16float, write>;

fn value(x: u32, y: u32, c: u32) -> f32 {
    if (x >= p.w) {
        return 0.0;
    }
    let rgb = clamp(textureLoad(picture, vec2<i32>(i32(x), i32(y)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return rgb[c];
}

@compute @workgroup_size(64, 1)
fn pack(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.stride || id.y >= 3u * p.h) {
        return;
    }
    let c = id.y / p.h;
    let y = id.y % p.h;
    var word: u32;
    if (p.half == 1u) {
        word = pack2x16float(vec2<f32>(value(2u * id.x, y, c), value(2u * id.x + 1u, y, c)));
    } else {
        word = bitcast<u32>(value(id.x, y, c));
    }
    planes[id.y * p.stride + id.x] = word;
}

fn fetch(x: u32, y: u32, c: u32) -> f32 {
    let row = (c * p.h + y) * p.stride;
    if (p.half == 1u) {
        let pair = unpack2x16float(result[row + x / 2u]);
        return select(pair.x, pair.y, (x & 1u) == 1u);
    }
    return bitcast<f32>(result[row + x]);
}

@compute @workgroup_size(16, 16)
fn unpack(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.w || id.y >= p.h) {
        return;
    }
    let c = vec3<f32>(fetch(id.x, id.y, 0u), fetch(id.x, id.y, 1u), fetch(id.x, id.y, 2u));
    textureStore(made, vec2<i32>(id.xy), vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
