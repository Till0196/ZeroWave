
void main() {
    float row = floor(gl_FragCoord.y);
    float crow = floor(row * 0.5);
    float y;
    vec2 c;
    // このフィールドが持っている行はそのまま、持っていない行は上下の平均。
    if (abs(mod(row, 2.0) - first_row) < 0.5) {
        y = luma_at(y_cur_t, uv_y0);
    } else {
        y = clamp((luma_at(y_cur_t, uv_ym) + luma_at(y_cur_t, uv_yp)) * 0.5, 0.0, 1.0);
    }
    if (abs(mod(crow, 2.0) - first_row) < 0.5) {
        c = chroma_at(c_cur_t, uv_c0);
    } else {
        c = clamp((chroma_at(c_cur_t, uv_cm) + chroma_at(c_cur_t, uv_cp)) * 0.5,
                  vec2(0.0), vec2(1.0));
    }
    gl_FragColor = to_rgb(y, c.x, c.y);
}
