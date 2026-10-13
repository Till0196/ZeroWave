
vec4 to_rgb(float y, float u, float v) {
    y = (y - luma_offset) * luma_gain;
    u = (u - chroma_offset) * chroma_gain;
    v = (v - chroma_offset) * chroma_gain;
    float kg = 1.0 - kr - kb;
    vec3 rgb = vec3(
        y + 2.0 * (1.0 - kr) * v,
        y - (2.0 * (1.0 - kb) * kb / kg) * u
          - (2.0 * (1.0 - kr) * kr / kg) * v,
        y + 2.0 * (1.0 - kb) * u);
    rgb = clamp(rgb, vec3(0.0), vec3(1.0));
    // 伝達特性と原色を画面へ（SDR の BT.709 なら何もしない。`glsl::Tone`）。
    rgb = tone(rgb);
#ifdef ROUND_EIGHT
    // 8ビットへの丸めを、こちらでやる。フレームバッファに任せると、
    // ちょうど半分に落ちた値をどちらへ倒すかがドライバー次第になる。
    // llvmpipe は偶数へ、Vulkan の lavapipe は上へ倒すので、
    // 一部の成分で1だけ違うフレームになる。
    // 参照実装 `colour::quantise` は `floor(v * 255 + 0.5)` なので、
    // 同じものをここで出す。
    //
    // `highp` の無い GPU では出さない。そこでは算術自体が仮数 10 ビット
    // で、丸めの境目にいるサンプルが 0.12 コード値ほど揺れる。mediump では
    // どのみち差 1 が数 % 出る。一致させられない GPU で、ピクセルあたり
    // 9 命令を払う理由が無い。
    rgb = floor(rgb * 255.0 + vec3(0.5)) / 255.0;
#endif
    return vec4(rgb, 1.0);
}
