// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコーダの情報の見た目。中身は [`super::video`]・[`super::sound`] の行から取り、
//! 右クリックの「この行をコピー」はその元の行をコピーする（全部のコピーと同じ文）。

use egui::{
    Align2, CornerRadius, FontId, Galley, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use mediadecoder::audio::Speaker;
use std::sync::Arc;

use super::Decoding;
use crate::ui::theme::{Mode, Theme, decoder as look, stream as skin};

const RISING_FOR: f64 = 3.0;

/// 背景の明るさで明暗を見分け、その明暗の [`Theme`] から取る。
pub(super) fn look_for(skin: &skin::Skin) -> look::Skin {
    let ground = skin.background;
    let sum = u32::from(ground.r()) + u32::from(ground.g()) + u32::from(ground.b());
    let mode = if sum > 3 * 128 {
        Mode::Light
    } else {
        Mode::Dark
    };
    Theme::of(mode).decoder
}

/// 返すのは映像・音声・レンダリングの枠の場所。
pub(super) fn cards(ui: &mut Ui, decoding: &Decoding, look: &look::Skin) -> [Rect; 3] {
    let video_lines = super::video(&decoding.video);
    let sound_lines = super::sound(decoding);
    let mut placed = [Rect::NOTHING; 3];
    egui::Frame::new()
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(look::GAP, look::GAP);
            if ui.available_width() >= 2.0 * look::CARD_MIN + look::GAP {
                ui.columns(2, |columns| {
                    placed[0] = video_card(&mut columns[0], decoding, &video_lines, look);
                    placed[1] = sound_card(&mut columns[1], decoding, &sound_lines, look);
                });
            } else {
                placed[0] = video_card(ui, decoding, &video_lines, look);
                placed[1] = sound_card(ui, decoding, &sound_lines, look);
            }
            placed[2] = render_card(ui, decoding, look);
        });
    placed
}

