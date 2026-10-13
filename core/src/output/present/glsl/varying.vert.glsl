#version 100
attribute vec2 corner;
uniform vec2 luma_step;
uniform vec2 chroma_step;
varying vec2 uv_y0;
varying vec2 uv_ym;
varying vec2 uv_yp;
varying vec2 uv_c0;
varying vec2 uv_cm;
varying vec2 uv_cp;
void main() {
    gl_Position = vec4(corner, 0.0, 1.0);
    // クリップ座標から 0..1 へ。三角形はレンダーターゲットの外まで伸びるので、外側は
    // そのまま外挿される。それでよい: サンプルは CLAMP_TO_EDGE で端に留まる。
    vec2 uv = corner * 0.5 + vec2(0.5);
    uv_y0 = uv;
    uv_ym = uv - luma_step;
    uv_yp = uv + luma_step;
    uv_c0 = uv;
    uv_cm = uv - chroma_step;
    uv_cp = uv + chroma_step;
}
