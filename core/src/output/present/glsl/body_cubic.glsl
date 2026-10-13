    vec2 m = motion_{plane}(p);
    float j = edge_{plane}(p);
    float near = {cur}(p.x + j, p.y - 1.0) + {cur}(p.x - j, p.y + 1.0);
    float far = {cur}(p.x + 3.0 * j, p.y - 3.0) + {cur}(p.x - 3.0 * j, p.y + 3.0);
    // 辺に沿った三次の内挿（-1, 9, 9, -1 の 16 分の 1）。
    float along = (9.0 * near - far) / 16.0;
    return clamp(clamp(along, m.x - m.y, m.x + m.y), 0.0, 1.0);
