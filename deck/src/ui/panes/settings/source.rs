// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「ソース」のページ。`[[source]]`（とその下のチューナー設定）、coaxd 起動設定、`[[profile]]`。

use egui::{Color32, FontId, Rect, Stroke, pos2, vec2};

use super::scan::{PLANS, boards_answer, offered, system_name, systems_of};
use super::{Boards, Checks, File, Form, Pressed, skin};
use crate::ui::icons::Icon;

const KINDS: &[(&str, &str)] = &[("coaxd", "coaxd"), ("mirakurun", "Mirakurun")];

const BY: &[(&str, &str)] = &[
    ("auto", "自動"),
    ("channel", "チャンネルで指定"),
    ("service", "サービスで指定"),
];

/// 暗黙のプロファイル「all」はファイルに書かず、名前は変えられない。
#[allow(clippy::too_many_arguments)]
pub(super) fn source(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
    profile: &str,
    active: &str,
    checks: &Checks,
    boards: &Boards,
    reach: &super::Reach,
    coaxd: &super::coaxd::Coaxd,
) -> Option<Pressed> {
    use crate::config::file::Source;
    let mut pressed = None;

    form.section("ソース");
    let checked = checks.lock().map(|c| c.clone()).unwrap_or_default();
    let mut remove = None;
    // 名前を変えたソース（前、後）。プロファイルの中の名前も追う。
    let mut renamed: Vec<(String, String)> = Vec::new();
    let first_coaxd = edited
        .sources
        .iter()
        .position(|source| source.kind == "coaxd");
    // チューナーの方式を調べるためのコピー（ソースを直しながら全体は借りられない）。
    let snapshot = edited.clone();
    for index in 0..edited.sources.len() {
        if index == 0 {
            *form.at += 8.0;
        }
        let top = *form.at;
        let source: &mut Source = &mut edited.sources[index];
        let was = loaded.sources.iter().find(|had| had.label == source.label);
        let row = form.line(skin::SELECT_HEIGHT);
        let left = row.left() + 12.0;
        form.select_at(
            Rect::from_min_size(pos2(left, row.top()), vec2(110.0, row.height())),
            ("source.kind", index),
            &mut source.kind,
            KINDS,
        );
        let before = source.label.clone();
        form.field(
            Rect::from_min_size(pos2(left + 118.0, row.top()), vec2(150.0, row.height())),
            ("source.label", index),
            &mut source.label,
            "名前",
        );
        if source.label != before {
            renamed.push((before, source.label.clone()));
        }
        let told = checked.get(&source.label);
        if let Some(told) = told {
            let (said, colour) = match told {
                Ok(version) => {
                    let version = version.rsplit(' ').next().unwrap_or(version);
                    (format!("接続済み（{version}）"), form.look.ok)
                }
                Err(_) => (String::from("接続できません"), form.look.error),
            };
            status(form, pos2(left + 280.0, row.center().y), &said, colour);
        }
        if form.button(
            Rect::from_min_size(
                pos2(row.right() - 60.0, row.top()),
                vec2(60.0, row.height()),
            ),
            ("source.delete", index),
            "削除",
        ) {
            remove = Some(index);
        }
        match source.kind.as_str() {
            "mirakurun" => {
                let url_was = was.map_or("", |had| had.url.as_str());
                labelled_field(
                    form,
                    "URL",
                    ("source.url", index),
                    &mut source.url,
                    url_was,
                    "http://ホスト:40772",
                );
                let by_was = was.map_or("auto", |had| had.by.as_str()).to_owned();
                labelled_select(
                    form,
                    "ストリーム取得",
                    ("source.by", index),
                    &mut source.by,
                    &by_was,
                    BY,
                );
                if let Some(Err(why)) = told {
                    let colour = form.look.error;
                    let left = form.label_left();
                    form.wrapped_in(left, why.clone(), colour, skin::TEXT_SIZE - 1.0);
                }
            }
            _ => {
                let endpoint_was = was.map_or("", |had| had.endpoint.as_str());
                labelled_field(
                    form,
                    "接続先",
                    ("source.endpoint", index),
                    &mut source.endpoint,
                    endpoint_was,
                    "この PC の coaxd（既定）",
                );
                let pcsc_was = was.is_some_and(|had| had.pcsc);
                form.check(
                    "",
                    "PC/SC 互換ソケットを配置する",
                    &mut source.pcsc,
                    pcsc_was,
                );
                if source.pcsc {
                    let usual = crate::pcsc::default_socket().display().to_string();
                    let socket_was = was.map_or("", |had| had.pcsc_socket.as_str());
                    labelled_field(
                        form,
                        "ソケットの場所",
                        ("source.pcsc_socket", index),
                        &mut source.pcsc_socket,
                        socket_was,
                        &usual,
                    );
                }
                if let Some(Err(why)) = told {
                    let colour = form.look.error;
                    let left = form.label_left();
                    form.wrapped_in(left, why.clone(), colour, skin::TEXT_SIZE - 1.0);
                }
                if Some(index) == first_coaxd
                    && let Some(picked) = tuners_under(form, &snapshot, source, boards, reach)
                {
                    pressed = Some(picked);
                }
            }
        }
        let bottom = *form.at;
        let colour = if source.kind == "mirakurun" {
            form.look.mirakurun
        } else {
            form.look.coaxd
        };
        let card = Rect::from_min_max(pos2(row.left(), top - 6.0), pos2(row.right() + 6.0, bottom));
        form.ui.painter().rect_stroke(
            card,
            6.0,
            Stroke::new(1.0, form.look.button_edge),
            egui::StrokeKind::Inside,
        );
        form.ui.painter().rect_filled(
            Rect::from_min_size(card.min, vec2(3.0, card.height())),
            1.5,
            colour.gamma_multiply(0.8),
        );
        *form.at += 14.0;
    }
    for (before, after) in renamed {
        for profile in &mut edited.profiles {
            for name in &mut profile.sources {
                if *name == before {
                    name.clone_from(&after);
                } else if let Some(tuner) = name.strip_prefix(&format!("{before}/")) {
                    *name = format!("{after}/{tuner}");
                }
            }
        }
    }
    if let Some(index) = remove {
        let label = edited.sources[index].label.clone();
        edited.sources.remove(index);
        for profile in &mut edited.profiles {
            profile
                .sources
                .retain(|name| name != &label && !name.starts_with(&format!("{label}/")));
        }
    }
    let row = form.line(skin::SELECT_HEIGHT);
    let add = Rect::from_min_size(row.min, vec2(88.0, row.height()));
    if let Some(picked) =
        form.menu_button(add, ("source.add", 0), "＋ 追加", &["coaxd", "Mirakurun"])
    {
        let kind = KINDS[picked].0;
        let base = kind.to_owned();
        let mut name = base.clone();
        let mut n = 2;
        while edited.sources.iter().any(|source| source.label == name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        edited.sources.push(Source {
            label: name,
            kind: kind.to_owned(),
            ..Source::default()
        });
    }
    if form.button(
        Rect::from_min_size(pos2(add.right() + 8.0, row.top()), vec2(96.0, row.height())),
        ("source.check", 0),
        "接続を確認",
    ) {
        pressed = Some(Pressed::Check);
    }

    let launch = edited.general.launch_coaxd;
    if let Some(put) = super::coaxd::section(form, coaxd, super::coaxd::endpoint_of(edited), launch)
    {
        pressed = Some(put);
    }
    form.key("general.launch_coaxd");
    form.check(
        "",
        "連携起動（ZeroWave が必要なときに coaxd を起動する）",
        &mut edited.general.launch_coaxd,
        loaded.general.launch_coaxd,
    );
    form.hint("オフにすると、起動している coaxd に接続するだけになります");

    if let Some(picked) = profiles(form, edited, profile, active, boards) {
        pressed = Some(picked);
    }
    pressed
}

fn status(form: &mut Form<'_, '_>, at: egui::Pos2, said: &str, colour: Color32) {
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

fn labelled_field(
    form: &mut Form<'_, '_>,
    label: &str,
    id: (&str, usize),
    value: &mut String,
    was: &str,
    hint: &str,
) {
    let changed = value != was;
    let value_rect = form.label_row(label, changed);
    form.field(
        Rect::from_min_size(
            pos2(
                value_rect.left(),
                value_rect.center().y - skin::SELECT_HEIGHT / 2.0,
            ),
            vec2(form.fit(skin::SELECT_WIDTH), skin::SELECT_HEIGHT),
        ),
        id,
        value,
        hint,
    );
}

fn labelled_select(
    form: &mut Form<'_, '_>,
    label: &str,
    id: (&str, usize),
    value: &mut String,
    was: &str,
    choices: &[(&str, &str)],
) {
    let changed = value != was;
    let value_rect = form.label_row(label, changed);
    form.select_at(
        Rect::from_min_size(
            pos2(
                value_rect.left(),
                value_rect.center().y - skin::SELECT_HEIGHT / 2.0,
            ),
            vec2(form.fit(skin::SELECT_WIDTH), skin::SELECT_HEIGHT),
        ),
        id,
        value,
        choices,
    );
}

fn profiles(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    profile: &str,
    active: &str,
    boards: &Boards,
) -> Option<Pressed> {
    use crate::config::file::Profile;
    let mut pressed = None;
    form.section("プロファイル");
    let (picked, select) = super::pick_profile(form, edited, profile);
    if let Some(picked) = picked {
        pressed = Some(picked);
    }
    let add = Rect::from_min_size(
        pos2(select.right() + 8.0, select.top()),
        vec2(72.0, select.height()),
    );
    if form.button(add, ("profile.add", 0), "＋ 追加") {
        let mut n = edited.profiles.len() + 1;
        let mut name = format!("プロファイル {n}");
        while edited.profiles.iter().any(|profile| profile.name == name) {
            n += 1;
            name = format!("プロファイル {n}");
        }
        edited.profiles.push(Profile {
            name: name.clone(),
            sources: edited
                .sources
                .iter()
                .map(|source| source.label.clone())
                .collect(),
            ..Profile::default()
        });
        pressed = Some(Pressed::Profile(name));
    }
    let is_main = profile.is_empty() || !edited.profiles.iter().any(|p| p.name == profile);
    if !is_main
        && form.button(
            Rect::from_min_size(
                pos2(add.right() + 8.0, select.top()),
                vec2(60.0, select.height()),
            ),
            ("profile.delete", 0),
            "削除",
        )
    {
        edited.profiles.retain(|p| p.name != profile);
        pressed = Some(Pressed::Profile(String::new()));
    }
    form.note(format!(
        "メニューの「プロファイル」から切り替えられます\n現在選択されているプロファイル「{}」",
        if active.is_empty() {
            super::ALL
        } else {
            active
        }
    ));
    if is_main {
        form.note(
            "all は書いた順の全ソースと全チューナーです。絞るにはプロファイルを追加してください",
        );
        return pressed;
    }
    let Some(index) = edited.profiles.iter().position(|p| p.name == profile) else {
        return pressed;
    };
    let before = edited.profiles[index].name.clone();
    labelled_field(
        form,
        "名前",
        ("profile.name", index),
        &mut edited.profiles[index].name,
        &before.clone(),
        "名前",
    );
    if edited.profiles[index].name != before && !edited.profiles[index].name.is_empty() {
        let after = edited.profiles[index].name.clone();
        if !before.is_empty() {
            for network in &mut edited.networks {
                if network.profile == before {
                    network.profile = after.clone();
                }
            }
        }
        pressed = Some(Pressed::Profile(after));
    }
    let rows = profile_rows(edited, boards);
    let known = |name: &str| {
        let label = name.split_once('/').map_or(name, |(label, _)| label);
        edited.sources.iter().any(|source| source.label == label)
    };
    let chosen: Vec<String> = edited.profiles[index].sources.clone();
    let mut ordered: Vec<(String, Row, bool)> = chosen
        .iter()
        .filter(|name| known(name))
        .map(|name| {
            let shown = rows.iter().find(|(row, _)| row == name).map_or_else(
                || Row {
                    kind: String::from("coaxd"),
                    said: name.clone(),
                },
                |(_, shown)| shown.clone(),
            );
            (name.clone(), shown, true)
        })
        .collect();
    ordered.extend(
        rows.iter()
            .filter(|(name, _)| !chosen.contains(name))
            .map(|(name, shown)| (name.clone(), shown.clone(), false)),
    );
    let top = *form.at;
    let head = form.line(26.0);
    for (x, text) in [
        (10.0, "利用"),
        (60.0, "ソースとチューナー（上から優先）"),
        (head.width() - 70.0, "順番"),
    ] {
        form.ui.painter().text(
            pos2(head.left() + x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    let mut next = chosen.clone();
    for (place, (name, shown, on)) in ordered.iter().enumerate() {
        let row = form.line(30.0);
        form.ui.painter().hline(
            row.x_range(),
            row.top() - 3.0,
            Stroke::new(1.0, form.look.edge),
        );
        if form.tick(
            Rect::from_min_size(pos2(row.left() + 10.0, row.top()), vec2(24.0, row.height())),
            ("profile.source", place),
            *on,
        ) {
            if *on {
                next.retain(|n| n != name);
            } else {
                next.push(name.clone());
            }
        }
        let x = row.left() + 60.0;
        let width = form.kind_tag(pos2(x, row.center().y), &shown.kind);
        form.ui.painter().text(
            pos2(x + width + 6.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            &shown.said,
            FontId::proportional(skin::TEXT_SIZE),
            if *on { form.look.text } else { form.look.label },
        );
        if *on {
            let at = next.iter().position(|n| n == name);
            let up = Rect::from_min_size(
                pos2(row.right() - 70.0, row.center().y - 12.0),
                vec2(26.0, 24.0),
            );
            if form.button(up, ("profile.up", place), Icon::ArrowUp)
                && let Some(at) = at
                && at > 0
            {
                next.swap(at, at - 1);
            }
            let down = Rect::from_min_size(pos2(up.right() + 4.0, up.top()), up.size());
            if form.button(down, ("profile.down", place), Icon::ArrowDown)
                && let Some(at) = at
                && at + 1 < next.len()
            {
                next.swap(at, at + 1);
            }
        }
    }
    let frame = Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), *form.at - 2.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, form.look.button_edge),
        egui::StrokeKind::Inside,
    );
    *form.at += 4.0;
    if next != chosen {
        edited.profiles[index].sources = next;
    }
    let missing: Vec<String> = edited.profiles[index]
        .sources
        .iter()
        .filter(|name| !known(name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let row = form.line(skin::SELECT_HEIGHT);
        if form.button(
            Rect::from_min_size(row.min, vec2(60.0, row.height())),
            ("profile.missing", index),
            "外す",
        ) {
            edited.profiles[index].sources.retain(|name| known(name));
        }
        form.ui.painter().text(
            pos2(row.left() + 68.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            format!("無いソースを指しています: {}", missing.join("、")),
            FontId::proportional(skin::TEXT_SIZE),
            form.look.typed,
        );
    }
    pressed
}

#[derive(Debug, Clone)]
struct Row {
    kind: String,
    said: String,
}

fn profile_rows(file: &File, boards: &Boards) -> Vec<(String, Row)> {
    let answer = boards_answer(boards);
    let listed: Vec<zwcore::input::source::coaxd::DaemonTuner> = match &answer {
        Some(Ok(boards)) => boards
            .iter()
            .flat_map(|board| &board.tuners)
            .map(|held| held.tuner.clone())
            .collect(),
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for source in &file.sources {
        let kind = source.kind.clone();
        if source.kind != "coaxd" {
            rows.push((
                source.label.clone(),
                Row {
                    kind,
                    said: source.label.clone(),
                },
            ));
            continue;
        }
        rows.push((
            source.label.clone(),
            Row {
                kind: kind.clone(),
                said: format!("{}（どのチューナーでも）", source.label),
            },
        ));
        for tuner in &listed {
            rows.push((
                format!("{}/{}", source.label, tuner.id),
                Row {
                    kind: kind.clone(),
                    said: tuner.label(),
                },
            ));
        }
    }
    rows
}

/// 方式を返さないチューナー（BDA）のチップ。チャンネルスキャンできる物だけ。
const DECLARABLE: &[&str] = &["ISDB-T", "ISDB-S", "ISDB-S3", "ISDB-C", "DVB-S2"];

fn satellite(system: &str) -> bool {
    let upper = system.to_ascii_uppercase().replace('-', "");
    upper.starts_with("ISDBS") || upper.starts_with("DVBS")
}

fn say(form: &mut Form<'_, '_>, at: egui::Pos2, text: &str, colour: Color32, size: f32) -> f32 {
    let galley =
        form.ui
            .painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(size), colour);
    let width = galley.size().x;
    form.ui
        .painter()
        .galley(pos2(at.x, at.y - galley.size().y / 2.0), galley, colour);
    width
}

fn chip(
    form: &mut Form<'_, '_>,
    at: egui::Pos2,
    text: &str,
    colour: Color32,
    fill: Color32,
) -> f32 {
    let galley =
        form.ui
            .painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(10.5), colour);
    let rect = Rect::from_min_size(pos2(at.x, at.y - 8.0), vec2(galley.size().x + 12.0, 16.0));
    form.ui.painter().rect_filled(rect, 3.0, fill);
    form.ui.painter().galley(
        pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
    rect.width()
}

/// 方式を返さない（BDA）チューナーの方式は人が選ぶ。LNB 給電と利得は coaxd が返した物だけ。
fn tuners_under(
    form: &mut Form<'_, '_>,
    file: &File,
    source: &mut crate::config::file::Source,
    boards: &Boards,
    reach: &super::Reach,
) -> Option<Pressed> {
    use crate::config::file::TunerSetting;
    let mut pressed = None;
    *form.at += 6.0;
    let head = form.line(26.0);
    let left = head.left() + 12.0;
    say(
        form,
        pos2(left, head.center().y),
        "チューナー",
        form.look.section,
        skin::SECTION_SIZE,
    );
    if form.button(
        Rect::from_min_size(
            pos2(left + 76.0, head.top() + 1.0),
            vec2(60.0, head.height() - 2.0),
        ),
        ("tuners.ask", 0),
        "更新",
    ) {
        pressed = Some(Pressed::Boards);
    }
    let listed = match boards_answer(boards) {
        None => {
            inner_note(
                form,
                "coaxd にチューナーを問い合わせています…",
                form.look.label,
            );
            return pressed;
        }
        Some(Err(why)) => {
            inner_note(
                form,
                &format!("チューナーの一覧が取れません: {why}"),
                form.look.error,
            );
            return pressed;
        }
        Some(Ok(listed)) => listed,
    };
    if listed.iter().all(|board| board.tuners.is_empty()) {
        inner_note(form, "チューナーが見つかりません", form.look.typed);
        return pressed;
    }
    for board in &listed {
        *form.at += 4.0;
        let row = form.line(24.0);
        let x =
            left + say(
                form,
                pos2(left, row.center().y),
                &board.model,
                form.look.text,
                skin::TEXT_SIZE + 0.5,
            ) + 8.0;
        if !board.backend.is_empty() {
            chip(
                form,
                pos2(x, row.center().y),
                &board.backend.to_ascii_uppercase(),
                form.look.label,
                form.look.select_fill,
            );
        }
        let shared = board.tuners.iter().find_map(|held| {
            held.controls
                .lnb
                .as_ref()
                .filter(|lnb| !lnb.shared.is_empty())
                .cloned()
        });
        if let Some(lnb) = &shared {
            let ids: Vec<String> = board
                .tuners
                .iter()
                .filter(|held| held.controls.lnb.is_some())
                .map(|held| held.tuner.id.clone())
                .collect();
            let mut value = ids
                .iter()
                .find_map(|id| source.tuner_settings.get(id).map(|one| one.lnb.clone()))
                .unwrap_or_default();
            let before = value.clone();
            lnb_row(
                form,
                left,
                ("board.lnb", &board.name),
                lnb,
                &mut value,
                true,
            );
            if value != before {
                for id in &ids {
                    let mut setting = source.tuner_settings.get(id).cloned().unwrap_or_default();
                    setting.lnb.clone_from(&value);
                    store(source, id, setting);
                }
            }
        }
        for held in &board.tuners {
            let tuner = &held.tuner;
            let id = tuner.id.clone();
            let mut setting: TunerSetting =
                source.tuner_settings.get(&id).cloned().unwrap_or_default();
            let top = *form.at;
            let indent = left + 14.0;
            let row = form.line(24.0);
            let name = if tuner.name.is_empty() {
                "チューナー"
            } else {
                tuner.name.as_str()
            };
            let mut x = indent
                + say(
                    form,
                    pos2(indent, row.center().y),
                    name,
                    form.look.text,
                    skin::TEXT_SIZE,
                )
                + 8.0;
            x += say(
                form,
                pos2(x, row.center().y),
                &id,
                form.look.group,
                skin::TEXT_SIZE - 1.0,
            ) + 8.0;
            if tuner.leased {
                chip(
                    form,
                    pos2(x, row.center().y),
                    "使用中",
                    form.look.typed,
                    form.look.typed.gamma_multiply(0.18),
                );
            } else {
                chip(
                    form,
                    pos2(x, row.center().y),
                    "空き",
                    form.look.label,
                    form.look.select_fill,
                );
            }
            let named = !tuner.deliveries.is_empty();
            let told: Vec<String> = {
                let mut systems: Vec<String> = tuner
                    .deliveries
                    .iter()
                    .map(|one| system_name(one))
                    .collect();
                systems.dedup();
                systems
            };
            // ISDB-S だけを返すチューナーに ISDB-S3 を足せる（Linux の DVB は S3 に番号が無い）。
            // 足した物は選局のとき ISDB-S の名前で要求する。
            let can_add_s3 = named
                && told.iter().any(|one| one == "ISDB-S")
                && !told.iter().any(|one| one == "ISDB-S3");
            let chosen = |setting: &TunerSetting, system: &str| {
                setting.systems.iter().any(|one| system_name(one) == system)
            };
            let added_s3 = can_add_s3 && chosen(&setting, "ISDB-S3");
            let offered_systems: Vec<String> = if named {
                let mut systems = told.clone();
                if added_s3 {
                    systems.push(String::from("ISDB-S3"));
                }
                systems
            } else {
                let mut systems: Vec<String> =
                    DECLARABLE.iter().map(|one| (*one).to_owned()).collect();
                for one in &setting.systems {
                    let shown = system_name(one);
                    if !systems.contains(&shown) {
                        systems.push(shown);
                    }
                }
                systems
            };
            let label_left = indent;
            let value_left = indent + 84.0;
            if named && offered_systems.len() < 2 && !can_add_s3 {
                let row = form.line(26.0);
                say(
                    form,
                    pos2(label_left, row.center().y),
                    "受信方式",
                    form.look.label,
                    skin::TEXT_SIZE,
                );
                say(
                    form,
                    pos2(value_left, row.center().y),
                    &offered_systems.join("・"),
                    form.look.text,
                    skin::TEXT_SIZE,
                );
            } else {
                let items: Vec<(String, bool, bool)> = offered_systems
                    .iter()
                    .map(|system| {
                        let on = if named && told.contains(system) {
                            setting.systems.is_empty() || chosen(&setting, system)
                        } else {
                            chosen(&setting, system)
                        };
                        (system.clone(), on, on)
                    })
                    .collect();
                let top_row = *form.at;
                say(
                    form,
                    pos2(label_left, top_row + 13.0),
                    "受信方式",
                    form.look.label,
                    skin::TEXT_SIZE,
                );
                let right = row.right() - 8.0;
                let (hit, bottom) = form.chips_at(
                    pos2(value_left, top_row + 1.0),
                    right,
                    ("tuner.system", id.as_str()),
                    &items,
                );
                let mut added = None;
                let mut end = bottom;
                let others: Vec<(&str, &str)> = if named {
                    if can_add_s3 && !added_s3 {
                        vec![("ISDB-S3", "ISDBS3")]
                    } else {
                        Vec::new()
                    }
                } else {
                    super::scan::OFFERED
                        .iter()
                        .map(|(_, shown, system)| (*shown, *system))
                        .filter(|(shown, _)| !offered_systems.iter().any(|had| had == shown))
                        .collect()
                };
                if !others.is_empty() {
                    let other =
                        Rect::from_min_size(pos2(value_left, bottom + 6.0), vec2(82.0, 24.0));
                    let names: Vec<&str> = others.iter().map(|(shown, _)| *shown).collect();
                    if let Some(picked) =
                        form.menu_button(other, ("tuner.other", id.as_str()), "＋ その他", &names)
                    {
                        added = Some(others[picked].1.to_owned());
                    }
                    end = other.bottom();
                }
                *form.at = end + 6.0;
                let all_told = |now: &[String]| told.iter().all(|one| now.contains(one));
                if let Some(hit) = hit {
                    let system = offered_systems[hit].clone();
                    let on = !items[hit].1;
                    let mut now: Vec<String> = items
                        .iter()
                        .filter(|(_, on, _)| *on)
                        .map(|(system, _, _)| system.clone())
                        .collect();
                    now.retain(|one| *one != system);
                    if on {
                        now.push(system);
                    }
                    now.sort_by_key(|one| offered_systems.iter().position(|known| known == one));
                    // 返された方式を全部使い何も足さないなら書かない（空は全部）。
                    let extra = now.iter().any(|one| !told.contains(one));
                    if !named {
                        setting.systems = now.iter().map(|one| spelled(one)).collect();
                    } else if all_told(&now) && !extra {
                        setting.systems.clear();
                    } else if now.iter().any(|one| told.contains(one)) {
                        setting.systems = now.iter().map(|one| spelled(one)).collect();
                    }
                }
                if let Some(system) = added {
                    if named && setting.systems.is_empty() {
                        setting.systems = told.iter().map(|one| spelled(one)).collect();
                    }
                    if !setting.systems.contains(&system) {
                        setting.systems.push(system);
                    }
                }
                if !named && setting.systems.is_empty() {
                    let row = form.line(34.0);
                    let galley = form.ui.painter().layout(
                        String::from(
                            "このチューナーは対応受信方式を識別できません。\n上記ボタンから選択してください。",
                        ),
                        FontId::proportional(skin::TEXT_SIZE - 1.0),
                        form.look.label,
                        row.right() - value_left,
                    );
                    form.ui
                        .painter()
                        .galley(pos2(value_left, row.top()), galley, form.look.label);
                }
            }
            let using: Vec<String> =
                zwcore::input::source::coaxd::systems_used(&tuner.deliveries, &setting.to_core())
                    .iter()
                    .map(|one| system_name(one))
                    .collect();
            if let Some(lnb) = held
                .controls
                .lnb
                .as_ref()
                .filter(|lnb| lnb.shared.is_empty())
                && using.iter().any(|one| satellite(one))
            {
                lnb_row(
                    form,
                    indent,
                    ("tuner.lnb", &id),
                    lnb,
                    &mut setting.lnb,
                    false,
                );
            }
            if let Some(gain) = &held.controls.gain {
                let applies = gain.deliveries.is_empty()
                    || gain
                        .deliveries
                        .iter()
                        .any(|one| using.contains(&system_name(one)));
                if applies {
                    gain_row(form, indent, &id, gain, &mut setting.gain);
                }
            }
            let mut probe = file.clone();
            if let Some(had) = probe
                .sources
                .iter_mut()
                .find(|one| one.label == source.label)
            {
                if setting.is_empty() {
                    had.tuner_settings.remove(&id);
                } else {
                    had.tuner_settings.insert(id.clone(), setting.clone());
                }
            }
            let each: Vec<String> = PLANS
                .iter()
                .filter(|plan| {
                    let (_, system) = offered(plan);
                    systems_of(&probe, tuner)
                        .iter()
                        .any(|one| zwcore::input::source::coaxd::same_system(one, system))
                })
                .map(|plan| {
                    let (shown, _) = offered(plan);
                    match reach.get(*plan).map(Vec::len) {
                        Some(count) if count > 0 => format!("{shown} {count} 局"),
                        _ => format!("{shown} 未実施"),
                    }
                })
                .collect();
            let row = form.line(24.0);
            say(
                form,
                pos2(indent, row.center().y),
                "スキャン結果",
                form.look.label,
                skin::TEXT_SIZE,
            );
            say(
                form,
                pos2(value_left, row.center().y),
                &if each.is_empty() {
                    String::from("—")
                } else {
                    each.join("・")
                },
                form.look.text,
                skin::TEXT_SIZE,
            );
            let bottom = *form.at - 2.0;
            form.ui.painter().rect_filled(
                Rect::from_min_max(pos2(left, top + 2.0), pos2(left + 2.0, bottom)),
                1.0,
                form.look.edge,
            );
            *form.at += 4.0;
            store(source, &id, setting);
        }
    }
    pressed
}

fn spelled(shown: &str) -> String {
    super::scan::OFFERED
        .iter()
        .find(|(_, name, _)| *name == shown)
        .map_or_else(|| shown.to_owned(), |(_, _, system)| (*system).to_owned())
}

fn inner_note(form: &mut Form<'_, '_>, text: &str, colour: Color32) {
    let left = form.label_left();
    *form.at += 2.0;
    form.wrapped_in(left, text.to_owned(), colour, skin::TEXT_SIZE - 0.5);
    *form.at += 2.0;
}

fn store(
    source: &mut crate::config::file::Source,
    id: &str,
    setting: crate::config::file::TunerSetting,
) {
    if setting.is_empty() {
        source.tuner_settings.remove(id);
    } else if source.tuner_settings.get(id) != Some(&setting) {
        source.tuner_settings.insert(id.to_owned(), setting);
    }
}

fn lnb_row(
    form: &mut Form<'_, '_>,
    left: f32,
    id: (&str, &str),
    lnb: &zwcore::input::source::coaxd::LnbControl,
    value: &mut String,
    shared: bool,
) {
    let row = form.line(skin::SELECT_HEIGHT);
    say(
        form,
        pos2(left, row.center().y),
        "LNB 給電",
        form.look.label,
        skin::TEXT_SIZE,
    );
    let usual = if lnb.off() {
        "既定（給電しない）"
    } else {
        "既定（給電する）"
    };
    let low = format!("給電する（{}）", lnb.low);
    let high = if lnb.high.is_empty() {
        String::from("給電する")
    } else {
        format!("給電する（{}）", lnb.high)
    };
    let mut choices: Vec<(&str, &str)> = vec![("", usual)];
    if !lnb.low.is_empty() {
        choices.push(("low", low.as_str()));
    }
    choices.push(("high", high.as_str()));
    let select = Rect::from_min_size(pos2(left + 84.0, row.top()), vec2(180.0, row.height()));
    form.select_at(select, id, value, &choices);
    if shared {
        say(
            form,
            pos2(select.right() + 10.0, row.center().y),
            "全チューナー共通",
            form.look.label,
            skin::TEXT_SIZE - 1.0,
        );
    }
}

fn gain_row(
    form: &mut Form<'_, '_>,
    left: f32,
    id: &str,
    gain: &zwcore::input::source::coaxd::GainControl,
    value: &mut String,
) {
    let row = form.line(skin::SELECT_HEIGHT);
    say(
        form,
        pos2(left, row.center().y),
        "利得",
        form.look.label,
        skin::TEXT_SIZE,
    );
    let steps: Vec<String> = gain
        .decibels
        .iter()
        .map(|db| format!("{db:+.1}dB").replace(".0dB", "dB"))
        .collect();
    let mut choices: Vec<(&str, &str)> = vec![
        ("", "既定"),
        ("auto", "自動"),
        ("min", "最小"),
        ("max", "最大"),
    ];
    choices.extend(steps.iter().map(|step| (step.as_str(), step.as_str())));
    let select = Rect::from_min_size(pos2(left + 84.0, row.top()), vec2(180.0, row.height()));
    form.select_at(select, ("tuner.gain", id), value, &choices);
    let mut notes: Vec<String> = Vec::new();
    if gain.decibels.is_empty() {
        notes.push(String::from("入／切のみ"));
    }
    if !gain.deliveries.is_empty() {
        let only: Vec<String> = gain.deliveries.iter().map(|one| system_name(one)).collect();
        notes.push(format!("{} のときだけ", only.join("・")));
    }
    if !notes.is_empty() {
        say(
            form,
            pos2(select.right() + 10.0, row.center().y),
            &notes.join("・"),
            form.look.label,
            skin::TEXT_SIZE - 1.0,
        );
    }
}
