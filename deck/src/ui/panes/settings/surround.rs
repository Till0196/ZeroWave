// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「5.1ch の設定」「22.2ch の設定」の別ウィンドウ。22.2ch は入力が多いので行が元のチャンネル。
//! 欄は文字列の下書きで持ち「OK」で読む（入力途中の `-` や `0.` が消えないよう）。

use egui::{FontId, Rect, pos2, vec2};

use super::{File, Form, Look, Pressed, State, skin};

pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 480.0;
pub const TWENTY_TWO_WIDTH: f32 = 640.0;
pub const TWENTY_TWO_HEIGHT: f32 = 720.0;

const FOOT: f32 = 46.0;

/// 5.1ch の並びの名前（`zwcore::sound::mix::FIVE_ONE`）。
const FIVE: [&str; 6] = ["左前", "右前", "中央", "LFE", "左後", "右後"];

const SEVEN: [&str; 8] = [
    "左前", "右前", "中央", "LFE", "左横", "右横", "左後", "右後",
];

const TWENTY_TWO: [&str; 24] = [
    "左前",
    "右前",
    "中央",
    "左前内側",
    "右前内側",
    "左横",
    "右横",
    "左後",
    "右後",
    "後中央",
    "上左前",
    "上右前",
    "上中央",
    "真上",
    "上左横",
    "上右横",
    "上左後",
    "上右後",
    "上後中央",
    "下左前",
    "下右前",
    "下中央",
    "LFE1",
    "LFE2",
];

const LAYERS: [(usize, &str); 4] = [(0, "中層"), (10, "上層"), (19, "下層"), (22, "低音")];

/// 既定のダウンミックス（ARIB STD-B21、−3 dB、LFE は落とす）。
const DOWNMIX: [[f32; 6]; 2] = [
    [1.0, 0.0, 0.707, 0.0, 0.707, 0.0],
    [0.0, 1.0, 0.707, 0.0, 0.0, 0.707],
];

type Cells = Vec<Vec<String>>;

#[derive(Debug, Clone)]
pub struct Draft {
    pub downmix: Cells,
    pub surround: Cells,
    /// 22.2ch → 7.1ch。行が 22.2ch の入力。
    pub seven_one: Cells,
    pub wrong: Option<String>,
    pub scroll: f32,
    pub content: f32,
}

impl Draft {
    pub fn of(file: &File) -> Self {
        let written = |key: &str| file.audio.matrix.get(key).cloned();
        Self {
            downmix: cells(&written("downmix").unwrap_or_else(downmix)),
            surround: cells(&written("surround").unwrap_or_else(identity)),
            seven_one: cells(&transpose(
                &written("seven_one").unwrap_or_else(seven_one_default),
            )),
            wrong: None,
            scroll: 0.0,
            content: 0.0,
        }
    }
}

fn downmix() -> Vec<Vec<f32>> {
    DOWNMIX.iter().map(|row| row.to_vec()).collect()
}

fn identity() -> Vec<Vec<f32>> {
    (0..6)
        .map(|row| (0..6).map(|at| if at == row { 1.0 } else { 0.0 }).collect())
        .collect()
}

/// 22.2ch → 7.1ch の既定（出力 × 入力）。
fn seven_one_default() -> Vec<Vec<f32>> {
    zwcore::sound::mix::seven_one_default()
        .iter()
        .map(|row| row.iter().map(|milli| *milli as f32 / 1000.0).collect())
        .collect()
}

fn transpose(rows: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let width = rows.first().map_or(0, Vec::len);
    (0..width)
        .map(|at| rows.iter().map(|row| row[at]).collect())
        .collect()
}

fn cells(rows: &[Vec<f32>]) -> Cells {
    rows.iter()
        .map(|row| row.iter().map(|value| written(*value)).collect())
        .collect()
}

fn written(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        String::from("0")
    } else {
        text.to_owned()
    }
}

