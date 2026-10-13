// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「フォントの選択」の別ウィンドウ。OS のフォントをファミリーごとに並べ、選んだフォント名を欄に入れる。

use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwsdk::host::fonts::{self as catalog, Face};

use super::{File, Look, skin};

pub mod samples;

pub const WIDTH: f32 = 620.0;
/// タイトルバーは OS が表示するので、その分を引く（デザインはタイトルバー込みで 600）。
pub const HEIGHT: f32 = 570.0;

pub const TITLE: &str = "フォントの選択";

const PAD: f32 = 14.0;
const SEARCH_HEIGHT: f32 = 30.0;
const FOOT: f32 = 54.0;
const NAME_WIDTH: f32 = 150.0;
const ROW_PAD: f32 = 9.0;
const ROW_GAP: f32 = 14.0;
const SAMPLE_SIZE: f32 = 17.0;
const SAMPLE_LINE: f32 = 24.0;
const CHIP_SIZE: f32 = 11.5;
const CHIP_HEIGHT: f32 = 19.0;
const CHIP_GAP: f32 = 5.0;
const CHIP_PAD: f32 = 9.0;
const NAME_BLOCK: f32 = 34.0;

/// 日本語の無いフォントでは英字だけ（無い文字は表示できない）。
const SAMPLE_JA: &str = "あア永 字幕と番組表 Aa 0123";
const SAMPLE_LATIN: &str = "Aa Bb Gg Qq 0123 &?!";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Setting(&'static str),
    Plugin { plugin: String, key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    Chosen(String),
    Auto,
    Cancelled,
}

#[derive(Debug, Clone)]
struct Family {
    name: String,
    english: String,
    faces: Vec<&'static Face>,
    japanese: bool,
    mono: bool,
}

impl Family {
    /// `near` に近い太さのフォント（斜体でない物を先に）。
    fn nearest(&self, near: u16) -> Option<&'static Face> {
        self.faces
            .iter()
            .copied()
            .min_by_key(|face| (face.italic, face.weight.abs_diff(near)))
    }

    fn matches(&self, wanted: &str) -> bool {
        wanted.is_empty()
            || catalog::key(&self.name).contains(wanted)
            || self.faces.iter().any(|face| {
                [face.family_name(), &face.family, face.name(), &face.full]
                    .iter()
                    .any(|name| catalog::key(name).contains(wanted))
            })
    }
}

#[derive(Clone)]
pub struct Pick {
    pub target: Target,
    pub place: String,
    filter: String,
    japanese: bool,
    mono: bool,
    face: Option<&'static Face>,
    families: Vec<Family>,
    started: bool,
    scroll_to_chosen: bool,
    focused: bool,
    current: String,
}

impl std::fmt::Debug for Pick {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Pick")
            .field("target", &self.target)
            .field("place", &self.place)
            .field("filter", &self.filter)
            .field("face", &self.face.map(Face::name))
            .finish_non_exhaustive()
    }
}

impl Pick {
    pub fn new(target: Target, place: impl Into<String>, current: &str) -> Self {
        catalog::warm();
        Self {
            target,
            place: place.into(),
            filter: String::new(),
            japanese: true,
            mono: false,
            face: None,
            families: Vec::new(),
            started: false,
            scroll_to_chosen: true,
            focused: false,
            current: current.trim().trim_matches('"').to_owned(),
        }
    }

    fn start(&mut self, catalog: &'static catalog::Catalog) {
        if self.started {
            return;
        }
        self.started = true;
        self.families = catalog
            .families()
            .into_iter()
            .map(|(name, faces)| Family {
                english: faces
                    .first()
                    .map(|face| face.family.clone())
                    .unwrap_or_default(),
                japanese: faces.iter().any(|face| face.japanese),
                mono: faces.iter().any(|face| face.monospaced),
                name,
                faces,
            })
            .collect();
        if let Some(face) = catalog.find(&self.current) {
            self.face = Some(face);
            // 日本語の無いフォントを選んでいたら、絞らずに出す。
            self.japanese &= face.japanese;
            self.mono = false;
        }
    }

    fn shown(&self) -> Vec<&Family> {
        let wanted = catalog::key(&self.filter);
        self.families
            .iter()
            .filter(|family| !self.japanese || family.japanese)
            .filter(|family| !self.mono || family.mono)
            .filter(|family| family.matches(&wanted))
            .collect()
    }

