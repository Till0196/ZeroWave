// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「ソース」のページの「coaxd 起動設定」と、「coaxd の更新」の別ウィンドウ。
//! coaxd は使われなくなると十数秒で終わるので、ページを開いている間は数秒ごとに見直す。

use std::time::{Duration, Instant};

use egui::{FontId, Rect, pos2, vec2};

use super::{Form, Look, Pressed, State, skin};

pub const WIDTH: f32 = 440.0;
pub const HEIGHT: f32 = 230.0;

const AGAIN: Duration = Duration::from_secs(3);

pub type Coaxd = std::sync::Arc<std::sync::Mutex<CoaxdFor>>;

#[derive(Debug, Default)]
pub struct CoaxdFor {
    pub seen: Option<crate::coaxd::Seen>,
    pub looked: Option<Instant>,
    pub looking: bool,
    pub told: Option<Told>,
    pub busy: bool,
    pub pending: Option<Pending>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    Done(String),
    Failed(String),
    /// 終わったが前のまま（動いている coaxd が終わらなかった）。
    Kept(String),
}

/// 動いているインストール先の coaxd を止めて入れ替えるので、先に確認する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub staged: crate::coaxd::Staged,
    pub now: String,
    pub pid: Option<u32>,
}

pub fn ask(coaxd: &Coaxd, endpoint: String) {
    if let Ok(mut slot) = coaxd.lock() {
        if slot.looking {
            return;
        }
        slot.looking = true;
    }
    let into = std::sync::Arc::clone(coaxd);
    let spawned = std::thread::Builder::new()
        .name(String::from("coaxd look"))
        .spawn(move || {
            let seen = crate::coaxd::look(&endpoint);
            if let Ok(mut slot) = into.lock() {
                slot.seen = Some(seen);
                slot.looked = Some(Instant::now());
                slot.looking = false;
            }
        });
    if spawned.is_err()
        && let Ok(mut slot) = coaxd.lock()
    {
        slot.looking = false;
    }
}

pub fn endpoint_of(file: &crate::config::File) -> String {
    file.sources
        .iter()
        .find(|source| source.kind == "coaxd")
        .map(|source| source.endpoint.clone())
        .unwrap_or_default()
}

pub(super) fn section(
    form: &mut Form<'_, '_>,
    coaxd: &Coaxd,
    endpoint: String,
    launch: bool,
) -> Option<Pressed> {
    let mut pressed = None;
    let (seen, told, busy, stale) = match coaxd.lock() {
        Ok(slot) => (
            slot.seen.clone(),
            slot.told.clone(),
            slot.busy,
            !slot.looking && slot.looked.is_none_or(|at| at.elapsed() >= AGAIN),
        ),
        Err(_) => (None, None, false, false),
    };
    if stale && !busy {
        ask(coaxd, endpoint);
    }
    form.ui.ctx().request_repaint_after(if seen.is_some() {
        AGAIN
    } else {
        Duration::from_millis(200)
    });

    form.section("coaxd 起動設定");
    let file = crate::coaxd::file();
    form.told(
        "ファイルパス",
        &file.as_ref().map_or_else(
            || String::from("（分かりません）"),
            |file| file.display().to_string(),
        ),
    );
    let look = form.look;
    let Some(seen) = seen else {
        let value = form.told_row("状態");
        status(
            form,
            pos2(value.left(), value.center().y),
            "確認しています…",
            look.label,
        );
        return None;
    };
    let (said, colour) = match (&seen.running, seen.present) {
        (Some(_), _) => ("coaxd 動作中", look.ok),
        (None, true) if launch => ("必要に応じて起動します", look.label),
        (None, true) => ("停止中（連携起動しません）", look.label),
        (None, false) => ("見つかりません", look.typed),
    };
    {
        let value = form.told_row("状態");
        status(form, pos2(value.left(), value.center().y), said, colour);
    }
    if let Some(running) = &seen.running {
        // インストール先の物でなければ、どこで動いているかを出す（止めも替えもしない）。
        match (seen.running_file(), seen.ours) {
            (_, Some(true)) => {}
            (Some(path), _) => {
                form.told("実行ファイル", &path.display().to_string());
                form.hint("連動起動の coaxd ではありません");
            }
            (None, _) => form.told_in("実行ファイル", "確認できません", look.label),
        }
        let started = crate::coaxd::shown_time_of(&running.started);
        let now = joined(&[
            running.version.as_str(),
            crate::coaxd::shown_arch(&running.arch),
            &if started.is_empty() {
                String::new()
            } else {
                format!("{started}から")
            },
        ]);
        if seen.ours == Some(true) || !seen.present {
            form.told("バージョン", &now);
        } else {
            form.told("バージョン", &format!("動作中 {now}"));
            form.hint(format!("ファイルパス {}", file_version(&seen)));
        }
    } else if seen.present {
        form.told("バージョン", &file_version(&seen));
    }

    let row = form.line(skin::SELECT_HEIGHT);
    let add = Rect::from_min_size(row.min, vec2(124.0, row.height()));
    let clicked = if seen.present {
        form.button_enabled(add, ("coaxd.put", 0), "coaxd の更新", !busy)
    } else {
        form.primary_button(add, ("coaxd.put", 0), "＋ coaxd の追加", !busy)
    };
    if clicked {
        pressed = Some(Pressed::PutCoaxd);
    }
    let mut x = add.right() + 8.0;
    if let Some(folder) = crate::coaxd::folder() {
        let open = Rect::from_min_size(pos2(x, row.top()), vec2(120.0, row.height()));
        if form.button(open, ("coaxd.folder", 0), "フォルダを開く") {
            let _ = std::fs::create_dir_all(&folder);
            pressed = Some(Pressed::Reveal(folder));
        }
        x = open.right() + 12.0;
    }
    let painter = form.ui.painter();
    let at = pos2(x, row.center().y);
    match told {
        Some(Told::Done(text)) => {
            painter.circle_filled(pos2(at.x + 3.0, at.y), 3.5, look.ok);
            painter.text(
                pos2(at.x + 12.0, at.y),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                look.ok,
            );
        }
        Some(Told::Failed(text)) => {
            painter.text(
                at,
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                look.error,
            );
        }
        Some(Told::Kept(text)) => {
            painter.text(
                at,
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                look.typed,
            );
        }
        None if busy => {
            painter.text(
                at,
                egui::Align2::LEFT_CENTER,
                "処理しています…",
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                look.label,
            );
        }
        None => {}
    }
    if !seen.present {
        form.note("coaxd を追加すると ZeroWave の起動と連動できます。");
    }
    pressed
}

