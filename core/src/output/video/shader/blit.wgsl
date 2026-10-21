// 拡大と縮小のしかたはパイプラインごとに決め打つ（`Scaler::code`、0 は
// バイリニア。字幕や OSD のオーバーレイはこの既定のまま）。uniform で選ばせると、使わない
// カーネルのループまで一つのシェーダーに入り、それだけで遅くなる：Apple M2 で
// 2560×1440 へのバイリニアが 0.2 ms のところ、Lanczos3 などの分岐が入っているだけで
// 7〜8 ms かかっていた。縮小か拡大かはレンダリングする側が決め、通らない側は 0 にして
// 渡す（`gpu::Video::blit_key`）。
override UP: u32 = 0u;
override DOWN: u32 = 0u;
// 分離可能なカーネルの縮小は横と縦の二回に分ける（`gpu::split`）。0 は一回で、
// 1 は横だけ（元の行のまま、色はいじらずに `split::FORMAT` の中間テクスチャへ）、2 は
// その中間テクスチャから縦だけ。一回でやると一画素で一辺の点数の二乗（二倍の縮小の
// Lanczos3 で 144 点）を読むが、分ければ和になる。分離可能なカーネルの縮小は
// いつも分けるので、一回の縮小は EWA だけ。
override PASS: u32 = 0u;

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var display: sampler;

// ---- 拡大縮小（`Scaler`）----

const PI: f32 = 3.14159265358979;
// 縮小のとき、一辺で読む点の上限（間は GPU の線形補間で埋める）。EWA は
// 点ごとに Jinc を二つ計算するので、もっと少なく（4K → FHD で、12 点では
// RTX 2060 で 15 ms 余計にかかった）。
const MOST_TAPS: f32 = 12.0;
const MOST_TAPS_EWA: f32 = 8.0;

// Mitchell–Netravali の族（B、C）。
fn bc_spline(x: f32, b: f32, c: f32) -> f32 {
    let a = abs(x);
    if (a < 1.0) {
        return ((12.0 - 9.0 * b - 6.0 * c) * a * a * a + (-18.0 + 12.0 * b + 6.0 * c) * a * a + (6.0 - 2.0 * b)) / 6.0;
    }
    if (a < 2.0) {
        return ((-b - 6.0 * c) * a * a * a + (6.0 * b + 30.0 * c) * a * a + (-12.0 * b - 48.0 * c) * a + (8.0 * b + 24.0 * c)) / 6.0;
    }
    return 0.0;
}

fn spline36(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1.0) {
        return ((13.0 / 11.0 * a - 453.0 / 209.0) * a - 3.0 / 209.0) * a + 1.0;
    }
    if (a < 2.0) {
        let t = a - 1.0;
        return ((-6.0 / 11.0 * t + 270.0 / 209.0) * t - 156.0 / 209.0) * t;
    }
    if (a < 3.0) {
        let t = a - 2.0;
        return ((1.0 / 11.0 * t - 45.0 / 209.0) * t + 26.0 / 209.0) * t;
    }
    return 0.0;
}

fn sinc(x: f32) -> f32 {
    if (abs(x) < 1e-5) { return 1.0; }
    let t = PI * x;
    return sin(t) / t;
}

fn lanczos3(x: f32) -> f32 {
    if (abs(x) >= 3.0) { return 0.0; }
    return sinc(x) * sinc(x / 3.0);
}

// EWA Lanczos の重みを距離の二乗 `d2` から。テーブルと半径（`EWA_TABLE`・`EWA_STEPS`・
// `EWA_RADIUS`）は `shader::ewa_table` が後ろに足す。
fn ewa_weight(d2: f32) -> f32 {
    let x = d2 * (EWA_STEPS / (EWA_RADIUS * EWA_RADIUS));
    if (x >= EWA_STEPS) { return 0.0; }
    let i = u32(x);
    return mix(EWA_TABLE[i], EWA_TABLE[i + 1u], x - f32(i));
}

fn kernel(mode: u32, x: f32) -> f32 {
    switch (mode) {
        case 2u: { return bc_spline(x, 1.0 / 3.0, 1.0 / 3.0); }
        case 3u: { return bc_spline(x, 0.0, 0.5); }
        case 4u: { return spline36(x); }
        case 5u: { return lanczos3(x); }
        default: { return 0.0; }
    }
}

fn radius(mode: u32) -> f32 {
    switch (mode) {
        case 2u, 3u: { return 2.0; }
        case 4u, 5u: { return 3.0; }
        default: { return EWA_RADIUS; }
    }
}

fn texel(p: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    return textureLoad(picture, clamp(p, vec2<i32>(0), size - 1), 0);
}

