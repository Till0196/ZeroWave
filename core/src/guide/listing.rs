// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表に出すもの。作るのは `guide::keeper`。

use std::sync::Arc;

use super::{Band, Key, Logo, Moment, Programme};

#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub channels: Vec<Channel>,
    pub from: Moment,
    pub hours: u32,
    /// 放送の現在時刻。無ければ線を引かない。
    pub now: Option<Moment>,
    pub band: Band,
    pub bands: Vec<Band>,
    pub watching: Option<Key>,
    /// 一度だけ。呼ぶ側が次の更新で消す。
    pub jump: Option<Moment>,
    pub search: Option<Arc<Search>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asked {
    pub query: String,
    pub band: Option<Band>,
    /// ARIB STD-B10 の大分類と中分類。
    pub genre: Option<(u8, Option<u8>)>,
    pub past: bool,
    pub titles_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub asked: Asked,
    pub hits: Vec<Hit>,
    pub more: bool,
    /// 種別だけ外した同じ条件で数える（一致の無い種別は載らない）。
    pub counts: Vec<(Band, usize)>,
}

impl Search {
    pub fn count_of(&self, band: Band) -> usize {
        self.counts
            .iter()
            .find(|(counted, _)| *counted == band)
            .map_or(0, |(_, count)| *count)
    }

    /// 先頭だけに切った数ではなく、一致した数。
    pub fn total(&self) -> usize {
        match self.asked.band {
            Some(band) => self.count_of(band),
            None => self.counts.iter().map(|(_, count)| count).sum(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub key: Key,
    pub channel: String,
    pub badge: String,
    pub logo: Option<Arc<Logo>>,
    pub programme: Programme,
}

/// 検索用に文字を正規化する（全角英数と空白を半角に、英字を小文字に）。一文字ずつなので
/// 元の文字の位置へ戻れる（[`marks`]）。
pub fn fold(text: &str) -> String {
    text.chars().flat_map(fold_letter).collect()
}

fn fold_letter(letter: char) -> std::char::ToLowercase {
    match letter {
        '\u{ff01}'..='\u{ff5e}' => {
            char::from_u32(u32::from(letter) - 0xff01 + 0x21).unwrap_or(letter)
        }
        '\u{3000}' => ' ',
        _ => letter,
    }
    .to_lowercase()
}

/// `query` の語に一致したバイトの範囲（元の文字の境界で）。重なり・接したものはまとめる。
pub fn marks(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let words: Vec<Vec<char>> = fold(query)
        .split_whitespace()
        .map(|word| word.chars().collect())
        .collect();
    let mut folded: Vec<char> = Vec::new();
    let mut origin: Vec<std::ops::Range<usize>> = Vec::new();
    for (at, letter) in text.char_indices() {
        for one in fold_letter(letter) {
            folded.push(one);
            origin.push(at..at + letter.len_utf8());
        }
    }
    let mut found: Vec<std::ops::Range<usize>> = Vec::new();
    for word in words.iter().filter(|word| !word.is_empty()) {
        let mut at = 0;
        while at + word.len() <= folded.len() {
            if folded[at..at + word.len()] == word[..] {
                found.push(origin[at].start..origin[at + word.len() - 1].end);
                at += word.len();
            } else {
                at += 1;
            }
        }
    }
    found.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in found {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[derive(Debug, Clone, Default)]
pub struct Channel {
    pub key: Key,
    /// 同じ TS の隣の列はサブチャンネル。
    pub transport: u16,
    pub name: String,
    pub badge: String,
    pub remote_key: Option<u8>,
    pub logo: Option<Arc<Logo>>,
    pub programmes: Vec<Programme>,
    pub filled: (usize, usize),
}

/// ロゴが届くまで代わりに出す文字。全角英字は半角にし、三文字まで。
pub fn badge_of(name: &str) -> String {
    let narrowed: String = name.chars().map(narrow).collect();
    let head: String = narrowed
        .chars()
        .take_while(|letter| letter.is_ascii_alphanumeric())
        .take(3)
        .collect();
    if !head.is_empty() {
        return head;
    }
    narrowed.chars().take(1).collect()
}

fn narrow(letter: char) -> char {
    match letter {
        'Ａ'..='Ｚ' => {
            char::from_u32(u32::from(letter) - 0xff21 + u32::from('A')).unwrap_or(letter)
        }
        'ａ'..='ｚ' => {
            char::from_u32(u32::from(letter) - 0xff41 + u32::from('a')).unwrap_or(letter)
        }
        '０'..='９' => {
            char::from_u32(u32::from(letter) - 0xff10 + u32::from('0')).unwrap_or(letter)
        }
        _ => letter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_words_are_marked_where_they_stand_in_the_text() {
        let text = "水曜ドラマ「灯台守の娘」ＺＷ灯台";
        let marked: Vec<&str> = marks(text, "灯台")
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(marked, vec!["灯台", "灯台"]);
        let marked: Vec<&str> = marks(text, "zw 娘")
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(marked, vec!["娘", "ＺＷ"]);
        assert_eq!(marks("abcd", "abc bcd"), vec![0..4]);
        assert!(marks("abcd", "  ").is_empty());
        assert!(marks("abcd", "xyz").is_empty());
    }

    #[test]
    fn the_count_follows_the_band_chosen() {
        let search = Search {
            asked: Asked::default(),
            counts: vec![(Band(0), 3), (Band(1), 1)],
            ..Search::default()
        };
        assert_eq!(search.total(), 4);
        assert_eq!(search.count_of(Band(2)), 0);
        let search = Search {
            asked: Asked {
                band: Some(Band(1)),
                ..Asked::default()
            },
            ..search
        };
        assert_eq!(search.total(), 1);
    }

    #[test]
    fn a_badge_takes_the_head_of_the_name() {
        assert_eq!(badge_of("ZWV"), "ZWV");
        assert_eq!(badge_of("XYZ"), "XYZ");
        assert_eq!(badge_of("ZWV総合"), "ZWV");
        assert_eq!(badge_of("ＺＷＶ総合"), "ZWV");
        assert_eq!(badge_of("ＢＳ架空"), "BS");
        assert_eq!(badge_of("テレビ架空"), "テ");
    }
}