fn video_card(ui: &mut Ui, decoding: &Decoding, lines: &[String], look: &look::Skin) -> Rect {
    let video = &decoding.video;
    card(ui, look, |ui| {
        let mut pills = Vec::new();
        if let Some((_, hardware)) = &video.decoder.decoder {
            let tone = if *hardware { Tone::On } else { Tone::Plain };
            pills.push((
                value(lines, "モード").to_owned(),
                tone,
                line(lines, "モード"),
            ));
            pills.push((
                value(lines, "デコーダデバイス").to_owned(),
                Tone::Plain,
                line(lines, "デコーダデバイス"),
            ));
        }
        head(ui, "映像", &pills, look);
        let coded = video.decoder.coded;
        let (big, small) = match coded {
            Some((codec, format)) => {
                let rate = super::fps(format.rate);
                let mut small = vec![
                    format!("{} bit", format.depth),
                    super::chroma_name(format.chroma).to_owned(),
                    format!("{}:{}", format.aspect.0, format.aspect.1),
                ];
                if rate != "—" {
                    small.push(format!("{rate} fps"));
                }
                (
                    format!(
                        "{}　{}×{}{}",
                        super::codec_name(codec),
                        format.width,
                        format.height,
                        if format.progressive { "p" } else { "i" }
                    ),
                    Some(small.join("・")),
                )
            }
            None => (String::from("—"), None),
        };
        headline(ui, &big, small.as_deref(), line(lines, "コーデック"), look);
        pairs(ui, |ui| {
            for key in [
                "デコーダデバイス",
                "出力サイズ",
                "基本 FPS",
                "再生レート",
                "ピクチャー構造",
            ] {
                if !line(lines, key).is_empty() {
                    let shown = if key == "ピクチャー構造" {
                        structure_rows(value(lines, key))
                    } else {
                        value(lines, key).to_owned()
                    };
                    pair(ui, key, &shown, line(lines, key), look);
                }
            }
        });
        section(ui, "フレーム", look);
        let counted = line(lines, "フレーム数");
        let late = rising(ui, "破棄", video.late);
        let troubles = rising(ui, "デコードエラー", video.troubles);
        tiles(
            ui,
            &[
                Tile::new(video.decoded.to_string(), "デコード", false, counted),
                Tile::new(video.drawn.to_string(), "表示", false, counted),
                Tile::new(video.late.to_string(), "破棄", late, counted),
                Tile::new(
                    video.troubles.to_string(),
                    "デコードエラー",
                    troubles,
                    line(lines, "デコードエラー"),
                ),
            ],
            look,
        );
        bar(ui, video.frames, look);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 4.0);
            for (nth, kind) in ["I", "P", "B"].into_iter().enumerate() {
                let key = format!("{kind} フレーム");
                legend(
                    ui,
                    Some(look.bars[nth]),
                    &format!("{kind} {}", video.frames[nth]),
                    line(lines, &key),
                    look,
                );
            }
            legend(
                ui,
                None,
                &format!("スキップ {}", video.skipped),
                line(lines, "スキップ（デコードせずに破棄）"),
                look,
            );
            legend(
                ui,
                None,
                &format!("リピートフィールド {}", video.repeats),
                line(lines, "リピートフィールド"),
                look,
            );
        });
        section(ui, "デインターレース・逆テレシネ", look);
        let progressive = coded.is_some_and(|(_, format)| format.progressive);
        let mut pills = Vec::new();
        let deinterlace = value(lines, "デインターレース");
        pills.push(match video.settings {
            Some(_) if !progressive => (
                deinterlace.to_owned(),
                Tone::On,
                line(lines, "デインターレース"),
            ),
            Some(_) => (
                deinterlace.to_owned(),
                Tone::Plain,
                line(lines, "デインターレース"),
            ),
            None => (
                format!("デインターレース {deinterlace}"),
                Tone::Plain,
                line(lines, "デインターレース"),
            ),
        });
        if !line(lines, "逆テレシネ").is_empty() {
            let telecine = value(lines, "逆テレシネ");
            let tone = if telecine == "有効" {
                Tone::Good
            } else {
                Tone::Plain
            };
            pills.push((
                format!("逆テレシネ {telecine}"),
                tone,
                line(lines, "逆テレシネ"),
            ));
        }
        if !line(lines, "検出").is_empty() {
            pills.push((
                format!("検出 {}", value(lines, "検出")),
                Tone::Plain,
                line(lines, "検出"),
            ));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
            for (text, tone, copied) in &pills {
                pill(ui, text, *tone, copied, look);
            }
        });
        let matched = line(lines, "フィールドマッチ");
        if !line(lines, "24p 化").is_empty() || !matched.is_empty() {
            pairs(ui, |ui| {
                if !line(lines, "24p 化").is_empty() {
                    pair(
                        ui,
                        "24p 化",
                        value(lines, "24p 化"),
                        line(lines, "24p 化"),
                        look,
                    );
                }
                if !matched.is_empty() {
                    let spent = &video.spent;
                    pair(
                        ui,
                        "フィールドマッチ",
                        &format!(
                            "{} フレーム　間引き {}　コーミング {}",
                            spent.matched, spent.woven, spent.combed
                        ),
                        matched,
                        look,
                    );
                    pair(
                        ui,
                        "切り替え",
                        &format!("30fps/60i {} 回", spent.film_changes),
                        matched,
                        look,
                    );
                }
            });
        }
    })
}

