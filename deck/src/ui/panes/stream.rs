// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ストリームの情報（情報タブと PID タブ）の別ウィンドウ。数えていないものは 0 ではなく「—」と書く。

use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use zwcore::filters::{Container, Counted};
use zwcore::si::{BroadcasterInfo, Identity, NetworkInfo};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{stream as skin, table};

mod delivery;
mod information;
mod pids;

use information::information;
use pids::pids;
pub use pids::{network_name, what_carries, what_declared, written};

#[derive(Debug, Clone, Default)]
pub struct StreamInfo {
    pub identity: Option<Identity>,
    pub services: Vec<Service>,
    pub networks: Vec<NetworkInfo>,
    pub counts: zwcore::filters::Counts,
    pub pids: Vec<Row>,
    pub programs: Vec<zwcore::filters::Program>,
    /// SDT に載っているサービス（現在のストリームも別のストリームも）。
    pub neighbours: Vec<Service>,
    /// 多重フレームヘッダ（TSMF）。搬送波ごと受けているときだけ。
    pub frame_header: Option<isdb::tsmf::Info>,
    pub broadcasters: Vec<BroadcasterInfo>,
    /// 緊急情報記述子が示すサービス（TS は NIT、MMT は MPT）。
    pub emergency: Vec<isdb::nitdescriptors::EmergencyService>,
    pub emm: Vec<(u16, u16)>,
    pub downloads: Vec<isdb::tstables::SoftwareDownload>,
    pub logos: Vec<zwcore::si::LogoImage>,
    pub partial: Option<zwcore::si::PartialInfo>,
    pub packages: Vec<isdb::mmttables::ListedPackage>,
    pub layouts: Vec<isdb::mmttables::Layout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub id: u16,
    pub name: String,
    pub kind: u8,
    pub network: u16,
    pub transport: u16,
    pub scrambled: bool,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pid: u16,
    pub counted: Counted,
    /// 「内容」の欄。`PAT`、`H-EIT`、`[0065] MPEG-2 Video`。
    pub what: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Information,
    Pids,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    pub open: bool,
    pub tab: Tab,
    pub chosen: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Copy,
    Close,
}

pub fn show(
    ui: &mut Ui,
    state: &mut State,
    info: &StreamInfo,
    skin: &skin::Skin,
) -> Option<Pressed> {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, skin.background);
    let picked = tabs(ui, full, state.tab, skin);
    if let Some(tab) = picked {
        state.tab = tab;
    }
    let foot = Rect::from_min_size(
        pos2(full.left(), full.bottom() - skin::BAR_HEIGHT),
        vec2(full.width(), skin::BAR_HEIGHT),
    );
    let mut pressed = feet(ui, foot, skin);
    if ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::C)) {
        pressed = Some(Pressed::Copy);
    }
    let body = Rect::from_min_max(
        pos2(full.left(), full.top() + skin::BAR_HEIGHT),
        pos2(full.right(), foot.top()),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    match state.tab {
        Tab::Information => information(&mut child, info, skin),
        Tab::Pids => pids(&mut child, info, &mut state.chosen, skin),
    }
    pressed
}

fn tabs(ui: &mut Ui, full: Rect, tab: Tab, skin: &skin::Skin) -> Option<Tab> {
    let rect = Rect::from_min_size(full.min, vec2(full.width(), skin::BAR_HEIGHT));
    ui.painter().rect_filled(rect, 0.0, skin.bar);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, skin.edge));
    let mut picked = None;
    let mut left = rect.left() + 6.0;
    for (label, which) in [("情報", Tab::Information), ("PID", Tab::Pids)] {
        let chosen = which == tab;
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            FontId::proportional(skin::TEXT_SIZE),
            if chosen {
                Color32::WHITE
            } else {
                skin.tab_text
            },
        );
        let button = Rect::from_min_size(
            pos2(left, rect.top() + 4.0),
            vec2(galley.size().x + 24.0, rect.height() - 8.0),
        );
        let response = ui.interact(button, ui.scope_id().with(("タブ", label)), Sense::click());
        let painter = ui.painter();
        if chosen {
            painter.rect_filled(button, 3.0, skin.tab_fill);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(button.left(), rect.bottom() - skin::TAB_LINE),
                    pos2(button.right(), rect.bottom()),
                ),
                0.0,
                skin.tab_line,
            );
        } else if response.hovered() {
            painter.rect_filled(button, 3.0, skin.button);
        }
        painter.galley(
            pos2(
                button.center().x - galley.size().x / 2.0,
                button.center().y - galley.size().y / 2.0,
            ),
            galley,
            skin.text,
        );
        if response.clicked() {
            picked = Some(which);
        }
        left = button.right() + 4.0;
    }
    picked
}

pub(super) fn feet(ui: &mut Ui, rect: Rect, skin: &skin::Skin) -> Option<Pressed> {
    ui.painter().rect_filled(rect, 0.0, skin.bar);
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, skin.edge));
    let (close, left) = foot_button(ui, rect, rect.right() - 10.0, "閉じる", skin);
    let (copy, _) = foot_button(ui, rect, left - 8.0, "コピー", skin);
    if close {
        Some(Pressed::Close)
    } else {
        copy.then_some(Pressed::Copy)
    }
}

