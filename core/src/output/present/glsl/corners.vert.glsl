#version 100
attribute vec2 corner;
// レンダーターゲットでの位置（0..1）。fragment は `floor(place * 大きさ)` でピクセルを出す。
// `gl_FragCoord` は GLSL ES 1.00 では mediump で、Mali-G31（panfrost）は本当に fp16 で
// 持つ: 1024 を超えると x + 0.5 が丸まり、奇数の列や行が隣の texel を読む。
varying vec2 place;
void main() {
    place = corner * 0.5 + 0.5;
    gl_Position = vec4(corner, 0.0, 1.0);
}