fn sound_card(ui: &mut Ui, decoding: &Decoding, lines: &[String], look: &look::Skin) -> Rect {
    let sound = &decoding.sound;
    card(ui, look, |ui| {
        let mut pills = Vec::new();
        if sound.passthrough {
            pills.push((
                String::from("パススルー"),
                Tone::On,
                line(lines, "パススルー"),
            ));
        } else if let Some(using) = &sound.decoder {
            pills.push((using.name.clone(), Tone::Plain, line(lines, "デコーダ")));
        }
        if !line(lines, "デュアルモノ").is_empty() {
            pills.push((
                format!("デュアルモノ {}", value(lines, "デュアルモノ")),
                Tone::Plain,
                line(lines, "デュアルモノ"),
            ));
        }
        head(ui, "音声", &pills, look);
        let codec = decoding.sound_codec.as_deref().unwrap_or("—");
        let (name, carried) = codec.split_once(' ').unwrap_or((codec, ""));
        let shape = sound.shape.as_ref();
        let big = match shape {
            Some(shape) => format!("{name}　{}", super::layout(&shape.speakers)),
            None => name.to_owned(),
        };
        let small = match shape {
            Some(shape) => [carried.to_owned(), format!("{} Hz", shape.rate)]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("・"),
            None => [carried, "再生前"]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("・"),
        };
        let copied = if shape.is_some() {
            line(lines, "コーデック")
        } else {
            lines
                .iter()
                .find(|line| line.as_str() == "再生前")
                .map_or("", String::as_str)
        };
        headline(ui, &big, Some(&small), copied, look);
        let keys = [
            ("デコーダ", "デコーダ"),
            ("サンプリング周波数", "サンプリング周波数"),
            ("チャンネル", "チャンネル"),
            ("ダウンミックス", "ダウンミックス"),
            ("係数", "ストリームのダウンミックス係数"),
            ("パススルー", "パススルー"),
        ];
        let rows = |ui: &mut Ui| {
            pairs(ui, |ui| {
                for (key, said) in keys {
                    if !line(lines, said).is_empty() {
                        pair(ui, key, value(lines, said), line(lines, said), look);
                    }
                }
            });
        };
        let width = ui.available_width();
        match shape {
            Some(shape) if width >= look::KEY + 150.0 + look::SPEAKERS + 12.0 => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let left = width - look::SPEAKERS - 12.0;
                    ui.allocate_ui_with_layout(
                        vec2(left, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left);
                            rows(ui);
                        },
                    );
                    speakers(ui, &shape.speakers, line(lines, "チャンネル"), look);
                });
            }
            Some(shape) => {
                rows(ui);
                speakers(ui, &shape.speakers, line(lines, "チャンネル"), look);
            }
            None => rows(ui),
        }
        section(ui, "サンプル", look);
        let heard = line(lines, "出力サンプル数");
        let missed = rising(ui, "欠落", sound.missed);
        tiles(
            ui,
            &[
                Tile::new(sound.heard.to_string(), "出力サンプル数", false, heard),
                Tile::new(sound.missed.to_string(), MISSED, missed, heard),
            ],
            look,
        );
    })
}

fn render_card(ui: &mut Ui, decoding: &Decoding, look: &look::Skin) -> Rect {
    let lines = super::sections(decoding)
        .into_iter()
        .find(|(title, _)| *title == "レンダリング")
        .map(|(_, lines)| lines)
        .unwrap_or_default();
    let gpu = lines.first().cloned().unwrap_or_default();
    let (name, parts) = split_gpu(decoding.painter.as_deref());
    card(ui, look, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(14.0, 4.0);
            text(ui, "レンダリング", 13.0, look.text, "");
            text(ui, name, 12.0, look.text, &gpu);
            ui.spacing_mut().item_spacing.x = 8.0;
            for part in parts {
                pill(ui, part, Tone::Plain, &gpu, look);
            }
        });
        if !line(&lines, "拡大").is_empty() {
            pairs(ui, |ui| {
                for key in ["拡大", "縮小"] {
                    pair(ui, key, value(&lines, key), line(&lines, key), look);
                }
            });
        }
    })
}

/// `Apple M2 (Metal, AGX)` を名前とラベルに分ける。括弧の無いもの（GLES）は名前だけ。
fn split_gpu(painter: Option<&str>) -> (&str, Vec<&str>) {
    let Some(painter) = painter else {
        return ("—", Vec::new());
    };
    match painter.rfind(" (") {
        Some(at) if painter.ends_with(')') => (
            &painter[..at],
            painter[at + 2..painter.len() - 1]
                .split(", ")
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect(),
        ),
        _ => (painter, Vec::new()),
    }
}

fn line<'a>(lines: &'a [String], key: &str) -> &'a str {
    lines
        .iter()
        .find(|line| {
            line.strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(": "))
        })
        .map_or("", String::as_str)
}

fn value<'a>(lines: &'a [String], key: &str) -> &'a str {
    let found = line(lines, key);
    found
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or("—")
}

