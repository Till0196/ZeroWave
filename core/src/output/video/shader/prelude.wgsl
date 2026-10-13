@group({group}) @binding({binding}) var<uniform> params: Params;

// Reading off the edge gives the edge sample, which is what the Rust
// reference does and what every deinterlacer has to do at the first and last
// rows: there is nothing above row nought to interpolate from.
//
// **The edge is the picture's and not the texture's.** `textureDimensions`
// would answer with the resource behind the texture, which on D3D12 is whole
// macroblocks -- 1088 rows for 1080 -- and those rows hold real samples, so
// the taps at the bottom would read them instead of repeating the last row.
// `params` carries the picture's size for that reason.
fn luma_size() -> vec2<i32> {
    return vec2<i32>(i32(params.width), i32(params.height));
}

fn chroma_size() -> vec2<i32> {
    return vec2<i32>(i32((params.width + 1u) / 2u), i32((params.height + 1u) / 2u));
}

fn clamped(x: i32, y: i32, size: vec2<i32>) -> vec2<i32> {
    return clamp(vec2<i32>(x, y), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
}

fn at(t: {kind}, x: i32, y: i32, size: vec2<i32>) -> f32 {
    let q = clamped(x, y, size);
    return {luma_load};
}
