// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! WebSocket（`ws://`・`wss://`）。繋いだ後は待たずに読み書きする。TLS の確かめ方と宛先の制限は HTTP と同じ。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::http::{HeaderName, HeaderValue};

use super::address::addresses_of;
use super::{Error, basic, tls};

const CONNECT_PATIENCE: Duration = Duration::from_secs(3);
/// 繋いで最初の応答（ハンドシェイク）を待つ上限。
const HANDSHAKE_PATIENCE: Duration = Duration::from_secs(10);
const MESSAGE_MOST: usize = 16 << 20;
/// 相手が close を送らずに切れた（RFC 6455 7.4.1）。
pub const ABNORMAL: u16 = 1006;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    /// 相手が閉じたか、切れた。
    Closed {
        code: u16,
        reason: String,
    },
}

pub struct Socket {
    socket: tungstenite::WebSocket<Stream>,
    /// ハンドシェイクの応答のヘッダー。名前は小文字。
    pub headers: Vec<(String, String)>,
    /// こちらから close を送った。
    closing: bool,
    /// 相手が閉じたか、切れた。
    ended: bool,
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Socket")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl Socket {
    /// `inner` は内部ネットワークへ繋いでよいか、`roots` は足すルート証明書（DER）。
    pub fn open(
        url: &str,
        headers: &[(String, String)],
        inner: bool,
        roots: &[Vec<u8>],
    ) -> Result<Self, Error> {
        let mut parsed =
            url::Url::parse(url).map_err(|why| Error::Argument(format!("{url}: {why}")))?;
        let secure = match parsed.scheme() {
            "ws" => false,
            "wss" => true,
            other => return Err(Error::Argument(format!("{other}: ws でも wss でもない"))),
        };
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::Argument(format!("{url}: ホストが無い")))?
            .trim_matches(['[', ']'])
            .to_owned();
        let port = parsed
            .port_or_known_default()
            .unwrap_or(if secure { 443 } else { 80 });
        let authorization =
            (!parsed.username().is_empty()).then(|| basic(parsed.username(), parsed.password()));
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);

        let mut request = parsed
            .as_str()
            .into_client_request()
            .map_err(|why| Error::Argument(why.to_string()))?;
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::Argument(format!("ヘッダー {name}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| Error::Argument(format!("ヘッダー {name} の値")))?;
            request.headers_mut().append(name, value);
        }
        if let Some(authorization) = authorization {
            let value = HeaderValue::from_str(&authorization)
                .map_err(|_| Error::Argument(String::from("ユーザー情報")))?;
            request
                .headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, value);
        }

        let tcp = connect(&host, port, inner)?;
        tcp.set_read_timeout(Some(HANDSHAKE_PATIENCE))
            .and_then(|()| tcp.set_write_timeout(Some(HANDSHAKE_PATIENCE)))
            .map_err(|_| Error::Connect)?;
        let stream = if secure {
            let name = rustls::pki_types::ServerName::try_from(host.clone())
                .map_err(|_| Error::Argument(format!("{host}: TLS の名前にならない")))?;
            let connection =
                rustls::ClientConnection::new(tls::config(roots), name).map_err(|_| Error::Tls)?;
            Stream::Tls(Box::new(rustls::StreamOwned::new(connection, tcp)))
        } else {
            Stream::Plain(tcp)
        };
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MESSAGE_MOST))
            .max_frame_size(Some(MESSAGE_MOST));
        let (mut socket, answer) =
            tungstenite::client::client_with_config(request, stream, Some(config)).map_err(
                |why| match why {
                    tungstenite::HandshakeError::Failure(why) => failure(why),
                    tungstenite::HandshakeError::Interrupted(_) => Error::Timeout,
                },
            )?;
        socket
            .get_mut()
            .tcp()
            .set_nonblocking(true)
            .map_err(|_| Error::Connect)?;
        let headers = answer
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok(Self {
            socket,
            headers,
            closing: false,
            ended: false,
        })
    }

    /// 送れないうちは溜めておき、次に読み書きするときに送る。
    pub fn send(&mut self, message: Message) -> Result<(), Error> {
        let message = match message {
            Message::Text(text) => tungstenite::Message::text(text),
            Message::Binary(bytes) => tungstenite::Message::binary(bytes),
            Message::Closed { code, reason } => {
                self.close_with(code, &reason);
                return Ok(());
            }
        };
        if self.closing || self.ended {
            return Err(Error::Closed(String::from("閉じてある")));
        }
        match self.socket.send(message) {
            Ok(()) => Ok(()),
            Err(tungstenite::Error::Io(why)) if why.kind() == std::io::ErrorKind::WouldBlock => {
                Ok(())
            }
            Err(why) => {
                self.ended = true;
                Err(failure(why))
            }
        }
    }

    /// 待たずに読めるものだけ。無ければ `None`。ping への応答は自動で返す。
    pub fn receive(&mut self) -> Option<Message> {
        let abnormal = || Message::Closed {
            code: ABNORMAL,
            reason: String::new(),
        };
        if self.ended {
            return Some(abnormal());
        }
        loop {
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    return Some(Message::Text(text.as_str().to_owned()));
                }
                Ok(tungstenite::Message::Binary(bytes)) => {
                    return Some(Message::Binary(bytes.to_vec()));
                }
                Ok(tungstenite::Message::Close(frame)) => {
                    // 応える close は tungstenite が送る。
                    let _ = self.socket.flush();
                    self.ended = true;
                    return Some(match frame {
                        Some(frame) => Message::Closed {
                            code: frame.code.into(),
                            reason: frame.reason.as_str().to_owned(),
                        },
                        None => Message::Closed {
                            code: 1005,
                            reason: String::new(),
                        },
                    });
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(why))
                    if why.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    let _ = self.socket.flush();
                    return None;
                }
                Err(_) => {
                    self.ended = true;
                    return Some(abnormal());
                }
            }
        }
    }

    /// 1001（去る）で閉じる。
    pub fn close(&mut self) {
        self.close_with(1001, "");
    }

    /// 相手の close は [`Self::receive`] で来る。
    pub fn close_with(&mut self, code: u16, reason: &str) {
        if !self.closing && !self.ended {
            self.closing = true;
            let frame = tungstenite::protocol::CloseFrame {
                code: code.into(),
                reason: reason.to_owned().into(),
            };
            let _ = self.socket.close(Some(frame));
            let _ = self.socket.flush();
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.close();
    }
}

