
// The chroma pair, read from the one texture that holds them both. Cb is the
// first component and Cr the second, which is what `GR88` and `GR32` mean.
fn at_pair(t: {kind}, x: i32, y: i32) -> vec2<f32> {
    let q = clamped(x, y, chroma_size());
    return {chroma_load};
}