// 一辺四点（半径 2）の重み：画素の中心から −1−f、−f、1−f、2−f。
fn weights4(mode: u32, f: f32) -> vec4<f32> {
    return vec4<f32>(kernel(mode, -1.0 - f), kernel(mode, -f), kernel(mode, 1.0 - f), kernel(mode, 2.0 - f));
}

fn row4(ib: vec2<i32>, j: i32, w: vec4<f32>, size: vec2<i32>) -> vec4<f32> {
    return texel(ib + vec2<i32>(-1, j), size) * w.x + texel(ib + vec2<i32>(0, j), size) * w.y
        + texel(ib + vec2<i32>(1, j), size) * w.z + texel(ib + vec2<i32>(2, j), size) * w.w;
}

// 一辺六点（半径 3）の重み：−2−f から 3−f まで。
struct Six {
    a: vec4<f32>,
    b: vec2<f32>,
};

fn weights6(mode: u32, f: f32) -> Six {
    return Six(
        vec4<f32>(kernel(mode, -2.0 - f), kernel(mode, -1.0 - f), kernel(mode, -f), kernel(mode, 1.0 - f)),
        vec2<f32>(kernel(mode, 2.0 - f), kernel(mode, 3.0 - f)),
    );
}

fn row6(ib: vec2<i32>, j: i32, w: Six, size: vec2<i32>) -> vec4<f32> {
    return texel(ib + vec2<i32>(-2, j), size) * w.a.x + texel(ib + vec2<i32>(-1, j), size) * w.a.y
        + texel(ib + vec2<i32>(0, j), size) * w.a.z + texel(ib + vec2<i32>(1, j), size) * w.a.w
        + texel(ib + vec2<i32>(2, j), size) * w.b.x + texel(ib + vec2<i32>(3, j), size) * w.b.y;
}

