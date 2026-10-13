// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組名に付く記号（🈑 🈔 🈞 …）を、番組名から取り除いてバッジとして表示する。
//! 「無」の足し引きはせず、局が書いた記号をそのまま出す。

use egui::{Color32, FontId, Pos2, Rect, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    pub label: &'static str,
    /// バッジにポインターを重ねたときに出す説明。
    pub means: &'static str,
    pub colour: Color32,
    pub before: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Titled {
    pub pre: Vec<Attr>,
    pub name: String,
    pub post: Vec<Attr>,
}

impl Titled {
    pub fn plain(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }
}

// 記号ごとの色。
const FOREST3: Color32 = Color32::from_rgb(0x29, 0xa6, 0x34);
const CERULEAN3: Color32 = Color32::from_rgb(0x14, 0x7e, 0xb3);
const VERMILION3: Color32 = Color32::from_rgb(0xd3, 0x3d, 0x17);
const ROSE4: Color32 = Color32::from_rgb(0xf5, 0x49, 0x8b);
const ROSE3: Color32 = Color32::from_rgb(0xdb, 0x2c, 0x6f);
const SEPIA4: Color32 = Color32::from_rgb(0xaf, 0x85, 0x5a);
const SEPIA3: Color32 = Color32::from_rgb(0x94, 0x66, 0x38);
const LIME3: Color32 = Color32::from_rgb(0x8e, 0xb1, 0x25);
const GRAY3: Color32 = Color32::from_rgb(0x8f, 0x99, 0xa8);
const GRAY2: Color32 = Color32::from_rgb(0x73, 0x80, 0x91);
const VIOLET3: Color32 = Color32::from_rgb(0x9d, 0x3f, 0x9d);
const GOLD3: Color32 = Color32::from_rgb(0xd1, 0x98, 0x0b);
const TURQUOISE3: Color32 = Color32::from_rgb(0x00, 0xa3, 0x96);

/// 記号のテーブル。
const ATTRIBUTES: &[(&[&str], Attr)] = &[
    (
        &["🈟", "新"],
        Attr {
            label: "新",
            means: "新番組",
            colour: FOREST3,
            before: true,
        },
    ),
    (
        &["🈞", "再"],
        Attr {
            label: "再",
            means: "再放送",
            colour: CERULEAN3,
            before: true,
        },
    ),
    (
        &["🈡", "終"],
        Attr {
            label: "終",
            means: "最終回",
            colour: VERMILION3,
            before: true,
        },
    ),
    (
        &["🈢", "生"],
        Attr {
            label: "生",
            means: "生放送",
            colour: ROSE4,
            before: true,
        },
    ),
    (
        &["🈕", "多"],
        Attr {
            label: "多",
            means: "音声多重放送",
            colour: ROSE3,
            before: false,
        },
    ),
    (
        &["🈖", "解"],
        Attr {
            label: "解",
            means: "音声解説",
            colour: SEPIA4,
            before: false,
        },
    ),
    (
        &["🈠", "初"],
        Attr {
            label: "初",
            means: "初回放送",
            colour: LIME3,
            before: false,
        },
    ),
    (
        &["🈐", "手"],
        Attr {
            label: "手",
            means: "手話通訳放送",
            colour: SEPIA3,
            before: false,
        },
    ),
    (
        &["🈑", "字"],
        Attr {
            label: "字",
            means: "字幕放送",
            colour: GRAY3,
            before: false,
        },
    ),
    (
        &["🈓", "デ"],
        Attr {
            label: "デ",
            means: "番組連動データ放送",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🈒", "双"],
        Attr {
            label: "双",
            means: "双方向放送",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🈔", "二"],
        Attr {
            label: "二",
            means: "二ヶ国語放送",
            colour: GOLD3,
            before: false,
        },
    ),
    (
        &["🈚", "無"],
        Attr {
            label: "無",
            means: "無料放送",
            colour: TURQUOISE3,
            before: false,
        },
    ),
    (
        &["🈜", "前"],
        Attr {
            label: "前",
            means: "前編",
            colour: GRAY2,
            before: false,
        },
    ),
    (
        &["🈝", "後"],
        Attr {
            label: "後",
            means: "後編",
            colour: GRAY2,
            before: false,
        },
    ),
    (
        &["🈻", "配"],
        Attr {
            label: "配",
            means: "インターネット同時配信",
            colour: TURQUOISE3,
            before: false,
        },
    ),
    (
        &["🅍", "SS"],
        Attr {
            label: "SS",
            means: "サラウンドステレオ",
            colour: GRAY2,
            before: false,
        },
    ),
    (
        &["🅎", "PPV"],
        Attr {
            label: "PPV",
            means: "PPV",
            colour: GRAY2,
            before: false,
        },
    ),
    (
        &["🆛", "[3D]", "3D"],
        Attr {
            label: "3D",
            means: "3Dテレビ放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆜", "[2ndScr]"],
        Attr {
            label: "2nd Screen",
            means: "セカンドスクリーン",
            colour: TURQUOISE3,
            before: false,
        },
    ),
    (
        &["🆝", "[2K]", "2K"],
        Attr {
            label: "2K",
            means: "2K放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆞", "[4K]", "4K"],
        Attr {
            label: "4K",
            means: "4K放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆟", "[8K]", "8K"],
        Attr {
            label: "8K",
            means: "8K放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆠", "[5.1]"],
        Attr {
            label: "5.1ch",
            means: "5.1chサラウンド音声",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🆡", "[7.1]"],
        Attr {
            label: "7.1ch",
            means: "7.1chサラウンド音声",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🆢", "[22.2]"],
        Attr {
            label: "22.2ch",
            means: "22.2chサラウンド音声",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🆣", "[60P]"],
        Attr {
            label: "60P",
            means: "60Hzプログレッシブスキャン放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆤", "[120P]"],
        Attr {
            label: "120P",
            means: "120Hzプログレッシブスキャン放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆥", "[d]"],
        Attr {
            label: "d",
            means: "データ放送",
            colour: TURQUOISE3,
            before: false,
        },
    ),
    (
        &["🆦", "[HC]"],
        Attr {
            label: "HC",
            means: "Hybridcast",
            colour: TURQUOISE3,
            before: false,
        },
    ),
    (
        &["🆧", "[HDR]"],
        Attr {
            label: "HDR",
            means: "HDR",
            colour: GOLD3,
            before: false,
        },
    ),
    (
        &["🆨", "[Hi-Res]"],
        Attr {
            label: "Hi-Res",
            means: "ハイレゾ音声",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🆩", "[Lossless]"],
        Attr {
            label: "Lossless",
            means: "ロスレス音声",
            colour: VIOLET3,
            before: false,
        },
    ),
    (
        &["🆪", "[SHV]"],
        Attr {
            label: "SHV",
            means: "SHV放送",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆫", "[UHD]"],
        Attr {
            label: "UHD",
            means: "UHDTV",
            colour: CERULEAN3,
            before: false,
        },
    ),
    (
        &["🆬", "[VOD]"],
        Attr {
            label: "VOD",
            means: "VOD",
            colour: TURQUOISE3,
            before: false,
        },
    ),
];

/// 番組名から取り除く四角囲みの文字。
fn is_squared(letter: char) -> bool {
    matches!(
        letter,
        '\u{1F130}'..='\u{1F14E}' | '\u{1F19B}'..='\u{1F1AC}' | '\u{1F201}'..='\u{1F23B}'
    )
}

/// 記号として読む文字。
fn is_attribute_letter(letter: char) -> bool {
    matches!(
        letter,
        '\u{1F14D}'..='\u{1F14E}'
            | '\u{1F19B}'..='\u{1F1AC}'
            | '\u{1F210}'..='\u{1F222}'
            | '\u{1F23B}'
    )
}

/// 括弧書きの記号（`[新]` `【字】` `(二)` など）。記号でないものが混じった括弧は触らない。
fn legacy_at(text: &str) -> Option<(&'static str, usize)> {
    const BRACKETS: &[(char, char)] = &[
        ('[', ']'),
        ('［', '］'),
        ('【', '】'),
        ('(', ')'),
        ('（', '）'),
    ];
    let open = text.chars().next()?;
    let (_, close) = BRACKETS.iter().find(|(o, _)| *o == open)?;
    let after_open = open.len_utf8();
    let end = text[after_open..].find(*close)?;
    let inner = &text[after_open..after_open + end];
    if inner.is_empty() || inner.chars().count() > 2 {
        return None;
    }
    let attr = match inner {
        "無料" => "無",
        other => other,
    };
    let known = ATTRIBUTES
        .iter()
        .any(|(_, known)| known.label == attr && !attr.is_ascii());
    known.then_some((
        ATTRIBUTES
            .iter()
            .find(|(_, known)| known.label == attr)
            .map(|(_, known)| known.label)?,
        after_open + end + close.len_utf8(),
    ))
}

/// `arib::substitute` が書く文字列のうち、テーブルにあるものだけ（`[COOL]` などは触らない）。
fn substituted_at(text: &str) -> Option<(&'static Attr, usize)> {
    if !text.starts_with('[') {
        return None;
    }
    ATTRIBUTES.iter().find_map(|(spellings, attr)| {
        spellings
            .iter()
            .filter(|spelling| spelling.starts_with('['))
            .find(|spelling| text.starts_with(*spelling))
            .map(|spelling| (attr, spelling.len()))
    })
}

fn attribute_of(spelling: &str) -> Option<&'static Attr> {
    ATTRIBUTES
        .iter()
        .find(|(spellings, _)| spellings.contains(&spelling))
        .map(|(_, attr)| attr)
}

fn attributes_in(text: &str, into: &mut Vec<&'static Attr>) {
    let mut push = |attr: &'static Attr| {
        if !into.iter().any(|known| known.label == attr.label) {
            into.push(attr);
        }
    };
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((attr, len)) = legacy_at(rest) {
            if let Some(attr) = attribute_of(attr) {
                push(attr);
            }
            rest = &rest[len..];
            continue;
        }
        if let Some((attr, len)) = substituted_at(rest) {
            push(attr);
            rest = &rest[len..];
            continue;
        }
        let letter = rest.chars().next().unwrap_or('\0');
        if is_attribute_letter(letter) {
            let mut spelled = [0u8; 4];
            if let Some(attr) = attribute_of(letter.encode_utf8(&mut spelled)) {
                push(attr);
            }
        }
        rest = &rest[letter.len_utf8()..];
    }
}

pub fn stripped_name(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while !rest.is_empty() {
        if let Some((_, len)) = legacy_at(rest) {
            rest = &rest[len..];
            continue;
        }
        if let Some((_, len)) = substituted_at(rest) {
            rest = &rest[len..];
            continue;
        }
        let letter = rest.chars().next().unwrap_or('\0');
        if !is_squared(letter) {
            out.push(letter);
        }
        rest = &rest[letter.len_utf8()..];
    }
    out.trim().to_owned()
}

/// 記号の囲み文字を `[字]` のような括弧書きに（バッジを置けない一行の欄に）。
pub fn bracketed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars() {
        if !is_squared(letter) {
            out.push(letter);
            continue;
        }
        let mut spelled = [0u8; 4];
        let plain = match attribute_of(letter.encode_utf8(&mut spelled)) {
            Some(attr) => attr.label,
            None => match letter {
                '🈗' => "天",
                '🈘' => "交",
                '🈙' => "映",
                '🈛' => "料",
                '🈁' => "ココ",
                '🈂' => "サ",
                '🈚' => "無",
                _ => "",
            },
        };
        if plain.is_empty() {
            continue;
        }
        out.push('[');
        out.push_str(plain);
        out.push(']');
    }
    out
}

pub fn cleaned(text: &str) -> String {
    text.chars()
        .filter(|letter| !is_attribute_letter(*letter))
        .collect()
}

pub fn titled(title: &str, detail: &str) -> Titled {
    let mut found: Vec<&'static Attr> = Vec::new();
    attributes_in(title, &mut found);
    attributes_in(detail, &mut found);
    let (pre, post): (Vec<&Attr>, Vec<&Attr>) = found.iter().partition(|attr| attr.before);
    Titled {
        pre: pre.into_iter().copied().collect(),
        name: stripped_name(title),
        post: post.into_iter().copied().collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    /// 記号ごとの色の背景に白い文字。番組表と番組説明。
    Coloured,
    /// 色を付けず細い枠で小さく。OSD（映像の上で目を引かないように）。
    Outlined,
}

struct Metrics {
    font: FontId,
    pad_x: f32,
    height: f32,
    gap: f32,
    radius: f32,
    lift: f32,
}

impl Metrics {
    fn of(size: f32, look: Look) -> Self {
        let small = size
            * match look {
                Look::Coloured => 0.8,
                Look::Outlined => 0.6,
            };
        Self {
            font: FontId::proportional(small),
            pad_x: (small * 0.15).max(1.0),
            height: small * 1.2,
            gap: (size * 0.15).max(2.0),
            radius: (small * 0.1).max(1.0),
            lift: 0.0,
        }
    }
}

pub fn width(painter: &egui::Painter, attrs: &[Attr], size: f32, look: Look) -> f32 {
    let metrics = Metrics::of(size, look);
    attrs
        .iter()
        .map(|attr| {
            let galley =
                painter.layout_no_wrap(attr.label.to_owned(), metrics.font.clone(), Color32::WHITE);
            galley.size().x + metrics.pad_x * 2.0 + metrics.gap
        })
        .sum()
}

/// `at` は左端と並べる文字の枠の縦の中心。右端を返す。
pub fn draw(
    painter: &egui::Painter,
    at: Pos2,
    attrs: &[Attr],
    size: f32,
    look: Look,
    ink: Color32,
    opacity: f32,
) -> f32 {
    let metrics = Metrics::of(size, look);
    let mut x = at.x;
    for attr in attrs {
        let text = match look {
            Look::Coloured => Color32::WHITE,
            Look::Outlined => ink,
        }
        .gamma_multiply(opacity);
        let galley = painter.layout_no_wrap(attr.label.to_owned(), metrics.font.clone(), text);
        let rect = Rect::from_min_size(
            pos2(x, at.y - metrics.lift - metrics.height / 2.0),
            vec2(galley.size().x + metrics.pad_x * 2.0, metrics.height),
        );
        match look {
            Look::Coloured => {
                painter.rect_filled(rect, metrics.radius, attr.colour.gamma_multiply(opacity));
            }
            Look::Outlined => {
                painter.rect_stroke(
                    rect,
                    metrics.radius,
                    egui::Stroke::new(1.0, ink.gamma_multiply(opacity * 0.8)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        // 文字のインク部分を上下の中央に。文字の枠で揃えると文字ごとに位置がずれる。
        let ink = if galley.mesh_bounds.is_positive() {
            galley.mesh_bounds.center().y
        } else {
            galley.size().y / 2.0
        };
        let y = rect.center().y - ink;
        painter.galley(pos2(rect.left() + metrics.pad_x, y), galley, text);
        x = rect.right() + metrics.gap;
    }
    x
}

#[allow(clippy::too_many_arguments)]
pub fn draw_line(
    painter: &egui::Painter,
    at: Pos2,
    width_room: f32,
    titled: &Titled,
    font: FontId,
    colour: Color32,
    look: Look,
    opacity: f32,
) -> f32 {
    let size = font.size;
    let pre = width(painter, &titled.pre, size, look);
    let post = width(painter, &titled.post, size, look);
    let name_room = (width_room - pre - post).max(0.0);
    let name = crate::ui::one_line(painter, &titled.name, font, colour, name_room, 0.0);
    let y = crate::ui::seat_em(&name, at.y - size / 2.0, size);
    // 名前のインク部分の中央に揃える（文字の枠だとフォントによって浮いて見える）。
    let middle = if name.mesh_bounds.is_positive() {
        y + name.mesh_bounds.center().y
    } else {
        at.y
    };
    let mut x = draw(
        painter,
        pos2(at.x, middle),
        &titled.pre,
        size,
        look,
        colour,
        opacity,
    );
    painter.galley(pos2(x, y), name.clone(), colour);
    x += name.size().x;
    if !titled.post.is_empty() {
        x += Metrics::of(size, look).gap;
        x = draw(
            painter,
            pos2(x, middle),
            &titled.post,
            size,
            look,
            colour,
            opacity,
        );
    }
    x
}

/// 前のバッジは一行目の先頭、後ろのバッジは最後の行の末尾に。使った高さを返す。
pub fn draw_wrapped(
    painter: &egui::Painter,
    at: Pos2,
    width_room: f32,
    titled: &Titled,
    font: FontId,
    colour: Color32,
    leading: f32,
) -> f32 {
    let size = font.size;
    let look = Look::Coloured;
    let pre = width(painter, &titled.pre, size, look);
    let mut job = egui::text::LayoutJob::single_section(
        titled.name.clone(),
        egui::TextFormat {
            font_id: font,
            color: colour,
            line_height: Some(leading),
            ..Default::default()
        },
    );
    job.sections[0].leading_space = pre;
    job.wrap.max_width = width_room;
    let galley = painter.layout_job(job);
    let ink_middle = |row: &egui::epaint::text::PlacedRow| {
        let bounds = row.visuals.mesh_bounds;
        if bounds.is_positive() {
            at.y + row.pos.y + bounds.center().y
        } else {
            at.y + row.pos.y + leading / 2.0
        }
    };
    let first_middle = galley
        .rows
        .first()
        .map(ink_middle)
        .unwrap_or(at.y + leading / 2.0);
    draw(
        painter,
        pos2(at.x, first_middle),
        &titled.pre,
        size,
        look,
        colour,
        1.0,
    );
    painter.galley(at, galley.clone(), colour);
    let mut height = galley.size().y.max(leading);
    if !titled.post.is_empty() {
        let post = width(painter, &titled.post, size, look);
        let gap = Metrics::of(size, look).gap;
        let (end_x, last_middle) = galley
            .rows
            .last()
            .map(|row| (row.rect().right(), ink_middle(row)))
            .unwrap_or((0.0, at.y + leading / 2.0));
        let (x, middle) = if titled.name.is_empty() {
            (at.x + pre, at.y + leading / 2.0)
        } else if end_x + gap + post <= width_room {
            (at.x + end_x + gap, last_middle)
        } else {
            height += leading;
            (at.x, last_middle + leading)
        };
        draw(
            painter,
            pos2(x, middle),
            &titled.post,
            size,
            look,
            colour,
            1.0,
        );
    }
    height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(attrs: &[Attr]) -> Vec<&'static str> {
        attrs.iter().map(|attr| attr.label).collect()
    }

    #[test]
    fn enclosed_letters_become_badges() {
        let got = titled("🈟連続テレビ小説　架空の朝🈑🈔", "");
        assert_eq!(got.name, "連続テレビ小説　架空の朝");
        assert_eq!(labels(&got.pre), vec!["新"]);
        assert_eq!(labels(&got.post), vec!["字", "二"]);
    }

    #[test]
    fn legacy_and_substituted_forms_are_read() {
        let got = titled("[新]ドラマ(字)【無料】 [4K]", "");
        assert_eq!(got.name, "ドラマ");
        assert_eq!(labels(&got.pre), vec!["新"]);
        assert_eq!(labels(&got.post), vec!["字", "無", "4K"]);
        assert_eq!(titled("[COOL]な番組", "").name, "[COOL]な番組");
        let got = titled("【字】[二]（解）ニュース ［生］", "");
        assert_eq!(labels(&got.pre), vec!["生"]);
        assert_eq!(labels(&got.post), vec!["字", "二", "解"]);
        assert_eq!(got.name, "ニュース");
        assert_eq!(titled("[字/日英]映画", "").name, "[字/日英]映画");
        assert_eq!(titled("(A)組", "").name, "(A)組");
    }

    #[test]
    fn attributes_in_the_detail_count() {
        let got = titled("番組", "🈖音声解説あり");
        assert_eq!(labels(&got.post), vec!["解"]);
        assert_eq!(cleaned("🈖音声解説あり"), "音声解説あり");
    }

    #[test]
    fn free_is_shown_as_written() {
        assert_eq!(labels(&titled("🈚番組", "").post), vec!["無"]);
        assert!(titled("番組", "").post.is_empty());
    }

    #[test]
    fn a_repeated_letter_makes_one_badge() {
        assert_eq!(labels(&titled("🈑番組🈑", "🈑").post), vec!["字"]);
    }

    #[test]
    fn bracketed_spells_the_letters_out() {
        assert_eq!(bracketed("🈟架空の朝🈑🈔"), "[新]架空の朝[字][二]");
        assert_eq!(bracketed("🈗天気"), "[天]天気");
        assert_eq!(bracketed("🆞映画"), "[4K]映画");
    }

    #[test]
    fn unknown_enclosed_letters_are_only_removed() {
        let got = titled("🈗天気🈙映画", "");
        assert_eq!(got.name, "天気映画");
        assert!(got.post.is_empty());
    }
}
