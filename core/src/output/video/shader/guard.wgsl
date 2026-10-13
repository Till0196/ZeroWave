    // The rows this field actually carries are woven through untouched.
    if ((p.y & 1) == i32(params.first_row)) {
        return {plane}_cur(p.x, p.y);
    }
