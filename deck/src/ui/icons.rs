// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! UI のアイコン。文字の記号はフォントで形が変わり mac では絵文字に落ちるので、egui のシェイプで組み立てる。
//! 形は 24 × 24 のグリッドで決める。

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2};

/// 線の太さ（グリッドの単位）。小さく表示しても潰れないよう、点では [`LEAST_WEIGHT`] より細くしない。
const WEIGHT: f32 = 2.2;
const LEAST_WEIGHT: f32 = 1.2;
const ROUND: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    PlayPause,
    Stop,
    Record,
    FastForward,
    Rewind,
    SkipForward,
    SkipBack,
    Camera,
    Plus,
    Minus,
    Speaker,
    /// スピーカーの右の波（[`Icon::Speaker`] と重ねる）。
    Waves,
    /// スピーカーの右の ✕（[`Icon::Speaker`] と重ねる。色を変えたいとき）。
    MutedMark,
    Mute,
    Check,
    Cross,
    More,
    /// ⋮⋮（ドラッグハンドル）。
    Grip,
    TriUp,
    TriDown,
    TriLeft,
    TriRight,
    ChevUp,
    ChevDown,
    ChevLeft,
    ChevRight,
    DoubleUp,
    DoubleDown,
    DoubleLeft,
    DoubleRight,
    ArrowUp,
    ArrowDown,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, colour: Color32) {
    let side = rect.width().min(rect.height());
    if side <= 0.0 {
        return;
    }
    let pen = Pen {
        origin: rect.center() - egui::vec2(side, side) / 2.0,
        scale: side / 24.0,
        colour,
    };
    let mut out = Vec::new();
    pen.shapes(icon, &mut out);
    painter.extend(out);
}

pub fn paint_at(painter: &Painter, centre: Pos2, side: f32, icon: Icon, colour: Color32) {
    paint(
        painter,
        Rect::from_center_size(centre, egui::vec2(side, side)),
        icon,
        colour,
    );
}

