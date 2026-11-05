// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ネットワーク通信。HTTP は reqwest を小さな tokio で動かし、同期で読める形で渡す。TLS は rustls（ring）で、
//! 証明書は OS のストアと同梱のルート（webpki-roots）で確かめる。

pub mod address;
#[cfg(feature = "plugins")]
pub mod broadcast;
pub mod http;
#[cfg(test)]
mod testing;
pub mod tls;
pub mod ws;

use std::sync::Arc;

/// 真を返したら、待っている通信をやめる。
pub type Stop = Arc<dyn Fn() -> bool + Send + Sync>;

pub fn never() -> Stop {
    Arc::new(|| false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// URL・メソッド・ヘッダーが正しくない。
    Argument(String),
    Name,
    /// 内部ネットワーク（LAN など）の宛先で、許していない。
    Inner,
    Connect,
    Tls,
    Timeout,
    Stopped,
    /// 応答の途中か、応答の前に切れた。
    Closed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Argument(what) => write!(out, "要求が正しくない（{what}）"),
            Self::Name => out.write_str("名前を引けない"),
            Self::Inner => out.write_str("内部ネットワークの宛先には接続しない"),
            Self::Connect => out.write_str("接続できない"),
            Self::Tls => out.write_str("TLS で接続できない（証明書を確かめられない）"),
            Self::Timeout => out.write_str("応答が無い"),
            Self::Stopped => out.write_str("止めた"),
            Self::Closed(why) => write!(out, "切れた（{why}）"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        let kind = match error {
            Error::Argument(_) => std::io::ErrorKind::InvalidInput,
            Error::Name | Error::Inner | Error::Connect => std::io::ErrorKind::ConnectionRefused,
            Error::Tls => std::io::ErrorKind::InvalidData,
            Error::Timeout => std::io::ErrorKind::TimedOut,
            Error::Stopped => std::io::ErrorKind::Interrupted,
            Error::Closed(_) => std::io::ErrorKind::UnexpectedEof,
        };
        Self::new(kind, error)
    }
}

fn basic(user: &str, password: Option<&str>) -> String {
    let decoded = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            let hex = |b: u8| char::from(b).to_digit(16);
            match (bytes[at], bytes.get(at + 1), bytes.get(at + 2)) {
                (b'%', Some(&high), Some(&low)) if hex(high).is_some() && hex(low).is_some() => {
                    out.push((hex(high).unwrap_or(0) * 16 + hex(low).unwrap_or(0)) as u8);
                    at += 3;
                }
                (b, ..) => {
                    out.push(b);
                    at += 1;
                }
            }
        }
        out
    };
    let mut pair = decoded(user);
    pair.push(b':');
    pair.extend(decoded(password.unwrap_or("")));
    format!("Basic {}", base64(&pair))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let joined = group
            .iter()
            .enumerate()
            .fold(0u32, |all, (at, b)| all | u32::from(*b) << (16 - 8 * at));
        for at in 0..4 {
            if at <= group.len() {
                out.push(char::from(
                    ALPHABET[(joined >> (18 - 6 * at) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_padded_to_fours() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn credentials_written_escaped_in_a_url_are_sent_as_they_are_meant() {
        assert_eq!(basic("user", Some("pass")), "Basic dXNlcjpwYXNz");
        assert_eq!(basic("a%40b", Some("p%3Aq")), basic("a@b", Some("p:q")));
        assert_eq!(basic("user", None), "Basic dXNlcjo=");
    }
}
