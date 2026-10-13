// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! サービスとストリームを指すキー（`"nid:sid"` と `"nid:tsid"`）。`0x` があれば十六進で読み、書くときは十進。

use std::fmt;

use zwcore::guide::Key;

pub fn number(text: &str) -> Option<u16> {
    let text = text.trim();
    if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u16::from_str_radix(digits, 16).ok()
    } else {
        text.parse().ok()
    }
}

pub fn key(text: &str) -> Option<Key> {
    let (network, service) = text.split_once(':')?;
    Some(Key {
        network: number(network)?,
        service: number(service)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stream {
    pub network: u16,
    pub transport: u16,
}

impl Stream {
    pub fn parse(text: &str) -> Option<Self> {
        let (network, transport) = text.split_once(':')?;
        Some(Self {
            network: number(network)?,
            transport: number(transport)?,
        })
    }
}

impl fmt::Display for Stream {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}:{}", self.network, self.transport)
    }
}

pub fn written(key: Key) -> String {
    format!("{}:{}", key.network, key.service)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Any,
    One(u16),
    Range(u16, u16),
}

impl Pattern {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == "*" {
            return Some(Self::Any);
        }
        if let Some((from, to)) = text.split_once('-') {
            let (from, to) = (number(from)?, number(to)?);
            return (from <= to).then_some(Self::Range(from, to));
        }
        number(text).map(Self::One)
    }

    pub fn matches(self, value: u16) -> bool {
        match self {
            Self::Any => true,
            Self::One(one) => one == value,
            Self::Range(from, to) => (from..=to).contains(&value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPattern {
    pub network: Pattern,
    pub service: Pattern,
}

impl KeyPattern {
    pub fn parse(text: &str) -> Option<Self> {
        let (network, service) = text.split_once(':')?;
        Some(Self {
            network: Pattern::parse(network)?,
            service: Pattern::parse(service)?,
        })
    }

    pub fn matches(self, key: Key) -> bool {
        self.network.matches(key.network) && self.service.matches(key.service)
    }
}

/// TOML の値が整数でも文字列でも一つの数として読む（`nids = [4, "0x7fe0-0x7fef"]`）。
pub fn pattern_of(value: &toml::Value) -> Option<Pattern> {
    match value {
        toml::Value::Integer(number) => u16::try_from(*number).ok().map(Pattern::One),
        toml::Value::String(text) => Pattern::parse(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_decimal_mix_in_one_key() {
        assert_eq!(
            key("0x7fe6:1024"),
            Some(Key {
                network: 0x7fe6,
                service: 1024
            })
        );
        assert_eq!(
            key("4:0x65"),
            Some(Key {
                network: 4,
                service: 101
            })
        );
        assert_eq!(key("4"), None);
        assert_eq!(key("4:x"), None);
    }

    #[test]
    fn without_a_prefix_it_is_decimal() {
        assert_eq!(number("101"), Some(101));
        assert_eq!(number("0x101"), Some(0x101));
        assert_eq!(number("7fe6"), None);
    }

    #[test]
    fn what_is_written_reads_back() {
        let one = Key {
            network: 0x7fe6,
            service: 0x400,
        };
        assert_eq!(written(one), "32742:1024");
        assert_eq!(key(&written(one)), Some(one));
        let stream = Stream {
            network: 4,
            transport: 0x4031,
        };
        assert_eq!(Stream::parse(&stream.to_string()), Some(stream));
    }

    #[test]
    fn a_pattern_is_one_number_a_range_or_anything() {
        assert!(Pattern::parse("*").unwrap().matches(9));
        assert!(Pattern::parse("7").unwrap().matches(7));
        assert!(!Pattern::parse("7").unwrap().matches(8));
        let range = Pattern::parse("0x7880-0x7fef").unwrap();
        assert!(range.matches(0x7fe6));
        assert!(!range.matches(0x7ff0));
        assert_eq!(Pattern::parse("9-3"), None, "逆さの範囲は受け付けない");
        let all_of_one_network = KeyPattern::parse("0x7fe6:*").unwrap();
        assert!(all_of_one_network.matches(Key {
            network: 0x7fe6,
            service: 1
        }));
        assert!(!all_of_one_network.matches(Key {
            network: 4,
            service: 1
        }));
    }

    #[test]
    fn a_toml_integer_or_string_is_a_pattern() {
        assert_eq!(pattern_of(&toml::Value::Integer(4)), Some(Pattern::One(4)));
        assert_eq!(
            pattern_of(&toml::Value::String(String::from("1-3"))),
            Some(Pattern::Range(1, 3))
        );
        assert_eq!(pattern_of(&toml::Value::Boolean(true)), None);
    }
}
