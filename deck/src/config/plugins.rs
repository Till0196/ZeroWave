// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインごとの設定（`plugins/<名前>.toml`）。設定ウィンドウとプラグイン本体が一つのテーブルを共有する。
//! 一覧とテーブルは中では JSON の文字列で持ち、ファイルには TOML の配列・テーブルのまま書く。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

static LIVE: Mutex<BTreeMap<String, zwsdk::host::IoLive>> = Mutex::new(BTreeMap::new());

pub fn live(name: &str) -> zwsdk::host::IoLive {
    let mut tables = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        tables
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(RwLock::new(load(name)))),
    )
}

pub fn values(name: &str) -> BTreeMap<String, String> {
    live(name).read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 一つ書く。テーブルに入れてその場で効かせ、ファイルにも残す。空の値は消す。
pub fn put(name: &str, key: &str, value: Option<&str>) {
    let merged = change(name, |values| match value {
        Some(value) => {
            values.insert(key.to_owned(), value.to_owned());
        }
        None => {
            values.remove(key);
        }
    });
    replace(name, merged);
}

/// 共有のテーブルをファイルの中身に差し替える。プラグイン本体が書いた `state` のキーは残す。
fn replace(name: &str, merged: BTreeMap<String, String>) {
    let states: Vec<String> = crate::plugins::named(name)
        .map(|plugin| {
            plugin
                .manifest()
                .config
                .iter()
                .filter(|setting| setting.kind == "state")
                .map(|setting| setting.key.clone())
                .collect()
        })
        .unwrap_or_default();
    let live = live(name);
    let mut table = live.write().unwrap_or_else(|e| e.into_inner());
    let kept: Vec<(String, String)> = states
        .into_iter()
        .filter_map(|key| table.get(&key).cloned().map(|value| (key, value)))
        .collect();
    *table = merged;
    table.extend(kept);
}

/// いまファイルにある物に重ねて書く（もう一つの deck が書いたキーを消さない）。
fn change(
    name: &str,
    apply: impl FnOnce(&mut BTreeMap<String, String>),
) -> BTreeMap<String, String> {
    let _lock = path_of(name).and_then(|path| super::lock(&path, true).ok().flatten());
    let mut values = load(name);
    apply(&mut values);
    save(name, &values);
    values
}

pub fn forget() {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub fn path_of(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    Some(crate::plugins::folder()?.join(format!("{name}.toml")))
}

pub fn load(name: &str) -> BTreeMap<String, String> {
    let Some(path) = path_of(name) else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(error) => {
            tracing::warn!("{} が読めない: {error}", path.display());
            return BTreeMap::new();
        }
    };
    table
        .into_iter()
        .filter_map(|(key, value)| Some((key, written(&value)?)))
        .collect()
}

fn written(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(text) => Some(text.clone()),
        toml::Value::Integer(number) => Some(number.to_string()),
        toml::Value::Float(number) => Some(number.to_string()),
        toml::Value::Boolean(flag) => Some(flag.to_string()),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().filter_map(scalar).collect();
            Some(zwsdk::config::list_text(&items))
        }
        toml::Value::Table(table) => {
            let pairs: Vec<(&str, String)> = table
                .iter()
                .filter_map(|(name, value)| Some((name.as_str(), scalar(value)?)))
                .collect();
            Some(zwsdk::config::map_text(&pairs))
        }
        toml::Value::Datetime(_) => None,
    }
}

fn scalar(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::Array(_) | toml::Value::Table(_) => None,
        other => written(other),
    }
}

fn writing(text: &str) -> toml::Value {
    if zwsdk::config::is_list_text(text) {
        return toml::Value::Array(
            zwsdk::config::list_of(text)
                .into_iter()
                .map(toml::Value::String)
                .collect(),
        );
    }
    if zwsdk::config::is_map_text(text) {
        return toml::Value::Table(
            zwsdk::config::map_of(text)
                .into_iter()
                .map(|(name, value)| (name, toml::Value::String(value)))
                .collect(),
        );
    }
    match text {
        "true" => toml::Value::Boolean(true),
        "false" => toml::Value::Boolean(false),
        text => match text.parse::<i64>() {
            Ok(number) if !text.starts_with('0') || text == "0" => toml::Value::Integer(number),
            _ => toml::Value::String(text.to_owned()),
        },
    }
}

