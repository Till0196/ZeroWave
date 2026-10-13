
struct Params {
    first_row: u32,
    mate_row: u32,
    luma_offset: f32,
    luma_gain: f32,
    chroma_offset: f32,
    chroma_gain: f32,
    kr: f32,
    kb: f32,
    width: u32,
    height: u32,
    // フィールドマッチで、画素ごとに残った縞を見るしきい値と、動きのしきい値
    // （body_match.wgsl）。
    comb: f32,
    motion: f32,
};