/// ピクチャー構造の一行を、判定と枚数の二行に。括弧が無ければそのまま。
fn structure_rows(said: &str) -> String {
    match said.split_once('（') {
        Some((judged, counts)) => {
            format!("{judged}\n{}", counts.strip_suffix('）').unwrap_or(counts))
        }
        None => said.to_owned(),
    }
}

fn copies(response: &Response, line: &str) {
    if line.is_empty() {
        return;
    }
    response.context_menu(|ui| {
        if ui.button("この行をコピー").clicked() {
            ui.ctx().copy_text(line.to_owned());
            ui.close();
        }
    });
}

/// 前に見た数より増えていれば、しばらく真。選局で減ったときは数え直し。
fn rising(ui: &Ui, key: &str, count: u64) -> bool {
    let id = egui::Id::unique(("デコーダの情報の増え", key));
    let now = ui.input(|input| input.time);
    let rose_at = ui.data_mut(|data| {
        let seen: Option<(u64, f64)> = data.get_temp(id);
        let rose_at = match seen {
            Some((before, _)) if count > before => now,
            Some((before, at)) if count == before => at,
            _ => f64::NEG_INFINITY,
        };
        data.insert_temp(id, (count, rose_at));
        rose_at
    });
    let since = now - rose_at;
    let red = since < RISING_FOR;
    if red {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(RISING_FOR - since));
    }
    red
}

fn card(ui: &mut Ui, look: &look::Skin, add: impl FnOnce(&mut Ui)) -> Rect {
    egui::Frame::new()
        .fill(look.card)
        .stroke(Stroke::new(1.0, look.edge))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(6.0, 9.0);
            add(ui);
        })
        .response
        .rect
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    On,
    Good,
}

fn head(ui: &mut Ui, title: &str, pills: &[(String, Tone, &str)], look: &look::Skin) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        text(ui, title, 13.0, look.text, "");
        for (shown, tone, copied) in pills {
            pill(ui, shown, *tone, copied, look);
        }
    });
}

fn headline(ui: &mut Ui, big: &str, small: Option<&str>, copied: &str, look: &look::Skin) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        text(ui, big, 18.0, look.strong, copied);
        if let Some(small) = small.filter(|small| !small.is_empty()) {
            text(ui, small, 11.5, look.label, copied);
        }
    });
}

/// 折り返す文字列の一まとまり。`copied` が空でなければ右クリックでコピーできる。
fn text(ui: &mut Ui, shown: &str, size: f32, colour: egui::Color32, copied: &str) {
    let response = ui.add(
        egui::Label::new(RichText::new(shown).size(size).color(colour))
            .wrap()
            .selectable(false)
            .sense(Sense::click()),
    );
    copies(&response, copied);
}

fn section(ui: &mut Ui, title: &str, look: &look::Skin) {
    text(ui, title, 10.5, look.dim, "");
}

fn laid(ui: &Ui, shown: &str, size: f32, colour: egui::Color32, wrap: f32) -> Arc<Galley> {
    ui.painter().layout(
        shown.to_owned(),
        FontId::proportional(size),
        colour,
        wrap.max(16.0),
    )
}

fn pill(ui: &mut Ui, shown: &str, tone: Tone, copied: &str, look: &look::Skin) -> Response {
    let (ground, colour) = match tone {
        Tone::Plain => (look.pill, look.label),
        Tone::On => (look.accent, look.accent_text),
        Tone::Good => (look.good_ground, look.good),
    };
    let galley = laid(ui, shown, 10.5, colour, ui.max_rect().width() - 16.0);
    let size = galley.size() + vec2(16.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, 9.0, ground);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, colour);
    copies(&response, copied);
    response
}

fn pairs(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        add(ui);
    });
}

fn pair(ui: &mut Ui, key: &str, shown: &str, copied: &str, look: &look::Skin) {
    let width = ui.available_width();
    let key_galley = laid(ui, key, 12.0, look.dim, look::KEY);
    let value_galley = laid(ui, shown, 12.0, look.text, width - look::KEY - 10.0);
    let height = key_galley.size().y.max(value_galley.size().y);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect.expand2(vec2(4.0, 2.0)), 3.0, look.raised);
    }
    ui.painter().galley(rect.min, key_galley, look.dim);
    ui.painter().galley(
        pos2(rect.left() + look::KEY + 10.0, rect.top()),
        value_galley,
        look.text,
    );
    copies(&response, copied);
}