fn foot_button(ui: &mut Ui, rect: Rect, right: f32, label: &str, skin: &skin::Skin) -> (bool, f32) {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(skin::TEXT_SIZE),
        skin.button_text,
    );
    let width = galley.size().x + 24.0;
    let button = Rect::from_min_size(
        pos2(right - width, rect.center().y - 10.0),
        vec2(width, 20.0),
    );
    let response = ui.interact(button, ui.scope_id().with(label), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(button, 3.0, skin.button_hover);
        painter.rect_stroke(
            button,
            3.0,
            Stroke::new(1.0, skin.button_hover_edge),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.rect_filled(button, 3.0, skin.button);
    }
    painter.galley(
        pos2(
            button.center().x - galley.size().x / 2.0,
            button.center().y - galley.size().y / 2.0,
        ),
        galley,
        skin.button_text,
    );
    (response.clicked(), button.left())
}

fn field(ui: &mut Ui, label: &str, value: &str, skin: &skin::Skin) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, skin::ROW), Sense::hover());
    let painter = ui.painter();
    painter.text(
        pos2(rect.left() + skin::PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(skin::TEXT_SIZE),
        skin.dim,
    );
    painter.text(
        pos2(rect.left() + skin::PAD + skin::LABEL, rect.center().y),
        egui::Align2::LEFT_CENTER,
        value,
        FontId::monospace(skin::TEXT_SIZE),
        skin.text,
    );
}

/// 画面に表示するのもコピーの文も、これ一つから作る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    /// 折りたためるノード。`open` は最初に開いているか。
    Branch {
        label: String,
        open: bool,
        children: Vec<Node>,
    },
    Leaf(String),
}

impl Node {
    pub(super) fn branch(label: impl Into<String>, open: bool, children: Vec<Node>) -> Self {
        Self::Branch {
            label: label.into(),
            open,
            children,
        }
    }

    pub(super) fn leaf(text: impl Into<String>) -> Self {
        Self::Leaf(text.into())
    }
}

pub(super) fn draw(ui: &mut Ui, nodes: &[Node], depth: usize, skin: &skin::Skin) {
    for node in nodes {
        match node {
            Node::Branch {
                label,
                open,
                children,
            } => branch(ui, label, *open, depth, skin, |ui| {
                draw(ui, children, depth + 1, skin);
            }),
            Node::Leaf(text) => leaf(ui, text, depth, skin),
        }
    }
}

/// 深さごとに空白二つずつ字下げしてコピーする。折りたたんでいるノードも中身ごと。
pub(super) fn write(out: &mut String, nodes: &[Node], depth: usize) {
    for node in nodes {
        let (text, children) = match node {
            Node::Branch {
                label, children, ..
            } => (label, Some(children)),
            Node::Leaf(text) => (text, None),
        };
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str(text);
        out.push('\n');
        if let Some(children) = children {
            write(out, children, depth + 1);
        }
    }
}

/// 開いているかは egui のメモリーからタイトルで調べる。`open` は最初の状態。
pub(super) fn branch(
    ui: &mut Ui,
    label: &str,
    open: bool,
    depth: usize,
    skin: &skin::Skin,
    children: impl FnOnce(&mut Ui),
) {
    let id = ui.scope_id().with(("ノード", label, depth));
    let mut opened: bool = ui.data(|d| d.get_temp(id)).unwrap_or(open);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, skin::ROW), Sense::click());
    if response.clicked() {
        opened = !opened;
        ui.data_mut(|d| d.insert_temp(id, opened));
    }
    let indent = 12.0 + depth as f32 * 20.0;
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, skin.edge);
    }
    icons::paint_at(
        ui.painter(),
        pos2(rect.left() + indent, rect.center().y),
        skin::TREE_MARK,
        if opened {
            Icon::TriDown
        } else {
            Icon::TriRight
        },
        skin.dim,
    );
    ui.painter().text(
        pos2(rect.left() + indent + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(skin::TEXT_SIZE),
        skin.text,
    );
    if opened {
        children(ui);
    }
}

pub(super) fn leaf(ui: &mut Ui, text: &str, depth: usize, skin: &skin::Skin) {
    row(ui, text, 20.0 + depth as f32 * 20.0, skin.leaf, skin);
}

fn row(ui: &mut Ui, text: &str, indent: f32, colour: Color32, skin: &skin::Skin) {
    let width = ui.available_width();
    let galley = ui.painter().layout(
        text.to_owned(),
        FontId::monospace(skin::TEXT_SIZE),
        colour,
        (width - indent - skin::PAD).max(40.0),
    );
    let height = galley.size().y.max(skin::ROW - 4.0) + 4.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, skin.edge);
    }
    ui.painter().galley(
        pos2(
            rect.left() + indent,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        colour,
    );
    response.context_menu(|ui| {
        if ui.button("この行をコピー").clicked() {
            ui.ctx().copy_text(text.trim().to_owned());
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests;
