// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GPU なしでテストするためのデインタレーサー。[`super::shader`] と同じ順・係数・クランプで
//! 書く（`tests/golden` で突き合わせる）。

use mediadecoder::video::{Frame, Plane};

use super::colour::{self, Conversion};
use super::field::{Output, Parity, Source};
use super::{Deinterlace, Settings};

/// シェーダーのように読む 1 つのプレーン：整数座標、端でクランプ、保存幅で正規化。stride を守る。
#[derive(Debug, Clone, Copy)]
pub struct Samples<'a> {
    plane: &'a Plane,
    depth: u8,
    component: usize,
    components: usize,
    /// `P010` は 6、ほかは 0。
    shift: u32,
    /// この成分のサンプル単位での映像の幅（共有するプレーンではプレーンの幅と違う）。
    width: usize,
}

impl<'a> Samples<'a> {
    pub fn new(plane: &'a Plane, depth: u8) -> Self {
        Self {
            plane,
            depth,
            component: 0,
            components: 1,
            shift: 0,
            width: plane.width,
        }
    }

    pub fn component(
        plane: &'a Plane,
        depth: u8,
        component: usize,
        components: usize,
        shift: u32,
    ) -> Self {
        Self {
            plane,
            depth,
            component,
            components: components.max(1),
            shift,
            width: plane.width / components.max(1),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.plane.height
    }

    pub fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.plane.height as i32 - 1) as usize;
        let full = ((1u32 << self.depth.clamp(8, 16)) - 1) as f32;
        self.code(x, y) as f32 / full
    }

    /// 八ビットまでは一バイト、超えれば二バイトのリトルエンディアン。
    pub fn code(&self, x: usize, y: usize) -> u32 {
        let row = self.plane.row(y);
        let x = x * self.components + self.component;
        let raw = if self.depth <= 8 {
            row.get(x).copied().unwrap_or(0) as u32
        } else {
            let at = x * 2;
            let low = row.get(at).copied().unwrap_or(0) as u32;
            let high = row.get(at + 1).copied().unwrap_or(0) as u32;
            low | (high << 8)
        };
        raw >> self.shift
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Planes<'a> {
    pub y: Samples<'a>,
    pub cb: Samples<'a>,
    pub cr: Samples<'a>,
}

impl<'a> Planes<'a> {
    pub fn planar(frame: &'a Frame) -> Self {
        let depth = frame.format.depth;
        Self {
            y: Samples::new(&frame.planes[0], depth),
            cb: Samples::new(&frame.planes[1], depth),
            cr: Samples::new(&frame.planes[2], depth),
        }
    }

    /// 輝度と、色差の対が一つのプレーン（`NV12`、`P010`）。
    pub fn biplanar(luma: &'a Plane, chroma: &'a Plane, depth: u8) -> Self {
        let shift = if depth > 8 { 16 - u32::from(depth) } else { 0 };
        Self {
            y: Samples::component(luma, depth, 0, 1, shift),
            cb: Samples::component(chroma, depth, 0, 2, shift),
            cr: Samples::component(chroma, depth, 1, 2, shift),
        }
    }
}

/// `before`・`after` は時間の前後の反対パリティのフィールド。
pub fn sample(
    quality: Deinterlace,
    before: &Samples<'_>,
    cur: &Samples<'_>,
    after: &Samples<'_>,
    rows: Rows,
    x: i32,
    y: i32,
) -> f32 {
    if quality == Deinterlace::Weave {
        if y.rem_euclid(2) as u32 == rows.mate {
            return before.at(x, y);
        }
        return cur.at(x, y);
    }
    if y.rem_euclid(2) as u32 == rows.first {
        return cur.at(x, y);
    }
    match quality {
        Deinterlace::Weave => cur.at(x, y),
        Deinterlace::Bob => ((cur.at(x, y - 1) + cur.at(x, y + 1)) * 0.5).clamp(0.0, 1.0),
        Deinterlace::Yadif => linear(before, cur, after, x, y),
        Deinterlace::Bwdif => cubic(before, cur, after, x, y),
    }
}

