    if (abs(mod(p.y, 2.0) - mate_row) < 0.5) {
        return {before}(p.x, p.y);
    }
    return {cur}(p.x, p.y);