    fn chosen_in(&self, family: &Family) -> Option<&'static Face> {
        self.face
            .filter(|face| family.faces.iter().any(|had| std::ptr::eq(*had, *face)))
    }
}

pub fn place_of(key: &str) -> &'static str {
    match key {
        "ui.font" => "表示 › UI のフォント",
        "ui.status_font" => "表示 › ステータスバーのフォント",
        "osd.font" => "OSD › フォント",
        "caption.font" => "字幕 › 字幕のフォント",
        "caption.superimpose_font" => "字幕 › 文字スーパーのフォント",
        "guide.font" => "番組表・番組情報 › 番組表のフォント",
        _ => "",
    }
}

/// 本体のキーは `settle` がコピーに書くが、プラグインの設定はその場でファイルに書く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginWrite {
    pub plugin: String,
    pub key: String,
    pub value: Option<String>,
}

pub fn settle(target: &Target, done: Done, edited: &mut File) -> Option<PluginWrite> {
    let value = match done {
        Done::Cancelled => return None,
        Done::Chosen(name) => Some(name),
        Done::Auto => None,
    };
    match target {
        Target::Setting(key) => {
            super::set_font(edited, key, value.unwrap_or_default());
            None
        }
        Target::Plugin { plugin, key } => Some(PluginWrite {
            plugin: plugin.clone(),
            key: key.clone(),
            value,
        }),
    }
}

pub fn named_sample(ctx: &egui::Context, value: &str, size: f32) -> Option<samples::Sample> {
    let value = value.trim().trim_matches('"');
    if value.is_empty() || std::path::Path::new(value).is_absolute() {
        return None;
    }
    catalog::warm();
    let face = catalog::ready()?.find(value)?;
    samples::sample(ctx, face, value, size)
}

struct Tones {
    ground: Color32,
    panel: Color32,
    raised: Color32,
    button: Color32,
    hover: Color32,
    chosen: Color32,
    edge: Color32,
    t1: Color32,
    t2: Color32,
    t3: Color32,
    accent: Color32,
}

impl Tones {
    fn of(look: &Look) -> Self {
        Self {
            ground: look.body,
            panel: look.dark,
            raised: look.select_fill,
            button: look.button_fill,
            hover: look.hover,
            chosen: look.chosen_fill,
            edge: look.edge,
            t1: look.text,
            t2: look.label,
            t3: look.group,
            accent: look.primary,
        }
    }
}

