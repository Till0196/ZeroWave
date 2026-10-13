// オーバーレイを、サーフェスが既に持っているものの上へリニア光で合成して重ねる
// （`colour::Mapping::lay`）。`blit.wgsl` の後ろに繋いで使う。
//
// サーフェスのブレンドは渡された符号値を混ぜる。PQ や HLG のサーフェスではそれはリニア光では
// なく、黒の 60% を 200 cd/m² のフレームに重ねると 80 cd/m² が残るべきところ、
// PQ で混ぜると 25 しか残らない。
// なのでこのパスは、サーフェスのコピー（`below`。サーフェスそのものは読めない）を読んで
// 曲線を外し、オーバーレイとリニア光で合成して、曲線を掛け直して書く。ブレンドは使わない。
@group(0) @binding(3) var below: texture_2d<f32>;

@fragment
fn fs_over(input: VsOut) -> @location(0) vec4<f32> {
    let taken = textureSample(picture, display, input.uv);
    // オーバーレイは乗算済み。割り戻してから曲線を掛ける（`fs` と同じ）。
    var rgb = taken.rgb;
    if (taken.a > 0.0) {
        rgb = rgb / taken.a;
    }
    // オーバーレイはサーフェスいっぱいに重なるので、フラグメントの位置がそのままサーフェスの画素。
    let under = textureLoad(below, vec2<i32>(input.position.xy), 0).rgb;
    let mixed = to_nits(rgb) * taken.a + undo(under) * (1.0 - taken.a);
    return vec4<f32>(encode(mixed), 1.0);
}