// `uv` の色を、出力一画素がフレームの `footprint` 画素分に当たるとして求める。
fn scaled(uv: vec2<f32>, position: vec2<f32>, footprint: vec2<f32>) -> vec4<f32> {
    // 分けた二回は縮小と決まっている（決めたのはレンダリングする側）。
    let shrinking = PASS != 0u || max(footprint.x, footprint.y) > 1.001;
    let mode = select(UP, DOWN, shrinking);
    if (mode == 0u) {
        return textureSampleLevel(picture, display, uv, 0.0);
    }
    let isize = vec2<i32>(textureDimensions(picture));
    let size = vec2<f32>(isize);
    if (mode == 1u) {
        return texel(vec2<i32>(floor(uv * size)), isize);
    }
    let pos = uv * size - 0.5;
    // 等倍で画素の中心に乗っていれば、元の画素そのもの（どのカーネルも中心の重みが 1 で
    // ほかは 0、リンギングの抑制も素通し）。フィルターが画面の大きさまで広げたフレームはここを通る。
    // 2482×1396 の Lanczos3 は Apple M2 で一枚 6 ms ほどかかっていた。
    let centre = round(pos);
    if (PASS == 0u && all(abs(footprint - vec2<f32>(1.0)) < vec2<f32>(1e-3)) && all(abs(pos - centre) < vec2<f32>(1e-3))) {
        return texel(vec2<i32>(centre), isize);
    }
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    // `UP < 2` と `DOWN < 2` はもう上で返している。定数で書いておくのは、通らない
    // 側をシェーダーから落とさせるため。
    if (!shrinking && UP >= 2u) {
        // 拡大：元の画素を直に読む。ここのしかたは `UP` そのもの。
        let mode = UP;
        let base = floor(pos);
        let f = pos - base;
        let ib = vec2<i32>(base);
        let n = i32(ceil(radius(mode)));
        if (mode == 6u) {
            // EWA：円形なので点ごとに。読むのは行ごとに円の中だけ（8×8 のうち
            // 重みが 0 でないのは三十三点ほど）。
            for (var j = 1 - n; j <= n; j++) {
                let dy = f32(j) - f.y;
                let reach2 = EWA_RADIUS * EWA_RADIUS - dy * dy;
                if (reach2 <= 0.0) {
                    continue;
                }
                let reach = sqrt(reach2);
                let lo = max(i32(ceil(f.x - reach)), 1 - n);
                let hi = min(i32(floor(f.x + reach)), n);
                for (var i = lo; i <= hi; i++) {
                    let d = vec2<f32>(f32(i) - f.x, dy);
                    let w = ewa_weight(dot(d, d));
                    sum += texel(ib + vec2<i32>(i, j), isize) * w;
                    total += w;
                }
            }
        } else if (n == 2) {
            // 分離可能なカーネル：横と縦の重みを先に、読むのは展開して書く。
            // ループと配列にしない：naga はループに境界チェックを入れるので
            // Metal が展開できず、重みの配列がメモリに落ちる。Apple M2 で
            // 2560×1440 への Catmull-Rom（十六点）が 6 ms かかっていた。
            let wx = weights4(mode, f.x);
            let wy = weights4(mode, f.y);
            sum = row4(ib, -1, wx, isize) * wy.x + row4(ib, 0, wx, isize) * wy.y
                + row4(ib, 1, wx, isize) * wy.z + row4(ib, 2, wx, isize) * wy.w;
            total = dot(wx, vec4<f32>(1.0)) * dot(wy, vec4<f32>(1.0));
        } else {
            let wx = weights6(mode, f.x);
            let wy = weights6(mode, f.y);
            sum = row6(ib, -2, wx, isize) * wy.a.x + row6(ib, -1, wx, isize) * wy.a.y
                + row6(ib, 0, wx, isize) * wy.a.z + row6(ib, 1, wx, isize) * wy.a.w
                + row6(ib, 2, wx, isize) * wy.b.x + row6(ib, 3, wx, isize) * wy.b.y;
            total = (dot(wx.a, vec4<f32>(1.0)) + wx.b.x + wx.b.y) * (dot(wy.a, vec4<f32>(1.0)) + wy.b.x + wy.b.y);
        }
        var out = sum / max(total, 1e-6);
        if (fit.antiring != 0u) {
            // リンギングの抑制：まわりの元の 2×2 の幅に収める。
            let a = texel(ib, isize);
            let b = texel(ib + vec2<i32>(1, 0), isize);
            let c = texel(ib + vec2<i32>(0, 1), isize);
            let d = texel(ib + vec2<i32>(1, 1), isize);
            out = clamp(out, min(min(a, b), min(c, d)), max(max(a, b), max(c, d)));
        }
        return clamp(out, vec4<f32>(0.0), vec4<f32>(1.0));
    }
    if (DOWN < 2u) {
        return textureSampleLevel(picture, display, uv, 0.0);
    }
    // 縮小：カーネルを縮小率に合わせて広げ、一辺 MOST_TAPS 点までを
    // 等間隔に読む（点の間は GPU の線形補間）。
    let scale = max(footprint, vec2<f32>(1.0));
    let support = radius(mode) * scale;
    let most = select(MOST_TAPS, MOST_TAPS_EWA, mode == 6u);
    let count = clamp(ceil(2.0 * support), vec2<f32>(2.0), vec2<f32>(most));
    let step = 2.0 * support / count;
    let nx = i32(count.x);
    let ny = i32(count.y);
    if (mode == 6u) {
        for (var j = 0; j < ny; j++) {
            for (var i = 0; i < nx; i++) {
                let t = pos - support + step * (vec2<f32>(f32(i), f32(j)) + 0.5);
                let d = (t - pos) / scale;
                let w = ewa_weight(dot(d, d));
                if (w != 0.0) {
                    sum += textureSampleLevel(picture, display, (t + 0.5) / size, 0.0) * w;
                    total += w;
                }
            }
        }
    } else if (PASS == 1u) {
        // 横だけ。縦は元の行の中心を読む（線形補間が効かない）。割ったまま
        // 返し、範囲に収めるのは縦のあと。
        let row = (round(pos.y) + 0.5) / size.y;
        for (var i = 0; i < nx; i++) {
            let t = -support.x + step.x * (f32(i) + 0.5);
            let w = kernel(mode, t / scale.x);
            sum += textureSampleLevel(picture, display, vec2<f32>((pos.x + t + 0.5) / size.x, row), 0.0) * w;
            total += w;
        }
        return sum / max(total, 1e-6);
    } else {
        // 縦だけ（`PASS` 2）。横は中間テクスチャの列そのもの（中間テクスチャの列はサーフェスの画素の列と同じ並び）。
        let column = (floor(position.x) + 0.5) / size.x;
        for (var j = 0; j < ny; j++) {
            let t = -support.y + step.y * (f32(j) + 0.5);
            let w = kernel(mode, t / scale.y);
            sum += textureSampleLevel(picture, display, vec2<f32>(column, (pos.y + t + 0.5) / size.y), 0.0) * w;
            total += w;
        }
    }
    return clamp(sum / max(total, 1e-6), vec4<f32>(0.0), vec4<f32>(1.0));
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    // 出力一画素がフレームの何画素分か（拡大か縮小か、縮小ならどれだけか）。
    // 微分は分岐の前で取る。
    let size = vec2<f32>(textureDimensions(picture));
    let dx = dpdx(input.uv) * size;
    let dy = dpdy(input.uv) * size;
    let footprint = vec2<f32>(length(vec2<f32>(dx.x, dy.x)), length(vec2<f32>(dx.y, dy.y)));
    let taken = scaled(input.uv, input.position.xy, footprint);
    if (PASS == 1u) {
        return taken;
    }
    return finish(taken);
}
