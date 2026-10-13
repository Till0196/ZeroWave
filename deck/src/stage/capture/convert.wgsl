// キャプチャの色変換。サーフェスの形式でレンダリングしたフレームを、保存する形式に直す。
// 1 ピクセルずつ読んで 1 ピクセルずつ書く（大きさは同じ）。

struct Params {
    mode: u32,
    // PQ を SDR に直すときの白（cd/m²）。
    white: f32,
    // scRGB の 1 が SDR でいくつか（SDR に直すとき）、何 cd/m² か（PQ に直すとき）。
    one: f32,
    pad: f32,
};

const SIGNAL: u32 = 0u;      // もう sRGB の信号。そのまま。
const LINEAR: u32 = 1u;      // リニアな光（*Srgb の形式を読んだ物）。sRGB に。
const PQ_SDR: u32 = 2u;      // PQ の信号を SDR の sRGB に。
const HLG_SDR: u32 = 3u;     // HLG の信号を SDR の sRGB に。
const SCRGB_SDR: u32 = 4u;   // scRGB を SDR の sRGB に。
const SCRGB_PQ: u32 = 5u;    // scRGB を BT.2020 の PQ の信号に（HDR のまま保存）。

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn srgb_encode(linear: vec3<f32>) -> vec3<f32> {
    let l = clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = l * 12.92;
    let high = 1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, l <= vec3<f32>(0.0031308));
}

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

fn pq_to_nits(signal: vec3<f32>) -> vec3<f32> {
    let p = pow(clamp(signal, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / PQ_M2));
    let num = max(p - PQ_C1, vec3<f32>(0.0));
    let den = PQ_C2 - PQ_C3 * p;
    return 10000.0 * pow(num / den, vec3<f32>(1.0 / PQ_M1));
}

fn nits_to_pq(nits: vec3<f32>) -> vec3<f32> {
    let y = pow(clamp(nits / 10000.0, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(PQ_M1));
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), vec3<f32>(PQ_M2));
}

// BT.2100 の HLG の逆 OETF（シーンの光 0〜1）。
fn hlg_to_linear(signal: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let s = clamp(signal, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = s * s / 3.0;
    let high = (exp((s - c) / a) + b) / 12.0;
    return select(high, low, s <= vec3<f32>(0.5));
}

fn bt2020_to_709(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        1.6605 * c.r - 0.5876 * c.g - 0.0728 * c.b,
        -0.1246 * c.r + 1.1329 * c.g - 0.0083 * c.b,
        -0.0182 * c.r - 0.1006 * c.g + 1.1187 * c.b,
    );
}

fn bt709_to_2020(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.6274 * c.r + 0.3293 * c.g + 0.0433 * c.b,
        0.0691 * c.r + 0.9195 * c.g + 0.0114 * c.b,
        0.0164 * c.r + 0.0880 * c.g + 0.8956 * c.b,
    );
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let v = textureLoad(source, vec2<i32>(floor(at.xy)), 0).rgb;
    var out = v;
    switch params.mode {
        case 1u: { out = srgb_encode(v); }
        case 2u: { out = srgb_encode(bt2020_to_709(pq_to_nits(v) / params.white)); }
        case 3u: {
            let peak = hlg_to_linear(vec3<f32>(0.75));
            out = srgb_encode(bt2020_to_709(hlg_to_linear(v) / peak));
        }
        case 4u: { out = srgb_encode(v * params.one); }
        case 5u: { out = nits_to_pq(bt709_to_2020(v * params.one)); }
        default: {}
    }
    return vec4<f32>(out, 1.0);
}
