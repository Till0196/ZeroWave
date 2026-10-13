#version 100
attribute vec2 corner;
uniform vec4 rect;
varying vec2 uv;
void main() {
    gl_Position = vec4(mix(rect.x, rect.z, corner.x), mix(rect.y, rect.w, corner.y), 0.0, 1.0);
    // フレームの行0はテクスチャの行0、つまり v=0。画面の上（クリップ +1）に
    // 行0を出したいので、v を裏返す。
    uv = vec2(corner.x, 1.0 - corner.y);
}
