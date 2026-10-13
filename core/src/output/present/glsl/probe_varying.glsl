void main() {
    gl_FragColor = vec4(
        texture2D(y_cur_t, uv_y0).r,
        texture2D(y_cur_t, uv_ym).r,
        texture2D(y_cur_t, uv_yp).r,
        1.0);
}
