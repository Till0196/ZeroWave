// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 待たない通信（`zw_net_v1` の `open`・`ws`・`poll`・`send`・`close`）。一つの接続に一つのスレッド。
//! HTTP は応答の本文を届いた分ずつ、WebSocket（RFC 6455）はメッセージごとに渡す。
//! guest は `poll` で届いた分を受け取り、何も無ければ待たずに 0 が返る。
//! `request` と同じく、`net` を宣言して真のときだけで、内部ネットワークへは `net_local` のときだけ。
//! 記録の形は [`crate::net::conn`]。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, channel, sync_channel};
use std::time::{Duration, Instant};

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::{HostState, Level, guest_bytes, guest_text};
use crate::net::conn::{kind, send as send_kind};
use crate::net::{
    CONNECT_TIMEOUT, Failure, Url, address_of, join, parse_url, request_head, upgrade_head,
};
use crate::slots::Slots;

/// 読み込みを待つ長さ。止める要求と、送るものがあるかをこの間隔で確かめる。
const TICK: Duration = Duration::from_millis(50);

/// HTTP の応答がこの間何も来なければ失敗にする（ストリームの応答も、ふつうは数十秒で何か来る）。
const HTTP_IDLE: Duration = Duration::from_secs(60);

const HEAD_MOST: usize = 64 << 10;

/// WebSocket の一つのメッセージの上限。
const MESSAGE_MOST: usize = 16 << 20;

/// 一回の `poll` で渡す上限。
const POLL_MOST: usize = 4 << 20;

/// guest が読まないうちに溜める記録の数。溢れたらスレッドが待つ。
const QUEUE: usize = 256;

const READ_SIZE: usize = 64 * 1024;

const ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub(crate) type Conns = Slots<Conn>;

#[derive(Debug)]
enum Piece {
    Head(u16, Vec<(String, String)>),
    Data(Vec<u8>),
    Text(Vec<u8>),
    End(u16, String),
    Fail(Failure),
}

impl Piece {
    fn encode(&self, out: &mut Vec<u8>) {
        let (kind, body): (u8, Vec<u8>) = match self {
            Self::Head(status, headers) => {
                let mut body = status.to_be_bytes().to_vec();
                let text = headers
                    .iter()
                    .map(|(name, value)| format!("{name}: {value}"))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                body.extend_from_slice(text.as_bytes());
                (kind::HEAD, body)
            }
            Self::Data(bytes) => (kind::DATA, bytes.clone()),
            Self::Text(bytes) => (kind::TEXT, bytes.clone()),
            Self::End(code, reason) => {
                let mut body = code.to_be_bytes().to_vec();
                body.extend_from_slice(reason.as_bytes());
                (kind::END, body)
            }
            Self::Fail(failure) => (kind::FAIL, failure.code().to_be_bytes().to_vec()),
        };
        out.push(kind);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
    }

    fn len(&self) -> usize {
        match self {
            Self::Data(bytes) | Self::Text(bytes) => bytes.len(),
            _ => 0,
        }
    }

    fn is_last(&self) -> bool {
        matches!(self, Self::End(..) | Self::Fail(_))
    }
}

enum Outgoing {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Close(u16, Vec<u8>),
}

