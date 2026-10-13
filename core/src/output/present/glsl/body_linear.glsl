    vec2 m = motion_{plane}(p);
    float j = edge_{plane}(p);
    float along = ({cur}(p.x + j, p.y - 1.0) + {cur}(p.x - j, p.y + 1.0)) * 0.5;
    return clamp(clamp(along, m.x - m.y, m.x + m.y), 0.0, 1.0);
