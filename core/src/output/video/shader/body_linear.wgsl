    let m = motion_{plane}(p);
    let j = edge_{plane}(p);
    let along = ({cur}(p.x + j, p.y - 1) + {cur}(p.x - j, p.y + 1)) * 0.5;
    return clamp(clamp(along, m.x - m.y, m.x + m.y), 0.0, 1.0);