/// guest が持つ接続。落とすとスレッドも止まる。
pub(crate) struct Conn {
    arrived: Receiver<Piece>,
    outgoing: Option<Sender<Outgoing>>,
    stop: Arc<AtomicBool>,
    /// 終わりを渡した。
    ended: bool,
    /// 次の `poll` に回す（上限を超えた分）。
    held: Option<Piece>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Conn {
    fn poll(&mut self) -> Option<Vec<u8>> {
        if self.ended {
            return None;
        }
        let mut out = Vec::new();
        let mut size = 0;
        loop {
            let piece = match self.held.take() {
                Some(piece) => piece,
                None => match self.arrived.try_recv() {
                    Ok(piece) => piece,
                    Err(TryRecvError::Empty) => break,
                    // スレッドが終わりを送らずに止まることは無いが、念のため。
                    Err(TryRecvError::Disconnected) => Piece::Fail(Failure::Disconnected),
                },
            };
            if !out.is_empty() && size + piece.len() > POLL_MOST {
                self.held = Some(piece);
                break;
            }
            size += piece.len();
            piece.encode(&mut out);
            if piece.is_last() {
                self.ended = true;
                break;
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn send(&self, outgoing: Outgoing) -> bool {
        self.outgoing
            .as_ref()
            .is_some_and(|sender| sender.send(outgoing).is_ok())
    }
}

struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    /// WebSocket なら `Sec-WebSocket-Protocol`（`,` 区切り）。
    protocols: Option<String>,
    inner: bool,
    tls: Arc<rustls::ClientConfig>,
}

fn spawn(state: &mut HostState, request: Request) -> u32 {
    let (sent, arrived) = sync_channel(QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let websocket = request.protocols.is_some();
    let (outgoing, to_send) = if websocket {
        let (outgoing, to_send) = channel();
        (Some(outgoing), Some(to_send))
    } else {
        (None, None)
    };
    let conn = Conn {
        arrived,
        outgoing,
        stop: Arc::clone(&stop),
        ended: false,
        held: None,
    };
    let handle = state.conns.put(conn);
    if handle == 0 {
        state.log(Level::Warning, "zw_net_v1: 同時に開ける接続の上限を超えた");
        return 0;
    }
    let out = Sink { sent, stop };
    let spawned = std::thread::Builder::new()
        .name(String::from("zw-net"))
        .spawn(move || match to_send {
            Some(to_send) => websocket_run(&request, &out, &to_send),
            None => http_run(&request, &out),
        });
    if spawned.is_err() {
        state.conns.take(handle);
        return 0;
    }
    handle
}

/// スレッドから guest へ。guest が落としたら送れなくなる。
struct Sink {
    sent: SyncSender<Piece>,
    stop: Arc<AtomicBool>,
}

impl Sink {
    fn put(&self, piece: Piece) -> bool {
        !self.stopped() && self.sent.send(piece).is_ok()
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// TCP を短い間隔で待ち、止める要求を確かめる。`wait` なら読めるまで（期限まで）待つ。
struct Line {
    tcp: TcpStream,
    stop: Arc<AtomicBool>,
    wait: bool,
    idle: Option<Duration>,
    last: Instant,
}

impl Line {
    fn check(&self) -> std::io::Result<()> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "止めた",
            ));
        }
        if self.idle.is_some_and(|idle| self.last.elapsed() > idle) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "応答が無い",
            ));
        }
        Ok(())
    }
}

