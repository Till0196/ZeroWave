
float at_y(sampler2D t, float x, float y) {
    return texture2D(t, spot(luma_size, x, y)).r;
}

// 色差の対を、両方を持つ1枚から読む。Cb が第1成分で Cr が第2成分、
// つまり `GL_LUMINANCE_ALPHA` の L と A。
vec2 at_c(sampler2D t, float x, float y) {
    vec4 s = texture2D(t, spot(chroma_size, x, y));
    return vec2(s.r, s.a);
}