const MISSED: &str = "破棄";

struct Tile<'a> {
    number: String,
    name: &'a str,
    rising: bool,
    copied: &'a str,
}

impl<'a> Tile<'a> {
    fn new(number: String, name: &'a str, rising: bool, copied: &'a str) -> Self {
        Self {
            number,
            name,
            rising,
            copied,
        }
    }
}

/// 数のタイル。四つ並ばない幅なら二つずつ。
fn tiles(ui: &mut Ui, tiles: &[Tile], look: &look::Skin) {
    const GAP: f32 = 6.0;
    let width = ui.available_width();
    let across = if width >= 4.0 * 90.0 + 3.0 * GAP {
        4
    } else {
        2
    }
    .min(tiles.len().max(1));
    let each = (width - GAP * (across - 1) as f32) / across as f32;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        for (row_nth, row) in tiles.chunks(across).enumerate() {
            let galleys: Vec<_> = row
                .iter()
                .map(|tile| {
                    let colour = if tile.rising { look.bad } else { look.strong };
                    (
                        laid(ui, &tile.number, 15.0, colour, each - 18.0),
                        laid(ui, tile.name, 10.5, look.dim, each - 18.0),
                        colour,
                    )
                })
                .collect();
            let height = galleys
                .iter()
                .map(|(number, name, _)| number.size().y + 1.0 + name.size().y)
                .fold(0.0, f32::max)
                + 14.0;
            let (band, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            for (nth, (tile, (number, name, colour))) in row.iter().zip(galleys).enumerate() {
                let rect = Rect::from_min_size(
                    pos2(band.left() + nth as f32 * (each + GAP), band.top()),
                    vec2(each, height),
                );
                let response = ui.interact(
                    rect,
                    ui.scope_id().with(("タイル", row_nth, nth, tile.name)),
                    Sense::click(),
                );
                ui.painter().rect_filled(rect, 6.0, look.raised);
                let below = number.size().y + 1.0;
                ui.painter()
                    .galley(rect.min + vec2(9.0, 7.0), number, colour);
                ui.painter()
                    .galley(rect.min + vec2(9.0, 7.0 + below), name, look.dim);
                copies(&response, tile.copied);
            }
        }
    });
}

fn bar(ui: &mut Ui, frames: [u64; 3], look: &look::Skin) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, 4.0, look.track);
    let total: u64 = frames.iter().sum();
    if total == 0 {
        return;
    }
    let first = frames.iter().position(|count| *count > 0);
    let last = frames.iter().rposition(|count| *count > 0);
    let mut x = rect.left();
    for (nth, count) in frames.into_iter().enumerate() {
        if count == 0 {
            continue;
        }
        let span = width * count as f32 / total as f32;
        let round = |edge: bool| if edge { 4 } else { 0 };
        let corners = CornerRadius {
            nw: round(first == Some(nth)),
            sw: round(first == Some(nth)),
            ne: round(last == Some(nth)),
            se: round(last == Some(nth)),
        };
        let piece = Rect::from_min_max(pos2(x, rect.top()), pos2(x + span, rect.bottom()));
        ui.painter().rect_filled(piece, corners, look.bars[nth]);
        x += span;
    }
}