/// 行は偶か奇なので 2 は無し。
pub const NO_MATE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rows {
    /// 見せるフィールドの行（0 か 1）。
    pub first: u32,
    /// 無ければ [`NO_MATE`]。
    pub mate: u32,
}

impl Rows {
    pub fn of(output: &Output) -> Self {
        Self {
            first: output.field.map_or(0, Parity::first_row),
            mate: output.weave.map_or(NO_MATE, Parity::first_row),
        }
    }

    pub fn field(first_row: u32) -> Self {
        Self {
            first: first_row,
            mate: NO_MATE,
        }
    }
}

/// 斜めの向きを取るのに要る、真上より揃っている量。偶然揃った斜めに平らな所を崩されないため。
pub const SLANT_BIAS: f32 = 0.002;

/// 時間の手がかり：抜けた行の前後のフィールドの平均と、動きの幅。止まっている所では幅が 0 に
/// なり平均がそのまま使われる。平均が上下の行の間に無いときは少なくともそこまで開く。
fn motion(
    before: &Samples<'_>,
    cur: &Samples<'_>,
    after: &Samples<'_>,
    x: i32,
    y: i32,
) -> (f32, f32) {
    let average = (before.at(x, y) + after.at(x, y)) * 0.5;
    let change = (before.at(x, y) - after.at(x, y)).abs();
    let c = cur.at(x, y - 1);
    let e = cur.at(x, y + 1);
    let above = (before.at(x, y - 1) - c)
        .abs()
        .max((after.at(x, y - 1) - c).abs());
    let below = (before.at(x, y + 1) - e)
        .abs()
        .max((after.at(x, y + 1) - e).abs());
    let outside = (c.min(e) - average).max(average - c.max(e)).max(0.0);
    (average, (change.max(above).max(below) * 0.5).max(outside))
}

/// 辺の向き（ELA）：真上と左右に一つ・二つずらした五つの向きで、三画素の幅の差の和が最小の向き。
/// `j` は上の行で右へ、下の行で左へ `j` ずれた向き。
pub fn edge(cur: &Samples<'_>, x: i32, y: i32) -> i32 {
    let score = |j: i32| {
        (-1..=1)
            .map(|k| (cur.at(x + k + j, y - 1) - cur.at(x + k - j, y + 1)).abs())
            .sum::<f32>()
    };
    let mut best = 0;
    let mut best_score = score(0);
    for j in [-1, 1, -2, 2] {
        let slanted = score(j) + SLANT_BIAS * j.abs() as f32;
        if slanted < best_score {
            best = j;
            best_score = slanted;
        }
    }
    best
}

/// 辺の向きに沿った上下二画素の平均を、動きの幅でクランプする。
fn linear(before: &Samples<'_>, cur: &Samples<'_>, after: &Samples<'_>, x: i32, y: i32) -> f32 {
    let (average, moved) = motion(before, cur, after, x, y);
    let j = edge(cur, x, y);
    let along = (cur.at(x + j, y - 1) + cur.at(x - j, y + 1)) * 0.5;
    along
        .clamp(average - moved, average + moved)
        .clamp(0.0, 1.0)
}

/// 辺の向きに沿った四画素の三次の内挿（-1, 9, 9, -1 の 16 分の 1）を、動きの幅でクランプする。
fn cubic(before: &Samples<'_>, cur: &Samples<'_>, after: &Samples<'_>, x: i32, y: i32) -> f32 {
    let (average, moved) = motion(before, cur, after, x, y);
    let j = edge(cur, x, y);
    let near = cur.at(x + j, y - 1) + cur.at(x - j, y + 1);
    let far = cur.at(x + 3 * j, y - 3) + cur.at(x - 3 * j, y + 3);
    let along = (9.0 * near - far) / 16.0;
    along
        .clamp(average - moved, average + moved)
        .clamp(0.0, 1.0)
}

