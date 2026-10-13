// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `state.toml`：いまどこか。アプリだけが書き、消せば次の起動で作り直す。
//! `[source]` は Mirakurun の局一覧のコピーで、起動時に一覧をすぐ出すための物。

use std::collections::BTreeMap;

use zwcore::guide::Key;

use super::key::{self, Stream};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub profile: String,
    pub last: Option<Key>,
    /// 最近見たチャンネル、プロファイルごと。新しい順で [`RECENT`] 件まで。
    pub recent: BTreeMap<String, Vec<Key>>,
    pub window: Option<Window>,
    /// 別ウィンドウの位置。キーは `Place::Kept` の名前。
    pub panes: BTreeMap<String, Window>,
    pub volume: Option<u32>,
    pub mute: bool,
    pub streams: BTreeMap<Stream, Known>,
    pub sources: BTreeMap<String, Offered>,
    /// 局ロゴの対応（SDT のロゴ伝送記述子、BS/CS の共通データ）。他の局の物も残す。
    pub logos: BTreeMap<Key, zwcore::guide::LogoOf>,
    pub services: BTreeMap<String, Lineup>,
    pub favourites: super::favourites::All,
    pub fonts: Option<toml::Table>,
    pub panel: Panel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Panel {
    pub rows: Option<u32>,
    pub folded: bool,
    pub detached: bool,
    pub tab: String,
    pub size: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lineup {
    pub url: String,
    pub by: String,
    pub fetched: Option<toml::value::Datetime>,
    pub channels: Vec<Listed>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listed {
    pub network: u16,
    pub service: u16,
    pub transport: Option<u16>,
    pub name: String,
    pub remote: Option<u32>,
    pub how: String,
    pub tlv: bool,
}

fn window_of(table: &toml::Table) -> Option<Window> {
    let number = |name: &str| table.get(name).and_then(toml::Value::as_integer);
    Some(Window {
        x: number("x")? as i32,
        y: number("y")? as i32,
        w: number("w")?.max(0) as u32,
        h: number("h")?.max(0) as u32,
        maximized: table
            .get("maximized")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
    })
}

fn window_table(window: Window) -> toml_edit::InlineTable {
    let mut table = toml_edit::InlineTable::new();
    table.insert("x", toml_edit::Value::from(i64::from(window.x)));
    table.insert("y", toml_edit::Value::from(i64::from(window.y)));
    table.insert("w", toml_edit::Value::from(i64::from(window.w)));
    table.insert("h", toml_edit::Value::from(i64::from(window.h)));
    if window.maximized {
        table.insert("maximized", toml_edit::Value::from(true));
    }
    table
}

/// メインウィンドウの位置（物理ピクセル）。最大化していても、戻したときの大きさを持つ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub maximized: bool,
}

pub const DEFAULT_VOLUME: u32 = 80;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Known {
    pub seen: Option<toml::value::Datetime>,
    pub remote: Option<u32>,
    pub services: BTreeMap<u16, String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offered {
    pub fetched: Option<toml::value::Datetime>,
    pub streams: Vec<Stream>,
}

pub const RECENT: usize = 20;

impl State {
    pub fn watched(&mut self, profile: &str, key: Key) {
        let list = self.recent.entry(profile.to_owned()).or_default();
        list.retain(|had| *had != key);
        list.insert(0, key);
        list.truncate(RECENT);
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut state = Self::default();
        if let Some(profile) = table.get("profile").and_then(toml::Value::as_str) {
            state.profile = profile.to_owned();
        }
        if let Some(last) = table.get("last").and_then(toml::Value::as_table) {
            let number = |name: &str| {
                last.get(name)
                    .and_then(toml::Value::as_integer)
                    .and_then(|value| u16::try_from(value).ok())
            };
            if let (Some(network), Some(service)) = (number("nid"), number("sid")) {
                state.last = Some(Key { network, service });
            }
        }
        if let Some(recent) = table.get("recent").and_then(toml::Value::as_table) {
            for (profile, keys) in recent {
                let keys: Vec<Key> = keys
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|key| {
                        let key = key.as_table()?;
                        let number = |name: &str| {
                            key.get(name)
                                .and_then(toml::Value::as_integer)
                                .and_then(|value| u16::try_from(value).ok())
                        };
                        Some(Key {
                            network: number("nid")?,
                            service: number("sid")?,
                        })
                    })
                    .take(RECENT)
                    .collect();
                state.recent.insert(profile.clone(), keys);
            }
        }
        if let Some(favourites) = table.get("favourites").and_then(toml::Value::as_table) {
            for (profile, list) in favourites {
                if let Ok(list) = list.clone().try_into() {
                    state.favourites.insert(profile.clone(), list);
                }
            }
        }
        state.fonts = table.get("fonts").and_then(toml::Value::as_table).cloned();
        state.window = table
            .get("window")
            .and_then(toml::Value::as_table)
            .and_then(window_of);
        if let Some(panes) = table.get("panes").and_then(toml::Value::as_table) {
            for (name, pane) in panes {
                if let Some(placed) = pane.as_table().and_then(window_of) {
                    state.panes.insert(name.clone(), placed);
                }
            }
        }
        if let Some(panel) = table.get("panel").and_then(toml::Value::as_table) {
            let number = |name: &str| {
                panel
                    .get(name)
                    .and_then(toml::Value::as_integer)
                    .and_then(|value| u32::try_from(value).ok())
            };
            let flag = |name: &str| panel.get(name).and_then(toml::Value::as_bool) == Some(true);
            state.panel = Panel {
                rows: number("rows").filter(|rows| *rows > 0),
                folded: flag("folded"),
                detached: flag("detached"),
                tab: panel
                    .get("tab")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                size: number("w").zip(number("h")),
            };
        }
        if let Some(volume) = table.get("volume").and_then(toml::Value::as_integer) {
            state.volume = Some(volume.clamp(0, 100) as u32);
        }
        state.mute = table
            .get("mute")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        if let Some(streams) = table.get("stream").and_then(toml::Value::as_table) {
            for (name, body) in streams {
                let (Some(stream), Some(body)) = (Stream::parse(name), body.as_table()) else {
                    continue;
                };
                let mut known = Known {
                    seen: body.get("seen").and_then(toml::Value::as_datetime).copied(),
                    remote: body
                        .get("remote")
                        .and_then(toml::Value::as_integer)
                        .and_then(|value| u32::try_from(value).ok()),
                    services: BTreeMap::new(),
                };
                if let Some(services) = body.get("services").and_then(toml::Value::as_table) {
                    for (sid, name) in services {
                        if let (Some(sid), Some(name)) = (key::number(sid), name.as_str()) {
                            known.services.insert(sid, name.to_owned());
                        }
                    }
                }
                state.streams.insert(stream, known);
            }
        }
        if let Some(logos) = table.get("logo").and_then(toml::Value::as_table) {
            for (name, body) in logos {
                let (Some(key), Some(body)) = (key::key(name), body.as_table()) else {
                    continue;
                };
                let logo = zwcore::guide::LogoOf {
                    id: body
                        .get("id")
                        .and_then(toml::Value::as_integer)
                        .and_then(|id| u16::try_from(id).ok()),
                    simple: body
                        .get("simple")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned),
                };
                state.logos.insert(key, logo);
            }
        }
        if let Some(sources) = table.get("source").and_then(toml::Value::as_table) {
            for (label, body) in sources {
                let Some(body) = body.as_table() else {
                    continue;
                };
                let offered = Offered {
                    fetched: body
                        .get("fetched")
                        .and_then(toml::Value::as_datetime)
                        .copied(),
                    streams: body
                        .get("streams")
                        .and_then(toml::Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(toml::Value::as_str)
                                .filter_map(Stream::parse)
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                state.sources.insert(label.clone(), offered);
            }
        }
        if let Some(lineups) = table.get("services").and_then(toml::Value::as_table) {
            for (label, body) in lineups {
                let Some(body) = body.as_table() else {
                    continue;
                };
                let text = |name: &str| {
                    body.get(name)
                        .and_then(toml::Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                let channels = body
                    .get("channels")
                    .and_then(toml::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(toml::Value::as_table)
                            .filter_map(|one| {
                                let number =
                                    |name: &str| one.get(name).and_then(toml::Value::as_integer);
                                Some(Listed {
                                    network: u16::try_from(number("nid")?).ok()?,
                                    service: u16::try_from(number("sid")?).ok()?,
                                    transport: number("tsid").and_then(|n| u16::try_from(n).ok()),
                                    name: one
                                        .get("name")
                                        .and_then(toml::Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                    remote: number("remote").and_then(|n| u32::try_from(n).ok()),
                                    how: one.get("how").and_then(toml::Value::as_str)?.to_owned(),
                                    tlv: one
                                        .get("tlv")
                                        .and_then(toml::Value::as_bool)
                                        .unwrap_or(false),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                state.services.insert(
                    label.clone(),
                    Lineup {
                        url: text("url"),
                        by: text("by"),
                        fetched: body
                            .get("fetched")
                            .and_then(toml::Value::as_datetime)
                            .copied(),
                        channels,
                    },
                );
            }
        }
        Ok(state)
    }

    pub fn write(&self) -> String {
        use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};
        let mut document = DocumentMut::new();
        document["profile"] = toml_edit::value(self.profile.as_str());
        if let Some(last) = self.last {
            let mut table = InlineTable::new();
            table.insert("nid", Value::from(i64::from(last.network)));
            table.insert("sid", Value::from(i64::from(last.service)));
            document["last"] = toml_edit::value(table);
        }
        if !self.recent.is_empty() {
            let mut recent = Table::new();
            for (profile, keys) in &self.recent {
                let mut list = Array::new();
                for key in keys {
                    let mut one = InlineTable::new();
                    one.insert("nid", Value::from(i64::from(key.network)));
                    one.insert("sid", Value::from(i64::from(key.service)));
                    list.push(one);
                }
                recent.insert(profile, toml_edit::value(list));
            }
            document["recent"] = Item::Table(recent);
        }
        if let Some(window) = self.window {
            document["window"] = toml_edit::value(window_table(window));
        }
        if !self.panes.is_empty() {
            let mut panes = Table::new();
            for (name, placed) in &self.panes {
                panes.insert(name, toml_edit::value(window_table(*placed)));
            }
            document["panes"] = Item::Table(panes);
        }
        if self.panel != Panel::default() {
            let mut table = InlineTable::new();
            if let Some(rows) = self.panel.rows {
                table.insert("rows", Value::from(i64::from(rows)));
            }
            if self.panel.folded {
                table.insert("folded", Value::from(true));
            }
            if self.panel.detached {
                table.insert("detached", Value::from(true));
            }
            if !self.panel.tab.is_empty() {
                table.insert("tab", Value::from(self.panel.tab.as_str()));
            }
            if let Some((w, h)) = self.panel.size {
                table.insert("w", Value::from(i64::from(w)));
                table.insert("h", Value::from(i64::from(h)));
            }
            document["panel"] = toml_edit::value(table);
        }
        if let Some(volume) = self.volume {
            document["volume"] = toml_edit::value(i64::from(volume));
        }
        if self.mute {
            document["mute"] = toml_edit::value(true);
        }
        let mut streams = Table::new();
        for (stream, known) in &self.streams {
            let mut table = InlineTable::new();
            if let Some(seen) = known.seen {
                table.insert("seen", Value::from(seen));
            }
            if let Some(remote) = known.remote {
                table.insert("remote", Value::from(i64::from(remote)));
            }
            let mut services = InlineTable::new();
            for (sid, name) in &known.services {
                services.insert(sid.to_string(), Value::from(name.as_str()));
            }
            table.insert("services", Value::from(services));
            streams.insert(&stream.to_string(), toml_edit::value(table));
        }
        document["stream"] = Item::Table(streams);
        if !self.logos.is_empty() {
            let mut logos = Table::new();
            for (key, logo) in &self.logos {
                let mut table = InlineTable::new();
                if let Some(id) = logo.id {
                    table.insert("id", Value::from(i64::from(id)));
                }
                if let Some(simple) = &logo.simple {
                    table.insert("simple", Value::from(simple.as_str()));
                }
                logos.insert(&key::written(*key), toml_edit::value(table));
            }
            document["logo"] = Item::Table(logos);
        }
        let mut sources = Table::new();
        for (label, offered) in &self.sources {
            let mut table = InlineTable::new();
            if let Some(fetched) = offered.fetched {
                table.insert("fetched", Value::from(fetched));
            }
            let mut list = Array::new();
            for stream in &offered.streams {
                list.push(stream.to_string());
            }
            table.insert("streams", Value::from(list));
            sources.insert(label, toml_edit::value(table));
        }
        document["source"] = Item::Table(sources);
        if !self.services.is_empty() {
            let mut lineups = Table::new();
            for (label, lineup) in &self.services {
                let mut table = Table::new();
                table.insert("url", toml_edit::value(lineup.url.as_str()));
                table.insert("by", toml_edit::value(lineup.by.as_str()));
                if let Some(fetched) = lineup.fetched {
                    table.insert("fetched", toml_edit::value(fetched));
                }
                let mut list = Array::new();
                for listed in &lineup.channels {
                    let mut one = InlineTable::new();
                    one.insert("nid", Value::from(i64::from(listed.network)));
                    one.insert("sid", Value::from(i64::from(listed.service)));
                    if let Some(transport) = listed.transport {
                        one.insert("tsid", Value::from(i64::from(transport)));
                    }
                    one.insert("name", Value::from(listed.name.as_str()));
                    if let Some(remote) = listed.remote {
                        one.insert("remote", Value::from(i64::from(remote)));
                    }
                    one.insert("how", Value::from(listed.how.as_str()));
                    if listed.tlv {
                        one.insert("tlv", Value::from(true));
                    }
                    let mut value = Value::from(one);
                    value.decor_mut().set_prefix("\n  ");
                    list.push_formatted(value);
                }
                list.set_trailing("\n");
                list.set_trailing_comma(true);
                table.insert("channels", toml_edit::value(list));
                lineups.insert(label, Item::Table(table));
            }
            let mut outer = Item::Table(lineups);
            if let Some(table) = outer.as_table_mut() {
                table.set_implicit(true);
            }
            document["services"] = outer;
        }
        #[derive(serde::Serialize)]
        struct Rest<'a> {
            #[serde(skip_serializing_if = "BTreeMap::is_empty")]
            favourites: &'a super::favourites::All,
            #[serde(skip_serializing_if = "Option::is_none")]
            fonts: &'a Option<toml::Table>,
        }
        let rest = Rest {
            favourites: &self.favourites,
            fonts: &self.fonts,
        };
        if let Ok(text) = toml::to_string(&rest)
            && let Ok(more) = text.parse::<DocumentMut>()
        {
            for (key, item) in more.iter() {
                document[key] = item.clone();
            }
        }
        document.to_string()
    }
}

/// いまの時刻を TOML の日時に。オフセットは JST で固定。
pub fn now() -> Option<toml::value::Datetime> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let local = seconds + 9 * 3600;
    let days = local.div_euclid(86_400);
    let of_day = local.rem_euclid(86_400);
    // Howard Hinnant の civil_from_days。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+09:00",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
    .parse()
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
profile = "自宅"
last = { nid = 4, sid = 101 }
window = { x = 100, y = 80, w = 1280, h = 720 }

[stream]
"4:0x4031" = { seen = 2026-09-20, remote = 1, services = { 101 = "ゼロウェーブBS", 102 = "ゼロウェーブBS" } }
"0x7fe6:0x7fe6" = { remote = 1, services = { 1024 = "ゼロウェーブテレビ１" } }

[source]
home = { fetched = 2026-09-20T18:00:00+09:00, streams = ["4:0x4031", "0x7fe6:0x7fe6"] }
"#;

    #[test]
    fn the_panel_round_trips() {
        let mut state = State::default();
        assert!(!state.write().contains("panel"));
        state.panel = Panel {
            rows: Some(4),
            folded: false,
            detached: true,
            tab: String::from("plugin:video_filter:filter"),
            size: Some((760, 420)),
        };
        let written = state.write();
        assert!(written.contains("panel = {"), "{written}");
        assert_eq!(State::parse(&written).unwrap().panel, state.panel);
        let read = State::parse("panel = { rows = 0, folded = true }").unwrap();
        assert_eq!(read.panel.rows, None, "零行は保存していないのと同じ");
        assert!(read.panel.folded);
    }

    #[test]
    fn the_window_place_survives_a_round_trip() {
        let state = State {
            window: Some(Window {
                x: -1900,
                y: 40,
                w: 1600,
                h: 900,
                maximized: true,
            }),
            ..State::default()
        };
        let read = State::parse(&state.write()).unwrap();
        assert_eq!(read.window, state.window);
    }

    #[test]
    fn the_pane_places_survive_a_round_trip() {
        let mut state = State::default();
        state.panes.insert(
            String::from("settings"),
            Window {
                x: 120,
                y: -30,
                w: 800,
                h: 600,
                maximized: false,
            },
        );
        state.panes.insert(
            String::from("guide"),
            Window {
                x: 0,
                y: 0,
                w: 1600,
                h: 900,
                maximized: true,
            },
        );
        let read = State::parse(&state.write()).unwrap();
        assert_eq!(read.panes, state.panes);
    }

    #[test]
    fn the_example_reads() {
        let state = State::parse(TEXT).unwrap();
        assert_eq!(state.profile, "自宅");
        assert_eq!(
            state.last,
            Some(Key {
                network: 4,
                service: 101
            })
        );
        assert_eq!(state.window.unwrap().w, 1280);
        let bs = Stream {
            network: 4,
            transport: 0x4031,
        };
        assert_eq!(state.streams[&bs].remote, Some(1));
        assert_eq!(state.streams[&bs].services[&102], "ゼロウェーブBS");
        let tokyo = Stream {
            network: 0x7fe6,
            transport: 0x7fe6,
        };
        assert_eq!(
            state.streams[&tokyo].services[&1024],
            "ゼロウェーブテレビ１"
        );
        assert_eq!(state.sources["home"].streams.len(), 2);
    }

    #[test]
    fn recent_channels_and_favourites_are_kept_per_profile() {
        let mut state = State::default();
        let key = |service: u16| Key {
            network: 4,
            service,
        };
        for service in 0..25 {
            state.watched("自宅", key(service));
        }
        state.watched("自宅", key(20));
        state.watched("", key(1));
        assert_eq!(state.recent["自宅"].len(), RECENT);
        assert_eq!(state.recent["自宅"][0], key(20));
        assert_eq!(state.recent["自宅"][1], key(24));
        assert_eq!(
            state.recent["自宅"]
                .iter()
                .filter(|k| **k == key(20))
                .count(),
            1
        );
        let mut folder = super::super::favourites::Node::folder("映画");
        folder
            .items
            .push(super::super::favourites::Node::station("局", key(3)));
        state.favourites.insert(
            String::from("自宅"),
            super::super::favourites::List {
                items: vec![
                    super::super::favourites::Node::station("ゼロウェーブBS", key(101)),
                    folder,
                ],
            },
        );
        let mut fonts = toml::Table::new();
        fonts.insert(String::from("ui"), toml::Value::from("Yu Gothic UI"));
        state.fonts = Some(fonts);
        let again = State::parse(&state.write()).unwrap();
        assert_eq!(again.recent, state.recent);
        assert_eq!(again.recent[""], [key(1)]);
        assert_eq!(again.favourites, state.favourites);
        assert_eq!(again.fonts, state.fonts);
    }

    #[test]
    fn a_mirakurun_lineup_is_kept_and_read_back() {
        let mut state = State::default();
        state.services.insert(
            String::from("home"),
            Lineup {
                url: String::from("http://tvrec:40772"),
                by: String::from("auto"),
                fetched: now(),
                channels: vec![
                    Listed {
                        network: 0x7fe6,
                        service: 1072,
                        transport: Some(0x7fe6),
                        name: String::from("ゼロウェーブ"),
                        remote: Some(7),
                        how: String::from("/api/channels/GR/23/stream"),
                        tlv: false,
                    },
                    Listed {
                        network: 11,
                        service: 101,
                        transport: None,
                        name: String::from("ゼロウェーブ４Ｋ"),
                        remote: None,
                        how: String::from("/api/services/1100101/stream"),
                        tlv: true,
                    },
                ],
            },
        );
        let text = state.write();
        assert_eq!(State::parse(&text).unwrap(), state, "{text}");
        assert!(text.contains("[services.home]"), "{text}");
        assert_eq!(
            text.lines().filter(|line| line.contains("nid =")).count(),
            2,
            "一局一行: {text}"
        );
    }

    #[test]
    fn what_is_written_reads_back_one_stream_per_line() {
        let state = State::parse(TEXT).unwrap();
        let text = state.write();
        assert_eq!(State::parse(&text).unwrap(), state, "{text}");
        let line = text
            .lines()
            .find(|line| line.starts_with("\"4:16433\""))
            .unwrap_or_else(|| panic!("ストリームの行が無い: {text}"));
        assert!(
            line.contains("services = { 101 = \"ゼロウェーブBS\""),
            "{line}"
        );
    }

    #[test]
    fn the_volume_and_the_mute_come_back() {
        let mut state = State::default();
        assert_eq!(state.volume, None);
        state.volume = Some(35);
        state.mute = true;
        let read = State::parse(&state.write()).unwrap();
        assert_eq!(read.volume, Some(35));
        assert!(read.mute);
        assert_eq!(
            State::parse(
                "volume = 500
"
            )
            .unwrap()
            .volume,
            Some(100)
        );
    }

    #[test]
    fn the_logo_map_is_kept() {
        let mut state = State::default();
        state.logos.insert(
            Key {
                network: 4,
                service: 101,
            },
            zwcore::guide::LogoOf {
                id: Some(291),
                simple: None,
            },
        );
        state.logos.insert(
            Key {
                network: 32751,
                service: 1024,
            },
            zwcore::guide::LogoOf {
                id: Some(5),
                simple: Some("ゼロウェーブ".to_owned()),
            },
        );
        let text = state.write();
        assert!(text.contains("\"4:101\" = { id = 291 }"), "{text}");
        let back = State::parse(&text).unwrap();
        assert_eq!(back.logos, state.logos);
    }

    #[test]
    fn an_empty_file_is_nothing_known() {
        let state = State::parse("").unwrap();
        assert_eq!(state, State::default());
        assert_eq!(State::parse(&state.write()).unwrap(), state);
    }

    #[test]
    fn now_is_a_datetime_in_jst() {
        let text = now().unwrap().to_string();
        assert!(text.ends_with("+09:00"), "{text}");
        assert!(text.starts_with("20"), "{text}");
    }
}