pub fn show(root: &mut Ui, pick: &mut Pick, look: &Look) -> Option<Done> {
    let tones = Tones::of(look);
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, tones.ground);
    let mut done = None;
    let mut top = full.top() + 12.0;
    if !pick.place.is_empty() {
        root.painter().text(
            pos2(full.left() + PAD, top),
            egui::Align2::LEFT_TOP,
            &pick.place,
            FontId::proportional(11.0),
            tones.t3,
        );
        top += 20.0;
    }
    let head = Rect::from_min_max(
        pos2(full.left() + PAD, top),
        pos2(full.right() - PAD, top + SEARCH_HEIGHT),
    );
    let list_top = head.bottom() + 12.0;
    root.painter().line_segment(
        [
            pos2(full.left(), list_top - 0.5),
            pos2(full.right(), list_top - 0.5),
        ],
        Stroke::new(1.0, tones.edge),
    );
    let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let list = Rect::from_min_max(pos2(full.left(), list_top), pos2(full.right(), foot.top()));
    let typing = head_row(root, head, pick, &tones);
    root.painter().rect_filled(list, 0.0, tones.panel);
    root.painter().line_segment(
        [foot.left_top(), foot.right_top()],
        Stroke::new(1.0, tones.edge),
    );
    match catalog::ready() {
        Some(catalog) => {
            pick.start(catalog);
            if let Some(chosen) = rows(root, list, pick, &tones) {
                done = Some(chosen);
            }
        }
        None => {
            root.painter().text(
                list.center(),
                egui::Align2::CENTER_CENTER,
                "フォントの一覧を読み込んでいます…",
                FontId::proportional(12.0),
                tones.t2,
            );
            root.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    if let Some(pressed) = foot_row(root, foot, pick, &tones) {
        done = Some(pressed);
    }
    // Enter は絞り込みの欄に文字を入力していないときに決める。
    let (escape, enter) = root.input(|input| {
        (
            input.key_pressed(egui::Key::Escape),
            input.key_pressed(egui::Key::Enter),
        )
    });
    if escape {
        done = Some(Done::Cancelled);
    }
    if enter
        && !typing
        && let Some(face) = pick.face
    {
        done = Some(Done::Chosen(face.name().to_owned()));
    }
    done
}

fn head_row(root: &mut Ui, head: Rect, pick: &mut Pick, tones: &Tones) -> bool {
    let chip_widths = ["日本語", "等幅"].map(|label| chip_width(root, label, 11.5, 10.0));
    let chips_width = chip_widths.iter().sum::<f32>() + 6.0;
    let search = Rect::from_min_max(
        head.min,
        pos2(head.right() - chips_width - 8.0, head.bottom()),
    );
    let painter = root.painter();
    painter.rect_filled(search, 6.0, tones.raised);
    painter.rect_stroke(
        search,
        6.0,
        Stroke::new(1.0, tones.edge),
        egui::StrokeKind::Inside,
    );
    let glass = pos2(search.left() + 15.0, search.center().y - 1.0);
    painter.circle_stroke(glass, 4.5, Stroke::new(1.5, tones.t3));
    painter.line_segment(
        [glass + vec2(3.3, 3.3), glass + vec2(6.5, 6.5)],
        Stroke::new(1.5, tones.t3),
    );
    let field = Rect::from_min_max(
        pos2(search.left() + 28.0, search.top() + 6.0),
        pos2(search.right() - 8.0, search.bottom() - 4.0),
    );
    let id = root.id().with("font-pick-search");
    let mut inner = root.new_child(egui::UiBuilder::new().max_rect(field));
    let edit = inner.add(
        egui::TextEdit::singleline(&mut pick.filter)
            .id(id)
            .frame(egui::Frame::NONE)
            .hint_text("名前で絞り込む（日本語名・英語名）")
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(12.0))
            .text_color(tones.t1),
    );
    if !pick.focused {
        pick.focused = true;
        edit.request_focus();
    }
    if edit.changed() {
        pick.scroll_to_chosen = false;
    }
    let typing = edit.has_focus() || edit.lost_focus();
    let mut x = search.right() + 8.0;
    for ((label, on), width) in [("日本語", &mut pick.japanese), ("等幅", &mut pick.mono)]
        .into_iter()
        .zip(chip_widths)
    {
        let rect = Rect::from_min_size(pos2(x, head.center().y - 11.0), vec2(width, 22.0));
        let response = root.interact(
            rect,
            root.id().with(("font-pick-chip", label)),
            Sense::click(),
        );
        let painter = root.painter();
        if *on {
            painter.rect_filled(rect, 11.0, tones.accent);
        } else {
            if response.hovered() {
                painter.rect_filled(rect, 11.0, tones.hover);
            }
            painter.rect_stroke(
                rect,
                11.0,
                Stroke::new(1.0, tones.edge),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(11.5),
            if *on { Color32::WHITE } else { tones.t2 },
        );
        if response.clicked() {
            *on = !*on;
        }
        x += width + 6.0;
    }
    typing
}

fn chip_width(ui: &Ui, label: &str, size: f32, pad: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(size), Color32::WHITE)
        .size()
        .x
        .ceil()
        + pad * 2.0
}

fn chip_places(ui: &Ui, faces: &[&'static Face], width: f32) -> (Vec<Rect>, f32) {
    let mut places = Vec::with_capacity(faces.len());
    let (mut x, mut y) = (0.0_f32, 0.0_f32);
    for face in faces {
        let w = chip_width(ui, face.style_name(), CHIP_SIZE, CHIP_PAD).min(width);
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += CHIP_HEIGHT + CHIP_GAP;
        }
        places.push(Rect::from_min_size(pos2(x, y), vec2(w, CHIP_HEIGHT)));
        x += w + CHIP_GAP;
    }
    let height = if faces.is_empty() {
        0.0
    } else {
        y + CHIP_HEIGHT
    };
    (places, height)
}

fn stepped(len: usize, at: Option<usize>, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match at {
        None => 0,
        Some(at) => at.saturating_add_signed(delta).min(len - 1),
    })
}

fn row_height(chips: f32) -> f32 {
    ROW_PAD * 2.0 + NAME_BLOCK.max(SAMPLE_LINE + 5.0 + chips)
}

