
float byte_of(float value) { return floor(value * 255.0 + 0.5); }

float at_y(sampler2D t, float x, float y) {
    vec4 s = texture2D(t, spot(luma_size, x, y));
    return (byte_of(s.r) + byte_of(s.a) * 256.0) / {full};
}

vec2 at_c(sampler2D t, float x, float y) {
    vec4 s = texture2D(t, spot(chroma_size, x, y));
    return vec2(byte_of(s.r) + byte_of(s.g) * 256.0,
                byte_of(s.b) + byte_of(s.a) * 256.0) / {full};
}
