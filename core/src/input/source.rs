// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送のバイト列がどこから来るか。coaxd・Mirakurun・録画の三つとも [`Source`] を実装する。

use std::fmt;
use std::io::Read;

use crate::guide::Guide;

#[cfg(feature = "coaxd")]
pub mod coaxd;
pub mod file;
#[cfg(feature = "mirakurun")]
pub mod mirakurun;
#[cfg(feature = "coaxd")]
pub mod pcsc;

/// 選局の単位。中身の意味はソースごとに違う（物理チャンネル、サービス、ファイル）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub key: Option<crate::guide::Key>,
    pub transport: Option<u16>,
    pub order: Option<u32>,
    pub how: String,
    /// バイト列から推測しない（先頭の数バイトだけでは時々間違える）。
    pub container: crate::filters::Container,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tuner {
    pub id: String,
    pub name: String,
    pub busy: bool,
    pub locked: Option<bool>,
    pub cnr: Option<f32>,
}

impl Tuner {
    pub fn shown(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Unreachable(String),
    Malformed(String),
    Refused(String),
    Unavailable(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(why) => write!(out, "ソースに接続できません。\n{why}"),
            Self::Malformed(why) => write!(out, "ソースの応答を読み取れません。\n{why}"),
            Self::Refused(why) => write!(out, "選局できません。\n{why}"),
            Self::Unavailable(what) => {
                write!(out, "{what} はこのバージョンでは使えません。")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Unreachable(error.to_string())
    }
}

pub type Bytes = Box<dyn Read + Send>;

/// こちらから切った（[`Source::interrupt`]）なら、`read` のエラーは終端として 0 を返す。
#[cfg(any(feature = "coaxd", feature = "mirakurun"))]
pub(crate) struct Streamed {
    answer: coax::http::Response<coax::daemon::Connection>,
    cut: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(any(feature = "coaxd", feature = "mirakurun"))]
impl Streamed {
    pub(crate) fn new(
        answer: coax::http::Response<coax::daemon::Connection>,
        cut: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self { answer, cut }
    }
}

#[cfg(any(feature = "coaxd", feature = "mirakurun"))]
impl Read for Streamed {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        match self.answer.read_body(into) {
            Ok(read) => Ok(read),
            Err(_) if self.cut.load(std::sync::atomic::Ordering::Relaxed) => Ok(0),
            Err(error) => Err(error),
        }
    }
}

#[cfg(any(feature = "coaxd", feature = "mirakurun"))]
impl fmt::Debug for Streamed {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Streamed").finish_non_exhaustive()
    }
}

/// 来ないバイト列。`0` は返さず（終わりになる）、時々 [`NotYet`] を返して停止の要求を確かめる機会を作る。
pub fn pending() -> Bytes {
    struct Pending {
        wait: std::sync::mpsc::Receiver<()>,
        _keep: std::sync::mpsc::Sender<()>,
    }
    impl Read for Pending {
        fn read(&mut self, _into: &mut [u8]) -> std::io::Result<usize> {
            let _ = self
                .wait
                .recv_timeout(std::time::Duration::from_millis(200));
            Err(std::io::Error::other(NotYet))
        }
    }
    let (keep, wait) = std::sync::mpsc::channel();
    Box::new(Pending { wait, _keep: keep })
}

#[derive(Debug)]
pub(crate) struct NotYet;

impl fmt::Display for NotYet {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("まだ来ない")
    }
}

impl std::error::Error for NotYet {}

#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
impl NotYet {
    pub(crate) fn is(error: &std::io::Error) -> bool {
        error.get_ref().is_some_and(|inner| inner.is::<Self>())
    }
}

pub trait Source: Send {
    fn name(&self) -> &str;

    fn channels(&mut self) -> Result<Vec<Channel>, Error>;

    fn open(&mut self, channel: &Channel) -> Result<(Bytes, crate::filters::Container), Error>;

    /// 番組表を一度に全部取得できるソースだけが返す。重いので起動のたびに呼ばない。
    fn guide(&mut self) -> Result<Option<Guide>, Error> {
        Ok(None)
    }

    fn guide_of(&mut self, _key: crate::guide::Key) -> Result<Option<Guide>, Error> {
        Ok(None)
    }

    fn tuners(&mut self) -> Result<Vec<Tuner>, Error> {
        Ok(Vec::new())
    }

    fn cnr(&mut self) -> Option<f32> {
        None
    }

    fn holding(&self) -> Option<String> {
        None
    }

    /// 選局の切り替えに要る（`read` は相手が応答しないと待ち続ける）。
    fn interrupt(&mut self) {}
}

#[cfg(all(test, any(feature = "coaxd", feature = "mirakurun")))]
mod tests {
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::Streamed;

    #[test]
    fn a_stream_cut_from_this_side_ends_rather_than_fails() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("a caller");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .expect("written");
            socket
        });
        let connection =
            coax::daemon::Connection::to(&coax::daemon::Endpoint::Network(address.to_string()))
                .expect("connected");
        let interrupter = connection.interrupter().expect("an interrupter");
        let answer = coax::http::get(connection, "/x", "test", &[]).expect("an answer");
        let cut = Arc::new(AtomicBool::new(false));
        let mut streamed = Streamed::new(answer, Arc::clone(&cut));
        let mut into = [0u8; 64];
        let read = streamed.read(&mut into).expect("the first chunk");
        assert_eq!(&into[..read], b"hello");
        cut.store(true, Ordering::Relaxed);
        interrupter.interrupt();
        assert_eq!(
            streamed.read(&mut into).expect("the end, not a fault"),
            0,
            "切断したあとは、ソースの終端と同じく 0"
        );
        drop(server.join().expect("the server"));
    }
}
