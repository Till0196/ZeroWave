// 符号値（0..1）から RGB へ。映像のガンマは掛けたまま（`fragment.wgsl`）。
// プラグインのデインタレース（`zw_rgb`）も同じ式を通る。
fn to_rgb(code_y: f32, code_cb: f32, code_cr: f32) -> vec3<f32> {
    let y = (code_y - params.luma_offset) * params.luma_gain + params.luma_lift;
    let cb = (code_cb - params.chroma_offset) * params.chroma_gain;
    let cr = (code_cr - params.chroma_offset) * params.chroma_gain;
    let u = cb * params.chroma_cos + cr * params.chroma_sin;
    let v = cr * params.chroma_cos - cb * params.chroma_sin;
    let kg = 1.0 - params.kr - params.kb;
    let rgb = vec3<f32>(
        y + 2.0 * (1.0 - params.kr) * v,
        y - (2.0 * (1.0 - params.kb) * params.kb / kg) * u
          - (2.0 * (1.0 - params.kr) * params.kr / kg) * v,
        y + 2.0 * (1.0 - params.kb) * u,
    );
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

