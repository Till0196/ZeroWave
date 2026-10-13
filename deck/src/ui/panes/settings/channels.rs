// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「チャンネル設定」（ネットワークのまとめ方）と「チャンネル一覧」（局ごとの好み）のページ。

use egui::{FontId, Rect, Sense, pos2, vec2};

use super::{File, Form, Pressed, skin};
use crate::ui::icons::Icon;

/// ネットワーク ID と表示名（`[[network]]`）と、その ID の局を受ける経路。
pub(super) fn channels(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    reach: &super::Reach,
) -> Option<Pressed> {
    use crate::config::file::Network;

    form.section("ネットワーク IDと表示名");
    form.note("IDと表示名を設定できます");
    let coaxd = edited
        .sources
        .iter()
        .find(|source| source.kind == "coaxd")
        .map(|source| source.kind.clone());
    const NAME: f32 = 100.0;
    const NIDS: f32 = 128.0;
    const ORDER: f32 = 58.0;
    const DELETE: f32 = 48.0;
    let top = *form.at;
    let head = form.line(26.0);
    let left = head.left() + 8.0;
    let route_left = left + NAME + 8.0 + NIDS + 8.0;
    let order_left = head.right() - 8.0 - DELETE - 8.0 - ORDER;
    // 受信経路の列が入らなければ列ごと出さない（順番のボタンと重なる）。
    let routes_fit = order_left - route_left >= 72.0;
    for (x, text) in [
        (left, "表示名"),
        (left + NAME + 8.0, "ネットワーク ID"),
        (route_left, if routes_fit { "受信経路" } else { "" }),
        (order_left, "順番"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    // 画面では「4, 0x7880-0x7fef」の一つの文字列で持ち、ファイルには配列で書く。
    let mut remove = None;
    let mut moved = None;
    for index in 0..edited.networks.len() {
        let row = form.line(32.0);
        form.ui.painter().hline(
            row.x_range(),
            row.top() - 3.0,
            egui::Stroke::new(1.0, form.look.edge),
        );
        let field = |x: f32, width: f32| {
            Rect::from_min_size(pos2(x, row.center().y - 13.0), vec2(width, 26.0))
        };
        let network = &mut edited.networks[index];
        form.field(
            field(left, NAME),
            ("network.name", index),
            &mut network.name,
            "表示名",
        );
        let mut text = nids_text(&network.nids);
        let before = text.clone();
        form.field(
            field(left + NAME + 8.0, NIDS),
            ("network.nids", index),
            &mut text,
            "例: 0x7880-0x7fef, 4",
        );
        if text != before {
            network.nids = nids_of(&text);
        }
        let ranges: Vec<(u16, u16)> = network
            .nids
            .iter()
            .filter_map(crate::config::key::pattern_of)
            .map(|pattern| match pattern {
                crate::config::key::Pattern::One(n) => (n, n),
                crate::config::key::Pattern::Range(from, to) => (from, to),
                crate::config::key::Pattern::Any => (0, u16::MAX),
            })
            .collect();
        let mut x = route_left;
        if let Some(kind) = coaxd.as_ref().filter(|_| routes_fit) {
            for plan in super::PLANS {
                let count = reach.get(*plan).map_or(0, |keys| {
                    keys.iter()
                        .filter(|key| {
                            ranges
                                .iter()
                                .any(|(from, to)| (*from..=*to).contains(&key.network))
                        })
                        .count()
                });
                if count == 0 || x > order_left - 60.0 {
                    continue;
                }
                x += form.kind_tag(pos2(x, row.center().y), kind) + 4.0;
                let (shown, _) = super::offered(plan);
                let galley = form.ui.painter().layout_no_wrap(
                    format!("{shown} {count} 局"),
                    FontId::proportional(skin::TEXT_SIZE - 1.0),
                    form.look.label,
                );
                let width = galley.size().x;
                form.ui.painter().galley(
                    pos2(x, row.center().y - galley.size().y / 2.0),
                    galley,
                    form.look.label,
                );
                x += width + 10.0;
            }
        }
        let up = Rect::from_min_size(pos2(order_left, row.center().y - 13.0), vec2(26.0, 26.0));
        if form.button(up, ("network.move", index, -1), Icon::ArrowUp) {
            moved = Some((index, -1i32));
        }
        let down = Rect::from_min_size(pos2(up.right() + 4.0, up.top()), up.size());
        if form.button(down, ("network.move", index, 1), Icon::ArrowDown) {
            moved = Some((index, 1));
        }
        if form.button(
            Rect::from_min_size(
                pos2(head.right() - 8.0 - DELETE, row.center().y - 13.0),
                vec2(DELETE, 26.0),
            ),
            ("network.delete", index),
            "削除",
        ) {
            remove = Some(index);
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
    if let Some((index, by)) = moved {
        let to = index as i32 + by;
        if to >= 0 && (to as usize) < edited.networks.len() {
            edited.networks.swap(index, to as usize);
        }
    }
    if let Some(index) = remove {
        edited.networks.remove(index);
    }
    let row = form.line(skin::SELECT_HEIGHT);
    if form.button(
        Rect::from_min_size(row.min, vec2(80.0, row.height())),
        ("network.add", 0),
        "＋ 追加",
    ) {
        edited.networks.push(Network::default());
    }
    if form.button(
        Rect::from_min_size(
            pos2(row.left() + 88.0, row.top()),
            vec2(100.0, row.height()),
        ),
        ("network.default", 0),
        "既定に戻す",
    ) {
        edited.networks = default_networks();
    }
    None
}

pub(crate) fn default_networks() -> Vec<crate::config::file::Network> {
    zwcore::guide::Network::defaults()
        .into_iter()
        .map(|network| crate::config::file::Network {
            name: network.name,
            nids: network
                .ranges
                .into_iter()
                .map(|(from, to)| {
                    toml::Value::String(if from == to {
                        format!("{from:#x}")
                    } else {
                        format!("{from:#x}-{to:#x}")
                    })
                })
                .collect(),
            profile: String::new(),
        })
        .collect()
}

pub fn with_default_networks(mut file: File) -> File {
    if file.networks.is_empty() {
        file.networks = default_networks();
    }
    file
}

/// 使っていないプロファイルの局は一覧に無いことがあるので名前を持つ。
pub(super) fn offered(
    listed: &[zwcore::input::source::Channel],
    receivable: Option<&std::collections::BTreeSet<zwcore::guide::Key>>,
) -> Offered {
    let mut offered = Offered::new();
    for channel in listed {
        if let Some(key) = channel.key {
            offered
                .entry(key)
                .or_insert_with(|| (channel.name.clone(), channel.order));
        }
    }
    for key in receivable.into_iter().flatten() {
        offered.entry(*key).or_insert_with(|| (String::new(), None));
    }
    offered
}

pub(super) type Offered = std::collections::BTreeMap<zwcore::guide::Key, (String, Option<u32>)>;

fn services_of(
    guide: &zwcore::guide::Guide,
    offered: Option<&Offered>,
) -> Vec<zwcore::guide::Service> {
    let Some(offered) = offered else {
        return guide.channels().cloned().collect();
    };
    offered
        .iter()
        .filter_map(|(key, (name, order))| match guide.service(*key) {
            Some(service) => service.watchable().then(|| service.clone()),
            None if !name.is_empty() => Some(zwcore::guide::Service {
                network: key.network,
                service: key.service,
                name: name.clone(),
                kind: 0x01,
                remote_key: order.and_then(|order| u8::try_from(order).ok()),
                ..zwcore::guide::Service::default()
            }),
            None => None,
        })
        .collect()
}

/// 局の無い種別のタブは出さない。番号・表示名・非表示はプロファイルへ（all は共通）。
#[allow(clippy::too_many_arguments)]
pub(super) fn channel_list(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    guide: &zwcore::guide::Guide,
    tab: &mut zwcore::guide::Band,
    mark: &mut Option<zwcore::guide::Key>,
    channels: &crate::config::channels::Channels,
    offered: Option<&Offered>,
    asking: bool,
    profile: &str,
    reach: &super::Reach,
    boards: &[zwcore::input::source::coaxd::Board],
    line_filter: &mut String,
    filter_rect: Rect,
) -> Option<Pressed> {
    use crate::config::key::{self, KeyPattern};
    use zwcore::guide::Band;

    let shown = crate::config::layered(edited, profile).channel;
    let coaxd_kind = String::from("coaxd");
    let mirakurun: Vec<String> = {
        let chosen = edited.profile(profile);
        edited
            .sources
            .iter()
            .filter(|source| source.kind == "mirakurun")
            .filter(|source| chosen.sources.is_empty() || chosen.sources.contains(&source.label))
            .map(|source| source.label.clone())
            .collect()
    };

    let all = services_of(guide, offered);
    // 並びは選局が先に試す順と同じ（`stage::tuning::mapped_channels`）。
    let order: Vec<String> = super::PLANS
        .iter()
        .map(|plan| (*plan).to_owned())
        .chain(
            reach
                .keys()
                .filter(|label| !super::PLANS.contains(&label.as_str()))
                .cloned(),
        )
        .collect();
    let lines_of = |key: zwcore::guide::Key| -> Vec<String> {
        order
            .iter()
            .filter(|label| reach.get(*label).is_some_and(|keys| keys.contains(&key)))
            .cloned()
            .collect()
    };
    {
        let labels: Vec<(String, String)> = order
            .iter()
            .filter(|label| reach.get(*label).is_some_and(|keys| !keys.is_empty()))
            .map(|label| {
                let (shown, _) = super::offered(label);
                (label.clone(), format!("coaxd {shown}"))
            })
            .collect();
        let mut choices: Vec<(&str, &str)> = vec![("", "すべて")];
        choices.extend(
            labels
                .iter()
                .map(|(label, said)| (label.as_str(), said.as_str())),
        );
        if !labels.iter().any(|(label, _)| label == line_filter) {
            line_filter.clear();
        }
        form.select_at(filter_rect, ("channel.line", 0), line_filter, &choices);
    }
    let bands: Vec<Band> = Band::all()
        .into_iter()
        .filter(|band| all.iter().any(|service| service.band() == *band))
        .collect();
    if bands.is_empty() {
        if !asking {
            form.note("選べる局がありません。チャンネルスキャンをするか、ソースを確認してください");
        }
        return None;
    }
    if !bands.contains(tab) {
        *tab = bands[0];
    }
    {
        let row = form.line(30.0);
        form.ui.painter().hline(
            row.x_range(),
            row.bottom(),
            egui::Stroke::new(1.0, form.look.edge),
        );
        let mut x = row.left();
        for band in &bands {
            let label = band.name();
            let galley = form.ui.painter().layout_no_wrap(
                label.clone(),
                FontId::proportional(skin::TEXT_SIZE),
                form.look.text,
            );
            let width = galley.size().x + 28.0;
            let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
            let chosen = *band == *tab;
            let hit = form.ui.interact(
                rect,
                form.ui.id().with(("channel.tab", band.0)),
                Sense::click(),
            );
            if hit.clicked() {
                *tab = *band;
            }
            form.ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &label,
                FontId::proportional(skin::TEXT_SIZE),
                if chosen {
                    form.look.chosen_text
                } else {
                    form.look.item
                },
            );
            if chosen {
                form.ui.painter().rect_filled(
                    Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max),
                    0.0,
                    form.look.chosen_mark,
                );
            }
            x += width + 4.0;
        }
    }
    const NUMBER: f32 = 44.0;
    const SERVICE: f32 = 150.0;
    const NAME: f32 = 120.0;
    const HIDE: f32 = 44.0;
    let head = form.line(24.0);
    let x0 = head.left() + 8.0;
    let columns = [
        x0,
        x0 + NUMBER + 10.0,
        x0 + NUMBER + 10.0 + SERVICE + 10.0,
        x0 + NUMBER + 10.0 + SERVICE + 10.0 + NAME + 10.0,
        x0 + NUMBER + 10.0 + SERVICE + 10.0 + NAME + 10.0 + HIDE + 10.0,
    ];
    for (x, text) in columns
        .iter()
        .zip(["番号", "サービス名", "表示名", "非表示"])
    {
        form.ui.painter().text(
            pos2(*x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    let mut services: Vec<&zwcore::guide::Service> = all
        .iter()
        .filter(|service| service.band() == *tab)
        .filter(|service| line_filter.is_empty() || lines_of(service.key()).contains(line_filter))
        .collect();
    services.sort_by_key(|service| service.order());
    if services.is_empty() {
        form.note("このタブに局はありません（番組表が集まると並びます）");
    }
    for (index, service) in services.into_iter().enumerate() {
        let key = service.key();
        let spelled = key::written(key);
        let row = form.line(30.0);
        let hidden = shown
            .hide
            .iter()
            .filter_map(|text| KeyPattern::parse(text))
            .any(|pattern| pattern.matches(key));
        let chosen = *mark == Some(key);
        if chosen {
            form.ui
                .painter()
                .rect_filled(row, 4.0, form.look.chosen_fill);
        } else {
            form.ui.painter().hline(
                row.x_range(),
                row.top() - 3.0,
                egui::Stroke::new(1.0, form.look.edge),
            );
        }
        let hit = form.ui.interact(
            Rect::from_min_max(
                pos2(columns[1], row.top()),
                pos2(columns[2] - 4.0, row.bottom()),
            ),
            form.ui.id().with(("channel.mark", index)),
            Sense::click(),
        );
        if hit.clicked() {
            *mark = if chosen { None } else { Some(key) };
        }
        let cell = |x: f32| Rect::from_min_size(pos2(x, row.center().y - 12.0), vec2(0.0, 24.0));
        let mut number = shown
            .key
            .get(&spelled)
            .map_or(String::new(), |n| n.to_string());
        let before = number.clone();
        form.field(
            Rect::from_min_size(cell(columns[0]).min, vec2(NUMBER - 4.0, 24.0)),
            ("channel.key", index),
            &mut number,
            &service
                .remote_key
                .map_or(String::from("—"), |n| n.to_string()),
        );
        if number != before {
            let number = number
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=99).contains(n));
            set_key(edited, profile, &spelled, number);
        }
        let unconfirmed = service.name.ends_with("（未確認）");
        let name_shown = service.name.trim_end_matches("（未確認）");
        let colour = if hidden {
            form.look.label
        } else {
            form.look.text
        };
        let name_cell = Rect::from_min_max(
            pos2(columns[1], row.top()),
            pos2(columns[2] - 8.0, row.bottom()),
        );
        form.ui.painter().with_clip_rect(name_cell).text(
            pos2(name_cell.left(), name_cell.center().y),
            egui::Align2::LEFT_CENTER,
            name_shown,
            FontId::proportional(skin::TEXT_SIZE),
            colour,
        );
        let mut name = shown.name.get(&spelled).cloned().unwrap_or_default();
        let before = name.clone();
        form.field(
            Rect::from_min_size(cell(columns[2]).min, vec2(NAME - 4.0, 24.0)),
            ("channel.name", index),
            &mut name,
            "（そのまま）",
        );
        if name != before {
            let name = (!name.trim().is_empty()).then_some(name);
            set_name(edited, profile, &spelled, name);
        }
        if form.tick(
            Rect::from_min_size(pos2(columns[3], row.top()), vec2(20.0, row.height())),
            ("channel.hide", index),
            hidden,
        ) {
            let mut hide = shown.hide.clone();
            if hidden {
                hide.retain(|text| {
                    KeyPattern::parse(text).is_none_or(|pattern| !pattern.matches(key))
                });
            } else {
                hide.push(spelled.clone());
            }
            set_hide(edited, profile, hide);
        }
        if unconfirmed {
            form.ui.painter().text(
                pos2(columns[4], row.center().y),
                egui::Align2::LEFT_CENTER,
                "未確認（NITから検出）",
                FontId::proportional(skin::TEXT_SIZE - 0.5),
                form.look.typed,
            );
        }
    }

    let mut pressed = None;
    if let Some(key) = *mark
        && let Some(service) = all.iter().find(|service| service.key() == key)
    {
        *form.at += 8.0;
        let top = *form.at;
        let spelled = key::written(key);
        let left = form.label_left() - skin::DOT;
        let row = form.line(24.0);
        form.ui.painter().text(
            pos2(left + 6.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            service.name.trim_end_matches("（未確認）"),
            FontId::proportional(skin::TEXT_SIZE + 0.5),
            form.look.text,
        );
        let row = form.line(24.0);
        form.ui.painter().text(
            pos2(left + 6.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            "ID",
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
        let mut id = format!(
            "NID {}・SID {}・TSID {}",
            zwcore::si::hex_id(key.network, 4),
            zwcore::si::hex_id(key.service, 4),
            zwcore::si::hex_id(service.transport, 4)
        );
        if let Some(slot) = slot_of(channels, key.network, service.transport) {
            let split = split_of(channels, key.network, service.transport);
            id.push_str(&format!("（TSMF {slot}"));
            if split > 1 {
                id.push_str(&format!("・搬送波 {split} 本"));
            }
            id.push('）');
        }
        let galley = form.ui.painter().layout_no_wrap(
            id,
            FontId::proportional(skin::TEXT_SIZE),
            form.look.text,
        );
        let id_width = galley.size().x;
        let value_left = left + 6.0 + 100.0;
        form.ui.painter().galley(
            pos2(value_left, row.center().y - galley.size().y / 2.0),
            galley,
            form.look.text,
        );
        let key_rect = Rect::from_min_size(
            pos2(value_left + id_width + 8.0, row.top()),
            vec2(spelled.len() as f32 * 7.0 + 8.0, row.height()),
        );
        let copy = form.ui.interact(
            key_rect,
            form.ui.id().with(("channel.copy", 0)),
            Sense::click(),
        );
        form.ui.painter().text(
            pos2(key_rect.left(), key_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &spelled,
            FontId::monospace(skin::TEXT_SIZE - 1.0),
            if copy.hovered() {
                form.look.chosen_text
            } else {
                form.look.group
            },
        );
        if copy.clicked() {
            pressed = Some(Pressed::CopyKey(spelled.clone()));
        }
        let copy = copy.on_hover_text("クリックでコピー");
        let _ = copy;
        let lines = lines_of(key);
        let mut rows: Vec<(String, String, String)> = Vec::new();
        for line in &lines {
            let using: Vec<String> = boards
                .iter()
                .flat_map(|board| &board.tuners)
                .map(|held| &held.tuner)
                .filter(|tuner| {
                    let (_, system) = super::offered(line);
                    super::scan::systems_of(edited, tuner)
                        .iter()
                        .any(|one| zwcore::input::source::coaxd::same_system(one, system))
                })
                .map(|tuner| tuner.label())
                .collect();
            let carrier = channels
                .lines
                .get(line)
                .and_then(|table| {
                    table.streams.get(&crate::config::key::Stream {
                        network: key.network,
                        transport: service.transport,
                    })
                })
                .and_then(|carriers| carriers.first())
                .map(carrier_text)
                .unwrap_or_default();
            let tuners = if using.is_empty() {
                String::from("使えるチューナーがありません")
            } else {
                using.join("・")
            };
            let (shown, _) = super::offered(line);
            rows.push((
                coaxd_kind.clone(),
                shown.to_owned(),
                format!("{tuners}　{carrier}"),
            ));
        }
        for label in &mirakurun {
            rows.push((
                String::from("mirakurun"),
                label.clone(),
                format!(
                    "NID {} / SID {}",
                    zwcore::si::hex_id(key.network, 4),
                    zwcore::si::hex_id(key.service, 4)
                ),
            ));
        }
        // 見出しは一行目の高さに（無いときの「スキャン結果にありません」の行も含めて）。
        let first = *form.at;
        if rows.is_empty() {
            let row = form.line(24.0);
            form.ui.painter().text(
                pos2(value_left, row.center().y),
                egui::Align2::LEFT_CENTER,
                "スキャン結果にありません",
                FontId::proportional(skin::TEXT_SIZE),
                form.look.label,
            );
        }
        for (at, (kind, route, detail)) in rows.iter().enumerate() {
            let row = form.line(24.0);
            let circle = pos2(value_left + 9.0, row.center().y);
            form.ui.painter().circle_filled(
                circle,
                9.0,
                if at == 0 {
                    form.look.chosen_mark.gamma_multiply(0.35)
                } else {
                    form.look.select_edge
                },
            );
            form.ui.painter().text(
                circle,
                egui::Align2::CENTER_CENTER,
                (at + 1).to_string(),
                FontId::proportional(10.5),
                if at == 0 {
                    form.look.chosen_text
                } else {
                    form.look.label
                },
            );
            let x = value_left + 26.0;
            let width = form.kind_tag(pos2(x, row.center().y), kind);
            form.ui.painter().text(
                pos2(x + width + 6.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                route,
                FontId::proportional(skin::TEXT_SIZE),
                form.look.text,
            );
            let cell = Rect::from_min_max(pos2(x + 176.0, row.top()), row.max);
            form.ui.painter().with_clip_rect(cell).text(
                pos2(cell.left(), cell.center().y),
                egui::Align2::LEFT_CENTER,
                detail,
                FontId::proportional(skin::TEXT_SIZE),
                form.look.label,
            );
        }
        form.ui.painter().text(
            pos2(left + 6.0, first + 12.0),
            egui::Align2::LEFT_CENTER,
            "受信",
            FontId::proportional(skin::TEXT_SIZE),
            form.look.label,
        );
        let row = form.line(20.0);
        form.ui.painter().text(
            pos2(left + 6.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            "受信チューナーの優先順位は「ソース」のプロファイルから変更できます。",
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
        let frame = Rect::from_min_max(
            pos2(left - 6.0, top - 6.0),
            pos2(form.body.right() - skin::PAD, *form.at + 2.0),
        );
        form.ui.painter().rect_stroke(
            frame,
            8.0,
            egui::Stroke::new(1.0, form.look.button_edge),
            egui::StrokeKind::Inside,
        );
        *form.at += 10.0;
    }
    pressed
}

fn carrier_text(carrier: &crate::config::channels::Carrier) -> String {
    let parameters = carrier.parameters();
    let get = |name: &str| {
        parameters
            .iter()
            .find(|(had, _)| had == name)
            .map(|(_, value)| value.clone())
    };
    let mut parts = Vec::new();
    if let Some(ch) = get("ch") {
        parts.push(ch);
    } else if let Some(freq) = get("freq").and_then(|freq| freq.parse::<f64>().ok()) {
        parts.push(format!("{:.0} MHz", freq / 1_000_000.0));
    }
    if let Some(stream) = carrier.stream_id() {
        let stream = stream
            .parse::<u32>()
            .map_or(stream.clone(), |id| zwcore::si::hex_id(id, 4));
        parts.push(format!("StreamID {stream}"));
    }
    if let Some((_, number)) = carrier.slot_parts() {
        parts.push(format!("スロット {number}"));
    }
    parts.join(" / ")
}

fn channel_override<'a>(edited: &'a mut File, profile: &str) -> Option<&'a mut toml::Table> {
    if profile.is_empty() {
        return None;
    }
    let chosen = edited.profiles.iter_mut().find(|p| p.name == profile)?;
    let value = chosen
        .overrides
        .entry(String::from("channel"))
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::Table::new());
    }
    value.as_table_mut()
}

fn set_in(table: &mut toml::Table, section: &str, spelled: &str, value: Option<toml::Value>) {
    let inner = table
        .entry(section.to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !inner.is_table() {
        *inner = toml::Value::Table(toml::Table::new());
    }
    if let Some(inner) = inner.as_table_mut() {
        match value {
            Some(value) => {
                inner.insert(spelled.to_owned(), value);
            }
            None => {
                inner.remove(spelled);
            }
        }
        if inner.is_empty() {
            table.remove(section);
        }
    }
}

fn tidy(edited: &mut File, profile: &str) {
    if let Some(chosen) = edited.profiles.iter_mut().find(|p| p.name == profile)
        && chosen
            .overrides
            .get("channel")
            .and_then(toml::Value::as_table)
            .is_some_and(toml::Table::is_empty)
    {
        chosen.overrides.remove("channel");
    }
}

/// 上書きは足すだけなので、消すと共通の物に戻る。
fn set_name(edited: &mut File, profile: &str, spelled: &str, name: Option<String>) {
    match channel_override(edited, profile) {
        Some(table) => {
            set_in(table, "name", spelled, name.map(toml::Value::String));
            tidy(edited, profile);
        }
        None => match name {
            Some(name) => {
                edited.channel.name.insert(spelled.to_owned(), name);
            }
            None => {
                edited.channel.name.remove(spelled);
            }
        },
    }
}

fn set_key(edited: &mut File, profile: &str, spelled: &str, number: Option<u32>) {
    match channel_override(edited, profile) {
        Some(table) => {
            let value = number.map(|n| toml::Value::Integer(i64::from(n)));
            set_in(table, "key", spelled, value);
            tidy(edited, profile);
        }
        None => match number {
            Some(n) => {
                edited.channel.key.insert(spelled.to_owned(), n);
            }
            None => {
                edited.channel.key.remove(spelled);
            }
        },
    }
}

/// 上書きは並びごと置き換えるので、共通の並びもコピーする。
fn set_hide(edited: &mut File, profile: &str, hide: Vec<String>) {
    match channel_override(edited, profile) {
        Some(table) => {
            table.insert(
                String::from("hide"),
                toml::Value::Array(hide.into_iter().map(toml::Value::String).collect()),
            );
        }
        None => edited.channel.hide = hide,
    }
}

fn slot_of(
    channels: &crate::config::channels::Channels,
    network: u16,
    transport: u16,
) -> Option<&str> {
    channels
        .carriers_of(crate::config::key::Stream { network, transport })
        .find_map(|(_, carrier)| carrier.slot())
}

/// 分割された TSMF の搬送波の本数。選ぶときはこの本数だけチューナーを取る。
fn split_of(channels: &crate::config::channels::Channels, network: u16, transport: u16) -> usize {
    channels
        .carriers_of(crate::config::key::Stream { network, transport })
        .map(|(_, carrier)| carrier.carriers().len())
        .max()
        .unwrap_or(0)
        .max(1)
}

fn nids_text(nids: &[toml::Value]) -> String {
    nids.iter()
        .map(|value| match value {
            toml::Value::Integer(n) => n.to_string(),
            toml::Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn nids_of(text: &str) -> Vec<toml::Value> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part.parse::<i64>() {
            Ok(n) if !part.starts_with("0x") => toml::Value::Integer(n),
            _ => toml::Value::String(part.to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slot_of_a_stream_comes_from_the_map() {
        use crate::config::channels::Channels;
        let map = Channels::parse(
            "[line.all.cable]\n\"4:0x4011\" = { freq = 201142857, slot = \"ts:2\" }\n",
            "all",
        )
        .expect("読める");
        assert_eq!(slot_of(&map, 4, 0x4011), Some("ts:2"));
        assert_eq!(slot_of(&map, 4, 0x4012), None, "別のストリームには付かない");
        assert_eq!(
            slot_of(&Channels::default(), 4, 0x4011),
            None,
            "チャンネルマップが無い"
        );
    }

    #[test]
    fn a_station_the_roster_has_not_met_is_listed_by_its_source_name() {
        let channel =
            |key: Option<zwcore::guide::Key>, name: &str| zwcore::input::source::Channel {
                name: name.to_owned(),
                key,
                order: Some(1),
                transport: None,
                how: String::new(),
                container: zwcore::filters::Container::Ts,
            };
        let key = zwcore::guide::Key {
            network: 0x7fef,
            service: 1024,
        };
        let listed = [
            channel(Some(key), "ゼロウェーブ総合"),
            channel(None, "物理 27"),
        ];
        let offered = offered(&listed, None);
        let services = services_of(&zwcore::guide::Guide::new(), Some(&offered));
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].key(), key);
        assert_eq!(services[0].name, "ゼロウェーブ総合");
        assert_eq!(services[0].remote_key, Some(1));
    }

    #[test]
    fn a_split_frame_says_how_many_carriers_it_takes() {
        use crate::config::channels::Channels;
        let map = Channels::parse(
            "[line.all.cable]\n\"4:0x4011\" = { freq = 201142857, carriers = [201142857, 207142857], slot = \"tlv:1\" }\n\"4:0x4021\" = { freq = 213142857 }\n",
            "all",
        )
        .expect("読める");
        assert_eq!(split_of(&map, 4, 0x4011), 2, "二本で運ばれている");
        assert_eq!(split_of(&map, 4, 0x4021), 1, "分かれていない");
        assert_eq!(
            split_of(&Channels::default(), 4, 0x4011),
            1,
            "チャンネルマップが無い"
        );
    }
}
