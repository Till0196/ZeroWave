    if ((p.y & 1) == i32(params.mate_row)) {
        return {before}(p.x, p.y);
    }
    return {cur}(p.x, p.y);
