
float byte_of(float value) { return floor(value * 255.0 + 0.5); }
float luma_at(sampler2D t, vec2 uv) {
    vec4 s = texture2D(t, uv);
    return (byte_of(s.r) + byte_of(s.a) * 256.0) / {full};
}
vec2 chroma_at(sampler2D t, vec2 uv) {
    vec4 s = texture2D(t, uv);
    return vec2(byte_of(s.r) + byte_of(s.g) * 256.0,
                byte_of(s.b) + byte_of(s.a) * 256.0) / {full};
}
