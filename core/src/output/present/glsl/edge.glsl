
float edge_score_{plane}(vec2 p, float j) {
    return abs({cur}(p.x - 1.0 + j, p.y - 1.0) - {cur}(p.x - 1.0 - j, p.y + 1.0))
         + abs({cur}(p.x + j, p.y - 1.0) - {cur}(p.x - j, p.y + 1.0))
         + abs({cur}(p.x + 1.0 + j, p.y - 1.0) - {cur}(p.x + 1.0 - j, p.y + 1.0));
}

float edge_{plane}(vec2 p) {
    float best = 0.0;
    float best_score = edge_score_{plane}(p, 0.0);
    float left = edge_score_{plane}(p, -1.0) + {slant};
    if (left < best_score) {
        best = -1.0;
        best_score = left;
    }
    float right = edge_score_{plane}(p, 1.0) + {slant};
    if (right < best_score) {
        best = 1.0;
        best_score = right;
    }
    float far_left = edge_score_{plane}(p, -2.0) + 2.0 * {slant};
    if (far_left < best_score) {
        best = -2.0;
        best_score = far_left;
    }
    float far_right = edge_score_{plane}(p, 2.0) + 2.0 * {slant};
    if (far_right < best_score) {
        best = 2.0;
    }
    return best;
}