fn rows(root: &mut Ui, list: Rect, pick: &mut Pick, tones: &Tones) -> Option<Done> {
    let shown: Vec<Family> = pick.shown().into_iter().cloned().collect();
    let mut inner = root.new_child(egui::UiBuilder::new().max_rect(list));
    inner.set_clip_rect(list);
    if shown.is_empty() {
        inner.painter().text(
            list.center(),
            egui::Align2::CENTER_CENTER,
            "当てはまるフォントがありません",
            FontId::proportional(12.0),
            tones.t3,
        );
        return None;
    }
    let right_width = (list.width() - PAD * 2.0 - NAME_WIDTH - ROW_GAP - 10.0).max(80.0);
    // 行の高さはラベルの並びで決まる（ラベルの幅は普通の文字で測るので、画像が来ても変わらない）。
    let mut tops = Vec::with_capacity(shown.len() + 1);
    let mut places = Vec::with_capacity(shown.len());
    let mut y = 0.0;
    for family in &shown {
        tops.push(y);
        let (chips, height) = chip_places(&inner, &family.faces, right_width);
        places.push(chips);
        y += row_height(height);
    }
    tops.push(y);
    let total = y;
    let typing = inner.memory(|memory| memory.has_focus(root.id().with("font-pick-search")));
    let (up, down, left, right) = inner.input(|input| {
        (
            input.key_pressed(egui::Key::ArrowUp),
            input.key_pressed(egui::Key::ArrowDown),
            input.key_pressed(egui::Key::ArrowLeft) && !typing,
            input.key_pressed(egui::Key::ArrowRight) && !typing,
        )
    });
    let at = shown
        .iter()
        .position(|family| pick.chosen_in(family).is_some());
    if up || down {
        if let Some(next) = stepped(shown.len(), at, if up { -1 } else { 1 }) {
            let weight = pick.face.map_or(400, |face| face.weight);
            pick.face = shown[next]
                .faces
                .iter()
                .copied()
                .min_by_key(|face| (face.italic, face.weight.abs_diff(weight)));
            pick.scroll_to_chosen = true;
        }
    } else if (left || right)
        && let Some(at) = at
    {
        let faces = &shown[at].faces;
        let now = faces
            .iter()
            .position(|face| pick.face.is_some_and(|had| std::ptr::eq(had, *face)));
        if let Some(next) = stepped(faces.len(), now, if left { -1 } else { 1 }) {
            pick.face = Some(faces[next]);
        }
    }
    let mut area = egui::ScrollArea::vertical()
        .id_salt("font-pick-rows")
        .auto_shrink([false, false]);
    if pick.scroll_to_chosen {
        pick.scroll_to_chosen = false;
        if let Some(at) = shown
            .iter()
            .position(|family| pick.chosen_in(family).is_some())
        {
            area = area.vertical_scroll_offset((tops[at] - list.height() / 3.0).max(0.0));
        }
    }
    let mut done = None;
    area.show_viewport(&mut inner, |ui, viewport| {
        ui.set_height(total);
        ui.set_width(ui.available_width());
        let origin = ui.min_rect().min;
        let first = tops[..shown.len()]
            .partition_point(|top| *top < viewport.top())
            .saturating_sub(1);
        for (at, family) in shown.iter().enumerate().skip(first) {
            if tops[at] > viewport.bottom() {
                break;
            }
            let rect = Rect::from_min_max(
                pos2(list.left(), origin.y + tops[at]),
                pos2(list.right(), origin.y + tops[at + 1]),
            );
            if let Some(said) = row(ui, rect, family, &places[at], pick, tones) {
                done = Some(said);
            }
        }
    });
    done
}