fn file_version(seen: &crate::coaxd::Seen) -> String {
    let version = if seen.version.is_empty() {
        "（バージョンを確認できません）"
    } else {
        seen.version.as_str()
    };
    joined(&[version, &seen.arch, &seen.put_at])
}

fn joined(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("・")
}

fn status(form: &mut Form<'_, '_>, at: egui::Pos2, said: &str, colour: egui::Color32) {
    let painter = form.ui.painter();
    painter.circle_filled(pos2(at.x + 3.0, at.y), 3.0, colour);
    painter.text(
        pos2(at.x + 12.0, at.y),
        egui::Align2::LEFT_CENTER,
        said,
        FontId::proportional(skin::TEXT_SIZE - 0.5),
        colour,
    );
}

pub fn show(root: &mut egui::Ui, state: &State, look: &Look) -> Option<Pressed> {
    let pending = state
        .coaxd
        .lock()
        .ok()
        .and_then(|slot| slot.pending.clone());
    let Some(pending) = pending else {
        return Some(Pressed::Close);
    };
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let body = full.shrink2(vec2(22.0, 18.0));
    let painter = root.painter();
    let mut at = body.top();
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        "coaxd を更新しますか",
        FontId::proportional(skin::TEXT_SIZE + 2.0),
        look.text,
    );
    at += 28.0;
    painter.text(
        pos2(body.left(), at),
        egui::Align2::LEFT_TOP,
        "視聴中の番組は一度途切れ、更新完了後に選局し直します。",
        FontId::proportional(skin::TEXT_SIZE),
        look.label,
    );
    at += 28.0;
    let table = Rect::from_min_size(pos2(body.left(), at), vec2(body.width(), 62.0));
    painter.rect_filled(table, skin::SELECT_ROUNDING, look.select_fill);
    painter.rect_stroke(
        table,
        skin::SELECT_ROUNDING,
        egui::Stroke::new(1.0, look.select_edge),
        egui::StrokeKind::Inside,
    );
    let after = joined(&[pending.staged.version.as_str(), &pending.staged.arch]);
    for (index, (label, value)) in [("現在", pending.now.as_str()), ("更新後", after.as_str())]
        .into_iter()
        .enumerate()
    {
        let y = table.top() + 18.0 + index as f32 * 26.0;
        painter.text(
            pos2(table.left() + 12.0, y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(skin::TEXT_SIZE),
            look.label,
        );
        painter.text(
            pos2(table.left() + 76.0, y),
            egui::Align2::LEFT_CENTER,
            value,
            FontId::proportional(skin::TEXT_SIZE),
            look.text,
        );
    }
    let mut row_at = body.bottom() - skin::SELECT_HEIGHT;
    let mut form = Form::new(root, look, full, &mut row_at);
    let row = Rect::from_min_size(
        pos2(body.left(), body.bottom() - skin::SELECT_HEIGHT),
        vec2(body.width(), skin::SELECT_HEIGHT),
    );
    let mut pressed = None;
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 196.0, row.top()),
            vec2(92.0, row.height()),
        ),
        ("coaxd.update.cancel", 0),
        "キャンセル",
    ) {
        pressed = Some(Pressed::UpdateCoaxd(false));
    }
    if form.primary_button(
        Rect::from_min_size(
            pos2(row.right() - 96.0, row.top()),
            vec2(96.0, row.height()),
        ),
        ("coaxd.update.go", 0),
        "更新する",
        true,
    ) {
        pressed = Some(Pressed::UpdateCoaxd(true));
    }
    pressed
}
