// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「プラグイン」のページと設定の別ウィンドウ。`plugins/<名前>.toml` に、適用を待たずその場で書いて効かせる。

use egui::{FontId, Rect, pos2, vec2};

use super::plugin_list::{self, Done};
use super::{Dropped, File, Form, Look, Pressed, State, Told, scroll_bar, scrolled, skin};

pub const WIDTH: f32 = 640.0;
pub const HEIGHT: f32 = 520.0;

const FOOT: f32 = 46.0;

pub fn show(root: &mut egui::Ui, name: &str, state: &mut State, look: &Look) -> Option<Pressed> {
    let full = root.available_rect_before_wrap();
    root.painter().rect_filled(full, 0.0, look.body);
    let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT), full.max);
    let body = Rect::from_min_max(full.min, pos2(full.right(), foot.top()));
    let (scroll, content) = state.plugin_scroll.get(name).copied().unwrap_or_default();
    let scroll = scrolled(root, body, scroll, content);
    let mut pressed;
    let mut at = body.top() + 14.0 - scroll;
    {
        let mut inner = root.new_child(egui::UiBuilder::new().max_rect(body));
        inner.set_clip_rect(body);
        let mut form = Form::new(&mut inner, look, body, &mut at);
        pressed = plugin_page(&mut form, name, state);
    }
    let content = at + scroll - body.top() + 8.0;
    scroll_bar(root, look, body, scroll, content);
    state
        .plugin_scroll
        .insert(name.to_owned(), (scroll, content));
    let mut at = 0.0;
    let mut form = Form::foot(root, look, foot, &mut at);
    let row = form.line(skin::SELECT_HEIGHT);
    if form.button(
        Rect::from_min_size(
            pos2(row.right() - 96.0, row.top()),
            vec2(96.0, row.height()),
        ),
        ("plugin.close", 0),
        "閉じる",
    ) {
        pressed = Some(Pressed::Close);
    }
    match plugin_list::dialog(root, look, name, &mut state.plugin_lists) {
        Some(Done::Put(key, text)) => {
            crate::config::plugins::put(name, &key, Some(&text));
            let stream = crate::plugins::named(name).is_some_and(|plugin| plugin.has("stream"));
            pressed = Some(Pressed::Replugged(stream));
        }
        Some(Done::Browse(browse)) => pressed = Some(browse),
        None => {}
    }
    pressed
}

fn plugin_pages() -> Vec<(String, Vec<zwsdk::host::Setting>)> {
    crate::plugins::loaded()
        .all()
        .iter()
        .filter(|plugin| {
            !plugin.manifest().config.is_empty() || plugin.manifest().ui.settings.is_some()
        })
        .map(|plugin| {
            (
                crate::plugins::stem_of(plugin),
                plugin.manifest().config.clone(),
            )
        })
        .collect()
}

fn hook_name(hook: &str) -> &str {
    match hook {
        "layer" => "レイヤー",
        "stream" => "ストリーム",
        "tuners" => "チューナー",
        "video" => "映像",
        "ui" => "設定画面",
        "control" => "リモコン",
        other => other,
    }
}

