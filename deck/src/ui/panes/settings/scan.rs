// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「チャンネルスキャン」のページと受信方式の定義。受信経路に名前は無く、方式で決まる。

use egui::{FontId, Rect, Stroke, pos2, vec2};

use super::{Boards, File, Form, Pressed, State, skin};

/// この並びが経路の並びで、選局は同じ局を先の経路から試す。
pub(crate) const PLANS: &[&str] = &["isdbt", "isdbs", "isdbs3", "isdbc", "j83b_5_6", "dvbs2"];

/// BS と 110°CS は同じ衛星のテーブルなので一つ。
pub(crate) const OFFERED: &[(&str, &str, &str)] = &[
    ("isdbt", "ISDB-T", "ISDBT"),
    ("isdbs", "ISDB-S", "ISDBS"),
    ("isdbs3", "ISDB-S3", "ISDBS3"),
    ("isdbc", "ISDB-C", "ISDBC"),
    ("j83b_5_6", "J.83B (5.6 MBaud)", "J83B_5_6"),
    ("dvbs2", "DVB-S2", "DVBS2"),
];

/// 地上とケーブルは四つ。
pub(crate) const BANDS: &[(&str, &str)] = &[
    ("uhf", "UHF"),
    ("exuhf", "旧UHF"),
    ("vhf", "VHF"),
    ("catv", "CATV"),
];

const ISDB_S_BANDS: &[(&str, &str)] = &[("bs", "BS"), ("cs", "110°CS")];

/// ISDB-S3 の 110°CS は左旋だけ。今は放送が無いが、再開したときにスキャンできるように残す。
const ISDB_S3_BANDS: &[(&str, &str)] = &[
    ("bs", "BS右旋"),
    ("bs-left", "BS左旋"),
    ("cs-left", "110°CS左旋"),
];

pub(crate) fn bands_for(plan: &str) -> &'static [(&'static str, &'static str)] {
    match plan {
        "isdbt" | "isdbc" => BANDS,
        "isdbs" => ISDB_S_BANDS,
        "isdbs3" => ISDB_S3_BANDS,
        _ => &[],
    }
}

pub(crate) fn offered(plan: &str) -> (&'static str, &'static str) {
    OFFERED
        .iter()
        .find(|(spelling, _, _)| *spelling == plan)
        .map_or(("?", ""), |(_, shown, system)| (*shown, *system))
}

pub(crate) fn walks_bands(plan: &str) -> bool {
    !bands_for(plan).is_empty()
}

