
// 端を越えた読み出しは端のサンプルになる。Rust の参照実装がやることで、どの
// デインタレーサーも最初と最後の行でやること: 行0の上には、補間の
// もとになるものが無い。
vec2 spot(vec2 size, float x, float y) {
    return (clamp(vec2(x, y), vec2(0.0), size - vec2(1.0)) + vec2(0.5)) / size;
}
