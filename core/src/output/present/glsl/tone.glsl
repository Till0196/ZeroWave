
// 映像の伝達特性と原色から、画面の符号値へ。`colour::Mapping` と一行ずつ同じ
// （`to_nits` → 原色 → `squeeze` → `encode`、戻すときは `undo`）。
// HLG は BT.2100 の EOTF（逆 OETF のあと OOTF、システムガンマは表示のピークで）、
// PQ は ST 2084、トーンマッピングは BT.2390 の EETF。

float tone_hlg_scene(float s) {
    s = clamp(s, 0.0, 1.0);
    return s <= 0.5 ? s * s / 3.0 : (exp((s - {hlg_c}) / {hlg_a}) + {hlg_b}) / 12.0;
}

float tone_scene_hlg(float e) {
    e = clamp(e, 0.0, 1.0);
    return e <= 1.0 / 12.0 ? sqrt(3.0 * e) : {hlg_a} * log(12.0 * e - {hlg_b}) + {hlg_c};
}

float tone_pq_nits(float s) {
    float p = pow(clamp(s, 0.0, 1.0), 1.0 / {pq_m2});
    float num = max(p - {pq_c1}, 0.0);
    float den = {pq_c2} - {pq_c3} * p;
    return 10000.0 * pow(num / den, 1.0 / {pq_m1});
}

float tone_nits_pq(float n) {
    float y = pow(clamp(n / 10000.0, 0.0, 1.0), {pq_m1});
    return pow(({pq_c1} + {pq_c2} * y) / (1.0 + {pq_c3} * y), {pq_m2});
}

float tone_srgb_linear(float c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

vec3 tone_to_nits(vec3 c) {
    vec3 n;
{source}
{gamut}
    return n;
}

// BT.2390 の EETF を一番大きい成分に掛け、ほかは同じ比で縮める（色相を保つ）。
vec3 tone_squeeze(vec3 n) {
    float most = max(max(n.r, n.g), n.b);
    if (most <= 0.0) {
        return n;
    }
    float e1 = tone_nits_pq(most) / {top};
    float e2 = e1;
    if (e1 >= {ks}) {
        float t = (e1 - {ks}) / (1.0 - {ks});
        float t2 = t * t;
        float t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * {ks}
           + (t3 - 2.0 * t2 + t) * (1.0 - {ks})
           + (-2.0 * t3 + 3.0 * t2) * {max_lum};
    }
    return n * (tone_pq_nits(e2 * {top}) / most);
}

vec3 tone_encode(vec3 n) {
{encode}
}

// 画面の符号値から cd/m² へ（画面を重ねるとき下の映像を読み戻す）。
vec3 tone_undo(vec3 e) {
{undo}
}

vec3 tone(vec3 c) {
    vec3 n = tone_to_nits(c);
{squeeze}
    return tone_encode(n);
}
