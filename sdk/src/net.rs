// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ネットワーク通信（STD-B24 第二分冊 (2/2) 付属 3 5.6.5、TR-B14 第三分冊 8）。`request` は同期で待つ（上限 `TIMEOUT`）。

use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub location: Option<String>,
    /// 受け取った表記と順序のまま（`getAllResponseHeaders` 用）。
    pub headers: Vec<(String, String)>,
}

/// 番号は `transmitTextDataOverIP` の返り値（TR-B14 表 8-7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Argument,
    Disconnected,
    Timeout,
    Connect,
    Name,
    Tls,
}

impl Failure {
    pub fn code(self) -> i32 {
        match self {
            Self::Argument => -1,
            Self::Disconnected => -2,
            Self::Timeout => -3,
            Self::Connect => -300,
            Self::Name => -400,
            Self::Tls => -500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    fn authority(&self) -> String {
        if self.port == if self.secure { 443 } else { 80 } {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

pub fn parse_url(uri: &str) -> Option<Url> {
    let (secure, rest) = match uri.strip_prefix("http://") {
        Some(rest) => (false, rest),
        None => (true, uri.strip_prefix("https://")?),
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') || host.ends_with(']') => {
            (host, port.parse().ok()?)
        }
        _ => (authority, if secure { 443 } else { 80 }),
    };
    Some(Url {
        secure,
        host: host.trim_matches(|c| c == '[' || c == ']').to_owned(),
        port,
        path: path.to_owned(),
    })
}

pub fn join(base: &str, name: &str) -> Option<String> {
    if name.starts_with("http://") || name.starts_with("https://") {
        return Some(name.to_owned());
    }
    let url = parse_url(base)?;
    let scheme = if url.secure { "https" } else { "http" };
    let authority = url.authority();
    let path = url.path.split(['?', '#']).next().unwrap_or("/");
    let absolute = if let Some(rest) = name.strip_prefix("~/") {
        format!("/{rest}")
    } else if name.starts_with('/') {
        name.to_owned()
    } else {
        let directory = path.rsplit_once('/').map_or("/", |(dir, _)| dir);
        format!("{directory}/{name}")
    };
    let (path_part, query) = match absolute.find('?') {
        Some(at) => (&absolute[..at], &absolute[at..]),
        None => (absolute.as_str(), ""),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in path_part.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(format!("{scheme}://{authority}/{}{query}", parts.join("/")))
}

/// 電文の `Denbun=` の値：英数と `-_.!~*'()` の他は `%XX`（RFC 2396）。
pub fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 待たない通信（`zw_net_v1` の `open`・`ws`・`poll`・`send`・`close`）の記録。
/// `poll` が返すのは記録の並び：種類（u8）、長さ（u32 BE）、中身。
pub mod conn {
    pub mod kind {
        /// i32 BE の番号（[`super::super::Failure::code`]）。
        pub const FAIL: u8 = 0;
        /// u16 BE の status と、`名前: 値` を `\r\n` で並べたもの。
        pub const HEAD: u8 = 1;
        /// HTTP の本文、WebSocket のバイナリのメッセージ。
        pub const DATA: u8 = 2;
        /// WebSocket のテキストのメッセージ。
        pub const TEXT: u8 = 3;
        /// u16 BE の終わり方（WebSocket の close のコード、HTTP は 0）と理由。
        pub const END: u8 = 4;
    }

    /// `send` の種類。`CLOSE` の中身は u16 BE のコードと理由。
    pub mod send {
        pub const TEXT: u32 = 1;
        pub const BINARY: u32 = 2;
        pub const CLOSE: u32 = 8;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Arrived {
        Head {
            status: u16,
            headers: Vec<(String, String)>,
        },
        Data(Vec<u8>),
        Text(String),
        End {
            code: u16,
            reason: String,
        },
        Fail(i32),
    }

    impl Arrived {
        pub fn len(&self) -> usize {
            match self {
                Self::Data(bytes) => bytes.len(),
                Self::Text(text) => text.len(),
                _ => 0,
            }
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }

        pub fn is_last(&self) -> bool {
            matches!(self, Self::End { .. } | Self::Fail(_))
        }

        pub fn encode(&self, out: &mut Vec<u8>) {
            let u16_and_text =
                |n: u16, text: &str| [&n.to_be_bytes()[..], text.as_bytes()].concat();
            let (kind, body) = match self {
                Self::Head { status, headers } => {
                    let text = headers
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}"))
                        .collect::<Vec<_>>()
                        .join("\r\n");
                    (kind::HEAD, u16_and_text(*status, &text))
                }
                Self::Data(bytes) => (kind::DATA, bytes.clone()),
                Self::Text(text) => (kind::TEXT, text.as_bytes().to_vec()),
                Self::End { code, reason } => (kind::END, u16_and_text(*code, reason)),
                Self::Fail(code) => (kind::FAIL, code.to_be_bytes().to_vec()),
            };
            out.push(kind);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&body);
        }
    }

    pub fn decode(mut bytes: &[u8]) -> Vec<Arrived> {
        let mut out = Vec::new();
        while let Some((&kind, rest)) = bytes.split_first() {
            let Some((len, rest)) = rest.split_first_chunk::<4>() else {
                break;
            };
            let Some(body) = rest.get(..u32::from_be_bytes(*len) as usize) else {
                break;
            };
            bytes = &rest[body.len()..];
            let u16_and_text = |body: &[u8]| {
                let (n, text) = body
                    .split_first_chunk::<2>()
                    .map_or((0, &[][..]), |(n, t)| (u16::from_be_bytes(*n), t));
                (n, String::from_utf8_lossy(text).into_owned())
            };
            out.push(match kind {
                kind::HEAD => {
                    let (status, text) = u16_and_text(body);
                    let headers = text
                        .split("\r\n")
                        .filter_map(|line| line.split_once(": "))
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect();
                    Arrived::Head { status, headers }
                }
                kind::DATA => Arrived::Data(body.to_vec()),
                kind::TEXT => Arrived::Text(String::from_utf8_lossy(body).into_owned()),
                kind::END => {
                    let (code, reason) = u16_and_text(body);
                    Arrived::End { code, reason }
                }
                _ => Arrived::Fail(
                    body.first_chunk::<4>()
                        .map_or(-2, |n| i32::from_be_bytes(*n)),
                ),
            });
        }
        out
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Outgoing {
        Text(Vec<u8>),
        Binary(Vec<u8>),
        Close(u16, Vec<u8>),
    }

    /// `open`・`ws` で開くもの。`protocols` があれば WebSocket（`Sec-WebSocket-Protocol` に `,` で並べる）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Open {
        pub method: String,
        pub url: String,
        pub headers: Vec<(String, String)>,
        pub body: Option<Vec<u8>>,
        pub protocols: Option<String>,
        /// 内部ネットワーク（LAN など）へ繋いでよいか。
        pub inner: bool,
    }
}

/// ネットワーク通信。使う側が [`install`] する。
#[cfg(not(target_arch = "wasm32"))]
mod network {
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    use super::conn::{Arrived, Open, Outgoing};
    use super::{Failure, Reply};

    pub trait Network: Send + Sync {
        fn has_ip(&self) -> bool;
        fn resolve(&self, host: &str) -> Option<String>;
        /// ICMP は特権が要るので TCP で。返るのは (相手のアドレス, 往復のミリ秒)。
        fn reach(
            &self,
            host: &str,
            port: u16,
            timeout: Duration,
            inner: bool,
        ) -> Option<(String, u32)>;
        /// 一つのプラグインの通信。放送で届いたルート証明書はこの中だけで使う。
        fn session(&self) -> Box<dyn Session>;
    }

    /// `inner` は内部ネットワーク（LAN など）へ繋いでよいか。
    pub trait Session: Send + std::fmt::Debug {
        /// 本文があり `Content-Type` が無ければ form。転送は五回まで（POST の 303 は GET に）。
        fn send(
            &self,
            method: &str,
            uri: &str,
            headers: &[(String, String)],
            body: Option<&[u8]>,
            inner: bool,
        ) -> Result<Reply, Failure>;
        /// 放送で届いたものを追加する（TR-B14 第三分冊）。選局で破棄する。
        fn add_root_certificate(&self, der: &[u8]) -> bool;
        /// 繋ぐのは後で。繋げなかったことは [`Conn::next`] で [`Arrived::Fail`] として来る。
        fn open(&self, open: Open) -> Box<dyn Conn>;
    }

    /// 待たない接続。落とすと切る。
    pub trait Conn: Send + std::fmt::Debug {
        fn next(&mut self) -> Option<Arrived>;
        /// WebSocket だけ。閉じていれば偽。
        fn send(&mut self, outgoing: Outgoing) -> bool;
    }

    static NETWORK: RwLock<Option<Arc<dyn Network>>> = RwLock::new(None);

    pub fn install(network: Arc<dyn Network>) {
        *NETWORK.write().unwrap_or_else(|e| e.into_inner()) = Some(network);
    }

    pub fn network() -> Option<Arc<dyn Network>> {
        NETWORK.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 差し込まれていなければ、どこへも繋がらない。
    pub fn session() -> Box<dyn Session> {
        network().map_or_else(
            || Box::new(Unplugged) as Box<dyn Session>,
            |network| network.session(),
        )
    }

    #[derive(Debug)]
    struct Unplugged;

    impl Session for Unplugged {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &[(String, String)],
            _: Option<&[u8]>,
            _: bool,
        ) -> Result<Reply, Failure> {
            Err(Failure::Connect)
        }

        fn add_root_certificate(&self, _: &[u8]) -> bool {
            false
        }

        fn open(&self, _: Open) -> Box<dyn Conn> {
            Box::new(Refused(true))
        }
    }

    /// 繋がらなかったことだけを渡す。
    #[derive(Debug)]
    struct Refused(bool);

    impl Conn for Refused {
        fn next(&mut self) -> Option<Arrived> {
            std::mem::take(&mut self.0).then(|| Arrived::Fail(Failure::Connect.code()))
        }

        fn send(&mut self, _: Outgoing) -> bool {
            false
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use network::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_split() {
        let url = parse_url("http://example.jp/a/b?c=1").unwrap();
        assert_eq!(
            (url.host.as_str(), url.port, url.path.as_str()),
            ("example.jp", 80, "/a/b?c=1")
        );
        assert!(!url.secure);
        let url = parse_url("https://example.jp:8443").unwrap();
        assert_eq!((url.port, url.path.as_str(), url.secure), (8443, "/", true));
        assert!(parse_url("ftp://x").is_none());
        assert!(parse_url("http://").is_none());
    }

    #[test]
    fn relative_names_are_joined_to_the_document_url() {
        let base = "http://example.jp/a/b/startup.bml";
        assert_eq!(
            join(base, "top.bml").as_deref(),
            Some("http://example.jp/a/b/top.bml")
        );
        assert_eq!(
            join(base, "../x.btb").as_deref(),
            Some("http://example.jp/a/x.btb")
        );
        assert_eq!(
            join(base, "/r/y.png").as_deref(),
            Some("http://example.jp/r/y.png")
        );
        assert_eq!(
            join(base, "~/z.css").as_deref(),
            Some("http://example.jp/z.css")
        );
        assert_eq!(
            join(base, "http://o.jp/q").as_deref(),
            Some("http://o.jp/q")
        );
        assert_eq!(
            join("http://h:8080/d/i.bml", "j?k=1").as_deref(),
            Some("http://h:8080/d/j?k=1")
        );
    }

    #[test]
    fn the_form_is_escaped_the_rfc_2396_way() {
        assert_eq!(escape(b"a b-1"), "a%20b-1");
        assert_eq!(escape(&[0xa4, 0xa2]), "%A4%A2");
    }

    #[test]
    fn what_arrived_is_carried_as_it_was() {
        let all = vec![
            conn::Arrived::Head {
                status: 200,
                headers: vec![(String::from("content-type"), String::from("text/plain"))],
            },
            conn::Arrived::Data(vec![0, 1, 2]),
            conn::Arrived::Text(String::from("文字")),
            conn::Arrived::End {
                code: 1000,
                reason: String::from("bye"),
            },
            conn::Arrived::Fail(Failure::Tls.code()),
        ];
        let mut bytes = Vec::new();
        for arrived in &all {
            arrived.encode(&mut bytes);
        }
        assert_eq!(conn::decode(&bytes), all);
    }
}
