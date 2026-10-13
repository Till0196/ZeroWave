
@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let luma_at = vec2<i32>(i32(input.position.x), i32(input.position.y));
    // 4:2:0 chroma is a quarter the samples, and interlaced 4:2:0 sites the
    // two fields' chroma differently, so it is filtered as a plane of its own
    // rather than resolved before the filter runs.
    let chroma_at = luma_at / 2;
    let rgb = to_rgb(deint_y(luma_at), deint_cb(chroma_at), deint_cr(chroma_at));
    // 映像に掛かっているガンマは掛けたまま置く。このテクスチャは `Rgba8Unorm` で、
    // 書いた値がそのまま 8 ビットに入るので、リニアに直してから入れると暗部が潰れる。
    // sRGB の符号 0..51（暗部の五分の一）はリニアでは 0..0.033 しかなく、8 ビットでは
    // 13 段にしかならない。実測で 256 段が 183 段まで落ちていた。
    //
    // リニアに直すのはサーフェスへ出すところ（`blit.wgsl`）。そこではサーフェスが
    // `…Srgb` かどうかが分かり、外した直後にハードウェアが掛け直すので、8 ビットに
    // 入るのは常にエンコードされた値のまま。
    return vec4<f32>(rgb, 1.0);
}