pub fn save(name: &str, cfg: &BTreeMap<String, String>) {
    let Some(path) = path_of(name) else {
        return;
    };
    let table: toml::Table = cfg
        .iter()
        .map(|(key, value)| (key.clone(), writing(value)))
        .collect();
    let text = toml::to_string(&table).unwrap_or_default();
    if let Err(error) = super::write_whole(&path, text) {
        tracing::warn!("{} に書けない: {error}", path.display());
    }
}

/// 受像機の設定（`receiver.zipcode` など）。一つのテーブルをどのプラグインにも渡す。
static RECEIVER: std::sync::LazyLock<Arc<RwLock<BTreeMap<String, String>>>> =
    std::sync::LazyLock::new(|| Arc::new(std::sync::RwLock::new(BTreeMap::new())));

pub fn set_receiver(name: &str, value: Option<String>) {
    let mut table = RECEIVER.write().unwrap_or_else(|e| e.into_inner());
    match value {
        Some(value) => {
            table.insert(name.to_owned(), value);
        }
        None => {
            table.remove(name);
        }
    }
}

pub fn io_of() -> zwcore::plugins::IoOf {
    give_control();
    Arc::new(|name: &str| {
        let owner = name.to_owned();
        zwsdk::host::Io {
            receiver: Some(Arc::clone(&RECEIVER)),
            live: Some(live(name)),
            store: Some(Arc::new(move |cfg: &BTreeMap<String, String>| {
                let merged = change(&owner, |values| {
                    values.extend(cfg.iter().map(|(k, v)| (k.clone(), v.clone())));
                });
                replace(&owner, merged);
            })),
            notify: Some(Arc::new(|notice: zwsdk::ui::Notice| {
                NOTICES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(notice);
            })),
            ..zwsdk::host::Io::default()
        }
    })
}

static NOTICES: std::sync::Mutex<Vec<zwsdk::ui::Notice>> = std::sync::Mutex::new(Vec::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Act(crate::keys::Action),
    Tune(zwcore::guide::Key),
}

static ASKED: Mutex<Vec<Asked>> = Mutex::new(Vec::new());

static CHANNELS: RwLock<Option<zwsdk::control::Channels>> = RwLock::new(None);

static NOW: RwLock<Option<zwsdk::control::Now>> = RwLock::new(None);

/// いま見ている局のロゴ（種類ごと）。ほかの局のロゴは渡さない（番組表はステージのスレッドにある）。
pub type Logos = ((u16, u16), Vec<Arc<zwcore::guide::Logo>>);

static LOGOS: RwLock<Option<Logos>> = RwLock::new(None);

#[derive(Debug)]
struct Remote;

impl zwsdk::host::Controls for Remote {
    fn act(&self, name: &str) -> bool {
        let Some(action) = crate::keys::Action::parse(name) else {
            return false;
        };
        ask(Asked::Act(action));
        true
    }

    fn tune(&self, network: u16, service: u16) -> bool {
        ask(Asked::Tune(zwcore::guide::Key { network, service }));
        true
    }

    fn channels(&self) -> zwsdk::control::Channels {
        let mut channels = CHANNELS
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        channels.actions = crate::keys::Action::all()
            .into_iter()
            .map(crate::keys::Action::name)
            .collect();
        channels
    }

    fn now(&self) -> zwsdk::control::Now {
        NOW.read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default()
    }

    fn logo(&self, network: u16, service: u16, logo_type: u8) -> Option<Vec<u8>> {
        let held = LOGOS.read().unwrap_or_else(|e| e.into_inner());
        let (station, logos) = held.as_ref()?;
        if *station != (network, service) {
            return None;
        }
        let logo = if logo_type == zwsdk::control::LOGO_ANY {
            zwcore::guide::logo::PREFERRED
                .iter()
                .find_map(|want| logos.iter().find(|logo| logo.logo_type == *want))
        } else {
            logos.iter().find(|logo| logo.logo_type == logo_type)
        }?;
        Some(logo.png_with_palette())
    }

