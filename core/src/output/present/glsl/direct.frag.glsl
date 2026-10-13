// フィルターの要らないフレーム（プログレッシブ、デインターレース済み、weave）を、テクスチャから画面へ
// 1 パスでレンダリングする：色の変換と拡大縮小（バイリニア）だけ。中間のレンダーターゲットを通さない。
varying vec2 uv;
void main() {
    vec4 c = texture2D(c_cur_t, uv);
    gl_FragColor = to_rgb(texture2D(y_cur_t, uv).r, c.r, c.a);
}
