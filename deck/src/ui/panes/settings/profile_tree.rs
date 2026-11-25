// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プロファイルのソースの表。ソースの下に coaxd のチューナー、Mirakurun のタイプと受信経路を入れ子で並べる。
//! ファイルには `coaxd`・`coaxd/<チューナー>`・`home`・`home/GR`・`home/GR:TER` の並びで書き、同じソースの物は続ける。

use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};

use super::form::Form;
use super::skin;
use crate::config::file::{join_entry, split_entry};
use crate::ui::icons::{self, Icon};

pub(super) enum Kids {
    None,
    /// coaxd のチューナー（識別子と表示名）。
    Tuners(Vec<(String, String)>),
    /// Mirakurun のタイプと、タイプごとの受信経路（返さない Mirakurun では空）。
    /// 偽なら受信経路は選べない（サービスで要求すると経路を名指しできない）。
    Lanes(Vec<(String, Vec<String>)>, bool),
}

pub(super) struct Shown<'a> {
    pub label: &'a str,
    pub kind: &'a str,
    pub kids: Kids,
}

/// プロファイルの並びをソースごとにまとめた物。`all` ならソースだけを書いた（全部を使う）。
#[derive(Debug, Clone, PartialEq)]
struct Group {
    label: String,
    all: bool,
    picks: Vec<String>,
}

fn groups_of(list: &[String]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for name in list {
        let (label, pick) = split_entry(name);
        let at = match groups.iter().position(|group| group.label == label) {
            Some(at) => at,
            None => {
                groups.push(Group {
                    label: label.clone(),
                    all: false,
                    picks: Vec::new(),
                });
                groups.len() - 1
            }
        };
        match pick {
            Some(pick) => {
                if !groups[at].picks.iter().any(|one| one == pick) {
                    groups[at].picks.push(pick.to_owned());
                }
            }
            None => groups[at].all = true,
        }
    }
    groups
}

fn list_of(groups: &[Group]) -> Vec<String> {
    let mut list = Vec::new();
    for group in groups {
        if group.all {
            list.push(join_entry(&group.label, None));
        } else {
            list.extend(
                group
                    .picks
                    .iter()
                    .map(|pick| join_entry(&group.label, Some(pick))),
            );
        }
    }
    list
}

enum Act {
    Source(String),
    MoveSource(String, isize),
    /// ソース、選ぶ物、Mirakurun のタイプと経路か。
    Pick(String, String, bool),
    MovePick(String, String, isize, bool),
}

#[derive(Clone, Copy, PartialEq)]
enum Tick {
    Off,
    Part,
    On,
    /// 親で全部を選んでいて、ここでは選べない。
    Dim,
}

pub(super) fn table(
    form: &mut Form<'_, '_>,
    scope: usize,
    shown: &[Shown<'_>],
    list: &mut Vec<String>,
) {
    let mut groups = groups_of(list);
    let top = *form.at;
    let head = form.line(26.0);
    for (x, text) in [
        (head.left() + 12.0, "ソースとチューナー（上から優先）"),
        (head.right() - 70.0, "順番"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    let mut order: Vec<&Shown<'_>> = groups
        .iter()
        .filter_map(|group| shown.iter().find(|one| one.label == group.label))
        .collect();
    order.extend(
        shown
            .iter()
            .filter(|one| !groups.iter().any(|group| group.label == one.label)),
    );
    let mut acts = Vec::new();
    for source in order {
        let group = groups.iter().find(|group| group.label == source.label);
        draw_source(form, scope, source, group, &mut acts);
    }
    let frame = Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), *form.at - 2.0));
    form.ui.painter().rect_stroke(
        frame,
        6.0,
        Stroke::new(1.0, form.look.button_edge),
        egui::StrokeKind::Inside,
    );
    *form.at += 4.0;
    for act in acts {
        apply(&mut groups, act);
    }
    let next = list_of(&groups);
    if next != *list {
        *list = next;
    }
}

