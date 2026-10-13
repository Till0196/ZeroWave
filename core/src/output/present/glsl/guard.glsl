    // このフィールドが実際に持っている行は、そのまま通す。
    if (abs(mod(p.y, 2.0) - first_row) < 0.5) {
        return {plane}_cur(p.x, p.y);
    }