pub(super) fn plugins(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    loaded: &File,
    told: &Told,
) -> Option<Pressed> {
    let mut pressed = None;
    form.section("プラグイン");
    let pages = plugin_pages();
    let available = crate::plugins::available();
    const ENABLED: f32 = 30.0;
    const NAME: f32 = 160.0;
    const SETTINGS: f32 = 76.0;
    let top = *form.at;
    let head = form.line(26.0);
    let left = head.left() + 10.0;
    for (x, text) in [
        (left, "有効"),
        (left + ENABLED + 8.0, "名前"),
        (left + ENABLED + 8.0 + NAME + 8.0, "バージョン・機能"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    if available.is_empty() {
        let row = form.line(30.0);
        form.ui.painter().text(
            pos2(left, row.center().y),
            egui::Align2::LEFT_CENTER,
            "プラグインはありません",
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
    }
    for (index, name) in available.iter().enumerate() {
        let row = form.line(32.0);
        form.ui.painter().hline(
            row.x_range(),
            row.top() - 3.0,
            egui::Stroke::new(1.0, form.look.edge),
        );
        let enabled = !edited.data.disabled.contains(name);
        let was = !loaded.data.disabled.contains(name);
        if enabled != was {
            form.ui.painter().circle_filled(
                pos2(row.left() + 4.0, row.center().y),
                3.0,
                form.look.changed,
            );
        }
        if form.tick(
            Rect::from_min_size(pos2(left, row.top()), vec2(20.0, row.height())),
            ("plugin.enabled", index),
            enabled,
        ) {
            if enabled {
                edited.data.disabled.push(name.clone());
            } else {
                edited.data.disabled.retain(|n| n != name);
            }
        }
        let plugin = crate::plugins::named(name);
        let shown = crate::plugins::shown_name(name);
        let said = match &plugin {
            Some(_) if !enabled && was => String::from("適用すると止めます"),
            Some(plugin) => {
                let hooks: Vec<&str> = plugin
                    .manifest()
                    .hooks
                    .iter()
                    .map(|hook| hook_name(hook))
                    .collect();
                format!("{}・{}", plugin.manifest().version, hooks.join("・"))
            }
            None if enabled && !was => String::from("適用すると読み込みます"),
            None if enabled && crate::plugins::reloading() => String::from("読み込んでいます…"),
            None if enabled => String::from("読み込めませんでした"),
            None => String::from("停止中"),
        };
        form.ui.painter().text(
            pos2(left + ENABLED + 8.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            shown,
            FontId::proportional(skin::TEXT_SIZE),
            if enabled {
                form.look.text
            } else {
                form.look.label
            },
        );
        form.ui.painter().text(
            pos2(left + ENABLED + 8.0 + NAME + 8.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            said,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
        if pages.iter().any(|(stem, _)| stem == name)
            && form.button(
                Rect::from_min_size(
                    pos2(row.right() - 10.0 - SETTINGS, row.center().y - 13.0),
                    vec2(SETTINGS, 26.0),
                ),
                ("plugin.settings", index),
                "設定…",
            )
        {
            pressed = Some(Pressed::OpenPlugin(name.clone()));
        }
    }
    let frame = Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), *form.at - 2.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        egui::Stroke::new(1.0, form.look.button_edge),
        egui::StrokeKind::Inside,
    );
    *form.at += 4.0;
    {
        let row = form.line(skin::SELECT_HEIGHT);
        let add = Rect::from_min_size(row.min, vec2(88.0, row.height()));
        if form.button(add, ("plugin.open", 0), "＋ 追加…") {
            pressed = Some(Pressed::AddPlugin);
        }
        let mut x = add.right() + 8.0;
        if let Some(folder) = crate::plugins::folder() {
            let open = Rect::from_min_size(pos2(x, row.top()), vec2(120.0, row.height()));
            if form.button(open, ("plugin.folder", 0), "フォルダを開く") {
                let _ = std::fs::create_dir_all(&folder);
                pressed = Some(Pressed::Reveal(folder));
            }
            x = open.right() + 12.0;
        }
        match told.lock().ok().and_then(|told| told.clone()) {
            Some(Ok(name)) => {
                let colour = form.look.ok;
                form.ui
                    .painter()
                    .circle_filled(pos2(x + 3.0, row.center().y), 3.5, colour);
                form.ui.painter().text(
                    pos2(x + 12.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{name} を追加しました"),
                    FontId::proportional(skin::TEXT_SIZE - 0.5),
                    colour,
                );
            }
            Some(Err(why)) => {
                form.ui.painter().text(
                    pos2(x, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    why,
                    FontId::proportional(skin::TEXT_SIZE - 0.5),
                    form.look.error,
                );
            }
            None => {}
        }
    }
    form.section("場所");
    form.key("data.plugins");
    form.text(
        "プラグインのフォルダ",
        &mut edited.data.plugins,
        &loaded.data.plugins,
        "（空欄ならプログラムと同じ場所の plugins）",
    );
    pressed
}

/// 動いているプラグインにもその場で見える（`config::plugins::live`）。
fn plugin_page(form: &mut Form<'_, '_>, name: &str, state: &mut State) -> Option<Pressed> {
    let pages = plugin_pages();
    let Some((name, settings)) = pages.iter().find(|(stem, _)| stem == name) else {
        form.told("", "このプラグインは読み込まれていません");
        return None;
    };
    let mut pressed = None;
    let plugin = crate::plugins::named(name);
    // stream の hook は選局のたびに設定を読むので、変えたら選局し直して効かせる。
    let stream = plugin.as_ref().is_some_and(|plugin| plugin.has("stream"));
    if lists_browsed(name, settings, state) {
        pressed = Some(Pressed::Replugged(stream));
    }
    let values = crate::config::plugins::values(name);
    let shown = crate::plugins::shown_name(name);
    form.section("プラグイン");
    let about = plugin.as_ref().map_or_else(String::new, |plugin| {
        let hooks: Vec<&str> = plugin
            .manifest()
            .hooks
            .iter()
            .map(|hook| hook_name(hook))
            .collect();
        format!("{}・{}", plugin.manifest().version, hooks.join("・"))
    });
    form.told("バージョン・機能", &about);
    let mut programs: Vec<(String, String, Rect)> = Vec::new();
    // `state` はプラグインが書いてページが読む物で、人が変える欄ではない。
    for setting in settings.iter().filter(|setting| setting.kind != "state") {
        if !setting.section.is_empty() {
            form.section(&setting.section);
        }
        let now = values
            .get(&setting.key)
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        let label = if setting.label.is_empty() {
            setting.key.clone()
        } else {
            setting.label.trim_end_matches([':', '：']).to_owned()
        };
        let changed = match setting.kind.as_str() {
            "bool" => {
                let was = zwsdk::host::truthy(&now);
                let mut on = was;
                form.check("", &label, &mut on, was);
                (on != was).then(|| on.to_string())
            }
            "choice" => {
                let choices: Vec<(&str, &str)> = setting
                    .choices
                    .iter()
                    .map(|(spelling, shown)| (spelling.as_str(), shown.as_str()))
                    .collect();
                let mut value = now.clone();
                form.select(&label, &mut value, &now, &choices);
                (value != now).then_some(value)
            }
            "list" | "map" => {
                let lists = &mut state.plugin_lists;
                let outcome = if setting.kind == "list" {
                    plugin_list::list(form, lists, name, setting, &label, &now)
                } else {
                    plugin_list::map(form, lists, name, setting, &label, &now)
                };
                if let Some(text) = outcome.changed {
                    crate::config::plugins::put(name, &setting.key, Some(&text));
                    pressed = Some(Pressed::Replugged(stream));
                }
                if outcome.pressed.is_some() {
                    pressed = outcome.pressed;
                }
                None
            }
            "font" => {
                let field = form.font_field(&label, &now, false, "自動", true, &setting.key);
                if field.open {
                    pressed = Some(Pressed::PickFont {
                        target: super::font_pick::Target::Plugin {
                            plugin: name.clone(),
                            key: setting.key.clone(),
                        },
                        place: format!("{shown} › {label}"),
                        current: now.clone(),
                    });
                }
                if field.file {
                    pressed = Some(Pressed::Browse {
                        plugin: name.clone(),
                        key: setting.key.clone(),
                        label: label.clone(),
                        folder: false,
                    });
                }
                if field.auto {
                    crate::config::plugins::put(name, &setting.key, None);
                    pressed = Some(Pressed::Replugged(stream));
                }
                programs.push((setting.key.clone(), label.clone(), field.row));
                None
            }
            "program" => {
                let mut value = now.clone();
                let (row, browse) =
                    form.program(&label, &mut value, &now, &setting.hint, &setting.key);
                if browse {
                    pressed = Some(Pressed::Browse {
                        plugin: name.clone(),
                        key: setting.key.clone(),
                        label: label.clone(),
                        folder: false,
                    });
                }
                programs.push((setting.key.clone(), label.clone(), row));
                (value != now).then_some(value)
            }
            // 不明な種類は書き込める欄として出す（マニフェストを信じすぎない）。
            _ => {
                let mut value = now.clone();
                form.text(&label, &mut value, &now, &setting.hint);
                (value != now).then_some(value)
            }
        };
        if let Some(value) = changed {
            crate::config::plugins::put(name, &setting.key, Some(&value));
            pressed = Some(Pressed::Replugged(stream));
        }
    }
    if let Some(chosen) = program_files(form, name, &programs, state) {
        crate::config::plugins::put(name, &chosen.0, Some(&chosen.1));
        pressed = Some(Pressed::Replugged(stream));
    }
    pressed
}

fn lists_browsed(name: &str, settings: &[zwsdk::host::Setting], state: &mut State) -> bool {
    let mine: Vec<(&zwsdk::host::Setting, String)> = {
        let Ok(mut browsed) = state.browsed.lock() else {
            return false;
        };
        let mut mine = Vec::new();
        browsed.retain(|(plugin, key, path)| {
            let setting = settings.iter().find(|setting| {
                &setting.key == key && (setting.kind == "list" || setting.kind == "map")
            });
            match setting {
                Some(setting) if plugin == name => {
                    mine.push((setting, path.to_string_lossy().into_owned()));
                    false
                }
                _ => true,
            }
        });
        mine
    };
    let mut written = false;
    for (setting, path) in mine {
        if setting.kind == "map" {
            state
                .plugin_lists
                .browsed_into_dialog(name, &setting.key, path);
            continue;
        }
        let now = crate::config::plugins::values(name)
            .get(&setting.key)
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        let mut items = zwsdk::config::list_of(&now);
        state
            .plugin_lists
            .browsed_into_list(name, &setting.key, &mut items, path);
        let text = zwsdk::config::list_text(&items);
        crate::config::plugins::put(name, &setting.key, Some(&text));
        written = true;
    }
    written
}

/// ドロップした位置が分からなければ（X11）、欄が二つ以上なら確認する。ファイルの名前では決めない。
fn program_files(
    form: &mut Form<'_, '_>,
    name: &str,
    programs: &[(String, String, Rect)],
    state: &mut State,
) -> Option<(String, String)> {
    let mut chosen = None;
    if let Ok(mut browsed) = state.browsed.lock()
        && let Some(at) = browsed.iter().position(|(plugin, _, _)| plugin == name)
    {
        let (_, key, path) = browsed.remove(at);
        chosen = Some((key, path.to_string_lossy().into_owned()));
    }
    let mine: Vec<Dropped> = state
        .drops
        .iter()
        .filter(|it| it.plugin == name)
        .cloned()
        .collect();
    state.drops.retain(|it| it.plugin != name);
    for dropped in mine {
        let written = dropped.path.to_string_lossy().into_owned();
        let under = dropped
            .at
            .and_then(|(x, y)| programs.iter().find(|(_, _, row)| row.contains(pos2(x, y))));
        match (under, programs) {
            (Some((key, _, _)), _) | (None, [(key, _, _)]) => {
                chosen = Some((key.clone(), written));
                state.choosing = None;
            }
            (None, []) => {}
            (None, _) => state.choosing = Some(dropped),
        }
    }
    let asking = state
        .choosing
        .as_ref()
        .filter(|it| it.plugin == name)
        .cloned();
    if let Some(dropped) = asking {
        form.section("どちらに入れますか");
        form.told("", &dropped.path.to_string_lossy());
        let row = form.line(skin::SELECT_HEIGHT);
        let mut x = row.left();
        for (key, label, _) in programs {
            let width = 12.0 * label.chars().count() as f32 + 32.0;
            let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
            if form.button(rect, ("program.drop", key), label) {
                chosen = Some((key.clone(), dropped.path.to_string_lossy().into_owned()));
                state.choosing = None;
            }
            x += width + 8.0;
        }
        let rect = Rect::from_min_size(pos2(x, row.top()), vec2(90.0, row.height()));
        if form.button(rect, "program.drop.cancel", "キャンセル") {
            state.choosing = None;
        }
    }
    chosen
}