fn read(rows: &Cells, what: &str) -> Result<Vec<Vec<f32>>, String> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|text| {
                    let text = text.trim();
                    if text.is_empty() {
                        return Ok(0.0);
                    }
                    match text.parse::<f32>() {
                        Ok(value) if value.is_finite() && value.abs() <= 10.0 => Ok(value),
                        _ => Err(format!("{what}の「{text}」が読めません（-10〜10）")),
                    }
                })
                .collect()
        })
        .collect()
}

fn is_default(rows: &Cells, default: &[Vec<f32>]) -> bool {
    read(rows, "").is_ok_and(|rows| {
        rows.len() == default.len()
            && rows.iter().zip(default).all(|(row, want)| {
                row.len() == want.len()
                    && row
                        .iter()
                        .zip(want)
                        .all(|(value, want)| (value - want).abs() < 0.0005)
            })
    })
}

fn heading(form: &mut Form<'_, '_>, id: &str, title: &str, default: bool) -> bool {
    *form.at += 6.0;
    let row = form.line(24.0);
    form.ui.painter().text(
        pos2(row.left(), row.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(skin::SECTION_SIZE),
        form.look.section,
    );
    let button = Rect::from_min_size(pos2(row.right() - 84.0, row.top()), vec2(84.0, 24.0));
    form.ui.painter().text(
        pos2(button.left() - 8.0, row.center().y),
        egui::Align2::RIGHT_CENTER,
        if default { "既定" } else { "カスタム" },
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        if default {
            form.look.label
        } else {
            form.look.chosen_text
        },
    );
    form.button(button, (id, "default"), "既定に戻す")
}

fn grid(
    form: &mut Form<'_, '_>,
    id: &str,
    rows: &mut Cells,
    row_names: &[&str],
    column_names: &[&str],
    label_width: f32,
    layers: &[(usize, &str)],
) {
    let head = form.line(18.0);
    let gap = 6.0;
    let columns = column_names.len() as f32;
    let cell_width = (head.width() - label_width - gap * columns) / columns;
    let x_of = |at: usize| head.left() + label_width + gap + at as f32 * (cell_width + gap);
    for (at, name) in column_names.iter().enumerate() {
        form.ui.painter().text(
            pos2(x_of(at) + cell_width / 2.0, head.center().y),
            egui::Align2::CENTER_CENTER,
            *name,
            FontId::proportional(skin::TEXT_SIZE - 1.5),
            form.look.label,
        );
    }
    for (index, row) in rows.iter_mut().enumerate() {
        if let Some((_, layer)) = layers.iter().find(|(first, _)| *first == index) {
            let line = form.line(20.0);
            form.ui.painter().text(
                pos2(line.left(), line.center().y + 2.0),
                egui::Align2::LEFT_CENTER,
                *layer,
                FontId::proportional(skin::TEXT_SIZE - 1.5),
                form.look.group,
            );
            form.ui.painter().hline(
                line.x_range(),
                line.bottom(),
                egui::Stroke::new(1.0, form.look.edge),
            );
        }
        let line = form.line(24.0);
        form.ui.painter().text(
            pos2(line.left() + label_width, line.center().y),
            egui::Align2::RIGHT_CENTER,
            row_names[index],
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
        for (at, cell) in row.iter_mut().enumerate() {
            form.field(
                Rect::from_min_size(pos2(x_of(at), line.top()), vec2(cell_width, 24.0)),
                (id, index, at),
                cell,
                "0",
            );
        }
    }
}

fn split(root: &mut egui::Ui, look: &Look) -> (Rect, Rect) {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let body = Rect::from_min_max(full.min, pos2(full.right(), foot.top()));
    (body, foot)
}

fn foot(root: &mut egui::Ui, look: &Look, foot: Rect, id: &str) -> Option<bool> {
    let mut at = 0.0;
    let mut form = Form::foot(root, look, foot, &mut at);
    let row = form.line(skin::SELECT_HEIGHT);
    let cancel = Rect::from_min_size(
        pos2(row.right() - 88.0, row.top()),
        vec2(88.0, row.height()),
    );
    let ok = Rect::from_min_size(
        pos2(cancel.left() - 96.0, row.top()),
        vec2(88.0, row.height()),
    );
    let mut pressed = None;
    if form.primary_button(ok, (id, "ok"), "OK", true) {
        pressed = Some(true);
    }
    if form.button(cancel, (id, "cancel"), "キャンセル") {
        pressed = Some(false);
    }
    pressed
}

pub fn five_one(
    root: &mut egui::Ui,
    state: &mut State,
    edited: &mut File,
    look: &Look,
) -> Option<Pressed> {
    let draft = state.five_one.get_or_insert_with(|| Draft::of(edited));
    let (body, foot_rect) = split(root, look);
    {
        let mut inner = root.new_child(egui::UiBuilder::new().max_rect(body));
        inner.set_clip_rect(body);
        let mut at = body.top() + 6.0;
        let mut form = Form::new(&mut inner, look, body, &mut at);
        let default = is_default(&draft.downmix, &downmix());
        if heading(&mut form, "downmix", "ステレオへのダウンミックス", default) {
            draft.downmix = cells(&downmix());
        }
        grid(
            &mut form,
            "downmix",
            &mut draft.downmix,
            &["左", "右"],
            &FIVE,
            44.0,
            &[],
        );
        let default = is_default(&draft.surround, &identity());
        if heading(&mut form, "surround", "チャンネルの入れ替え", default) {
            draft.surround = cells(&identity());
        }
        grid(
            &mut form,
            "surround",
            &mut draft.surround,
            &FIVE,
            &FIVE,
            44.0,
            &[],
        );
        form.note("行が出力するチャンネル、列が元のチャンネルです。");
        if let Some(wrong) = &draft.wrong {
            let wrong = wrong.clone();
            let colour = form.look.error;
            let row = form.line(18.0);
            form.ui.painter().text(
                pos2(row.left(), row.center().y),
                egui::Align2::LEFT_CENTER,
                wrong,
                FontId::proportional(skin::TEXT_SIZE - 1.0),
                colour,
            );
        }
    }
    match foot(root, look, foot_rect, "five_one") {
        Some(true) => match accept_five_one(draft, edited) {
            Ok(()) => {
                state.five_one = None;
                Some(Pressed::Close)
            }
            Err(why) => {
                draft.wrong = Some(why);
                None
            }
        },
        Some(false) => {
            state.five_one = None;
            Some(Pressed::Close)
        }
        None => None,
    }
}

fn accept_five_one(draft: &Draft, edited: &mut File) -> Result<(), String> {
    let downmix_rows = read(&draft.downmix, "ステレオへのダウンミックス")?;
    let surround_rows = read(&draft.surround, "チャンネルの入れ替え")?;
    let custom = [
        (
            "downmix",
            !is_default(&draft.downmix, &downmix()),
            downmix_rows,
        ),
        (
            "surround",
            !is_default(&draft.surround, &identity()),
            surround_rows,
        ),
    ];
    let matrix = &mut edited.audio.matrix;
    for (key, custom, rows) in custom {
        if custom {
            matrix.insert(key.to_owned(), rows);
        } else {
            matrix.remove(key);
        }
    }
    Ok(())
}

pub fn twenty_two(
    root: &mut egui::Ui,
    state: &mut State,
    edited: &mut File,
    look: &Look,
) -> Option<Pressed> {
    let draft = state.twenty_two.get_or_insert_with(|| Draft::of(edited));
    let (body, foot_rect) = split(root, look);
    draft.scroll = super::scrolled(root, body, draft.scroll, draft.content);
    {
        let mut inner = root.new_child(egui::UiBuilder::new().max_rect(body));
        inner.set_clip_rect(body);
        let mut at = body.top() + 6.0 - draft.scroll;
        let mut form = Form::new(&mut inner, look, body, &mut at);
        let default_rows = transpose(&seven_one_default());
        let default = is_default(&draft.seven_one, &default_rows);
        if heading(&mut form, "seven_one", "7.1ch へのダウンミックス", default) {
            draft.seven_one = cells(&default_rows);
        }
        grid(
            &mut form,
            "seven_one",
            &mut draft.seven_one,
            &TWENTY_TWO,
            &SEVEN,
            76.0,
            &LAYERS,
        );
        form.note("行が元のチャンネル（22.2ch）、列が出力するチャンネル（7.1ch）です");
        if let Some(wrong) = &draft.wrong {
            let wrong = wrong.clone();
            let colour = form.look.error;
            let row = form.line(18.0);
            form.ui.painter().text(
                pos2(row.left(), row.center().y),
                egui::Align2::LEFT_CENTER,
                wrong,
                FontId::proportional(skin::TEXT_SIZE - 1.0),
                colour,
            );
        }
        draft.content = *form.at + draft.scroll - body.top() + 8.0;
    }
    super::scroll_bar(root, look, body, draft.scroll, draft.content);
    match foot(root, look, foot_rect, "twenty_two") {
        Some(true) => match accept_twenty_two(draft, edited) {
            Ok(()) => {
                state.twenty_two = None;
                Some(Pressed::Close)
            }
            Err(why) => {
                draft.wrong = Some(why);
                None
            }
        },
        Some(false) => {
            state.twenty_two = None;
            Some(Pressed::Close)
        }
        None => None,
    }
}

fn accept_twenty_two(draft: &Draft, edited: &mut File) -> Result<(), String> {
    let rows = read(&draft.seven_one, "7.1ch へのダウンミックス")?;
    if is_default(&draft.seven_one, &transpose(&seven_one_default())) {
        edited.audio.matrix.remove("seven_one");
    } else {
        edited
            .audio
            .matrix
            .insert(String::from("seven_one"), transpose(&rows));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cells_read_back_and_refuse_what_is_not_a_number() {
        let draft = Draft::of(&File::default());
        assert_eq!(draft.downmix[0][2], "0.707");
        assert_eq!(read(&draft.downmix, "x").unwrap()[0][2], 0.707);
        let mut rows = draft.surround.clone();
        rows[0][0] = String::new();
        assert_eq!(read(&rows, "x").unwrap()[0][0], 0.0);
        rows[0][0] = String::from("-");
        assert!(read(&rows, "チャンネルの入れ替え").is_err());
    }

    #[test]
    fn only_the_changed_matrices_are_written() {
        let mut file = File::default();
        let mut draft = Draft::of(&file);
        accept_five_one(&draft, &mut file).unwrap();
        assert!(file.audio.matrix.is_empty(), "既定のままなら書かない");
        draft.downmix[0][3] = String::from("0.5");
        accept_five_one(&draft, &mut file).unwrap();
        assert_eq!(file.audio.matrix["downmix"][0][3], 0.5);
        assert!(!file.audio.matrix.contains_key("surround"));
        let mut draft = Draft::of(&file);
        assert_eq!(draft.downmix[0][3], "0.5", "書いてある物が入って開く");
        draft.downmix = cells(&downmix());
        accept_five_one(&draft, &mut file).unwrap();
        assert!(file.audio.matrix.is_empty());
    }

    /// 22.2ch のテーブルはウィンドウでは入力が行、ファイルでは出力が行。
    #[test]
    fn the_twenty_two_table_is_written_outputs_first() {
        let mut file = File::default();
        let mut draft = Draft::of(&file);
        assert_eq!(draft.seven_one.len(), 24);
        assert_eq!(draft.seven_one[0].len(), 8);
        accept_twenty_two(&draft, &mut file).unwrap();
        assert!(!file.audio.matrix.contains_key("seven_one"));
        draft.seven_one[9][4] = String::from("1");
        accept_twenty_two(&draft, &mut file).unwrap();
        let written = &file.audio.matrix["seven_one"];
        assert_eq!(written.len(), 8);
        assert_eq!(written[0].len(), 24);
        assert_eq!(written[4][9], 1.0);
        assert_eq!(Draft::of(&file).seven_one[9][4], "1");
    }
}