fn legend(
    ui: &mut Ui,
    swatch: Option<egui::Color32>,
    shown: &str,
    copied: &str,
    look: &look::Skin,
) {
    let mark = if swatch.is_some() { 12.0 } else { 0.0 };
    let galley = laid(ui, shown, 10.5, look.dim, ui.max_rect().width() - mark);
    let size = vec2(galley.size().x + mark, galley.size().y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if let Some(colour) = swatch {
        let square =
            Rect::from_center_size(pos2(rect.left() + 4.0, rect.center().y), vec2(8.0, 8.0));
        ui.painter().rect_filled(square, 2.0, colour);
    }
    ui.painter()
        .galley(pos2(rect.left() + mark, rect.top()), galley, look.dim);
    copies(&response, copied);
}

type Cell = (&'static str, usize, usize, bool);

/// ふつうは 3×3（前・横と LFE・後ろ）、22.2ch は上・中・下の 3 層を 5 列 7 行で。
fn placed(speakers: &[Speaker]) -> (usize, usize, Vec<Cell>) {
    use Speaker::*;
    const PLAIN: [(&str, &[Speaker], usize, usize); 9] = [
        ("FL", &[FrontLeft], 0, 0),
        ("FC", &[FrontCentre], 1, 0),
        ("FR", &[FrontRight], 2, 0),
        ("SiL", &[SideLeft], 0, 1),
        ("LFE", &[LowFrequency, LowFrequencyRight], 1, 1),
        ("SiR", &[SideRight], 2, 1),
        ("BL", &[BackLeft], 0, 2),
        ("BC", &[BackCentre], 1, 2),
        ("BR", &[BackRight], 2, 2),
    ];
    const LAYERED: [(Speaker, usize, usize); 24] = [
        (TopFrontLeft, 1, 0),
        (TopFrontCentre, 2, 0),
        (TopFrontRight, 3, 0),
        (TopSideLeft, 1, 1),
        (TopCentre, 2, 1),
        (TopSideRight, 3, 1),
        (TopBackLeft, 1, 2),
        (TopBackCentre, 2, 2),
        (TopBackRight, 3, 2),
        (FrontLeft, 0, 3),
        (FrontLeftCentre, 1, 3),
        (FrontCentre, 2, 3),
        (FrontRightCentre, 3, 3),
        (FrontRight, 4, 3),
        (SideLeft, 0, 4),
        (LowFrequency, 1, 4),
        (LowFrequencyRight, 3, 4),
        (SideRight, 4, 4),
        (BackLeft, 0, 5),
        (BackCentre, 2, 5),
        (BackRight, 4, 5),
        (BottomFrontLeft, 1, 6),
        (BottomFrontCentre, 2, 6),
        (BottomFrontRight, 3, 6),
    ];
    let plain = speakers.iter().all(|speaker| {
        *speaker == Unplaced || PLAIN.iter().any(|(_, which, _, _)| which.contains(speaker))
    });
    if plain {
        let cells = PLAIN
            .iter()
            .map(|(name, which, column, row)| {
                let on = which.iter().any(|speaker| speakers.contains(speaker));
                (*name, *column, *row, on)
            })
            .collect();
        (3, 3, cells)
    } else {
        let cells = LAYERED
            .iter()
            .map(|(speaker, column, row)| {
                (
                    super::speaker_name(*speaker),
                    *column,
                    *row,
                    speakers.contains(speaker),
                )
            })
            .collect();
        (5, 7, cells)
    }
}

fn speakers(ui: &mut Ui, present: &[Speaker], copied: &str, look: &look::Skin) {
    let (columns, rows, cells) = placed(present);
    let (cell, gap, named) = if columns == 3 {
        (vec2(26.0, 22.0), 4.0, true)
    } else {
        (vec2(16.0, 11.0), 3.0, false)
    };
    let grid = vec2(
        columns as f32 * cell.x + (columns - 1) as f32 * gap,
        rows as f32 * cell.y + (rows - 1) as f32 * gap,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(look::SPEAKERS.max(grid.x), grid.y), Sense::click());
    let origin = pos2(rect.center().x - grid.x / 2.0, rect.top());
    for (name, column, row, on) in cells {
        let at = Rect::from_min_size(
            origin + vec2(column as f32 * (cell.x + gap), row as f32 * (cell.y + gap)),
            cell,
        );
        ui.painter()
            .rect_filled(at, 4.0, if on { look.accent } else { look.raised });
        if named {
            ui.painter().text(
                at.center(),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(9.0),
                if on { look.accent_text } else { look.dim },
            );
        }
    }
    let response = if copied.is_empty() {
        response
    } else {
        response.on_hover_text(copied)
    };
    copies(&response, copied);
}

#[cfg(test)]
mod tests;