    fn host(&self) -> zwsdk::control::Host {
        zwsdk::control::Host {
            name: String::from("ZeroWave"),
            version: String::from(env!("CARGO_PKG_VERSION")),
            build: String::from(env!("ZW_BUILD").trim()),
        }
    }
}

fn ask(asked: Asked) {
    ASKED.lock().unwrap_or_else(|e| e.into_inner()).push(asked);
}

pub fn give_control() {
    zwcore::plugins::give_control(Arc::new(Remote));
}

pub fn take_asked() -> Vec<Asked> {
    std::mem::take(&mut *ASKED.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn set_channels(channels: zwsdk::control::Channels) {
    *CHANNELS.write().unwrap_or_else(|e| e.into_inner()) = Some(channels);
}

pub fn set_now(now: zwsdk::control::Now, logos: Option<Logos>) {
    *NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(now);
    *LOGOS.write().unwrap_or_else(|e| e.into_inner()) = logos;
}

pub fn take_notices() -> Vec<zwsdk::ui::Notice> {
    std::mem::take(&mut *NOTICES.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_put_is_seen_by_everyone_holding_the_table() {
        let name = format!("zw-live-{}", std::process::id());
        let held = live(&name);
        put(&name, "trace", Some("true"));
        assert_eq!(
            held.read().unwrap().get("trace").map(String::as_str),
            Some("true")
        );
        put(&name, "trace", None);
        assert!(!held.read().unwrap().contains_key("trace"), "消せば既定へ");
        if let Some(path) = path_of(&name) {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn what_is_written_reads_back() {
        let mut cfg = BTreeMap::new();
        cfg.insert(String::from("net"), String::from("true"));
        cfg.insert(String::from("model_id"), String::from("ZW-1"));
        cfg.insert(String::from("slots"), String::from("52"));
        cfg.insert(String::from("zip"), String::from("0123"));
        cfg.insert(
            String::from("ts_args"),
            zwsdk::config::list_text(&["-o", r"C:\Program Files\out", ""]),
        );
        cfg.insert(
            String::from("ts_env"),
            zwsdk::config::map_text(&[("B", "${HOME}/x"), ("A", "1")]),
        );
        cfg.insert(String::from("none"), String::from("[]"));
        let table: toml::Table = cfg
            .iter()
            .map(|(key, value)| (key.clone(), writing(value)))
            .collect();
        let text = toml::to_string(&table).expect("書ける");
        assert!(text.contains("net = true"), "{text}");
        assert!(text.contains("slots = 52"), "{text}");
        assert!(text.contains(r#"zip = "0123""#), "{text}");
        assert!(text.contains("none = []"), "{text}");
        assert!(
            text.contains("[ts_env]"),
            "テーブルは TOML のテーブルで: {text}"
        );
        let read: toml::Table = toml::from_str(&text).expect("読める");
        assert!(read["ts_args"].is_array(), "{text}");
        let back: BTreeMap<String, String> = read
            .into_iter()
            .filter_map(|(key, value)| Some((key, written(&value)?)))
            .collect();
        assert_eq!(back, cfg);
    }

    #[test]
    fn a_hand_written_list_or_table_is_read() {
        let read: toml::Table = toml::from_str(
            "args = [\"-v\", 3, [\"x\"]]\n[env]\nB = \"2\"\nA = true\nC = { x = 1 }\n",
        )
        .expect("読める");
        assert_eq!(written(&read["args"]).as_deref(), Some(r#"["-v","3"]"#));
        assert_eq!(
            written(&read["env"]).as_deref(),
            Some(r#"{"A":"true","B":"2"}"#)
        );
        assert_eq!(
            written(&toml::Value::Boolean(true)).as_deref(),
            Some("true")
        );
    }

    #[test]
    fn text_that_only_looks_like_a_list_stays_text() {
        assert_eq!(writing("[a b]"), toml::Value::String(String::from("[a b]")));
        assert_eq!(
            writing(r#"{"B":"1","A":"2"}"#),
            toml::Value::String(String::from(r#"{"B":"1","A":"2"}"#))
        );
    }
}
