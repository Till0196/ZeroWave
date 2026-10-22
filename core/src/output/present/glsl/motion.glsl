// yadif・bwdif の時間と空間の判定。返すのは前後のフレームの平均と、そこから動いてよい幅。
vec2 motion_{plane}(vec2 p) {
    float d = ({before}(p.x, p.y) + {after}(p.x, p.y)) * 0.5;
    float c = {cur}(p.x, p.y - 1.0);
    float e = {cur}(p.x, p.y + 1.0);
    float changed = abs({before}(p.x, p.y) - {after}(p.x, p.y)) * 0.5;
    float from_before = (abs({before}(p.x, p.y - 1.0) - c) + abs({before}(p.x, p.y + 1.0) - e)) * 0.5;
    float from_after = (abs({after}(p.x, p.y - 1.0) - c) + abs({after}(p.x, p.y + 1.0) - e)) * 0.5;
    float b = ({before}(p.x, p.y - 2.0) + {after}(p.x, p.y - 2.0)) * 0.5 - c;
    float f = ({before}(p.x, p.y + 2.0) + {after}(p.x, p.y + 2.0)) * 0.5 - e;
    float most = max(max(d - e, d - c), min(b, f));
    float least = min(min(d - e, d - c), max(b, f));
    float temporal = max(changed, max(from_before, from_after));
    return vec2(d, max(temporal, max(least, -most)));
}
