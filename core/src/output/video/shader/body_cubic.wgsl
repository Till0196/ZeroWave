    let m = motion_{plane}(p);
    let j = edge_{plane}(p);
    let near = {cur}(p.x + j, p.y - 1) + {cur}(p.x - j, p.y + 1);
    let far = {cur}(p.x + 3 * j, p.y - 3) + {cur}(p.x - 3 * j, p.y + 3);
    // 辺に沿った三次の内挿（-1, 9, 9, -1 の 16 分の 1）。
    let along = (9.0 * near - far) / 16.0;
    return clamp(clamp(along, m.x - m.y, m.x + m.y), 0.0, 1.0);