fn row(
    ui: &mut Ui,
    rect: Rect,
    family: &Family,
    chips: &[Rect],
    pick: &mut Pick,
    tones: &Tones,
) -> Option<Done> {
    let chosen = pick.chosen_in(family);
    let response = ui.interact(
        rect,
        ui.id().with(("font-pick-row", &family.name)),
        Sense::click(),
    );
    let painter = ui.painter().clone();
    if chosen.is_some() {
        painter.rect_filled(rect, 0.0, tones.chosen);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, tones.raised);
    }
    painter.line_segment(
        [
            rect.left_bottom() - vec2(0.0, 0.5),
            rect.right_bottom() - vec2(0.0, 0.5),
        ],
        Stroke::new(1.0, tones.ground),
    );
    let inner = rect.shrink2(vec2(PAD, ROW_PAD));
    let names = Rect::from_min_size(inner.min, vec2(NAME_WIDTH, inner.height()));
    let clip = painter.with_clip_rect(names.intersect(painter.clip_rect()));
    let one_line = family.english == family.name;
    let name_y = if one_line {
        names.top() + NAME_BLOCK / 2.0 - 8.0
    } else {
        names.top()
    };
    clip.text(
        pos2(names.left(), name_y),
        egui::Align2::LEFT_TOP,
        &family.name,
        FontId::proportional(12.5),
        tones.t1,
    );
    if !one_line {
        clip.text(
            pos2(names.left(), names.top() + 18.0),
            egui::Align2::LEFT_TOP,
            &family.english,
            FontId::proportional(10.5),
            tones.t3,
        );
    }
    let right = Rect::from_min_max(pos2(names.right() + ROW_GAP, inner.top()), inner.max);
    let face = chosen.or_else(|| family.nearest(400));
    let line = Rect::from_min_size(right.min, vec2(right.width(), SAMPLE_LINE));
    if let Some(face) = face {
        let text = if face.japanese {
            SAMPLE_JA
        } else {
            SAMPLE_LATIN
        };
        if let Some(sample) = samples::sample(ui.ctx(), face, text, SAMPLE_SIZE) {
            let tint = if chosen.is_some() { tones.t1 } else { tones.t2 };
            samples::paint_left(&painter, line, sample, tint);
        }
    }
    let mut done = None;
    let mut clicked_chip = false;
    for (face, place) in family.faces.iter().copied().zip(chips) {
        let chip = place.translate(vec2(right.left(), line.bottom() + 5.0));
        let id = ui.id().with(("font-pick-face", &face.path, face.index));
        let pressed = ui.interact(chip, id, Sense::click());
        let on = chosen.is_some_and(|had| std::ptr::eq(had, face));
        // 選んだ行の背景はボタンの背景に近いので、その行のラベルは少し明るく。
        let fill = if on {
            tones.accent
        } else if pressed.hovered() {
            tones.accent.gamma_multiply(0.35)
        } else if chosen.is_some() {
            tones.hover
        } else {
            tones.button
        };
        painter.rect_filled(chip, CHIP_HEIGHT / 2.0, fill);
        let tint = if on { Color32::WHITE } else { tones.t2 };
        match samples::sample(ui.ctx(), face, face.style_name(), CHIP_SIZE) {
            Some(sample) => {
                samples::paint_centred(&painter, chip.shrink2(vec2(4.0, 1.0)), sample, tint);
            }
            None => {
                painter
                    .with_clip_rect(chip.intersect(painter.clip_rect()))
                    .text(
                        chip.center(),
                        egui::Align2::CENTER_CENTER,
                        face.style_name(),
                        FontId::proportional(CHIP_SIZE),
                        tint,
                    );
            }
        }
        let mut hover = format!("{}（{}）", face.name(), face.weight);
        if face.variable() {
            hover.push_str(" 可変");
        }
        let pressed = pressed.on_hover_text(hover);
        if pressed.clicked() {
            pick.face = Some(face);
            clicked_chip = true;
        }
        if pressed.double_clicked() {
            done = Some(Done::Chosen(face.name().to_owned()));
        }
    }
    if !clicked_chip && response.clicked() {
        let near = pick.face.map_or(400, |face| face.weight);
        pick.face = family.nearest(near);
    }
    if done.is_none()
        && response.double_clicked()
        && let Some(face) = pick.face
    {
        done = Some(Done::Chosen(face.name().to_owned()));
    }
    done
}

