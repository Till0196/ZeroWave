// フィールドマッチの計測：一枚を「自分の二つのフィールドで組み合わせる（c）」のと
// 「表示するフィールドと前のフレームの反対のフィールドで組み合わせる（p）」のとで、
// コーミングの画素を数える。コーミングは四行にわたって符号が交互で（MPEG-2 の量子化
// ノイズは一行の飛び出しになる）、前のフレームから動いている画素（止まった
// 縞は映像の模様）。反対のフィールドの行だけを見る。読み戻すのは判定の履歴のためだけ。

struct Comb {
    width: u32,
    height: u32,
    // 表示するフィールドの行（0 か 1）。見るのは反対の行。
    first_row: u32,
    // 縞のしきい値（隣の行との差。0..1、8/255）。
    stripe: f32,
    // 動きのしきい値（前のフレームの同じ画素との差。0..1、7/255）。
    motion: f32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var cur: {kind};
@group(0) @binding(1) var before: {kind};
// [0] c のコーミング、[1] p のコーミング、[2] 動いた画素、[3] 表示するフィールドの前のフレームとの差の和、
// [4] 反対のフィールドの同じ物（8 ビットの単位。24p の周期の判定に使う）。
@group(0) @binding(2) var<storage, read_write> combed: array<atomic<u32>, 8>;
@group(0) @binding(3) var<uniform> comb: Comb;

fn at(t: {kind}, x: i32, y: i32) -> f32 {
    let q = clamp(vec2<i32>(x, y), vec2<i32>(0, 0), vec2<i32>(i32(comb.width) - 1, i32(comb.height) - 1));
    return {luma_load};
}

// 行 a・b・c・d（a と c が表示するフィールド、b と d が候補のフィールド）が交互に振れて
// いるか：隣り合う差の符号が二度とも反転し、どの差もしきい値より大きい。
fn striped(a: f32, b: f32, c: f32, d: f32, threshold: f32) -> bool {
    let t2 = threshold * threshold;
    return (a - b) * (b - c) < -t2 && (b - c) * (c - d) < -t2;
}

// 一つのグループ（16x8）の中で、各々の数を木の形に足し合わせる。画素ごとに
// 共有の変数へ atomic で足すと、128 の呼び出しが同じアドレスを取り合う。
const LANES: u32 = 128u;
var<workgroup> combs: array<vec3<u32>, LANES>;
var<workgroup> diffs: array<vec2<u32>, LANES>;

@compute @workgroup_size(16, 8, 1)
fn main(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    // c のコーミング・p のコーミング・動いた画素と、二つのフィールドの差。
    var comb_here = vec3<u32>(0u);
    var diff_here = vec2<u32>(0u);
    let other = 1u - comb.first_row;
    let x = group.x * 16u + local.x;
    let row = group.y * 8u + local.y;
    let y = row * 2u + other;
    if x < comb.width && y < comb.height {
        let ix = i32(x);
        let iy = i32(y);
        let a = at(cur, ix, iy - 1);
        let c = at(cur, ix, iy + 1);
        let bc = at(cur, ix, iy);
        let dc = at(cur, ix, iy + 2);
        let bp = at(before, ix, iy);
        let dp = at(before, ix, iy + 2);
        let ap = at(before, ix, iy - 1);
        // 上の行 a は表示するフィールド、この行 b は反対のフィールド。
        diff_here = vec2<u32>(u32(abs(a - ap) * 255.0 + 0.5), u32(abs(bc - bp) * 255.0 + 0.5));
        if abs(bc - bp) > comb.motion {
            comb_here = vec3<u32>(
                select(0u, 1u, striped(a, bc, c, dc, comb.stripe)),
                select(0u, 1u, striped(a, bp, c, dp, comb.stripe)),
                1u,
            );
        }
    }
    combs[index] = comb_here;
    diffs[index] = diff_here;
    workgroupBarrier();
    for (var half = LANES / 2u; half > 0u; half = half / 2u) {
        if index < half {
            combs[index] = combs[index] + combs[index + half];
            diffs[index] = diffs[index] + diffs[index + half];
        }
        workgroupBarrier();
    }
    if index == 0u {
        atomicAdd(&combed[0], combs[0].x);
        atomicAdd(&combed[1], combs[0].y);
        atomicAdd(&combed[2], combs[0].z);
        atomicAdd(&combed[3], diffs[0].x);
        atomicAdd(&combed[4], diffs[0].y);
    }
}
