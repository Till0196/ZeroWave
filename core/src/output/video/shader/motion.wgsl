// yadif・bwdif の時間と空間の判定。返すのは前後のフレームの平均と、そこから動いてよい幅。
fn motion_{plane}(p: vec2<i32>) -> vec2<f32> {
    let d = ({before}(p.x, p.y) + {after}(p.x, p.y)) * 0.5;
    let c = {cur}(p.x, p.y - 1);
    let e = {cur}(p.x, p.y + 1);
    let changed = abs({before}(p.x, p.y) - {after}(p.x, p.y)) * 0.5;
    let from_before = (abs({before}(p.x, p.y - 1) - c) + abs({before}(p.x, p.y + 1) - e)) * 0.5;
    let from_after = (abs({after}(p.x, p.y - 1) - c) + abs({after}(p.x, p.y + 1) - e)) * 0.5;
    let b = ({before}(p.x, p.y - 2) + {after}(p.x, p.y - 2)) * 0.5 - c;
    let f = ({before}(p.x, p.y + 2) + {after}(p.x, p.y + 2)) * 0.5 - e;
    let most = max(max(d - e, d - c), min(b, f));
    let least = min(min(d - e, d - c), max(b, f));
    let temporal = max(changed, max(from_before, from_after));
    return vec2<f32>(d, max(temporal, max(least, -most)));
}
