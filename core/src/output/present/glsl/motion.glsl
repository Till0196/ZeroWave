
vec2 motion_{plane}(vec2 p) {
    float average = ({before}(p.x, p.y) + {after}(p.x, p.y)) * 0.5;
    float change = abs({before}(p.x, p.y) - {after}(p.x, p.y));
    float c = {cur}(p.x, p.y - 1.0);
    float e = {cur}(p.x, p.y + 1.0);
    float above = max(abs({before}(p.x, p.y - 1.0) - c), abs({after}(p.x, p.y - 1.0) - c));
    float below = max(abs({before}(p.x, p.y + 1.0) - e), abs({after}(p.x, p.y + 1.0) - e));
    float outside = max(max(min(c, e) - average, average - max(c, e)), 0.0);
    return vec2(average, max(max(change, max(above, below)) * 0.5, outside));
}
