    let m1 = {cur}(p.x, p.y - 1);
    let p1 = {cur}(p.x, p.y + 1);
    return clamp((m1 + p1) * 0.5, 0.0, 1.0);