fn apply(groups: &mut Vec<Group>, act: Act) {
    let at =
        |groups: &Vec<Group>, label: &str| groups.iter().position(|group| group.label == label);
    match act {
        Act::Source(label) => match at(groups, &label) {
            Some(at) => {
                groups.remove(at);
            }
            None => groups.push(Group {
                label,
                all: true,
                picks: Vec::new(),
            }),
        },
        Act::MoveSource(label, by) => {
            if let Some(at) = at(groups, &label) {
                let to = at as isize + by;
                if to >= 0 && (to as usize) < groups.len() {
                    groups.swap(at, to as usize);
                }
            }
        }
        Act::Pick(label, pick, lanes) => {
            let at = match at(groups, &label) {
                Some(at) => at,
                None => {
                    groups.push(Group {
                        label,
                        all: false,
                        picks: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            let group = &mut groups[at];
            // タイプを選ぶとその経路の行は要らない。外すときは経路の行も外す。
            let under = format!("{pick}:");
            let routes_of = |one: &String| lanes && one.starts_with(&under);
            if group.picks.contains(&pick) {
                group.picks.retain(|one| *one != pick && !routes_of(one));
            } else if lanes && !pick.contains(':') && group.picks.iter().any(routes_of) {
                group.picks.retain(|one| !routes_of(one));
            } else {
                group.picks.push(pick);
            }
            if group.picks.is_empty() && !group.all {
                groups.remove(at);
            }
        }
        Act::MovePick(label, pick, by, lanes) => {
            let Some(at) = at(groups, &label) else {
                return;
            };
            let picks = &mut groups[at].picks;
            // 動かすのは同じ段（同じタイプの経路どうし、またはチューナーどうし）の中だけ。
            let kin = |one: &str| {
                lanes
                    .then(|| one.split_once(':').map(|(kind, _)| kind.to_owned()))
                    .flatten()
            };
            let Some(from) = picks.iter().position(|one| *one == pick) else {
                return;
            };
            let same: Vec<usize> = (0..picks.len())
                .filter(|&i| kin(&picks[i]) == kin(&pick))
                .collect();
            let Some(place) = same.iter().position(|&i| i == from) else {
                return;
            };
            let to = place as isize + by;
            if to >= 0 && (to as usize) < same.len() {
                picks.swap(from, same[to as usize]);
            }
        }
    }
}

fn draw_source(
    form: &mut Form<'_, '_>,
    scope: usize,
    source: &Shown<'_>,
    group: Option<&Group>,
    acts: &mut Vec<Act>,
) {
    let all = group.is_some_and(|group| group.all);
    let picks: &[String] = group.map_or(&[], |group| group.picks.as_slice());
    let tick = if all {
        Tick::On
    } else if picks.is_empty() {
        Tick::Off
    } else {
        Tick::Part
    };
    let label = source.label.to_owned();
    // 選んだ物は一覧に無くなっても出す。
    let (kids, nested): (Vec<(String, String, Vec<String>)>, bool) = match &source.kids {
        Kids::None => (Vec::new(), false),
        Kids::Tuners(tuners) => {
            let mut kids: Vec<(String, String, Vec<String>)> = tuners
                .iter()
                .map(|(id, said)| (id.clone(), said.clone(), Vec::new()))
                .collect();
            for pick in picks {
                if !kids.iter().any(|(id, _, _)| id == pick) {
                    kids.push((pick.clone(), pick.clone(), Vec::new()));
                }
            }
            (kids, false)
        }
        Kids::Lanes(lanes, with_routes) => {
            let mut kids: Vec<(String, String, Vec<String>)> = lanes
                .iter()
                .map(|(kind, routes)| {
                    let routes = if *with_routes {
                        routes.clone()
                    } else {
                        Vec::new()
                    };
                    (kind.clone(), kind.clone(), routes)
                })
                .collect();
            for pick in picks {
                let (kind, route) = match pick.split_once(':') {
                    Some((kind, route)) => (kind, Some(route)),
                    None => (pick.as_str(), None),
                };
                let at = match kids.iter().position(|(one, _, _)| one == kind) {
                    Some(at) => at,
                    None => {
                        kids.push((kind.to_owned(), kind.to_owned(), Vec::new()));
                        kids.len() - 1
                    }
                };
                if let Some(route) = route
                    && *with_routes
                    && !kids[at].2.iter().any(|one| one == route)
                {
                    kids[at].2.push(route.to_owned());
                }
            }
            let nested = kids.iter().any(|(_, _, routes)| !routes.is_empty());
            (kids, nested)
        }
    };
    let said = match &source.kids {
        Kids::Tuners(_) if !kids.is_empty() => {
            let count = if all { kids.len() } else { picks.len() };
            (tick != Tick::Off).then(|| format!("{count} チューナー選択"))
        }
        Kids::Lanes(..) if !kids.is_empty() => {
            let count = if all {
                kids.len()
            } else {
                kids.iter()
                    .filter(|(kind, _, _)| {
                        picks
                            .iter()
                            .any(|pick| pick == kind || pick.starts_with(&format!("{kind}:")))
                    })
                    .count()
            };
            (tick != Tick::Off).then(|| format!("{count} タイプ選択"))
        }
        _ => None,
    };
    let open_id = form.ui.scope_id().with(("profile.open", scope, &label));
    let open = !kids.is_empty() && is_open(form, open_id, tick == Tick::Part);
    let caret = (!kids.is_empty()).then_some((open_id, open));
    let ordered = tick != Tick::Off;
    let row = Line {
        level: 0,
        caret,
        tick,
        tag: Some(source.kind),
        number: None,
        text: source.label,
        said: said.as_deref(),
        order: ordered,
    };
    let id = ("profile.source", scope, label.as_str());
    match draw_line(form, id, &row) {
        Some(Pressed::Tick) => acts.push(Act::Source(label.clone())),
        Some(Pressed::Move(by)) => acts.push(Act::MoveSource(label.clone(), by)),
        None => {}
    }
    if !open {
        return;
    }
    let numbered = matches!(source.kids, Kids::Tuners(_));
    let lanes = matches!(source.kids, Kids::Lanes(..));
    for (key, text, routes) in &kids {
        let picked = picks.iter().position(|one| one == key);
        let routes_picked: Vec<&String> = picks
            .iter()
            .filter(|one| lanes && one.starts_with(&format!("{key}:")))
            .collect();
        let tick = if all {
            Tick::Dim
        } else if picked.is_some() {
            Tick::On
        } else if !routes_picked.is_empty() {
            Tick::Part
        } else {
            Tick::Off
        };
        let kid_open_id = form
            .ui
            .scope_id()
            .with(("profile.open", scope, &label, key));
        let kid_open =
            nested && !routes.is_empty() && is_open(form, kid_open_id, tick == Tick::Part);
        let said = (nested && tick != Tick::Off && !routes.is_empty()).then(|| {
            let count = if matches!(tick, Tick::On | Tick::Dim) {
                routes.len()
            } else {
                routes_picked.len()
            };
            format!("{count} ルート選択")
        });
        let number = numbered.then(|| picked.filter(|_| !all).map_or(0, |place| place + 1));
        let line = Line {
            level: 1,
            caret: (nested && !routes.is_empty()).then_some((kid_open_id, kid_open)),
            tick,
            tag: None,
            number,
            text,
            said: said.as_deref(),
            order: numbered && picked.is_some() && !all,
        };
        let id = ("profile.pick", scope, label.as_str(), key.as_str());
        match draw_line(form, id, &line) {
            Some(Pressed::Tick) if tick != Tick::Dim => {
                acts.push(Act::Pick(label.clone(), key.clone(), lanes));
            }
            Some(Pressed::Move(by)) => {
                acts.push(Act::MovePick(label.clone(), key.clone(), by, lanes));
            }
            _ => {}
        }
        if !kid_open {
            continue;
        }
        let whole = all || picked.is_some();
        for route in routes {
            let pick = format!("{key}:{route}");
            let place = routes_picked.iter().position(|one| **one == pick);
            let tick = if whole {
                Tick::Dim
            } else if place.is_some() {
                Tick::On
            } else {
                Tick::Off
            };
            let line = Line {
                level: 2,
                caret: None,
                tick,
                tag: None,
                number: Some(place.filter(|_| !whole).map_or(0, |place| place + 1)),
                text: route,
                said: None,
                order: place.is_some() && !whole,
            };
            let id = ("profile.route", scope, label.as_str(), pick.as_str());
            match draw_line(form, id, &line) {
                Some(Pressed::Tick) if tick != Tick::Dim => {
                    acts.push(Act::Pick(label.clone(), pick.clone(), true));
                }
                Some(Pressed::Move(by)) => {
                    acts.push(Act::MovePick(label.clone(), pick.clone(), by, true));
                }
                _ => {}
            }
        }
    }
}

/// 触っていなければ、一部だけを選んでいるときに開く。
fn is_open(form: &Form<'_, '_>, id: egui::Id, part: bool) -> bool {
    form.ui
        .data(|data| data.get_temp::<bool>(id))
        .unwrap_or(part)
}

struct Line<'a> {
    level: usize,
    /// 開閉できる行の識別子と、いま開いているか。
    caret: Option<(egui::Id, bool)>,
    tick: Tick,
    tag: Option<&'a str>,
    /// 試す順の番号。0 は選んでいない（場所だけ空ける）。
    number: Option<usize>,
    text: &'a str,
    said: Option<&'a str>,
    order: bool,
}

enum Pressed {
    Tick,
    Move(isize),
}

const STEP: f32 = 22.0;
const CARET: f32 = 12.0;
const GAP: f32 = 8.0;
const BADGE: f32 = 16.0;

fn draw_line(
    form: &mut Form<'_, '_>,
    id: impl std::hash::Hash + Copy + std::fmt::Debug,
    line: &Line<'_>,
) -> Option<Pressed> {
    let look = form.look;
    let height = if line.level == 0 { 34.0 } else { 30.0 };
    let row = form.line(height);
    let band = Rect::from_min_max(
        pos2(row.left() + 1.0, row.top() - 3.0),
        pos2(row.right() - 1.0, row.bottom() + 3.0),
    );
    if line.level == 0 {
        form.ui.painter().rect_filled(band, 0.0, look.dark);
    }
    form.ui.painter().hline(
        row.x_range(),
        row.top() - 3.0,
        Stroke::new(
            1.0,
            if line.level == 0 {
                look.button_edge
            } else {
                look.edge
            },
        ),
    );
    let cy = row.center().y;
    let mut x = row.left() + 12.0;
    for _ in 0..line.level {
        form.ui.painter().vline(
            x + STEP / 2.0 - 1.0,
            band.y_range(),
            Stroke::new(1.0, look.edge),
        );
        x += STEP;
    }
    if let Some((open_id, open)) = line.caret {
        let hit = Rect::from_center_size(pos2(x + CARET / 2.0, cy), vec2(CARET + 8.0, height));
        if form.ui.interact(hit, open_id, Sense::click()).clicked() {
            form.ui.data_mut(|data| data.insert_temp(open_id, !open));
        }
        icons::paint(
            form.ui.painter(),
            Rect::from_center_size(pos2(x + CARET / 2.0, cy), vec2(10.0, 10.0)),
            if open { Icon::TriDown } else { Icon::TriRight },
            look.label,
        );
    }
    x += CARET + GAP;
    let tick_rect = Rect::from_center_size(
        pos2(x + skin::CHECK / 2.0, cy),
        vec2(skin::CHECK, skin::CHECK),
    );
    let mut pressed = None;
    let hit = Rect::from_min_max(
        pos2(x - 4.0, row.top()),
        pos2(x + skin::CHECK + 4.0, row.bottom()),
    );
    if line.tick != Tick::Dim
        && form
            .ui
            .interact(hit, form.ui.scope_id().with((id, "tick")), Sense::click())
            .clicked()
    {
        pressed = Some(Pressed::Tick);
    }
    paint_tick(form, tick_rect, line.tick);
    x += skin::CHECK + GAP;
    if let Some(kind) = line.tag {
        x += form.kind_tag(pos2(x, cy), kind) + GAP;
    }
    if let Some(number) = line.number {
        if number > 0 {
            let centre = pos2(x + BADGE / 2.0, cy);
            form.ui
                .painter()
                .circle_filled(centre, BADGE / 2.0, look.primary.gamma_multiply(0.35));
            form.ui.painter().text(
                centre,
                egui::Align2::CENTER_CENTER,
                number.to_string(),
                FontId::proportional(10.0),
                look.chosen_text,
            );
        }
        x += BADGE + GAP;
    }
    let colour = if matches!(line.tick, Tick::On | Tick::Part) {
        look.text
    } else {
        look.label
    };
    let width = say(form, pos2(x, cy), line.text, colour, skin::TEXT_SIZE);
    x += width + GAP;
    if let Some(said) = line.said {
        say(form, pos2(x, cy), said, look.group, skin::TEXT_SIZE - 1.0);
    }
    if line.order {
        let up = Rect::from_min_size(pos2(row.right() - 64.0, cy - 11.0), vec2(26.0, 22.0));
        if form.button(up, (id, "up"), Icon::ArrowUp) {
            pressed = Some(Pressed::Move(-1));
        }
        let down = Rect::from_min_size(pos2(up.right() + 4.0, up.top()), up.size());
        if form.button(down, (id, "down"), Icon::ArrowDown) {
            pressed = Some(Pressed::Move(1));
        }
    }
    pressed
}

fn paint_tick(form: &Form<'_, '_>, rect: Rect, tick: Tick) {
    let look = form.look;
    let painter = form.ui.painter();
    let (fill, edge, mark): (Color32, Color32, Option<Color32>) = match tick {
        Tick::Off => (look.select_fill, look.select_edge, None),
        Tick::On | Tick::Part => (look.tick_fill, look.tick_edge, Some(look.tick_mark)),
        Tick::Dim => (
            look.tick_fill.gamma_multiply(0.4),
            look.tick_edge.gamma_multiply(0.4),
            Some(look.tick_mark.gamma_multiply(0.55)),
        ),
    };
    painter.rect_filled(rect, 2.0, fill);
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    let Some(mark) = mark else {
        return;
    };
    if tick == Tick::Part {
        painter.hline(
            (rect.left() + 3.0)..=(rect.right() - 3.0),
            rect.center().y,
            Stroke::new(1.6, mark),
        );
    } else {
        icons::paint(painter, rect.shrink(1.5), Icon::Check, mark);
    }
}

fn say(form: &Form<'_, '_>, at: egui::Pos2, text: &str, colour: Color32, size: f32) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_profile_reads_and_writes_back_by_source() {
        let read = list(&[
            "coaxd/tuner-a",
            "home/GR:TER",
            "coaxd/tuner-b",
            "home/GR:CATV",
            "other",
        ]);
        assert_eq!(
            list_of(&groups_of(&read)),
            list(&[
                "coaxd/tuner-a",
                "coaxd/tuner-b",
                "home/GR:TER",
                "home/GR:CATV",
                "other"
            ])
        );
    }

    #[test]
    fn picking_a_type_takes_the_place_of_its_routes() {
        let mut groups = groups_of(&list(&["home/GR:TER", "home/BS:CATV"]));
        apply(
            &mut groups,
            Act::Pick(String::from("home"), String::from("GR"), true),
        );
        // 経路を選んでいたタイプを押すと、まず経路を外す。
        assert_eq!(list_of(&groups), list(&["home/BS:CATV"]));
        apply(
            &mut groups,
            Act::Pick(String::from("home"), String::from("GR"), true),
        );
        assert_eq!(list_of(&groups), list(&["home/BS:CATV", "home/GR"]));
        apply(
            &mut groups,
            Act::Pick(String::from("home"), String::from("GR"), true),
        );
        apply(
            &mut groups,
            Act::Pick(String::from("home"), String::from("BS:CATV"), true),
        );
        assert!(groups.is_empty());
    }

    #[test]
    fn routes_move_only_among_the_routes_of_their_type() {
        let mut groups = groups_of(&list(&["home/GR:TER", "home/BS:CATV", "home/GR:CATV"]));
        apply(
            &mut groups,
            Act::MovePick(String::from("home"), String::from("GR:CATV"), -1, true),
        );
        assert_eq!(
            list_of(&groups),
            list(&["home/GR:CATV", "home/BS:CATV", "home/GR:TER"])
        );
        apply(&mut groups, Act::MoveSource(String::from("home"), 1));
        apply(&mut groups, Act::Source(String::from("other")));
        apply(&mut groups, Act::MoveSource(String::from("other"), -1));
        assert_eq!(list_of(&groups)[0], "other");
    }
}