fn is_tick(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

impl Read for Line {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.tcp.read(buf) {
                Ok(n) => {
                    self.last = Instant::now();
                    return Ok(n);
                }
                Err(error) if is_tick(&error) => {
                    if !self.wait {
                        return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.tcp.write(buf) {
                Err(error) if is_tick(&error) => continue,
                other => return other,
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

enum Pipe {
    Plain(Line),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, Line>>),
}

impl Pipe {
    fn line(&mut self) -> &mut Line {
        match self {
            Self::Plain(line) => line,
            Self::Tls(tls) => &mut tls.sock,
        }
    }

    /// 書くときは書き終えるまで待つ（TLS のレコードを途中で切らない）。
    fn write_all_now(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let wait = std::mem::replace(&mut self.line().wait, true);
        let written = self.write_all(bytes).and_then(|()| self.flush());
        self.line().wait = wait;
        written
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(line) => line.read(buf),
            Self::Tls(tls) => match tls.read(buf) {
                // 相手が close_notify を送らずに閉じた。応答の終わりとして扱う。
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
                other => other,
            },
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(line) => line.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(line) => line.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

fn io_failure(error: &std::io::Error) -> Failure {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Failure::Timeout,
        _ => Failure::Disconnected,
    }
}

fn connect(url: &Url, request: &Request, stop: &Arc<AtomicBool>) -> Result<Pipe, Failure> {
    let address = address_of(&url.host, url.port, request.inner)?;
    let tcp = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|error| {
        if error.kind() == std::io::ErrorKind::TimedOut {
            Failure::Timeout
        } else {
            Failure::Connect
        }
    })?;
    let _ = tcp.set_read_timeout(Some(TICK));
    let _ = tcp.set_write_timeout(Some(TICK));
    let _ = tcp.set_nodelay(true);
    let line = Line {
        tcp,
        stop: Arc::clone(stop),
        wait: true,
        idle: Some(HTTP_IDLE),
        last: Instant::now(),
    };
    if !url.secure {
        return Ok(Pipe::Plain(line));
    }
    let name =
        rustls::pki_types::ServerName::try_from(url.host.clone()).map_err(|_| Failure::Tls)?;
    let connection =
        rustls::ClientConnection::new(Arc::clone(&request.tls), name).map_err(|_| Failure::Tls)?;
    let mut tls = rustls::StreamOwned::new(connection, line);
    while tls.conn.is_handshaking() {
        if let Err(error) = tls.conn.complete_io(&mut tls.sock) {
            return Err(match error.kind() {
                std::io::ErrorKind::TimedOut => Failure::Timeout,
                std::io::ErrorKind::Interrupted => Failure::Disconnected,
                _ => Failure::Tls,
            });
        }
    }
    Ok(Pipe::Tls(Box::new(tls)))
}

/// 応答の頭（status 行とヘッダー）。`rest` は頭の後ろに読んでしまった分。
#[derive(Debug, PartialEq, Eq)]
struct Head {
    status: u16,
    headers: Vec<(String, String)>,
    rest: Vec<u8>,
}

impl Head {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 空行まで。`None` はまだ足りない。
fn parse_head(bytes: &[u8]) -> Result<Option<Head>, Failure> {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        if bytes.len() > HEAD_MOST {
            return Err(Failure::Disconnected);
        }
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or(Failure::Disconnected)?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Ok(Some(Head {
        status,
        headers,
        rest: bytes[end + 4..].to_vec(),
    }))
}

fn read_head(pipe: &mut Pipe) -> Result<Head, Failure> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; READ_SIZE];
    loop {
        if let Some(head) = parse_head(&bytes)? {
            return Ok(head);
        }
        match pipe.read(&mut chunk) {
            Ok(0) => return Err(Failure::Disconnected),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(error) => return Err(io_failure(&error)),
        }
    }
}

/// `Transfer-Encoding: chunked` を届いた分ずつ解く。
#[derive(Debug, Default)]
struct Chunks {
    state: ChunkState,
    line: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ChunkState {
    #[default]
    Size,
    Data(u64),
    /// データの後ろの CRLF。
    DataEnd,
    Trailer,
    Done,
}

impl Chunks {
    /// 解いた本文を `out` に足す。
    fn feed(&mut self, mut bytes: &[u8], out: &mut Vec<u8>) -> Result<(), Failure> {
        while !bytes.is_empty() && self.state != ChunkState::Done {
            match self.state {
                ChunkState::Data(left) => {
                    let n = (left as usize).min(bytes.len());
                    out.extend_from_slice(&bytes[..n]);
                    bytes = &bytes[n..];
                    self.state = if left as usize == n {
                        ChunkState::DataEnd
                    } else {
                        ChunkState::Data(left - n as u64)
                    };
                }
                _ => {
                    let Some(at) = bytes.iter().position(|&b| b == b'\n') else {
                        self.line.extend_from_slice(bytes);
                        if self.line.len() > 8 << 10 {
                            return Err(Failure::Disconnected);
                        }
                        return Ok(());
                    };
                    self.line.extend_from_slice(&bytes[..at]);
                    bytes = &bytes[at + 1..];
                    let line = String::from_utf8_lossy(&self.line).trim().to_owned();
                    self.line.clear();
                    self.state = match self.state {
                        ChunkState::Size => {
                            let size = u64::from_str_radix(
                                line.split(';').next().unwrap_or("").trim(),
                                16,
                            )
                            .map_err(|_| Failure::Disconnected)?;
                            if size == 0 {
                                ChunkState::Trailer
                            } else {
                                ChunkState::Data(size)
                            }
                        }
                        ChunkState::DataEnd => ChunkState::Size,
                        _ if line.is_empty() => ChunkState::Done,
                        _ => ChunkState::Trailer,
                    };
                }
            }
        }
        Ok(())
    }

    fn done(&self) -> bool {
        self.state == ChunkState::Done
    }
}

fn http_run(request: &Request, out: &Sink) {
    let piece = match http_body(request, out) {
        Ok(()) => Piece::End(0, String::new()),
        Err(failure) => Piece::Fail(failure),
    };
    out.put(piece);
}

/// 転送は五回まで（POST の 303 は GET に）。https から http へは転送しない。
fn http_body(request: &Request, out: &Sink) -> Result<(), Failure> {
    let mut at = request.url.clone();
    let mut method = request.method.clone();
    let mut body = request.body.clone();
    for _ in 0..6 {
        let url = parse_url(&at).ok_or(Failure::Argument)?;
        let head = request_head(&method, &url, &request.headers, body.as_ref().map(Vec::len))?;
        let mut pipe = connect(&url, request, &out.stop)?;
        pipe.write_all_now(head.as_bytes())
            .and_then(|()| pipe.write_all_now(body.as_deref().unwrap_or(&[])))
            .map_err(|error| io_failure(&error))?;
        let head = read_head(&mut pipe)?;
        let moved = matches!(head.status, 301 | 302 | 303 | 307 | 308);
        if let Some(location) = head.get("location").filter(|_| moved) {
            if head.status == 303 || (matches!(head.status, 301 | 302) && method == "POST") {
                method = String::from("GET");
                body = None;
            }
            let next = join(&at, location).unwrap_or_else(|| location.to_owned());
            if at.starts_with("https://") && !next.starts_with("https://") {
                return Err(Failure::Tls);
            }
            at = next;
            continue;
        }
        return stream_body(&mut pipe, head, out);
    }
    Err(Failure::Connect)
}

fn stream_body(pipe: &mut Pipe, head: Head, out: &Sink) -> Result<(), Failure> {
    let chunked = head
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    let mut left: Option<u64> = if chunked {
        None
    } else {
        head.get("content-length").and_then(|n| n.parse().ok())
    };
    let mut chunks = chunked.then(Chunks::default);
    // 長さも chunked も無ければ、相手が閉じるまでが本文。
    let until_close = chunks.is_none() && left.is_none();
    let Head {
        status,
        headers,
        rest,
    } = head;
    if !out.put(Piece::Head(status, headers)) {
        return Ok(());
    }
    let mut give = |bytes: &[u8]| -> Result<bool, Failure> {
        let mut body = Vec::new();
        match (&mut chunks, &mut left) {
            (Some(chunks), _) => chunks.feed(bytes, &mut body)?,
            (None, Some(left)) => {
                let n = (*left as usize).min(bytes.len());
                body.extend_from_slice(&bytes[..n]);
                *left -= n as u64;
            }
            (None, None) => body.extend_from_slice(bytes),
        }
        if !body.is_empty() && !out.put(Piece::Data(body)) {
            return Ok(true);
        }
        Ok(chunks.as_ref().is_some_and(Chunks::done) || left == Some(0))
    };
    if give(&rest)? {
        return Ok(());
    }
    let mut chunk = vec![0u8; READ_SIZE];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) if until_close => return Ok(()),
            Ok(0) => return Err(Failure::Disconnected),
            Ok(n) => {
                if give(&chunk[..n])? {
                    return Ok(());
                }
            }
            Err(error) => return Err(io_failure(&error)),
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= group.len() {
                out.push(char::from(TABLE[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn random<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; N];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    bytes
}

fn accept_of(key: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        format!("{key}{ACCEPT_GUID}").as_bytes(),
    );
    base64(digest.as_ref())
}

/// client から送るフレームはマスクする（RFC 6455 5.3）。
fn frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

#[derive(Debug, PartialEq, Eq)]
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// 一つのフレームを取り出す。`None` はまだ足りない。
fn take_frame(buf: &mut Vec<u8>) -> Result<Option<Frame>, Failure> {
    let [first, second, ..] = buf[..] else {
        return Ok(None);
    };
    let masked = second & 0x80 != 0;
    let (len, mut at) = match second & 0x7f {
        126 => {
            let Some(n) = buf.get(2..4) else {
                return Ok(None);
            };
            (u64::from(u16::from_be_bytes([n[0], n[1]])), 4)
        }
        127 => {
            let Some(n) = buf.get(2..10) else {
                return Ok(None);
            };
            (u64::from_be_bytes(n.try_into().unwrap_or_default()), 10)
        }
        n => (u64::from(n), 2),
    };
    if len > MESSAGE_MOST as u64 {
        return Err(Failure::Disconnected);
    }
    let mask = if masked {
        let Some(m) = buf.get(at..at + 4) else {
            return Ok(None);
        };
        at += 4;
        Some([m[0], m[1], m[2], m[3]])
    } else {
        None
    };
    let end = at + len as usize;
    if buf.len() < end {
        return Ok(None);
    }
    let mut payload = buf[at..end].to_vec();
    if let Some(mask) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    buf.drain(..end);
    Ok(Some(Frame {
        fin: first & 0x80 != 0,
        opcode: first & 0x0f,
        payload,
    }))
}

fn websocket_run(request: &Request, out: &Sink, to_send: &Receiver<Outgoing>) {
    let piece = match websocket(request, out, to_send) {
        Ok((code, reason)) => Piece::End(code, reason),
        Err(failure) => Piece::Fail(failure),
    };
    out.put(piece);
}

fn websocket(
    request: &Request,
    out: &Sink,
    to_send: &Receiver<Outgoing>,
) -> Result<(u16, String), Failure> {
    let http = if let Some(rest) = request.url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = request.url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        return Err(Failure::Argument);
    };
    let url = parse_url(&http).ok_or(Failure::Argument)?;
    let key = base64(&random::<16>());
    let head = upgrade_head(
        &url,
        &request.headers,
        &key,
        request.protocols.as_deref().unwrap_or(""),
    )?;
    let mut pipe = connect(&url, request, &out.stop)?;
    pipe.write_all_now(head.as_bytes())
        .map_err(|error| io_failure(&error))?;
    let head = read_head(&mut pipe)?;
    if head.status != 101
        || !head
            .get("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
        || head.get("sec-websocket-accept") != Some(accept_of(&key).as_str())
    {
        tracing::debug!("WebSocket のハンドシェイクに失敗: {}", head.status);
        return Err(Failure::Connect);
    }
    let Head {
        status,
        headers,
        rest,
    } = head;
    if !out.put(Piece::Head(status, headers)) {
        return Ok((1001, String::new()));
    }
    let line = pipe.line();
    line.wait = false;
    line.idle = None;
    let mut buf = rest;
    let mut message: Option<(u8, Vec<u8>)> = None;
    let mut closing = false;
    let mut chunk = vec![0u8; READ_SIZE];
    loop {
        if out.stopped() {
            if !closing {
                let _ = pipe.write_all_now(&frame(0x8, &1001u16.to_be_bytes(), random()));
            }
            return Ok((1001, String::new()));
        }
        while let Ok(outgoing) = to_send.try_recv() {
            if closing {
                continue;
            }
            let bytes = match outgoing {
                Outgoing::Text(text) => frame(0x1, &text, random()),
                Outgoing::Binary(data) => frame(0x2, &data, random()),
                Outgoing::Close(code, reason) => {
                    closing = true;
                    let mut payload = code.to_be_bytes().to_vec();
                    payload.extend_from_slice(&reason[..reason.len().min(123)]);
                    frame(0x8, &payload, random())
                }
            };
            pipe.write_all_now(&bytes)
                .map_err(|error| io_failure(&error))?;
        }
        while let Some(frame_in) = take_frame(&mut buf)? {
            match frame_in.opcode {
                0x0 => {
                    let Some((_, bytes)) = message.as_mut() else {
                        return Err(Failure::Disconnected);
                    };
                    if bytes.len() + frame_in.payload.len() > MESSAGE_MOST {
                        return Err(Failure::Disconnected);
                    }
                    bytes.extend_from_slice(&frame_in.payload);
                }
                0x1 | 0x2 => {
                    if message.is_some() {
                        return Err(Failure::Disconnected);
                    }
                    message = Some((frame_in.opcode, frame_in.payload));
                }
                0x8 => {
                    let code = frame_in
                        .payload
                        .get(..2)
                        .map_or(1005, |c| u16::from_be_bytes([c[0], c[1]]));
                    let reason = String::from_utf8_lossy(frame_in.payload.get(2..).unwrap_or(&[]))
                        .into_owned();
                    if !closing {
                        let _ = pipe.write_all_now(&frame(0x8, &code.to_be_bytes(), random()));
                    }
                    return Ok((code, reason));
                }
                0x9 => {
                    pipe.write_all_now(&frame(0xA, &frame_in.payload, random()))
                        .map_err(|error| io_failure(&error))?;
                    continue;
                }
                0xA => continue,
                _ => return Err(Failure::Disconnected),
            }
            if frame_in.fin
                && let Some((opcode, bytes)) = message.take()
            {
                let piece = if opcode == 0x1 {
                    Piece::Text(bytes)
                } else {
                    Piece::Data(bytes)
                };
                if !out.put(piece) {
                    return Ok((1001, String::new()));
                }
            }
        }
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok((1006, String::new())),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_failure(&error)),
        }
    }
}

/// `名前: 値` の行（`request` と同じ形）。
fn headers_of(state: &HostState, buf: u32) -> Vec<(String, String)> {
    state
        .bufs
        .get(buf)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (k, v) = line.split_once(':')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect()
}

fn refused(state: &HostState) -> bool {
    if state.io.net_allowed() {
        return false;
    }
    state.log(Level::Warning, "zw_net_v1: net が有効でないので接続しない");
    true
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::NET,
        "open",
        |mut caller: Caller<'_, HostState>,
         method_ptr: u32,
         method_len: u32,
         url_ptr: u32,
         url_len: u32,
         headers: u32,
         body: u32|
         -> u32 {
            let method = guest_text(&mut caller, method_ptr, method_len);
            let url = guest_text(&mut caller, url_ptr, url_len);
            let state = caller.data_mut();
            if refused(state) {
                return 0;
            }
            let request = Request {
                method,
                url,
                headers: headers_of(state, headers),
                body: state.bufs.get(body).cloned(),
                protocols: None,
                inner: state.io.net_inner(),
                tls: state.net.tls_config(),
            };
            spawn(state, request)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "ws",
        |mut caller: Caller<'_, HostState>,
         url_ptr: u32,
         url_len: u32,
         protocols_ptr: u32,
         protocols_len: u32,
         headers: u32|
         -> u32 {
            let url = guest_text(&mut caller, url_ptr, url_len);
            let protocols = guest_text(&mut caller, protocols_ptr, protocols_len);
            let state = caller.data_mut();
            if refused(state) {
                return 0;
            }
            let request = Request {
                method: String::from("GET"),
                url,
                headers: headers_of(state, headers),
                body: None,
                protocols: Some(protocols),
                inner: state.io.net_inner(),
                tls: state.net.tls_config(),
            };
            spawn(state, request)
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "poll",
        |mut caller: Caller<'_, HostState>, handle: u32| -> u32 {
            let state = caller.data_mut();
            match state.conns.get_mut(handle).and_then(Conn::poll) {
                Some(bytes) => state.put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "send",
        |mut caller: Caller<'_, HostState>, handle: u32, kind: u32, ptr: u32, len: u32| -> u32 {
            let Some(bytes) = guest_bytes(&mut caller, ptr, len) else {
                return 0;
            };
            let outgoing = match kind {
                send_kind::TEXT => Outgoing::Text(bytes),
                send_kind::BINARY => Outgoing::Binary(bytes),
                send_kind::CLOSE => {
                    let code = bytes
                        .get(..2)
                        .map_or(1000, |c| u16::from_be_bytes([c[0], c[1]]));
                    Outgoing::Close(code, bytes.get(2..).unwrap_or(&[]).to_vec())
                }
                _ => return 0,
            };
            caller
                .data()
                .conns
                .get(handle)
                .is_some_and(|conn| conn.send(outgoing))
                .into()
        },
    )?;
    linker.func_wrap(
        abi::import::NET,
        "close",
        |mut caller: Caller<'_, HostState>, handle: u32| {
            caller.data_mut().conns.take(handle);
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::conn::Arrived;
    use std::net::TcpListener;

    #[test]
    fn base64_matches_the_rfc_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_accept_key_is_the_one_in_rfc_6455() {
        assert_eq!(
            accept_of("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn chunks_are_decoded_however_they_are_split() {
        let wire = b"3\r\nabc\r\n2;x=1\r\nde\r\n0\r\nX: y\r\n\r\n";
        for step in 1..wire.len() {
            let mut chunks = Chunks::default();
            let mut out = Vec::new();
            for part in wire.chunks(step) {
                chunks.feed(part, &mut out).unwrap();
            }
            assert_eq!(out, b"abcde", "{step}");
            assert!(chunks.done(), "{step}");
        }
    }

    #[test]
    fn frames_are_taken_when_whole() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x81, 0x05]);
        wire.extend_from_slice(b"hello");
        wire.extend_from_slice(&[0x02, 126, 0x01, 0x00]);
        wire.extend(std::iter::repeat_n(7u8, 256));
        let mut buf = wire[..4].to_vec();
        assert_eq!(take_frame(&mut buf).unwrap(), None);
        buf.extend_from_slice(&wire[4..]);
        assert_eq!(
            take_frame(&mut buf).unwrap(),
            Some(Frame {
                fin: true,
                opcode: 1,
                payload: b"hello".to_vec()
            })
        );
        let second = take_frame(&mut buf).unwrap().unwrap();
        assert_eq!(
            (second.fin, second.opcode, second.payload.len()),
            (false, 2, 256)
        );
        assert!(buf.is_empty());
        let mut masked = frame(0x1, b"hi", [1, 2, 3, 4]);
        assert_eq!(
            take_frame(&mut masked).unwrap().map(|f| f.payload),
            Some(b"hi".to_vec())
        );
    }

    fn request(url: String, protocols: Option<String>) -> Request {
        Request {
            method: String::from("GET"),
            url,
            headers: vec![(String::from("X-Test"), String::from("1"))],
            body: None,
            protocols,
            inner: true,
            tls: crate::net::Session::new().tls_config(),
        }
    }

    fn collect(arrived: &Receiver<Piece>) -> Vec<Piece> {
        let mut pieces = Vec::new();
        while let Ok(piece) = arrived.recv_timeout(Duration::from_secs(5)) {
            let last = piece.is_last();
            pieces.push(piece);
            if last {
                break;
            }
        }
        pieces
    }

    fn sink() -> (Sink, Receiver<Piece>) {
        let (sent, arrived) = sync_channel(QUEUE);
        (
            Sink {
                sent,
                stop: Arc::new(AtomicBool::new(false)),
            },
            arrived,
        )
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut got = Vec::new();
        let mut byte = [0u8; 1];
        while !got.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            got.push(byte[0]);
        }
        String::from_utf8(got).unwrap()
    }

    #[test]
    fn a_chunked_reply_arrives_piece_by_piece() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let serving = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let head = read_request(&mut stream);
            assert!(head.starts_with("GET /s HTTP/1.1\r\n"), "{head}");
            assert!(head.contains("X-Test: 1\r\n"), "{head}");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(200));
            stream.write_all(b"2\r\nde\r\n0\r\n\r\n").unwrap();
        });
        let (out, arrived) = sink();
        http_run(&request(format!("http://127.0.0.1:{port}/s"), None), &out);
        serving.join().unwrap();
        let pieces = collect(&arrived);
        assert!(matches!(&pieces[0], Piece::Head(200, _)), "{pieces:?}");
        let body: Vec<u8> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Data(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(body, b"abcde");
        assert!(pieces.len() >= 4, "二回に分けて届く: {pieces:?}");
        assert!(
            matches!(pieces.last(), Some(Piece::End(0, _))),
            "{pieces:?}"
        );
    }

    #[test]
    fn a_redirect_is_followed() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let serving = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            drop(stream);
            let (mut stream, _) = server.accept().unwrap();
            let head = read_request(&mut stream);
            assert!(head.starts_with("GET /b "), "{head}");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
        });
        let (out, arrived) = sink();
        http_run(&request(format!("http://127.0.0.1:{port}/a"), None), &out);
        serving.join().unwrap();
        let pieces = collect(&arrived);
        assert!(matches!(&pieces[0], Piece::Head(200, _)), "{pieces:?}");
        assert!(
            matches!(&pieces[1], Piece::Data(b) if b == b"ok"),
            "{pieces:?}"
        );
    }

    #[test]
    fn a_websocket_echoes_and_closes() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let serving = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let head = read_request(&mut stream);
            assert!(head.contains("Upgrade: websocket\r\n"), "{head}");
            assert!(head.contains("Connection: Upgrade\r\n"), "{head}");
            assert!(head.contains("Sec-WebSocket-Protocol: chat\r\n"), "{head}");
            let key = head
                .lines()
                .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap()
                .trim()
                .to_owned();
            let reply = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                accept_of(&key)
            );
            stream.write_all(reply.as_bytes()).unwrap();
            // 分けて送る（continuation）。
            stream.write_all(&[0x01, 0x02, b'h', b'e']).unwrap();
            stream.write_all(&[0x80, 0x03, b'l', b'l', b'o']).unwrap();
            stream.write_all(&[0x89, 0x01, b'p']).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 256];
            let mut seen = Vec::new();
            while seen.len() < 2 {
                let n = stream.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                while let Some(f) = take_frame(&mut buf).unwrap() {
                    seen.push(f);
                }
            }
            // 送るものは届いた ping より先に出ることがある。
            seen.sort_by_key(|f| f.opcode);
            assert_eq!(
                (seen[0].opcode, seen[0].payload.as_slice()),
                (0x1, &b"hi"[..])
            );
            assert_eq!(
                (seen[1].opcode, seen[1].payload.as_slice()),
                (0xA, &b"p"[..])
            );
            stream.write_all(&[0x88, 0x02, 0x03, 0xe8]).unwrap();
        });
        let (out, arrived) = sink();
        let (to, to_send) = channel();
        to.send(Outgoing::Text(b"hi".to_vec())).unwrap();
        let req = request(
            format!("ws://127.0.0.1:{port}/w"),
            Some(String::from("chat")),
        );
        let running = std::thread::spawn(move || websocket_run(&req, &out, &to_send));
        let pieces = collect(&arrived);
        running.join().unwrap();
        serving.join().unwrap();
        assert!(matches!(&pieces[0], Piece::Head(101, _)), "{pieces:?}");
        assert!(
            matches!(&pieces[1], Piece::Text(b) if b == b"hello"),
            "{pieces:?}"
        );
        assert!(matches!(&pieces[2], Piece::End(1000, _)), "{pieces:?}");
    }

    #[test]
    fn polling_hands_records_and_stops_after_the_end() {
        let (sent, arrived) = sync_channel(QUEUE);
        let mut conn = Conn {
            arrived,
            outgoing: None,
            stop: Arc::new(AtomicBool::new(false)),
            ended: false,
            held: None,
        };
        assert_eq!(conn.poll(), None);
        sent.send(Piece::Head(
            200,
            vec![(String::from("A"), String::from("b"))],
        ))
        .unwrap();
        sent.send(Piece::Data(b"xy".to_vec())).unwrap();
        sent.send(Piece::End(0, String::new())).unwrap();
        let bytes = conn.poll().unwrap();
        assert_eq!(&bytes[..5], &[kind::HEAD, 0, 0, 0, 6]);
        assert_eq!(&bytes[5..11], b"\x00\xc8A: b");
        assert_eq!(&bytes[11..18], &[kind::DATA, 0, 0, 0, 2, b'x', b'y']);
        assert_eq!(&bytes[18..], &[kind::END, 0, 0, 0, 2, 0, 0]);
        assert_eq!(
            crate::net::conn::decode(&bytes),
            [
                Arrived::Head {
                    status: 200,
                    headers: vec![(String::from("A"), String::from("b"))]
                },
                Arrived::Data(b"xy".to_vec()),
                Arrived::End {
                    code: 0,
                    reason: String::new()
                }
            ]
        );
        assert_eq!(conn.poll(), None, "終わりの後は無い");
    }
}
