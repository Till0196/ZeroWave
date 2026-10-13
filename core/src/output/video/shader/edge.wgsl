
fn edge_score_{plane}(p: vec2<i32>, j: i32) -> f32 {
    return abs({cur}(p.x - 1 + j, p.y - 1) - {cur}(p.x - 1 - j, p.y + 1))
         + abs({cur}(p.x + j, p.y - 1) - {cur}(p.x - j, p.y + 1))
         + abs({cur}(p.x + 1 + j, p.y - 1) - {cur}(p.x + 1 - j, p.y + 1));
}

fn edge_{plane}(p: vec2<i32>) -> i32 {
    var best = 0;
    var best_score = edge_score_{plane}(p, 0);
    let left = edge_score_{plane}(p, -1) + {slant};
    if (left < best_score) {
        best = -1;
        best_score = left;
    }
    let right = edge_score_{plane}(p, 1) + {slant};
    if (right < best_score) {
        best = 1;
        best_score = right;
    }
    let far_left = edge_score_{plane}(p, -2) + 2.0 * {slant};
    if (far_left < best_score) {
        best = -2;
        best_score = far_left;
    }
    let far_right = edge_score_{plane}(p, 2) + 2.0 * {slant};
    if (far_right < best_score) {
        best = 2;
    }
    return best;
}
