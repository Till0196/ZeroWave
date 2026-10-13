// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! お気に入りチャンネル。プロファイルごとのツリーで `state.toml` の `[favourites]` に置く。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use zwcore::guide::Key;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct List {
    #[serde(rename = "item", skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Node>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Node {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nid: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<u16>,
    #[serde(rename = "item", skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Node>,
}

impl Node {
    pub fn station(name: impl Into<String>, key: Key) -> Self {
        Self {
            name: name.into(),
            nid: Some(key.network),
            sid: Some(key.service),
            items: Vec::new(),
        }
    }

    pub fn folder(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn key(&self) -> Option<Key> {
        Some(Key {
            network: self.nid?,
            service: self.sid?,
        })
    }
}

pub type All = BTreeMap<String, List>;

pub fn of(profile: &str) -> List {
    super::load_state()
        .favourites
        .remove(profile)
        .unwrap_or_default()
}

pub fn save(profile: &str, list: List) -> Result<(), String> {
    super::update_state(|state| {
        if list.items.is_empty() {
            state.favourites.remove(profile);
        } else {
            state.favourites.insert(profile.to_owned(), list);
        }
    });
    Ok(())
}

pub fn add(profile: &str, node: Node) -> Result<(), String> {
    super::update_state(|state| {
        state
            .favourites
            .entry(profile.to_owned())
            .or_default()
            .items
            .push(node);
    });
    Ok(())
}

pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(super::path_of(super::STATE)?)
        .ok()?
        .modified()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
[["自宅".item]]
name = "ゼロウェーブBS"
nid = 4
sid = 101

[["自宅".item]]
name = "映画"

[["自宅".item.item]]
name = "ゼロウェーブシネマ"
nid = 4
sid = 200

[[""."item"]]
name = "ゼロウェーブ総合"
nid = 32751
sid = 1024
"#;

    #[test]
    fn the_example_reads_and_round_trips() {
        let all: All = toml::from_str(TEXT).unwrap();
        let home = &all["自宅"];
        assert_eq!(home.items.len(), 2);
        assert_eq!(
            home.items[0].key(),
            Some(Key {
                network: 4,
                service: 101
            })
        );
        assert_eq!(home.items[1].key(), None);
        assert_eq!(home.items[1].items[0].name, "ゼロウェーブシネマ");
        assert_eq!(all[""].items[0].name, "ゼロウェーブ総合");
        let again: All = toml::from_str(&toml::to_string(&all).unwrap()).unwrap();
        assert_eq!(again, all);
    }
}