pub(crate) fn bands_walked(line: &crate::config::file::Line) -> Vec<String> {
    if !line.bands.is_empty() {
        return line
            .bands
            .iter()
            .map(|band| band.to_ascii_lowercase())
            .collect();
    }
    match line.key() {
        "isdbc" => vec!["uhf", "exuhf", "vhf", "catv"],
        "isdbs" => vec!["bs", "cs"],
        "isdbs3" => vec!["bs", "bs-left", "cs-left"],
        _ => vec!["uhf", "exuhf", "catv"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(crate) fn line_of(file: &File, plan: &str) -> crate::config::file::Line {
    file.lines
        .iter()
        .find(|line| line.key() == plan)
        .cloned()
        .unwrap_or_else(|| crate::config::file::Line {
            plan: plan.to_owned(),
            ..crate::config::file::Line::default()
        })
}

pub(crate) fn systems_of(
    file: &File,
    tuner: &zwcore::input::source::coaxd::DaemonTuner,
) -> Vec<String> {
    let setting = file
        .sources
        .iter()
        .filter(|source| source.kind == "coaxd")
        .find_map(|source| source.tuner_settings.get(&tuner.id))
        .cloned()
        .unwrap_or_default();
    zwcore::input::source::coaxd::systems_used(&tuner.deliveries, &setting.to_core())
}

pub(crate) fn system_name(system: &str) -> String {
    let upper = system.to_ascii_uppercase();
    let named = match upper.replace('-', "").as_str() {
        "ISDBT" => "ISDB-T",
        "ISDBS" => "ISDB-S",
        "ISDBS3" => "ISDB-S3",
        "ISDBC" | "DVBC/ANNEX_A" | "DVBC/ANNEXA" => "ISDB-C",
        "J83B_5_6" | "J83B" | "DVBC/ANNEX_B" | "DVBC/ANNEXB" => "J.83B (5.6 MBaud)",
        "DVBS2" => "DVB-S2",
        "DVBS" => "DVB-S",
        "DVBT" => "DVB-T",
        "DVBT2" => "DVB-T2",
        _ => return system.to_owned(),
    };
    named.to_owned()
}

pub(crate) fn serving<'a>(
    file: &File,
    boards: &'a [zwcore::input::source::coaxd::Board],
    plan: &str,
    patterns: &[String],
) -> Vec<&'a zwcore::input::source::coaxd::DaemonTuner> {
    let (_, system) = offered(plan);
    boards
        .iter()
        .flat_map(|board| &board.tuners)
        .map(|held| &held.tuner)
        .filter(|tuner| {
            systems_of(file, tuner)
                .iter()
                .any(|one| zwcore::input::source::coaxd::same_system(one, system))
                && (patterns.is_empty() || patterns.iter().any(|pattern| tuner.matches(pattern)))
        })
        .collect()
}

/// coaxd を起動して待つことがあるので別のスレッドで問い合わせる。
pub fn ask_boards(state: &mut State, file: &File) {
    state.boards_asked = true;
    state.reach = None;
    state.channels = None;
    state.results = None;
    let Some(source) = file
        .sources
        .iter()
        .find(|source| source.kind == "coaxd")
        .cloned()
    else {
        if let Ok(mut boards) = state.boards.lock() {
            *boards = Some(Err(String::from("coaxd のソースがありません")));
        }
        return;
    };
    if let Ok(mut boards) = state.boards.lock() {
        *boards = None;
    }
    let into = std::sync::Arc::clone(&state.boards);
    std::thread::Builder::new()
        .name(String::from("boards"))
        .spawn(move || {
            let answer =
                zwcore::input::source::coaxd::boards_as_told(&source.endpoint, source.priority);
            if let Ok(mut boards) = into.lock() {
                *boards = Some(answer);
            }
        })
        .ok();
}

pub(super) fn boards_answer(
    boards: &Boards,
) -> Option<Result<Vec<zwcore::input::source::coaxd::Board>, String>> {
    boards.lock().ok().and_then(|boards| boards.clone())
}

pub(super) fn boards_now(state: &State) -> Vec<zwcore::input::source::coaxd::Board> {
    boards_answer(&state.boards)
        .and_then(Result::ok)
        .unwrap_or_default()
}

pub(super) fn result_text(
    table: &crate::config::channels::LineTable,
    keys: Option<&Vec<zwcore::guide::Key>>,
) -> String {
    match keys {
        Some(keys) if !keys.is_empty() => match scanned_on(table) {
            Some(date) => format!("サービス数 {}・{date}", keys.len()),
            None => format!("サービス数 {}", keys.len()),
        },
        _ => format!("多重化ストリーム数 {}", table.streams.len()),
    }
}

fn scanned_on(table: &crate::config::channels::LineTable) -> Option<String> {
    let date = table.scanned.as_ref()?.date?;
    Some(format!("{}/{}実施", date.month, date.day))
}

pub(super) fn scan(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    state: &mut State,
) -> Option<Pressed> {
    let mut pressed = None;
    let coaxd = edited.sources.iter().any(|source| source.kind == "coaxd");
    let boards = boards_now(state);
    let asking = state.boards.lock().is_ok_and(|boards| boards.is_none());

    form.section("プロファイル");
    let (picked, _) = super::pick_profile(form, edited, &state.profile);
    if picked.is_some() {
        return picked;
    }
    let patterns = edited.coaxd_tuners(&edited.profile(&state.profile));
    let profile = state.profile.clone();
    let all = crate::config::channels_profile(&profile) == super::ALL;
    let (own, borrowed) = state
        .results
        .get_or_insert_with(|| {
            (
                super::scan_results::results_of(&profile),
                if all {
                    std::collections::BTreeMap::new()
                } else {
                    super::scan_results::results_of("")
                },
            )
        })
        .clone();

    form.section("受信方式");
    {
        let right = form.body.right() - skin::PAD;
        let button = Rect::from_min_size(pos2(right - 64.0, *form.at - 24.0), vec2(64.0, 24.0));
        if form.button_enabled(button, ("scan.refresh", 0), "更新", coaxd) {
            pressed = Some(Pressed::Boards);
        }
    }
    if !coaxd {
        form.note("スキャンには coaxd のソースが必要です（「ソース」のページで追加してください）");
    } else if asking && state.boards_asked {
        form.note("coaxd にチューナーを問い合わせています…");
    }

    // 対応するチューナーが無くても全部の方式を並べる（つないだ後にスキャンするため）。
    let rows: Vec<&str> = PLANS.to_vec();

    const CHECK: f32 = 24.0;
    const SYSTEM: f32 = 136.0;
    const RESULT: f32 = 188.0;
    const DELETE: f32 = 52.0;
    let top = *form.at;
    let head = form.line(26.0);
    let look = form.look;
    let tuner_left = head.left() + 10.0 + CHECK + SYSTEM;
    let delete_left = head.right() - 10.0 - DELETE;
    let result_left = delete_left - 8.0 - RESULT;
    for (x, text) in [
        (head.left() + 10.0 + CHECK, "方式"),
        (tuner_left, "チューナー"),
        (result_left, "スキャン結果"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            look.label,
        );
    }
    for plan in &rows {
        let row = form.line(28.0);
        form.ui
            .painter()
            .hline(row.x_range(), row.top() - 3.0, Stroke::new(1.0, look.edge));
        let tuners = serving(edited, &boards, plan, &patterns);
        let on_default = !tuners.is_empty();
        let on = *state.scan_on.get(*plan).unwrap_or(&on_default);
        if form.tick(
            Rect::from_min_size(
                pos2(row.left() + 10.0, row.top()),
                vec2(CHECK - 4.0, row.height()),
            ),
            ("scan.on", *plan),
            on,
        ) {
            state.scan_on.insert((*plan).to_owned(), !on);
        }
        let (shown, _) = offered(plan);
        let (tuner_text, tuner_colour) = if tuners.is_empty() {
            (
                String::from("対応チューナーが接続されていません"),
                look.typed,
            )
        } else {
            (
                tuners
                    .iter()
                    .map(|tuner| tuner.label())
                    .collect::<Vec<_>>()
                    .join("・"),
                look.text,
            )
        };
        let (result, result_colour) = match (own.get(*plan), borrowed.get(*plan)) {
            (Some(had), _) => (had.clone(), look.label),
            (None, Some(had)) => (format!("all の結果を使用（{had}）"), look.typed),
            (None, None) => (String::from("スキャン未実施"), look.label),
        };
        if form.button_enabled(
            Rect::from_min_size(
                pos2(delete_left, row.top() + 2.0),
                vec2(DELETE, row.height() - 4.0),
            ),
            ("scan.delete", *plan),
            "削除",
            own.contains_key(*plan),
        ) {
            pressed = Some(Pressed::AskDeleteScan((*plan).to_owned()));
        }
        for (left, right, text, colour) in [
            (
                row.left() + 10.0 + CHECK,
                tuner_left - 8.0,
                shown.to_owned(),
                look.text,
            ),
            (tuner_left, result_left - 8.0, tuner_text, tuner_colour),
            (result_left, delete_left - 8.0, result, result_colour),
        ] {
            form.fitted(
                pos2(left, row.center().y),
                &text,
                right - left,
                FontId::proportional(skin::TEXT_SIZE),
                colour,
            );
        }
        if walks_bands(plan) {
            let line = line_of(edited, plan);
            let walked = bands_walked(&line);
            let was = walked.clone();
            let bands = bands_for(plan);
            let items: Vec<(String, bool, bool)> = bands
                .iter()
                .map(|(spelling, name)| {
                    let on = walked.iter().any(|band| band == spelling);
                    ((*name).to_owned(), on, on)
                })
                .collect();
            let from = pos2(row.left() + 10.0 + CHECK, *form.at);
            form.ui.painter().text(
                pos2(from.x, from.y + 11.0),
                egui::Align2::LEFT_CENTER,
                "帯域",
                FontId::proportional(skin::TEXT_SIZE - 1.0),
                look.label,
            );
            let (hit, bottom) = form.chips_at(
                pos2(from.x + 36.0, from.y - 1.0),
                row.right(),
                ("scan.band", *plan),
                &items,
            );
            *form.at = bottom + 8.0;
            if let Some(hit) = hit {
                let band = bands[hit].0.to_owned();
                let mut now = was;
                if now.contains(&band) {
                    now.retain(|one| *one != band);
                } else {
                    now.push(band);
                }
                now.sort_by_key(|one| bands.iter().position(|(spelling, _)| spelling == one));
                if !now.is_empty() {
                    match edited.lines.iter_mut().find(|line| line.key() == *plan) {
                        Some(line) => line.bands = now,
                        None => edited.lines.push(crate::config::file::Line {
                            plan: (*plan).to_owned(),
                            bands: now,
                            ..crate::config::file::Line::default()
                        }),
                    }
                }
            }
        }
    }
    let frame = Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), *form.at - 2.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, look.button_edge),
        egui::StrokeKind::Inside,
    );
    *form.at += 4.0;
    form.note("受信方式とチューナーは「ソース」のチューナー設定で割り当てます。");

    let walking = state
        .scanning
        .lock()
        .is_ok_and(|scanning| scanning.running());
    let row = form.line(skin::SELECT_HEIGHT);
    let mut x = row.left();
    let wanted = rows.iter().any(|plan| {
        let on_default = !serving(edited, &boards, plan, &patterns).is_empty();
        *state.scan_on.get(*plan).unwrap_or(&on_default)
    });
    let go = Rect::from_min_size(pos2(x, row.top()), vec2(112.0, row.height()));
    if form.primary_button(
        go,
        ("scan.go", 0),
        "スキャン開始",
        coaxd && wanted && !walking,
    ) {
        // チェックの既定（受けるチューナーがあればスキャンする）を書き出してから始める。
        for plan in &rows {
            let on_default = !serving(edited, &boards, plan, &patterns).is_empty();
            state
                .scan_on
                .entry((*plan).to_owned())
                .or_insert(on_default);
        }
        pressed = Some(Pressed::Scan);
    }
    x = go.right() + 8.0;
    let tuners = Rect::from_min_size(pos2(x, row.top()), vec2(136.0, row.height()));
    if form.button_enabled(tuners, ("scan.tuners", 0), "チューナーの状態…", coaxd) {
        pressed = Some(Pressed::OpenTuners);
    }
    x = tuners.right() + 8.0;
    let mut top = row.top();
    let mut place = |form: &mut Form<'_, '_>, x: &mut f32, wide: f32| {
        if *x + wide > row.right() {
            let next = form.line(skin::SELECT_HEIGHT);
            *x = next.left();
            top = next.top();
        }
        let rect = Rect::from_min_size(pos2(*x, top), vec2(wide, row.height()));
        *x = rect.right() + 8.0;
        rect
    };
    let others = !all || !edited.profiles.is_empty();
    let copy = place(form, &mut x, 168.0);
    if form.button_enabled(copy, ("scan.copy", 0), "スキャン結果の複製…", others) {
        pressed = Some(Pressed::OpenScanCopy);
    }
    let file = place(form, &mut x, 196.0);
    if let Some(path) = crate::config::path_of("channels.toml")
        && form.button(file, ("scan.file", 0), "スキャン結果のファイルを開く")
    {
        pressed = Some(Pressed::Reveal(path));
    }
    if walking {
        form.note("スキャンしています（進行状況は別のウィンドウに表示されます）");
    }
    pressed
}
