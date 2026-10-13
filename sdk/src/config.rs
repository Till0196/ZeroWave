// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一覧（`kind = "list"`）とマップ（`kind = "map"`）の設定。`zw_cfg_v1` は文字列しか渡せないので
//! JSON の文字列で持ち、`plugins/<名前>.toml` に書くときだけ TOML の配列・テーブルにする。

use serde_json::Value;

/// 無い・読めないなら空。native では `ZW_` ＋大文字の環境変数。
pub fn list(key: &str) -> Vec<String> {
    crate::plug::host::cfg(key)
        .map(|text| list_of(&text))
        .unwrap_or_default()
}

pub fn map(key: &str) -> Vec<(String, String)> {
    crate::plug::host::cfg(key)
        .map(|text| map_on(&text))
        .unwrap_or_default()
}

/// 名前の頭にこれが付いた項目は外してある（値は残すが効かない）。
pub const OFF: char = '#';

/// （名前、値、効くか）。外した物は [`OFF`] を除いた名前で。
pub fn entries_of(text: &str) -> Vec<(String, String, bool)> {
    let mut entries: Vec<(String, String, bool)> = map_of(text)
        .into_iter()
        .map(|(name, value)| match name.strip_prefix(OFF) {
            Some(bare) => (bare.to_owned(), value, false),
            None => (name, value, true),
        })
        .collect();
    entries.sort();
    entries
}

pub fn entries_text<S: AsRef<str>, T: AsRef<str>>(entries: &[(S, T, bool)]) -> String {
    let pairs: Vec<(String, &str)> = entries
        .iter()
        .map(|(name, value, on)| {
            let name = name.as_ref();
            (
                if *on {
                    name.to_owned()
                } else {
                    format!("{OFF}{name}")
                },
                value.as_ref(),
            )
        })
        .collect();
    map_text(&pairs)
}

pub fn map_on(text: &str) -> Vec<(String, String)> {
    map_of(text)
        .into_iter()
        .filter(|(name, _)| !name.starts_with(OFF))
        .collect()
}

/// 文字列でない項目は文字列にする。読めなければ空。
pub fn list_of(text: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => items.iter().filter_map(scalar).collect(),
        _ => Vec::new(),
    }
}

pub fn map_of(text: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(table)) => table
            .iter()
            .filter_map(|(name, value)| Some((name.clone(), scalar(value)?)))
            .collect(),
        _ => Vec::new(),
    };
    pairs.sort();
    pairs
}

pub fn list_text<S: AsRef<str>>(items: &[S]) -> String {
    let items: Vec<&str> = items.iter().map(AsRef::as_ref).collect();
    serde_json::to_string(&items).unwrap_or_else(|_| String::from("[]"))
}

pub fn map_text<S: AsRef<str>, T: AsRef<str>>(pairs: &[(S, T)]) -> String {
    let table: std::collections::BTreeMap<&str, &str> = pairs
        .iter()
        .map(|(name, value)| (name.as_ref(), value.as_ref()))
        .collect();
    serde_json::to_string(&table).unwrap_or_else(|_| String::from("{}"))
}

/// 読んで書き直すと同じ文字列になるか。ファイルへ配列で書くかを決める。
pub fn is_list_text(text: &str) -> bool {
    text.starts_with('[') && list_text(&list_of(text)) == text
}

pub fn is_map_text(text: &str) -> bool {
    text.starts_with('{') && map_text(&map_of(text)) == text
}

pub fn env_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg_attr(not(feature = "host"), allow(dead_code))]
pub(crate) fn default_text(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let items: Vec<String> = items.iter().filter_map(scalar).collect();
            list_text(&items)
        }
        Value::Object(table) => {
            let pairs: Vec<(String, String)> = table
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), scalar(value)?)))
                .collect();
            map_text(&pairs)
        }
        other => scalar(other).unwrap_or_default(),
    }
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null => Some(String::new()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_survives_the_round_trip() {
        let items = ["-o", r"C:\Program Files\out", "", "\"引用\""];
        let text = list_text(&items);
        assert_eq!(list_of(&text), items);
        assert!(is_list_text(&text));
        assert_eq!(list_text::<&str>(&[]), "[]");
        assert!(is_list_text("[]"));
        assert_eq!(list_of("-v -x"), Vec::<String>::new(), "読めなければ空");
        assert_eq!(list_of("[1, true, \"a\"]"), ["1", "true", "a"]);
    }

    #[test]
    fn a_map_is_kept_in_name_order() {
        let text = map_text(&[("B", "2"), ("A", "$HOME/x")]);
        assert_eq!(text, r#"{"A":"$HOME/x","B":"2"}"#);
        assert_eq!(
            map_of(&text),
            [
                (String::from("A"), String::from("$HOME/x")),
                (String::from("B"), String::from("2"))
            ]
        );
        assert!(is_map_text(&text));
        assert!(
            !is_map_text(r#"{"B":"2","A":"1"}"#),
            "順序が違えば元の文字列と同じではない"
        );
        assert!(!is_list_text("[ ]"));
        assert!(!is_list_text("[x"));
    }

    #[test]
    fn an_entry_can_be_switched_off() {
        let old = r#"{"A":"1","B":"2"}"#;
        assert_eq!(map_on(old).len(), 2);
        let entries = entries_of(old);
        assert!(entries.iter().all(|(_, _, on)| *on));
        let text = entries_text(&[("A", "1", true), ("B", "2", false)]);
        assert_eq!(text, r##"{"#B":"2","A":"1"}"##);
        assert_eq!(
            entries_of(&text),
            [
                (String::from("A"), String::from("1"), true),
                (String::from("B"), String::from("2"), false)
            ]
        );
        assert_eq!(map_on(&text), [(String::from("A"), String::from("1"))]);
        assert!(is_map_text(&text));
    }

    #[test]
    fn a_default_may_be_text_a_list_or_a_table() {
        let read = |json: &str| default_text(&serde_json::from_str(json).unwrap());
        assert_eq!(read(r#""abc""#), "abc");
        assert_eq!(read("3"), "3");
        assert_eq!(read(r#"["-v","x y"]"#), r#"["-v","x y"]"#);
        assert_eq!(read(r#"{"Z":"1","A":"2"}"#), r#"{"A":"2","Z":"1"}"#);
    }

    #[test]
    fn an_environment_name_is_letters_digits_and_underscores() {
        assert!(env_name("TOOL_LOG_DIR"));
        assert!(env_name("a1"));
        assert!(!env_name(""));
        assert!(!env_name("A-B"));
        assert!(!env_name("Ａ"));
        assert!(!env_name("A B"));
    }
}