fn connect(host: &str, port: u16, inner: bool) -> Result<TcpStream, Error> {
    let mut last = Error::Connect;
    for address in addresses_of(host, port, inner)? {
        match TcpStream::connect_timeout(&address, CONNECT_PATIENCE) {
            Ok(tcp) => return Ok(tcp),
            Err(why) if why.kind() == std::io::ErrorKind::TimedOut => last = Error::Timeout,
            Err(_) => last = Error::Connect,
        }
    }
    Err(last)
}

fn failure(error: tungstenite::Error) -> Error {
    match error {
        tungstenite::Error::Io(why) => {
            if why
                .get_ref()
                .is_some_and(|inside| inside.is::<rustls::Error>())
            {
                Error::Tls
            } else if matches!(
                why.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                Error::Timeout
            } else {
                Error::Closed(why.to_string())
            }
        }
        tungstenite::Error::Http(answer) => {
            Error::Closed(format!("WebSocket に切り替わらない（{}）", answer.status()))
        }
        tungstenite::Error::Url(why) => Error::Argument(why.to_string()),
        other => Error::Closed(other.to_string()),
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => tls.get_ref(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(into),
            Self::Tls(tls) => tls.read(into),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(bytes),
            Self::Tls(tls) => tls.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn echo(tls: Option<std::sync::Arc<rustls::ServerConfig>>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let stream: Box<dyn crate::net::testing::Both> = match tls {
                Some(config) => Box::new(rustls::StreamOwned::new(
                    rustls::ServerConnection::new(config).unwrap(),
                    tcp,
                )),
                None => Box::new(tcp),
            };
            let Ok(mut socket) = tungstenite::accept(stream) else {
                return;
            };
            while let Ok(message) = socket.read() {
                if message.is_close() {
                    // 応える close を送り切る。
                    let _ = socket.flush();
                    continue;
                }
                if socket.send(message).is_err() {
                    break;
                }
            }
        });
        port
    }

    fn next(socket: &mut Socket) -> Message {
        let began = Instant::now();
        loop {
            if let Some(message) = socket.receive() {
                return message;
            }
            assert!(began.elapsed() < Duration::from_secs(5), "届かない");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn what_is_sent_comes_back_without_waiting() {
        let port = echo(None);
        let mut socket = Socket::open(&format!("ws://127.0.0.1:{port}/"), &[], true, &[]).unwrap();
        assert_eq!(socket.receive(), None, "何も来ていなければ待たずに戻る");
        socket
            .send(Message::Text(String::from("こんにちは")))
            .unwrap();
        assert_eq!(next(&mut socket), Message::Text(String::from("こんにちは")));
        socket.send(Message::Binary(vec![1, 2, 3])).unwrap();
        assert_eq!(next(&mut socket), Message::Binary(vec![1, 2, 3]));
        socket.close_with(1000, "bye");
        let began = Instant::now();
        let closed = loop {
            match socket.receive() {
                Some(Message::Closed { code, .. }) => break code,
                Some(other) => panic!("{other:?}"),
                None => assert!(began.elapsed() < Duration::from_secs(5), "届かない"),
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(closed, 1000, "相手が応えた close");
    }

    #[test]
    fn wss_is_checked_like_https() {
        let (ca, server) = crate::net::testing::authority();
        let port = echo(Some(server));
        let url = format!("wss://127.0.0.1:{port}/");
        let mut socket = Socket::open(&url, &[], true, &[ca]).unwrap();
        socket.send(Message::Text(String::from("secure"))).unwrap();
        assert_eq!(next(&mut socket), Message::Text(String::from("secure")));

        let (_, other) = crate::net::testing::authority();
        let port = echo(Some(other));
        let url = format!("wss://127.0.0.1:{port}/");
        assert_eq!(Socket::open(&url, &[], true, &[]).err(), Some(Error::Tls));
    }

    #[test]
    fn the_lan_is_refused_unless_allowed() {
        assert_eq!(
            Socket::open("ws://127.0.0.1:9/", &[], false, &[]).err(),
            Some(Error::Inner)
        );
        assert!(matches!(
            Socket::open("http://127.0.0.1:9/", &[], true, &[]),
            Err(Error::Argument(_))
        ));
    }
}