pub fn plane(
    quality: Deinterlace,
    before: &Samples<'_>,
    cur: &Samples<'_>,
    after: &Samples<'_>,
    first_row: u32,
) -> Vec<f32> {
    let rows = Rows::field(first_row);
    let mut out = Vec::with_capacity(cur.width() * cur.height());
    for y in 0..cur.height() as i32 {
        for x in 0..cur.width() as i32 {
            out.push(sample(quality, before, cur, after, rows, x, y));
        }
    }
    out
}

/// ゴールデン画像の中身。シェーダーと同じ順。
pub fn picture(
    before: &Frame,
    cur: &Frame,
    after: &Frame,
    output: &Output,
    settings: Settings,
) -> Vec<u8> {
    picture_of(
        &Planes::planar(before),
        &Planes::planar(cur),
        &Planes::planar(after),
        cur.format.depth,
        output,
        settings,
    )
}

/// どの出力がどれを読むかは [`Output`] が決める。
pub fn picture_of(
    before: &Planes<'_>,
    cur: &Planes<'_>,
    after: &Planes<'_>,
    depth: u8,
    output: &Output,
    settings: Settings,
) -> Vec<u8> {
    let pick = |source: Source| match source {
        Source::Previous => before,
        Source::Current => cur,
        Source::Next => after,
    };
    let before = pick(output.before);
    let after = pick(output.after);
    let rows = Rows::of(output);
    let quality = match output.field {
        None => Deinterlace::Weave,
        Some(_) => settings.deinterlace,
    };
    let conversion = Conversion::new(settings.matrix, depth, settings.limited);

    let width = cur.y.width();
    let height = cur.y.height();
    let mut out = Vec::with_capacity(width * height * 4);
    let luma = [before.y, cur.y, after.y];
    let cb = [before.cb, cur.cb, after.cb];
    let cr = [before.cr, cur.cr, after.cr];
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let sample_of = |plane: &[Samples<'_>; 3], x: i32, y: i32| {
                sample(quality, &plane[0], &plane[1], &plane[2], rows, x, y)
            };
            let rgb = conversion.to_rgb(
                sample_of(&luma, x, y),
                sample_of(&cb, x / 2, y / 2),
                sample_of(&cr, x / 2, y / 2),
            );
            out.extend_from_slice(&rgb.map(colour::quantise));
            out.push(255);
        }
    }
    out
}

/// 各列の二次差分の和。効いているデインタレーサーは bob より小さい。
pub fn comb(plane: &Plane, depth: u8) -> u64 {
    let samples = Samples::new(plane, depth);
    let mut total = 0u64;
    for y in 1..plane.height.saturating_sub(1) {
        for x in 0..plane.width {
            let up = samples.code(x, y - 1) as i64;
            let here = samples.code(x, y) as i64;
            let down = samples.code(x, y + 1) as i64;
            total += (2 * here - up - down).unsigned_abs();
        }
    }
    total
}

