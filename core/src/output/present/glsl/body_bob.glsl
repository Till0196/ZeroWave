    float m1 = {cur}(p.x, p.y - 1.0);
    float p1 = {cur}(p.x, p.y + 1.0);
    return clamp((m1 + p1) * 0.5, 0.0, 1.0);
