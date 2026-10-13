
varying vec2 place;
void main() {
    // 行0がフレームの一番上。GL の原点は左下で、テクスチャの行0もアップロードしたフレームの
    // 行0なので、ここで裏返す必要は無い。読み戻しは下から上へ返るので、
    // 出てくるバイト列はそのまま上から下のラスタになる。
    // `gl_FragCoord` は使わない（mediump。corners.vert.glsl のコメント）。
    vec2 p = floor(place * luma_size);
    // 4:2:0 の色差はサンプルが4分の1で、インタレースでは2つのフィールドが違う位置に
    // 置かれる。だから変換してからフィルターを掛けるのではなく、プレーンのままフィルターを掛ける。
    vec2 q = floor(p * 0.5);
    gl_FragColor = to_rgb(deint_y(p), deint_cb(q), deint_cr(q));
}
