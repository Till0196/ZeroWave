// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! シェーダーの雛形の `{name}` に値を埋め込む。`format!` だと素の `{` `}` を二重に書く羽目になる。

/// 埋め込んだ値の中の `{name}` は埋めないので、入れ子は内側を先に埋める。
pub(crate) fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (name, value) in values {
        debug_assert!(!name.is_empty(), "名前の無い埋め込み");
        text = text.replace(&format!("{{{name}}}"), value);
    }
    debug_assert!(
        leftover(&text).is_none(),
        "埋め残しがある: {:?}",
        leftover(&text)
    );
    text
}

pub(crate) fn fill_planes(template: &str, plane: &str) -> String {
    let before = format!("{plane}_before");
    let cur = format!("{plane}_cur");
    let after = format!("{plane}_after");
    fill(
        template,
        &[
            ("plane", plane),
            ("before", &before),
            ("cur", &cur),
            ("after", &after),
        ],
    )
}

/// 埋め残した `{name}` を探す（見逃すと実行時にシェーダーの構文エラーになる）。
/// 本物の波括弧は必ず空白か改行を挟むので当たらない。
#[cfg(debug_assertions)]
fn leftover(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(open) = text[at..].find('{') {
        let open = at + open;
        let mut end = open + 1;
        while end < bytes.len() && (bytes[end].is_ascii_lowercase() || bytes[end] == b'_') {
            end += 1;
        }
        if end > open + 1 && end < bytes.len() && bytes[end] == b'}' {
            return Some(text[open..=end].to_string());
        }
        at = open + 1;
    }
    None
}

#[cfg(not(debug_assertions))]
fn leftover(_text: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_replaced_everywhere_it_appears() {
        assert_eq!(fill("{a} and {a}", &[("a", "x")]), "x and x");
    }

    #[test]
    fn the_braces_of_the_shader_itself_are_left_alone() {
        let text = fill("fn f() { return {v}; }", &[("v", "1.0")]);
        assert_eq!(text, "fn f() { return 1.0; }");
    }

    #[test]
    fn an_empty_value_removes_only_its_own_name() {
        assert_eq!(fill("[{a}][{b}]", &[("a", ""), ("b", "y")]), "[][y]");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn what_was_not_filled_is_found() {
        assert_eq!(leftover("fn f() { {plane} }"), Some("{plane}".to_string()));
        assert_eq!(leftover("fn f() { return 1.0; }"), None);
    }
}
