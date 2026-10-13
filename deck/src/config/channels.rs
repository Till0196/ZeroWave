// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `channels.toml`：プロファイルと受信経路ごとのストリームと搬送波（`[line.<プロファイル>.<方式>]`）。
//! 搬送波の欄で読むのは `system`・`slot`・`cnr`・`keep`・`seen` だけ。残りは coaxd に渡す。

use std::collections::BTreeMap;

use super::key::Stream;

/// 一つのプロファイルのチャンネルマップ。ほかのプロファイルのセクションには触らない。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channels {
    pub profile: String,
    pub lines: BTreeMap<String, LineTable>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineTable {
    pub scanned: Option<toml::value::Datetime>,
    /// ストリーム → 搬送波。同じストリームが幾つもの搬送波に見えることがあるので常に配列。良い順。
    pub streams: BTreeMap<Stream, Vec<Carrier>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carrier {
    pub fields: toml::Table,
}

impl Carrier {
    fn of(value: &toml::Value) -> Option<Self> {
        Some(Self {
            fields: value.as_table()?.clone(),
        })
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.fields.get(name)?.as_str()
    }

    pub fn system(&self) -> Option<&str> {
        self.text("system")
    }

    pub fn slot(&self) -> Option<&str> {
        self.text("slot")
    }

    /// 分割された多重化フレームの搬送波の並び。
    pub fn carriers(&self) -> Vec<String> {
        let Some(toml::Value::Array(list)) = self.fields.get("carriers") else {
            return Vec::new();
        };
        list.iter()
            .map(|value| match value {
                toml::Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect()
    }

    pub fn cnr(&self) -> Option<f64> {
        self.fields.get("cnr")?.as_float()
    }

    /// 並べるときのキー。同じ搬送波かの判定にも使うので文字列で比べる。
    pub fn order(&self) -> String {
        for name in ["freq", "ch", "stream_id"] {
            if let Some(value) = self.fields.get(name) {
                return match value {
                    toml::Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
            }
        }
        String::new()
    }

    /// 同じ搬送波か。衛星は `stream_id` も比べる。
    pub fn same_carrier(&self, other: &Self) -> bool {
        self.order() == other.order()
            && self.slot() == other.slot()
            && self.stream_id() == other.stream_id()
    }

    pub fn stream_id(&self) -> Option<String> {
        self.fields.get("stream_id").map(|value| match value {
            toml::Value::String(text) => text.clone(),
            other => other.to_string(),
        })
    }

    #[must_use]
    pub fn with(&self, name: &str, value: toml::Value) -> Self {
        let mut fields = self.fields.clone();
        fields.insert(name.to_owned(), value);
        Self { fields }
    }

    pub fn seen(&self) -> Option<toml::value::Datetime> {
        self.fields.get("seen")?.as_datetime().copied()
    }

    pub fn keep(&self) -> bool {
        self.fields
            .get("keep")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
    }

    /// coaxd に要求する形：`system`・`slot`・`keep`・`cnr` を除いた欄（[`Self::tune_words`] で直す）。
    pub fn parameters(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "cnr" | "keep" | "system" | "slot" | "carriers" | "seen"
                )
            })
            .filter_map(|(name, value)| {
                let text = match value {
                    toml::Value::String(text) => text.clone(),
                    toml::Value::Integer(number) => number.to_string(),
                    toml::Value::Float(number) => number.to_string(),
                    toml::Value::Boolean(flag) => flag.to_string(),
                    _ => return None,
                };
                Some((name.clone(), text))
            })
            .collect()
    }

    pub fn slot_parts(&self) -> Option<(bool, u8)> {
        let slot = self.slot()?;
        let (kind, number) = slot.split_once(':')?;
        let number = number.trim().parse().ok()?;
        match kind.trim() {
            "ts" => Some((false, number)),
            "tlv" => Some((true, number)),
            _ => None,
        }
    }

    /// coaxd に渡す選局パラメーター。名前に `.` を含むものはプラグイン向けで、デーモンには渡らない。
    pub fn tune_words(&self, plan: &str) -> Vec<(String, String)> {
        let mut words = Vec::new();
        let system = self.system().unwrap_or(plan);
        if !system.is_empty() {
            words.push((String::from("DELIVERY_SYSTEM"), system.to_ascii_uppercase()));
        }
        for (name, value) in self.parameters() {
            let name = match name.as_str() {
                "freq" => String::from("FREQUENCY"),
                "ch" => String::from("CHANNEL"),
                other => other.to_ascii_uppercase(),
            };
            words.push((name, value));
        }
        if let Some((tlv, number)) = self.slot_parts() {
            let kind = if tlv { "tlv" } else { "ts" };
            words.push((
                zwsdk::abi::word::PART.to_owned(),
                format!("{kind}:{number}"),
            ));
        }
        let carriers = self.carriers();
        if carriers.len() > 1 {
            words.push((zwsdk::abi::word::CARRIERS.to_owned(), carriers.join(",")));
        }
        words
    }
}

impl LineTable {
    /// チャンネルスキャンの結果を重ねる。該当しない行と `keep()` が真の行は残し、ユーザーが書いた項目は持ち越す。
    pub fn merge(
        &mut self,
        found: BTreeMap<Stream, Vec<Carrier>>,
        swept: &Swept,
        at: Option<toml::value::Datetime>,
    ) {
        self.streams.retain(|stream, carriers| {
            if found.contains_key(stream) {
                return true;
            }
            if !swept.covers(carriers) {
                return true;
            }
            carriers.retain(Carrier::keep);
            !carriers.is_empty()
        });
        for (stream, carriers) in found {
            let had = self.streams.remove(&stream).unwrap_or_default();
            let mut next: Vec<Carrier> = carriers
                .into_iter()
                .map(|mut carrier| {
                    if let Some(old) = had.iter().find(|old| old.same_carrier(&carrier)) {
                        for (name, value) in &old.fields {
                            if matches!(name.as_str(), "freq" | "ch" | "cnr" | "slot" | "system") {
                                continue;
                            }
                            carrier.fields.entry(name.clone()).or_insert(value.clone());
                        }
                    }
                    carrier
                })
                .collect();
            let kept: Vec<Carrier> = had
                .into_iter()
                .filter(|old| old.keep() && !next.iter().any(|now| now.same_carrier(old)))
                .collect();
            next.sort_by(|a, b| {
                b.cnr()
                    .unwrap_or(f64::MIN)
                    .total_cmp(&a.cnr().unwrap_or(f64::MIN))
                    .then_with(|| a.order().cmp(&b.order()))
            });
            next.extend(kept);
            self.streams.insert(stream, next);
        }
        if at.is_some() {
            self.scanned = at;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Swept {
    #[default]
    Whole,
    Some(Vec<String>),
}

impl Swept {
    fn covers(&self, carriers: &[Carrier]) -> bool {
        match self {
            Self::Whole => true,
            Self::Some(touched) => carriers
                .iter()
                .all(|carrier| touched.iter().any(|at| *at == carrier.order())),
        }
    }
}

impl Channels {
    pub fn of(profile: &str) -> Self {
        Self {
            profile: profile.to_owned(),
            lines: BTreeMap::new(),
        }
    }

    pub fn parse(text: &str, profile: &str) -> Result<Self, String> {
        let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut lines = BTreeMap::new();
        let profile = profile.to_owned();
        let Some(named) = table.get("line") else {
            return Ok(Self { profile, lines });
        };
        let named = named
            .as_table()
            .ok_or_else(|| String::from("[line] はテーブルでないといけない"))?;
        let Some(named) = named.get(&profile) else {
            return Ok(Self { profile, lines });
        };
        let named = named
            .as_table()
            .ok_or_else(|| format!("[line.{profile}] はテーブルでないといけない"))?;
        for (plan, body) in named {
            let label = format!("{profile}.{plan}");
            let body = body
                .as_table()
                .ok_or_else(|| format!("[line.{label}] はテーブルでないといけない"))?;
            let mut line = LineTable::default();
            for (name, value) in body {
                if name == "scanned" {
                    line.scanned = value.as_datetime().copied();
                    continue;
                }
                let Some(stream) = Stream::parse(name) else {
                    return Err(format!("[line.{label}] のキー「{name}」が nid:tsid でない"));
                };
                let carriers: Vec<Carrier> = match value {
                    toml::Value::Array(items) => items.iter().filter_map(Carrier::of).collect(),
                    one => Carrier::of(one).into_iter().collect(),
                };
                if carriers.is_empty() {
                    return Err(format!(
                        "[line.{label}] の「{name}」は搬送波のテーブルか、その列でないといけない"
                    ));
                }
                line.streams.insert(stream, carriers);
            }
            lines.insert(plan.clone(), line);
        }
        Ok(Self { profile, lines })
    }

    pub fn write(&self) -> String {
        let mut root = toml::Table::new();
        let mut named = toml::Table::new();
        for (label, line) in &self.lines {
            let mut body = toml::Table::new();
            if let Some(scanned) = line.scanned {
                body.insert(String::from("scanned"), toml::Value::Datetime(scanned));
            }
            for (stream, carriers) in &line.streams {
                let value = if carriers.len() == 1 {
                    toml::Value::Table(carriers[0].fields.clone())
                } else {
                    toml::Value::Array(
                        carriers
                            .iter()
                            .map(|carrier| toml::Value::Table(carrier.fields.clone()))
                            .collect(),
                    )
                };
                body.insert(stream.to_string(), value);
            }
            named.insert(label.clone(), toml::Value::Table(body));
        }
        let mut profiles = toml::Table::new();
        profiles.insert(self.profile.clone(), toml::Value::Table(named));
        root.insert(String::from("line"), toml::Value::Table(profiles));
        toml::to_string(&root).unwrap_or_default()
    }

    /// 変更した方式のテーブルだけを差し替えた文字列（人も書くファイルなのでコメントと並びを残す）。
    pub fn written_over(&self, had: &str) -> String {
        use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};
        let Ok(mut document) = had.parse::<DocumentMut>() else {
            return self.write();
        };
        let into_inline = |carrier: &Carrier| {
            let mut table = InlineTable::new();
            for (name, value) in &carrier.fields {
                let value = match value {
                    toml::Value::String(text) => Value::from(text.as_str()),
                    toml::Value::Integer(number) => Value::from(*number),
                    toml::Value::Float(number) => Value::from(*number),
                    toml::Value::Boolean(flag) => Value::from(*flag),
                    _ => continue,
                };
                table.insert(name, value);
            }
            table
        };
        if !document.contains_key("line") {
            document["line"] = Item::Table(Table::new());
        }
        let Some(profiles) = document["line"].as_table_mut() else {
            return self.write();
        };
        profiles.set_implicit(true);
        let named = profiles
            .entry(self.profile.as_str())
            .or_insert(Item::Table(Table::new()));
        let Some(named) = named.as_table_mut() else {
            return self.write();
        };
        named.set_implicit(true);
        for (label, line) in &self.lines {
            // テーブルそのものは置き換えない（付いていたコメントが消える）。
            let body = named
                .entry(label.as_str())
                .or_insert(Item::Table(Table::new()));
            let Some(body) = body.as_table_mut() else {
                continue;
            };
            let stale: Vec<String> = body
                .iter()
                .map(|(name, _)| name.to_owned())
                .filter(|name| {
                    name != "scanned"
                        && Stream::parse(name)
                            .is_some_and(|stream| !line.streams.contains_key(&stream))
                })
                .collect();
            for name in stale {
                body.remove(&name);
            }
            if let Some(scanned) = line.scanned {
                body["scanned"] = toml_edit::value(
                    scanned
                        .to_string()
                        .parse::<toml_edit::Datetime>()
                        .map_or_else(|_| Value::from(scanned.to_string()), Value::from),
                );
            }
            for (stream, carriers) in &line.streams {
                body[&stream.to_string()] = if carriers.len() == 1 {
                    toml_edit::value(into_inline(&carriers[0]))
                } else {
                    let mut array = Array::new();
                    for carrier in carriers {
                        array.push(Value::InlineTable(into_inline(carrier)));
                    }
                    toml_edit::value(array)
                };
            }
        }
        document.to_string()
    }

    /// `[line.<from>]` のセクションを `[line.<to>]` へ移す。移す物が無いか `to` がもうあれば `None`。
    pub fn renamed(had: &str, from: &str, to: &str) -> Option<String> {
        use toml_edit::DocumentMut;
        let mut document = had.parse::<DocumentMut>().ok()?;
        let profiles = document.get_mut("line")?.as_table_mut()?;
        if from == to || profiles.contains_key(to) {
            return None;
        }
        let (_, moved) = profiles.remove_entry(from)?;
        profiles.insert(to, moved);
        Some(document.to_string())
    }

    pub fn removed(had: &str, profile: &str, plan: &str) -> Option<String> {
        use toml_edit::DocumentMut;
        let mut document = had.parse::<DocumentMut>().ok()?;
        let profiles = document.get_mut("line")?.as_table_mut()?;
        let lines = profiles.get_mut(profile)?.as_table_mut()?;
        lines.remove(plan)?;
        if lines.is_empty() {
            profiles.remove(profile);
        }
        Some(document.to_string())
    }

    pub fn carriers_of(&self, stream: Stream) -> impl Iterator<Item = (&str, &Carrier)> {
        self.lines.iter().flat_map(move |(label, line)| {
            line.streams
                .get(&stream)
                .into_iter()
                .flatten()
                .map(move |carrier| (label.as_str(), carrier))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
[line.all.roof]
scanned = 2026-09-20T15:02:11+09:00
"0x7f01:0x7f01" = { ch = 20, cnr = 26.5 }
"0x7f02:0x7f02" = [{ ch = 30, cnr = 31.5 }, { ch = 40, cnr = 18.0 }]

[line.all.cable]
"4:0x4011" = { freq = 201142857, slot = "ts:1", keep = true }
"#;

    #[test]
    fn the_example_reads() {
        let channels = Channels::parse(TEXT, "all").unwrap();
        let roof = &channels.lines["roof"];
        assert!(roof.scanned.is_some());
        let first = &roof.streams[&Stream {
            network: 0x7f01,
            transport: 0x7f01,
        }];
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].cnr(), Some(26.5));
        assert_eq!(
            first[0].parameters(),
            [(String::from("ch"), String::from("20"))]
        );
        let second = &roof.streams[&Stream {
            network: 0x7f02,
            transport: 0x7f02,
        }];
        assert_eq!(second.len(), 2, "幾つもの搬送波は列");
        let cable = &channels.lines["cable"].streams[&Stream {
            network: 4,
            transport: 0x4011,
        }][0];
        assert!(cable.keep());
        assert_eq!(cable.slot(), Some("ts:1"));
        assert_eq!(cable.system(), None, "無ければ行の plan");
    }

    #[test]
    fn what_is_written_reads_back() {
        let channels = Channels::parse(TEXT, "all").unwrap();
        let text = channels.write();
        assert!(text.contains("\"32513:32513\""), "{text}");
        assert!(
            text.contains("scanned = 2026-09-20T15:02:11+09:00"),
            "{text}"
        );
        assert_eq!(Channels::parse(&text, "all").unwrap(), channels);
    }

    fn carrier(pairs: &[(&str, toml::Value)]) -> Carrier {
        let mut fields = toml::Table::new();
        for (name, value) in pairs {
            fields.insert((*name).to_owned(), value.clone());
        }
        Carrier { fields }
    }

    fn stream(network: u16, transport: u16) -> Stream {
        Stream { network, transport }
    }

    #[test]
    fn a_sweep_only_replaces_what_it_touched() {
        let mut line = LineTable::default();
        line.streams.insert(
            stream(4, 0x4011),
            vec![carrier(&[("freq", toml::Value::Integer(201_142_857))])],
        );
        line.streams.insert(
            stream(4, 0x4012),
            vec![carrier(&[
                ("freq", toml::Value::Integer(207_142_857)),
                ("keep", toml::Value::Boolean(true)),
            ])],
        );
        line.streams.insert(
            stream(4, 0x4013),
            vec![carrier(&[("freq", toml::Value::Integer(999_999_999))])],
        );
        let swept = Swept::Some(vec![String::from("201142857"), String::from("207142857")]);
        line.merge(BTreeMap::new(), &swept, None);
        assert!(
            !line.streams.contains_key(&stream(4, 0x4011)),
            "見つからず、keep も無い"
        );
        assert!(line.streams.contains_key(&stream(4, 0x4012)), "keep は残る");
        assert!(
            line.streams.contains_key(&stream(4, 0x4013)),
            "スキャンしていない範囲は触らない"
        );
    }

    #[test]
    fn two_carriers_are_kept_in_the_better_order() {
        let mut line = LineTable::default();
        line.streams.insert(
            stream(0x7f02, 0x7f02),
            vec![carrier(&[
                ("ch", toml::Value::Integer(30)),
                ("name", toml::Value::String(String::from("二本目"))),
            ])],
        );
        let mut found = BTreeMap::new();
        found.insert(
            stream(0x7f02, 0x7f02),
            vec![
                carrier(&[
                    ("ch", toml::Value::Integer(30)),
                    ("cnr", toml::Value::Float(18.0)),
                ]),
                carrier(&[
                    ("ch", toml::Value::Integer(40)),
                    ("cnr", toml::Value::Float(31.5)),
                ]),
            ],
        );
        line.merge(found, &Swept::Whole, None);
        let carriers = &line.streams[&stream(0x7f02, 0x7f02)];
        assert_eq!(carriers.len(), 2);
        assert_eq!(carriers[0].cnr(), Some(31.5), "良いほうが先");
        assert_eq!(
            carriers[1].fields.get("name").and_then(toml::Value::as_str),
            Some("二本目"),
            "人が書いた欄は残る"
        );
    }

    #[test]
    fn the_tune_words_translate_the_slot() {
        let carrier = carrier(&[
            ("freq", toml::Value::Integer(201_142_857)),
            ("slot", toml::Value::String(String::from("ts:2"))),
        ]);
        assert_eq!(carrier.slot_parts(), Some((false, 2)));
        let words = carrier.tune_words("isdbc");
        assert!(words.contains(&(String::from("DELIVERY_SYSTEM"), String::from("ISDBC"))));
        assert!(words.contains(&(String::from("FREQUENCY"), String::from("201142857"))));
        assert!(words.contains(&(String::from("multiplex.part"), String::from("ts:2"))));
        assert!(
            !words
                .iter()
                .any(|(name, _)| name == "slot" || name == "SLOT"),
            "{words:?}"
        );
    }

    #[test]
    fn writing_over_keeps_what_a_person_wrote() {
        let had = r#"# 屋根のアンテナ
[line.all.roof]
"32513:32513" = { ch = 20 }

# ケーブル。触らないでほしい
[line.all.cable]
"4:16401" = { freq = 201142857, keep = true }
"#;
        let mut channels = Channels::of("all");
        let mut line = LineTable::default();
        line.streams.insert(
            stream(0x7f01, 0x7f01),
            vec![carrier(&[
                ("ch", toml::Value::Integer(20)),
                ("cnr", toml::Value::Float(26.5)),
            ])],
        );
        channels.lines.insert(String::from("roof"), line);
        let written = channels.written_over(had);
        assert!(written.contains("# 屋根のアンテナ"), "{written}");
        assert!(
            written.contains("# ケーブル。触らないでほしい"),
            "変更していない方式のコメントが消えた: {written}"
        );
        assert!(written.contains("cnr = 26.5"), "{written}");
        assert!(written.contains("keep = true"), "{written}");
        assert_eq!(
            Channels::parse(&written, "all").expect("読み戻せる").lines["cable"],
            Channels::parse(had, "all").expect("元も読める").lines["cable"],
        );
    }

    #[test]
    fn a_stream_on_two_lines_is_found_on_both() {
        let channels = Channels::parse(
            "[line.all.a]\n\"4:1\" = { freq = 1 }\n[line.all.b]\n\"4:1\" = { freq = 2 }\n",
            "all",
        )
        .unwrap();
        let found: Vec<&str> = channels
            .carriers_of(Stream {
                network: 4,
                transport: 1,
            })
            .map(|(label, _)| label)
            .collect();
        assert_eq!(found, ["a", "b"]);
    }

    /// TOML の裸のキーは英数字だけなので、日本語の名前は引用符で囲む。
    #[test]
    fn each_profile_reads_and_writes_its_own_lines() {
        let had = r#"[line.all.isdbt]
"0x7f01:0x7f01" = { ch = 23 }

[line."北アンテナ".isdbt]
"0x7f01:0x7f01" = { ch = 23 }

[line."南アンテナ".isdbt]
"0x7f11:0x7f11" = { ch = 23 }
"#;
        let south = Channels::parse(had, "南アンテナ").unwrap();
        assert_eq!(south.lines.len(), 1);
        assert!(
            south.lines["isdbt"]
                .streams
                .contains_key(&stream(0x7f11, 0x7f11))
        );
        assert!(Channels::parse(had, "東アンテナ").unwrap().lines.is_empty());

        let mut north = Channels::parse(had, "北アンテナ").unwrap();
        north.lines.get_mut("isdbt").unwrap().streams.insert(
            stream(0x7f02, 0x7f02),
            vec![carrier(&[("ch", toml::Value::Integer(27))])],
        );
        let written = north.written_over(had);
        assert_eq!(Channels::parse(&written, "北アンテナ").unwrap(), north);
        assert_eq!(
            Channels::parse(&written, "南アンテナ").unwrap(),
            south,
            "ほかのプロファイルには触らない"
        );
        assert_eq!(
            Channels::parse(&written, "all").unwrap(),
            Channels::parse(had, "all").unwrap()
        );
    }

    #[test]
    fn renaming_a_profile_moves_its_lines() {
        let had = r#"[line.all.isdbt]
"0x7f01:0x7f01" = { ch = 23 }

# 南のアンテナ
[line."南アンテナ".isdbt]
"0x7f11:0x7f11" = { ch = 23 }
"#;
        let moved = Channels::renamed(had, "南アンテナ", "南").expect("移せる");
        assert!(moved.contains("# 南のアンテナ"), "{moved}");
        assert!(
            Channels::parse(&moved, "南アンテナ")
                .unwrap()
                .lines
                .is_empty()
        );
        assert_eq!(
            Channels::parse(&moved, "南").unwrap().lines,
            Channels::parse(had, "南アンテナ").unwrap().lines
        );
        assert_eq!(
            Channels::parse(&moved, "all").unwrap(),
            Channels::parse(had, "all").unwrap()
        );
        assert_eq!(
            Channels::renamed(had, "南アンテナ", "all"),
            None,
            "行き先がもうある"
        );
        assert_eq!(
            Channels::renamed(had, "北アンテナ", "北"),
            None,
            "移す物が無い"
        );
    }

    #[test]
    fn one_system_of_one_profile_can_be_removed() {
        let had = r#"[line.all.isdbt]
"0x7f01:0x7f01" = { ch = 23 }

[line."南アンテナ".isdbt]
"0x7f11:0x7f11" = { ch = 23 }

[line."南アンテナ".isdbs]
"4:16400" = { freq = 1049480 }
"#;
        let removed = Channels::removed(had, "南アンテナ", "isdbt").expect("消せる");
        let south = Channels::parse(&removed, "南アンテナ").unwrap();
        assert_eq!(south.lines.keys().collect::<Vec<_>>(), ["isdbs"]);
        assert_eq!(
            Channels::parse(&removed, "all").unwrap(),
            Channels::parse(had, "all").unwrap()
        );
        let emptied = Channels::removed(&removed, "南アンテナ", "isdbs").expect("消せる");
        assert!(
            !emptied.contains("南アンテナ"),
            "空になったセクションも消す: {emptied}"
        );
        assert_eq!(Channels::removed(had, "北アンテナ", "isdbt"), None);
    }

    #[test]
    fn a_key_that_is_not_a_stream_is_refused() {
        assert!(Channels::parse("[line.all.a]\nname = { ch = 1 }\n", "all").is_err());
        assert!(Channels::parse("[line.all.a]\n\"4:1\" = 3\n", "all").is_err());
        assert!(Channels::parse("", "all").unwrap().lines.is_empty());
    }
}
