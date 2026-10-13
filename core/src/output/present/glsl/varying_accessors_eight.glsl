
float luma_at(sampler2D t, vec2 uv) { return texture2D(t, uv).r; }
vec2 chroma_at(sampler2D t, vec2 uv) { vec4 s = texture2D(t, uv); return vec2(s.r, s.a); }
