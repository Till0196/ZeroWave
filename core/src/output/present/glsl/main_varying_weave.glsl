
void main() {
    float row = floor(gl_FragCoord.y);
    float crow = floor(row * 0.5);
    float y;
    vec2 c;
    if (abs(mod(row, 2.0) - mate_row) < 0.5) {
        y = luma_at(y_before_t, uv_y0);
    } else {
        y = luma_at(y_cur_t, uv_y0);
    }
    if (abs(mod(crow, 2.0) - mate_row) < 0.5) {
        c = chroma_at(c_before_t, uv_c0);
    } else {
        c = chroma_at(c_cur_t, uv_c0);
    }
    gl_FragColor = to_rgb(y, c.x, c.y);
}