fn foot_row(root: &mut Ui, foot: Rect, pick: &Pick, tones: &Tones) -> Option<Done> {
    let inner = foot.shrink2(vec2(PAD, 12.0));
    let painter = root.painter();
    let (shown, colour) = match pick.face {
        Some(face) => (face.name().to_owned(), tones.t1),
        None if pick.current.is_empty() => (String::from("自動"), tones.t3),
        None => (pick.current.clone(), tones.t2),
    };
    let mut done = None;
    let mut x = inner.right();
    let mut buttons = Vec::new();
    for (label, primary) in [("OK", true), ("キャンセル", false), ("自動に戻す", false)] {
        let width = chip_width(root, label, 12.0, 18.0).max(72.0);
        x -= width;
        buttons.push((
            label,
            primary,
            Rect::from_min_size(pos2(x, inner.top()), vec2(width, inner.height())),
        ));
        x -= 8.0;
    }
    let name_rect = Rect::from_min_max(inner.min, pos2(x - 8.0, inner.bottom()));
    painter.with_clip_rect(name_rect).text(
        pos2(name_rect.left(), name_rect.center().y),
        egui::Align2::LEFT_CENTER,
        shown,
        FontId::proportional(12.0),
        colour,
    );
    for (label, primary, rect) in buttons {
        let enabled = label != "OK" || pick.face.is_some();
        let response = root.interact(
            rect,
            root.id().with(("font-pick-button", label)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let painter = root.painter();
        let (fill, text) = match (primary, enabled, response.hovered()) {
            (true, true, _) => (tones.accent, Color32::WHITE),
            (true, false, _) => (tones.button, tones.t3),
            (false, _, true) => (tones.hover, tones.t1),
            (false, _, false) => (tones.button, tones.t1),
        };
        painter.rect_filled(rect, skin::BUTTON_ROUNDING, fill);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            text,
        );
        if response.clicked() {
            done = match label {
                "OK" => pick.face.map(|face| Done::Chosen(face.name().to_owned())),
                "キャンセル" => Some(Done::Cancelled),
                _ => Some(Done::Auto),
            };
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_stops_at_the_ends() {
        assert_eq!(stepped(0, None, 1), None);
        assert_eq!(stepped(3, None, 1), Some(0));
        assert_eq!(stepped(3, Some(0), -1), Some(0));
        assert_eq!(stepped(3, Some(1), 1), Some(2));
        assert_eq!(stepped(3, Some(2), 1), Some(2));
    }

    fn file() -> File {
        let mut file = File::default();
        file.ui.font = String::from("メイリオ");
        file
    }

    #[test]
    fn chosen_goes_into_the_field() {
        let mut edited = file();
        let target = Target::Setting("ui.font");
        let wrote = settle(
            &target,
            Done::Chosen(String::from("游ゴシック Bold")),
            &mut edited,
        );
        assert_eq!(wrote, None);
        assert_eq!(edited.ui.font, "游ゴシック Bold");
    }

    #[test]
    fn cancel_changes_nothing() {
        let mut edited = file();
        let before = edited.clone();
        assert_eq!(
            settle(&Target::Setting("ui.font"), Done::Cancelled, &mut edited),
            None
        );
        assert_eq!(edited, before);
        let plugin = Target::Plugin {
            plugin: String::from("databroadcast"),
            key: String::from("font_round"),
        };
        assert_eq!(settle(&plugin, Done::Cancelled, &mut edited), None);
        assert_eq!(edited, before);
    }

    #[test]
    fn auto_empties_the_field() {
        let mut edited = file();
        edited.caption.superimpose_font = String::from("BIZ UDゴシック");
        settle(
            &Target::Setting("caption.superimpose_font"),
            Done::Auto,
            &mut edited,
        );
        assert_eq!(edited.caption.superimpose_font, "");
        assert_eq!(edited.ui.font, "メイリオ");
    }

    #[test]
    fn plugin_fields_are_written_by_the_caller() {
        let mut edited = file();
        let before = edited.clone();
        let target = Target::Plugin {
            plugin: String::from("databroadcast"),
            key: String::from("font_round"),
        };
        assert_eq!(
            settle(
                &target,
                Done::Chosen(String::from("BIZ UDゴシック")),
                &mut edited
            ),
            Some(PluginWrite {
                plugin: String::from("databroadcast"),
                key: String::from("font_round"),
                value: Some(String::from("BIZ UDゴシック")),
            })
        );
        assert_eq!(
            settle(&target, Done::Auto, &mut edited).map(|write| write.value),
            Some(None)
        );
        assert_eq!(edited, before);
    }

    #[test]
    fn every_setting_key_has_a_place() {
        for key in [
            "ui.font",
            "ui.status_font",
            "osd.font",
            "caption.font",
            "caption.superimpose_font",
            "guide.font",
        ] {
            assert!(!place_of(key).is_empty(), "{key}");
        }
    }

    #[test]
    fn rows_grow_with_wrapped_chips() {
        assert_eq!(row_height(0.0), ROW_PAD * 2.0 + NAME_BLOCK);
        let one = row_height(CHIP_HEIGHT);
        let two = row_height(CHIP_HEIGHT * 2.0 + CHIP_GAP);
        assert!(two > one);
    }
}