struct Pen {
    origin: Pos2,
    scale: f32,
    colour: Color32,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.origin.x + x * self.scale,
            self.origin.y + y * self.scale,
        )
    }

    fn stroke(&self) -> Stroke {
        Stroke::new((WEIGHT * self.scale).max(LEAST_WEIGHT), self.colour)
    }

    /// egui は時計回り（y 下向き）で縁のぼかしを外に付けるので、向きを揃える。
    fn fill(&self, points: &[(f32, f32)]) -> Shape {
        let mut points: Vec<Pos2> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        let area: f32 = (0..points.len())
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                a.x * b.y - b.x * a.y
            })
            .sum();
        if area < 0.0 {
            points.reverse();
        }
        Shape::convex_polygon(points, self.colour, Stroke::NONE)
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        Shape::rect_filled(
            Rect::from_min_max(self.at(x0, y0), self.at(x1, y1)),
            ROUND * self.scale,
            self.colour,
        )
    }

    fn dot(&self, x: f32, y: f32, r: f32) -> Shape {
        Shape::circle_filled(self.at(x, y), r * self.scale, self.colour)
    }

    fn line(&self, points: &[(f32, f32)]) -> Shape {
        Shape::line(
            points.iter().map(|&(x, y)| self.at(x, y)).collect(),
            self.stroke(),
        )
    }

    fn closed(&self, points: &[(f32, f32)]) -> Shape {
        Shape::closed_line(
            points.iter().map(|&(x, y)| self.at(x, y)).collect(),
            self.stroke(),
        )
    }

    fn chevron(&self, dx: f32, dy: f32, shift: f32) -> Shape {
        let (cx, cy) = (12.0 + dx * shift, 12.0 + dy * shift);
        let (px, py) = (-dy, dx);
        self.line(&[
            (cx - dx * 3.5 + px * 7.0, cy - dy * 3.5 + py * 7.0),
            (cx + dx * 3.5, cy + dy * 3.5),
            (cx - dx * 3.5 - px * 7.0, cy - dy * 3.5 - py * 7.0),
        ])
    }

    fn shapes(&self, icon: Icon, out: &mut Vec<Shape>) {
        match icon {
            Icon::Play => out.push(self.fill(&[(7.0, 4.5), (19.5, 12.0), (7.0, 19.5)])),
            Icon::Pause => {
                out.push(self.rect(6.0, 4.5, 10.5, 19.5));
                out.push(self.rect(13.5, 4.5, 18.0, 19.5));
            }
            Icon::PlayPause => {
                out.push(self.fill(&[(2.5, 6.0), (12.0, 12.0), (2.5, 18.0)]));
                out.push(self.rect(13.5, 6.0, 16.5, 18.0));
                out.push(self.rect(19.0, 6.0, 22.0, 18.0));
            }
            Icon::Stop => out.push(self.rect(5.5, 5.5, 18.5, 18.5)),
            Icon::Record => out.push(self.dot(12.0, 12.0, 7.0)),
            Icon::FastForward => {
                out.push(self.fill(&[(2.5, 6.0), (12.0, 12.0), (2.5, 18.0)]));
                out.push(self.fill(&[(12.0, 6.0), (21.5, 12.0), (12.0, 18.0)]));
            }
            Icon::Rewind => {
                out.push(self.fill(&[(21.5, 6.0), (12.0, 12.0), (21.5, 18.0)]));
                out.push(self.fill(&[(12.0, 6.0), (2.5, 12.0), (12.0, 18.0)]));
            }
            Icon::SkipForward => {
                out.push(self.fill(&[(4.5, 6.0), (15.0, 12.0), (4.5, 18.0)]));
                out.push(self.rect(16.5, 6.0, 19.5, 18.0));
            }
            Icon::SkipBack => {
                out.push(self.fill(&[(19.5, 6.0), (9.0, 12.0), (19.5, 18.0)]));
                out.push(self.rect(4.5, 6.0, 7.5, 18.0));
            }
            Icon::Camera => {
                out.push(self.closed(&[
                    (3.0, 8.0),
                    (7.5, 8.0),
                    (9.5, 5.0),
                    (14.5, 5.0),
                    (16.5, 8.0),
                    (21.0, 8.0),
                    (21.0, 19.0),
                    (3.0, 19.0),
                ]));
                out.push(Shape::circle_stroke(
                    self.at(12.0, 13.5),
                    3.5 * self.scale,
                    self.stroke(),
                ));
            }
            Icon::Plus => {
                out.push(self.line(&[(5.0, 12.0), (19.0, 12.0)]));
                out.push(self.line(&[(12.0, 5.0), (12.0, 19.0)]));
            }
            Icon::Minus => out.push(self.line(&[(5.0, 12.0), (19.0, 12.0)])),
            Icon::Speaker => {
                out.push(self.fill(&[(3.0, 9.0), (8.5, 9.0), (8.5, 15.0), (3.0, 15.0)]));
                out.push(self.fill(&[(7.5, 9.0), (13.0, 4.5), (13.0, 19.5), (7.5, 15.0)]));
            }
            Icon::Waves => {
                for (radius, sweep) in [(4.5f32, 0.8f32), (8.0, 0.9)] {
                    let points: Vec<(f32, f32)> = (0..=12)
                        .map(|step| {
                            let angle = -sweep + 2.0 * sweep * step as f32 / 12.0;
                            (13.0 + radius * angle.cos(), 12.0 + radius * angle.sin())
                        })
                        .collect();
                    out.push(self.line(&points));
                }
            }
            Icon::MutedMark => {
                out.push(self.line(&[(15.5, 9.0), (21.5, 15.0)]));
                out.push(self.line(&[(21.5, 9.0), (15.5, 15.0)]));
            }
            Icon::Mute => {
                self.shapes(Icon::Speaker, out);
                self.shapes(Icon::MutedMark, out);
            }
            Icon::Check => out.push(self.line(&[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)])),
            Icon::Cross => {
                out.push(self.line(&[(6.0, 6.0), (18.0, 18.0)]));
                out.push(self.line(&[(18.0, 6.0), (6.0, 18.0)]));
            }
            Icon::More => {
                for x in [5.5, 12.0, 18.5] {
                    out.push(self.dot(x, 12.0, 1.8));
                }
            }
            Icon::Grip => {
                for x in [9.0, 15.0] {
                    for y in [6.0, 12.0, 18.0] {
                        out.push(self.dot(x, y, 1.7));
                    }
                }
            }
            Icon::TriUp => out.push(self.fill(&[(12.0, 6.5), (19.5, 17.0), (4.5, 17.0)])),
            Icon::TriDown => out.push(self.fill(&[(4.5, 7.0), (19.5, 7.0), (12.0, 17.5)])),
            Icon::TriLeft => out.push(self.fill(&[(17.0, 4.5), (17.0, 19.5), (6.5, 12.0)])),
            Icon::TriRight => out.push(self.fill(&[(7.0, 4.5), (17.5, 12.0), (7.0, 19.5)])),
            Icon::ChevUp => out.push(self.chevron(0.0, -1.0, 0.0)),
            Icon::ChevDown => out.push(self.chevron(0.0, 1.0, 0.0)),
            Icon::ChevLeft => out.push(self.chevron(-1.0, 0.0, 0.0)),
            Icon::ChevRight => out.push(self.chevron(1.0, 0.0, 0.0)),
            Icon::DoubleUp => {
                out.push(self.chevron(0.0, -1.0, 3.5));
                out.push(self.chevron(0.0, -1.0, -3.5));
            }
            Icon::DoubleDown => {
                out.push(self.chevron(0.0, 1.0, 3.5));
                out.push(self.chevron(0.0, 1.0, -3.5));
            }
            Icon::DoubleLeft => {
                out.push(self.chevron(-1.0, 0.0, 3.5));
                out.push(self.chevron(-1.0, 0.0, -3.5));
            }
            Icon::DoubleRight => {
                out.push(self.chevron(1.0, 0.0, 3.5));
                out.push(self.chevron(1.0, 0.0, -3.5));
            }
            Icon::ArrowUp => {
                out.push(self.line(&[(12.0, 5.0), (12.0, 19.5)]));
                out.push(self.line(&[(6.0, 11.0), (12.0, 5.0), (18.0, 11.0)]));
            }
            Icon::ArrowDown => {
                out.push(self.line(&[(12.0, 4.5), (12.0, 19.0)]));
                out.push(self.line(&[(6.0, 13.0), (12.0, 19.0), (18.0, 13.0)]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Icon] = &[
        Icon::Play,
        Icon::Pause,
        Icon::PlayPause,
        Icon::Stop,
        Icon::Record,
        Icon::FastForward,
        Icon::Rewind,
        Icon::SkipForward,
        Icon::SkipBack,
        Icon::Camera,
        Icon::Plus,
        Icon::Minus,
        Icon::Speaker,
        Icon::Waves,
        Icon::MutedMark,
        Icon::Mute,
        Icon::Check,
        Icon::Cross,
        Icon::More,
        Icon::Grip,
        Icon::TriUp,
        Icon::TriDown,
        Icon::TriLeft,
        Icon::TriRight,
        Icon::ChevUp,
        Icon::ChevDown,
        Icon::ChevLeft,
        Icon::ChevRight,
        Icon::DoubleUp,
        Icon::DoubleDown,
        Icon::DoubleLeft,
        Icon::DoubleRight,
        Icon::ArrowUp,
        Icon::ArrowDown,
    ];

    fn shapes(icon: Icon, rect: Rect) -> Vec<Shape> {
        let side = rect.width().min(rect.height());
        let pen = Pen {
            origin: rect.center() - egui::vec2(side, side) / 2.0,
            scale: side / 24.0,
            colour: Color32::WHITE,
        };
        let mut out = Vec::new();
        pen.shapes(icon, &mut out);
        out
    }

    #[test]
    fn stays_inside() {
        for side in [12.0f32, 14.0, 16.0, 24.0] {
            let rect = Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(side, side));
            for &icon in ALL {
                let shapes = shapes(icon, rect);
                assert!(!shapes.is_empty(), "{icon:?}");
                for shape in shapes {
                    let bounds = shape.visual_bounding_rect();
                    assert!(
                        rect.expand(0.5).contains_rect(bounds),
                        "{icon:?} {side}: {bounds:?} は {rect:?} の外"
                    );
                }
            }
        }
    }

    #[test]
    fn fills_are_clockwise() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(24.0, 24.0));
        for &icon in ALL {
            for shape in shapes(icon, rect) {
                if let Shape::Path(path) = shape
                    && path.closed
                    && path.fill != Color32::TRANSPARENT
                {
                    let p = &path.points;
                    let area: f32 = (0..p.len())
                        .map(|i| {
                            let (a, b) = (p[i], p[(i + 1) % p.len()]);
                            a.x * b.y - b.x * a.y
                        })
                        .sum();
                    assert!(area > 0.0, "{icon:?}");
                }
            }
        }
    }

    #[test]
    fn least_weight() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(12.0, 12.0));
        for shape in shapes(Icon::Check, rect) {
            if let Shape::Path(path) = shape {
                assert!(path.stroke.width >= LEAST_WEIGHT);
            }
        }
    }
}