pub fn comb_rgba(rgba: &[u8], width: usize, height: usize) -> u64 {
    let green = |x: usize, y: usize| rgba[(y * width + x) * 4 + 1] as i64;
    let mut total = 0u64;
    for y in 1..height.saturating_sub(1) {
        for x in 0..width {
            total += (2 * green(x, y) - green(x, y - 1) - green(x, y + 1)).unsigned_abs();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: usize, height: usize, value: u8) -> Plane {
        Plane {
            data: vec![value; width * height],
            stride: width,
            width,
            height,
        }
    }

    #[test]
    fn a_still_picture_of_one_value_comes_back_unchanged() {
        let plane = flat(16, 16, 200);
        let samples = Samples::new(&plane, 8);
        for quality in [Deinterlace::Bob, Deinterlace::Yadif, Deinterlace::Bwdif] {
            for y in 0..16 {
                let got = sample(quality, &samples, &samples, &samples, Rows::field(0), 8, y);
                assert!(
                    (got - 200.0 / 255.0).abs() < 1e-5,
                    "{quality:?} row {y} gave {got}"
                );
            }
        }
    }

    #[test]
    fn the_rows_the_field_owns_are_left_alone() {
        let mut plane = flat(8, 8, 0);
        for y in 0..8 {
            for x in 0..8 {
                plane.data[y * 8 + x] = (y * 8 + x) as u8;
            }
        }
        let samples = Samples::new(&plane, 8);
        for first_row in [0, 1] {
            for y in (first_row as i32..8).step_by(2) {
                let got = sample(
                    Deinterlace::Bwdif,
                    &samples,
                    &samples,
                    &samples,
                    Rows::field(first_row),
                    3,
                    y,
                );
                assert!((got - samples.at(3, y)).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn a_moving_edge_is_held_to_what_this_field_says() {
        let white = flat(8, 8, 235);
        let black = flat(8, 8, 16);
        let (white, black) = (Samples::new(&white, 8), Samples::new(&black, 8));
        for quality in [Deinterlace::Yadif, Deinterlace::Bwdif] {
            let got = sample(quality, &black, &white, &black, Rows::field(0), 4, 3);
            assert!(got > 0.8, "{quality:?} gave {got}");
        }
    }

    #[test]
    fn the_comb_metric_sees_combing_and_nothing_else() {
        assert_eq!(comb(&flat(16, 16, 128), 8), 0);
        let mut striped = flat(16, 16, 0);
        for y in 0..16 {
            for x in 0..16 {
                striped.data[y * 16 + x] = if y % 2 == 0 { 235 } else { 16 };
            }
        }
        assert_eq!(comb(&striped, 8), 14 * 16 * 2 * 219);
    }

    #[test]
    fn two_planes_and_three_filter_to_the_same_picture() {
        use mediadecoder::es::Format;
        use mediadecoder::video::Frame;

        let (width, height) = (24usize, 16usize);
        let of =
            |x: usize, y: usize, plane: usize| ((x * 7 + y * 13 + plane * 29) % 200 + 20) as u8;
        let plane = |width: usize, height: usize, plane: usize| Plane {
            data: (0..width * height)
                .map(|at| of(at % width, at / width, plane))
                .collect(),
            stride: width,
            width,
            height,
        };
        let format = Format {
            width: width as u32,
            height: height as u32,
            ..Format::default()
        };
        let frame = Frame {
            tag: 0,
            format,
            planes: [
                plane(width, height, 0),
                plane(width / 2, height / 2, 1),
                plane(width / 2, height / 2, 2),
            ],
            structure: mediadecoder::es::Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: false,
        };
        let chroma = Plane {
            data: (0..(width / 2) * (height / 2) * 2)
                .map(|at| {
                    let x = (at / 2) % (width / 2);
                    let y = (at / 2) / (width / 2);
                    of(x, y, 1 + at % 2)
                })
                .collect(),
            stride: width,
            width,
            height: height / 2,
        };

        for quality in [
            Deinterlace::Bob,
            Deinterlace::Yadif,
            Deinterlace::Bwdif,
            Deinterlace::Weave,
        ] {
            let settings = Settings {
                deinterlace: quality,
                ..Settings::default()
            };
            let output = Output {
                field: Some(Parity::Top),
                weave: None,
                before: Source::Previous,
                after: Source::Next,
                at: 0,
                of: 2,
                matched: false,
            };
            let three = Planes::planar(&frame);
            let two = Planes::biplanar(&frame.planes[0], &chroma, 8);
            assert_eq!(
                picture_of(&three, &three, &three, 8, &output, settings),
                picture_of(&two, &two, &two, 8, &output, settings),
                "{quality:?}"
            );
        }
    }

    #[test]
    fn ten_bit_samples_are_read_as_two_bytes() {
        let plane = Plane {
            data: vec![0x00, 0x02, 0xff, 0x03],
            stride: 4,
            width: 2,
            height: 1,
        };
        let samples = Samples::new(&plane, 10);
        assert_eq!(samples.code(0, 0), 512);
        assert_eq!(samples.code(1, 0), 1023);
        assert!((samples.at(1, 0) - 1.0).abs() < 1e-6);
    }
}
