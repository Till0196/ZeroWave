    // コーミングの少ない組み合わせ（comb.wgsl が数えた）でフィールドを組み合わせる。c に
    // 傾け、p はコーミングが四分の三より少なくなるときだけ（192/256）。
    // 組み合わせた上で、縞が四行にわたって交互で動いている画素だけ動き適応（三次）で補間。
    let shifted = combed[1] * 4u < combed[0] * 3u;
    var b: f32;
    var two_below: f32;
    if (shifted) {
        b = {before}(p.x, p.y);
        two_below = {before}(p.x, p.y + 2);
    } else {
        b = {cur}(p.x, p.y);
        two_below = {cur}(p.x, p.y + 2);
    }
    let above = {cur}(p.x, p.y - 1);
    let below = {cur}(p.x, p.y + 1);
    let moving = abs({cur}(p.x, p.y) - {before}(p.x, p.y)) > params.motion;
    let t2 = params.comb * params.comb;
    let striped = (above - b) * (b - below) < -t2 && (b - below) * (below - two_below) < -t2;
    if (!(moving && striped)) {
        return b;
    }
