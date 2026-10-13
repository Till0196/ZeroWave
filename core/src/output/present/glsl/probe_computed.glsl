uniform vec2 luma_size;
vec2 spot(vec2 size, float x, float y) {
    return (clamp(vec2(x, y), vec2(0.0), size - vec2(1.0)) + vec2(0.5)) / size;
}
varying vec2 place;
void main() {
    float x = floor(place.x * luma_size.x);
    float y = floor(place.y * luma_size.y);
    gl_FragColor = vec4(
        texture2D(y_cur_t, spot(luma_size, x, y)).r,
        texture2D(y_cur_t, spot(luma_size, x, y - 1.0)).r,
        texture2D(y_cur_t, spot(luma_size, x, y + 1.0)).r,
        1.0);
}
