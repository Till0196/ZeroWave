
fn motion_{plane}(p: vec2<i32>) -> vec2<f32> {
    let average = ({before}(p.x, p.y) + {after}(p.x, p.y)) * 0.5;
    let change = abs({before}(p.x, p.y) - {after}(p.x, p.y));
    let c = {cur}(p.x, p.y - 1);
    let e = {cur}(p.x, p.y + 1);
    let above = max(abs({before}(p.x, p.y - 1) - c), abs({after}(p.x, p.y - 1) - c));
    let below = max(abs({before}(p.x, p.y + 1) - e), abs({after}(p.x, p.y + 1) - e));
    let outside = max(max(min(c, e) - average, average - max(c, e)), 0.0);
    return vec2<f32>(average, max(max(change, max(above, below)) * 0.5, outside));
}
