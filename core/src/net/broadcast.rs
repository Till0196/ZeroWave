// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインの通信。既定では内部ネットワークへ繋がず、選局で全部止める（文書のスレッドは同期で待つので、
//! 止めないと前の局の文書が残る）。

use std::io::Read as _;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use zwsdk::net::conn::{Arrived, Open, Outgoing};
use zwsdk::net::{Failure, Reply, TIMEOUT, Url, parse_url};

use super::address::addresses_of;
use super::http::{Client, Options, Request};
use super::{Error, Stop, tls, ws};

/// 応答の本文の上限。
const MOST: usize = 16 << 20;
/// 放送で届くルート証明書の、一つのプラグインあたりの上限。
const MOST_ROOTS: usize = 8;
const CONNECT_PATIENCE: Duration = Duration::from_secs(3);
/// 待たない HTTP は、この間何も届かなければ失敗にする（ストリームの応答も、ふつうは数十秒で何か来る）。
const OPEN_PATIENCE: Duration = Duration::from_secs(60);
/// 読まれないうちに溜める記録の数と大きさ。どちらかを超えたらスレッドが待つ。
const QUEUE: usize = 256;
const QUEUE_BYTES: usize = 32 << 20;
/// プラグインが送る WebSocket のメッセージを、送り出すまで溜める数。超えたら送れない。
const OUTGOING: usize = 64;
const READ_SIZE: usize = 64 << 10;
/// WebSocket で何も来ないときに、次を見るまでの間。
const TICK: Duration = Duration::from_millis(10);

static HOLD: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 選局が始まった。通信を全部止め、放送で届いたルート証明書を捨てる。
pub fn hold() {
    HOLD.store(true, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

pub fn release() {
    HOLD.store(false, Ordering::SeqCst);
}

fn held() -> bool {
    HOLD.load(Ordering::SeqCst)
}

fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// 何度呼んでもよい。
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| zwsdk::net::install(Arc::new(Broadcast)));
}

#[derive(Debug)]
struct Broadcast;

impl zwsdk::net::Network for Broadcast {
    /// UDP ソケットを外向きに connect して経路があるかを見るだけ。ローカルアドレスは見ない（Windows は未定のまま返す）。
    fn has_ip(&self) -> bool {
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            return false;
        };
        // TEST-NET だけだとその宛先を破棄するマシンで偽になる。
        ["192.0.2.1:9", "8.8.8.8:53", "1.1.1.1:53"]
            .iter()
            .any(|target| {
                socket.connect(target).is_ok()
                    && socket
                        .local_addr()
                        .is_ok_and(|address| !address.ip().is_loopback())
            })
    }

    fn resolve(&self, host: &str) -> Option<String> {
        (host, 0u16)
            .to_socket_addrs()
            .ok()?
            .next()
            .map(|address| address.ip().to_string())
    }

    fn reach(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        inner: bool,
    ) -> Option<(String, u32)> {
        if held() {
            return None;
        }
        let address = *addresses_of(host, port, inner).ok()?.first()?;
        let began = Instant::now();
        TcpStream::connect_timeout(&address, timeout.min(CONNECT_PATIENCE)).ok()?;
        Some((
            address.ip().to_string(),
            began.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
        ))
    }

    fn session(&self) -> Box<dyn zwsdk::net::Session> {
        Box::new(Session::default())
    }
}

/// （選局の回、ルートの数、内部ネットワークを許すか、待つ長さ）が変わったら作り直す。
type Made = (u64, usize, bool, Duration);

/// 一つのプラグインの通信。放送で届いたものはこの中だけで使い、選局で捨てる。
#[derive(Debug, Default)]
pub struct Session {
    roots: Mutex<Vec<(u64, Vec<u8>)>>,
    clients: Mutex<Vec<(Made, Client)>>,
}

impl Session {
    fn roots(&self) -> Vec<Vec<u8>> {
        let now = generation();
        let mut roots = self.roots.lock().unwrap_or_else(|e| e.into_inner());
        roots.retain(|(at, _)| *at == now);
        roots.iter().map(|(_, der)| der.clone()).collect()
    }

    fn client(&self, inner: bool, patience: Duration) -> Result<Client, Failure> {
        let roots = self.roots();
        let key = (generation(), roots.len(), inner, patience);
        let mut cached = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        cached.retain(|((at, count, ..), _)| *at == key.0 && *count == key.1);
        if let Some((_, client)) = cached.iter().find(|(had, _)| *had == key) {
            return Ok(client.clone());
        }
        let client = Client::new(&Options {
            inner,
            patience,
            roots,
            // 古い局のサーバーのため。
            sha1: true,
            agent: None,
        })
        .map_err(failure)?;
        cached.push((key, client.clone()));
        Ok(client)
    }
}

impl zwsdk::net::Session for Session {
    fn send(
        &self,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        inner: bool,
    ) -> Result<Reply, Failure> {
        if held() {
            return Err(Failure::Disconnected);
        }
        let url = parse_url(uri).ok_or(Failure::Argument)?;
        let target = target(method, &url)?;
        let headers = kept(headers, body.is_some());
        let client = self.client(inner, TIMEOUT)?;
        let mut request = Request::new(method, &target)
            .stop(stop())
            .deadline(Instant::now() + TIMEOUT);
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let mut answer = client.send(request).map_err(failure)?;
        let body = answer.read_most(MOST).map_err(failure)?;
        Ok(Reply {
            status: answer.status,
            content_type: answer.header("content-type").map(str::to_owned),
            location: answer.header("location").map(str::to_owned),
            body,
            headers: answer.headers,
        })
    }

    fn add_root_certificate(&self, der: &[u8]) -> bool {
        let now = generation();
        let mut roots = self.roots.lock().unwrap_or_else(|e| e.into_inner());
        roots.retain(|(at, _)| *at == now);
        if roots.iter().any(|(_, had)| had == der)
            || roots.len() >= MOST_ROOTS
            || !tls::is_certificate(der)
        {
            return false;
        }
        roots.push((now, der.to_vec()));
        true
    }

    fn open(&self, open: Open) -> Box<dyn zwsdk::net::Conn> {
        let (sent, arrived) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicBool::new(false));
        let queued = Arc::new((Mutex::new(0usize), Condvar::new()));
        let out = Sink {
            sent,
            dropped: Arc::clone(&dropped),
            queued: Arc::clone(&queued),
            generation: generation(),
        };
        let websocket = open.protocols.is_some();
        let (outgoing, to_send) = sync_channel(OUTGOING);
        let started = if websocket {
            let tls = tls::config(&self.roots(), true);
            std::thread::Builder::new()
                .name(String::from("zw-net"))
                .spawn(move || websocket_run(&open, tls, &out, &to_send))
        } else {
            let client = self.client(open.inner, OPEN_PATIENCE);
            std::thread::Builder::new()
                .name(String::from("zw-net"))
                .spawn(move || http_run(&open, client, &out))
        };
        Box::new(Conn {
            arrived,
            outgoing: websocket.then_some(outgoing),
            dropped,
            queued,
            failed: started.is_err(),
        })
    }
}

/// 落とすとスレッドも止まる。
#[derive(Debug)]
struct Conn {
    arrived: Receiver<Arrived>,
    outgoing: Option<SyncSender<Outgoing>>,
    dropped: Arc<AtomicBool>,
    /// 届いて、まだ渡していないバイト数。
    queued: Arc<(Mutex<usize>, Condvar)>,
    /// スレッドを立てられなかった。
    failed: bool,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.queued.1.notify_all();
    }
}

impl zwsdk::net::Conn for Conn {
    fn next(&mut self) -> Option<Arrived> {
        if std::mem::take(&mut self.failed) {
            return Some(Arrived::Fail(Failure::Connect.code()));
        }
        match self.arrived.try_recv() {
            Ok(arrived) => {
                let (bytes, room) = &*self.queued;
                let mut bytes = bytes.lock().unwrap_or_else(|e| e.into_inner());
                *bytes = bytes.saturating_sub(arrived.len());
                room.notify_all();
                Some(arrived)
            }
            Err(TryRecvError::Empty) => None,
            // 終わりを送らずに止まることは無いが、念のため。
            Err(TryRecvError::Disconnected) => Some(Arrived::Fail(Failure::Disconnected.code())),
        }
    }

    fn send(&mut self, outgoing: Outgoing) -> bool {
        self.outgoing.as_ref().is_some_and(|sender| {
            !matches!(
                sender.try_send(outgoing),
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
            )
        })
    }
}

/// スレッドから受け取る側へ。落とされたら送れなくなる。
struct Sink {
    sent: SyncSender<Arrived>,
    dropped: Arc<AtomicBool>,
    queued: Arc<(Mutex<usize>, Condvar)>,
    generation: u64,
}

impl Sink {
    /// 溜まりすぎていれば、読まれるか落とされるまで待つ。
    fn put(&self, arrived: Arrived) -> bool {
        let size = arrived.len();
        {
            let (bytes, room) = &*self.queued;
            let mut bytes = bytes.lock().unwrap_or_else(|e| e.into_inner());
            while *bytes > 0 && *bytes + size > QUEUE_BYTES {
                if self.dropped() || self.moved_on() {
                    return false;
                }
                bytes = room
                    .wait_timeout(bytes, Duration::from_millis(100))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            *bytes += size;
        }
        !self.dropped() && self.sent.send(arrived).is_ok()
    }

    fn dropped(&self) -> bool {
        self.dropped.load(Ordering::SeqCst)
    }

    /// 選局した。
    fn moved_on(&self) -> bool {
        held() || generation() != self.generation
    }

    fn stop(&self) -> Stop {
        let dropped = Arc::clone(&self.dropped);
        let at = self.generation;
        Arc::new(move || dropped.load(Ordering::SeqCst) || held() || generation() != at)
    }
}

fn http_run(open: &Open, client: Result<Client, Failure>, out: &Sink) {
    let last = match http(open, client, out) {
        Ok(()) => Arrived::End {
            code: 0,
            reason: String::new(),
        },
        Err(failure) => Arrived::Fail(failure.code()),
    };
    out.put(last);
}

fn http(open: &Open, client: Result<Client, Failure>, out: &Sink) -> Result<(), Failure> {
    if out.moved_on() {
        return Err(Failure::Disconnected);
    }
    let url = parse_url(&open.url).ok_or(Failure::Argument)?;
    let target = target(&open.method, &url)?;
    let headers = kept(&open.headers, open.body.is_some());
    let mut request = Request::new(&open.method, &target).stop(out.stop());
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    if let Some(body) = open.body.as_deref() {
        request = request.body(body);
    }
    let mut answer = client?.send(request).map_err(failure)?;
    let head = Arrived::Head {
        status: answer.status,
        headers: std::mem::take(&mut answer.headers),
    };
    if !out.put(head) {
        return Ok(());
    }
    let mut chunk = vec![0u8; READ_SIZE];
    loop {
        match answer.read(&mut chunk) {
            Ok(0) if out.moved_on() => return Err(Failure::Disconnected),
            Ok(0) => return Ok(()),
            Ok(read) => {
                if !out.put(Arrived::Data(chunk[..read].to_vec())) {
                    return Ok(());
                }
            }
            Err(why) => return Err(failure(read_error(why))),
        }
    }
}

fn read_error(why: std::io::Error) -> Error {
    match why.into_inner().map(|inside| inside.downcast::<Error>()) {
        Some(Ok(error)) => *error,
        Some(Err(other)) => Error::Closed(other.to_string()),
        None => Error::Closed(String::from("読めない")),
    }
}

fn websocket_run(
    open: &Open,
    tls: Arc<rustls::ClientConfig>,
    out: &Sink,
    to_send: &Receiver<Outgoing>,
) {
    let last = match websocket(open, tls, out, to_send) {
        Ok((code, reason)) => Arrived::End { code, reason },
        Err(failure) => Arrived::Fail(failure.code()),
    };
    out.put(last);
}

fn websocket(
    open: &Open,
    tls: Arc<rustls::ClientConfig>,
    out: &Sink,
    to_send: &Receiver<Outgoing>,
) -> Result<(u16, String), Failure> {
    if out.moved_on() {
        return Err(Failure::Disconnected);
    }
    // ユーザー情報は送らせない（`request` と同じ）。書き方の揺れ（`\` など）は URL として読んでから確かめる。
    let url = url::Url::parse(&open.url).map_err(|_| Failure::Argument)?;
    if !matches!(url.scheme(), "ws" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Failure::Argument);
    }
    let mut headers: Vec<(String, String)> = kept(&open.headers, false)
        .into_iter()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            name != "upgrade" && !name.starts_with("sec-websocket-")
        })
        .collect();
    let protocols = open.protocols.as_deref().unwrap_or("").trim();
    if !protocols.is_empty() {
        headers.push((String::from("Sec-WebSocket-Protocol"), protocols.to_owned()));
    }
    let stop = out.stop();
    let mut socket =
        ws::Socket::open(url.as_str(), &headers, open.inner, tls, &*stop).map_err(failure)?;
    let head = Arrived::Head {
        status: 101,
        headers: std::mem::take(&mut socket.headers),
    };
    if !out.put(head) {
        return Ok((1001, String::new()));
    }
    loop {
        if out.dropped() || out.moved_on() {
            socket.close();
            return Ok((1001, String::new()));
        }
        while let Ok(outgoing) = to_send.try_recv() {
            let message = match outgoing {
                Outgoing::Text(text) => {
                    ws::Message::Text(String::from_utf8_lossy(&text).into_owned())
                }
                Outgoing::Binary(bytes) => ws::Message::Binary(bytes),
                Outgoing::Close(code, reason) => ws::Message::Closed {
                    code,
                    reason: String::from_utf8_lossy(&reason[..reason.len().min(123)]).into_owned(),
                },
            };
            socket.send(message).map_err(failure)?;
        }
        let Some(message) = socket.receive() else {
            std::thread::sleep(TICK);
            continue;
        };
        let arrived = match message {
            ws::Message::Text(text) => Arrived::Text(text),
            ws::Message::Binary(bytes) => Arrived::Data(bytes),
            ws::Message::Closed { code, reason } => return Ok((code, reason)),
        };
        if !out.put(arrived) {
            return Ok((1001, String::new()));
        }
    }
}

fn stop() -> Stop {
    let now = generation();
    Arc::new(move || held() || generation() != now)
}

fn failure(error: Error) -> Failure {
    match error {
        Error::Argument(_) => Failure::Argument,
        Error::Name => Failure::Name,
        Error::Inner | Error::Connect => Failure::Connect,
        Error::Tls => Failure::Tls,
        Error::Timeout => Failure::Timeout,
        Error::Stopped | Error::Closed(_) => Failure::Disconnected,
    }
}

fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// `@`（ユーザー情報）や空白・改行で要求を壊させない。
fn is_host(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:".contains(&b))
}

/// 要求する URL。メソッドは token の文字だけ、パスは空白・制御文字・ASCII 外を `%XX` に。
fn target(method: &str, url: &Url) -> Result<String, Failure> {
    if !is_token(method) || !is_host(&url.host) {
        return Err(Failure::Argument);
    }
    let path = url.path.split('#').next().unwrap_or("/");
    let mut escaped = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b <= b' ' || b >= 0x7f {
            escaped.push_str(&format!("%{b:02X}"));
        } else {
            escaped.push(char::from(b));
        }
    }
    if !escaped.starts_with('/') {
        escaped.insert(0, '/');
    }
    let (scheme, usual) = if url.secure {
        ("https", 443)
    } else {
        ("http", 80)
    };
    let host = if url.host.contains(':') {
        format!("[{}]", url.host)
    } else {
        url.host.clone()
    };
    Ok(if url.port == usual {
        format!("{scheme}://{host}{escaped}")
    } else {
        format!("{scheme}://{host}:{}{escaped}", url.port)
    })
}

/// 接続の仕方を変えるヘッダー（ブラウザーも文書には書かせない物）。
const OWN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "expect",
    "accept-encoding",
];

/// 決まっているヘッダーと、要求を壊すヘッダーは捨てる。`Accept` と（本文があれば）`Content-Type` は既定を足す。
fn kept(headers: &[(String, String)], with_body: bool) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = headers
        .iter()
        .filter(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            is_token(name)
                && !OWN_HEADERS.contains(&lower.as_str())
                && !lower.starts_with("proxy-")
                && !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        })
        .cloned()
        .collect();
    let has = |kept: &[(String, String)], name: &str| {
        kept.iter().any(|(had, _)| had.eq_ignore_ascii_case(name))
    };
    if !has(&kept, "accept") {
        kept.push((String::from("Accept"), String::from("*/*")));
    }
    if with_body && !has(&kept, "content-type") {
        kept.push((
            String::from("Content-Type"),
            String::from("application/x-www-form-urlencoded"),
        ));
    }
    kept
}

#[cfg(test)]
mod tests {
    use zwsdk::net::{Network as _, Session as _};

    use super::*;
    use crate::net::testing::serve;

    /// 選局（[`hold`]）はどの通信にも効くので、試験を一つずつ。
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        static ALONE: Mutex<()> = Mutex::new(());
        ALONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn holding_fails_requests_at_once() {
        let _alone = alone();
        hold();
        let session = Session::default();
        assert_eq!(
            session
                .send("GET", "http://192.0.2.1/", &[], None, false)
                .err(),
            Some(Failure::Disconnected)
        );
        let mut conn = session.open(open("http://192.0.2.1/", None));
        assert_eq!(
            arrivals(&mut *conn).pop(),
            Some(Arrived::Fail(Failure::Disconnected.code()))
        );
        release();
    }

    #[test]
    fn the_request_line_cannot_be_broken() {
        let _alone = alone();
        let url = parse_url("http://example.jp/a b\r\nX-Evil: 1#frag").unwrap();
        assert_eq!(
            target("GET", &url).unwrap(),
            "http://example.jp/a%20b%0D%0AX-Evil:%201"
        );
        assert_eq!(
            target("GET /x HTTP/1.1\r\nA:", &url),
            Err(Failure::Argument)
        );
        let url = parse_url("http://a.jp\r\nX/").unwrap();
        assert_eq!(target("GET", &url), Err(Failure::Argument), "名前に改行");
        let url = parse_url("http://u@a.jp/").unwrap();
        assert_eq!(target("GET", &url), Err(Failure::Argument), "ユーザー情報");
        let url = parse_url("http://[::1]:8080/x").unwrap();
        assert_eq!(target("GET", &url).unwrap(), "http://[::1]:8080/x");
    }

    #[test]
    fn headers_the_host_owns_are_not_taken_from_the_document() {
        let _alone = alone();
        let headers = [
            (String::from("Content-Length"), String::from("0")),
            (String::from("Host"), String::from("evil")),
            (String::from("X-Ok"), String::from("1")),
            (String::from("X-Bad"), String::from("1\r\nX-Evil: 1")),
            (String::from("Transfer-Encoding"), String::from("chunked")),
            (String::from("Proxy-Authorization"), String::from("x")),
            (String::from("Expect"), String::from("100-continue")),
        ];
        let kept = kept(&headers, true);
        let names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["X-Ok", "Accept", "Content-Type"]);
    }

    #[test]
    fn broadcast_roots_are_kept_per_session_and_bounded() {
        let _alone = alone();
        let session = Session::default();
        assert!(!session.add_root_certificate(b"not a certificate"));
        assert!(!Session::default().add_root_certificate(&[0x30, 0x82, 0, 0]));
        let (ca, _) = crate::net::testing::authority();
        assert!(session.add_root_certificate(&ca));
        assert!(!session.add_root_certificate(&ca), "同じ物は一度だけ");
        assert_eq!(session.roots().len(), 1);
        assert!(
            Session::default().roots().is_empty(),
            "ほかのプラグインには無い"
        );
    }

    #[test]
    fn the_lan_is_reached_only_when_allowed() {
        let _alone = alone();
        let (port, _) = serve(1, None, |_, stream| {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
            );
        });
        let url = format!("http://127.0.0.1:{port}/");
        let session = Session::default();
        assert_eq!(
            session.send("GET", &url, &[], None, false).err(),
            Some(Failure::Connect)
        );
        let reply = session.send("GET", &url, &[], None, true).unwrap();
        assert_eq!(
            (
                reply.status,
                reply.body.as_slice(),
                reply.content_type.as_deref()
            ),
            (200, &b"ok"[..], Some("text/plain"))
        );
        assert!(Broadcast.reach("127.0.0.1", port, TIMEOUT, false).is_none());
    }

    fn open(url: &str, protocols: Option<&str>) -> Open {
        Open {
            method: String::from("GET"),
            url: url.to_owned(),
            headers: Vec::new(),
            body: None,
            protocols: protocols.map(str::to_owned),
            inner: true,
        }
    }

    /// 終わりまで。
    fn arrivals(conn: &mut dyn zwsdk::net::Conn) -> Vec<Arrived> {
        let began = Instant::now();
        let mut all = Vec::new();
        while all.last().is_none_or(|last: &Arrived| !last.is_last()) {
            match conn.next() {
                Some(arrived) => all.push(arrived),
                None => {
                    assert!(began.elapsed() < Duration::from_secs(5), "{all:?}");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        all
    }

    #[test]
    fn a_body_arrives_after_its_head_and_ends() {
        let _alone = alone();
        let (port, _) = serve(1, None, |_, stream| {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
            );
        });
        let session = Session::default();
        let mut conn = session.open(open(&format!("http://127.0.0.1:{port}/"), None));
        let all = arrivals(&mut *conn);
        assert!(
            matches!(all.first(), Some(Arrived::Head { status: 200, .. })),
            "{all:?}"
        );
        let body: Vec<u8> = all
            .iter()
            .filter_map(|arrived| match arrived {
                Arrived::Data(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        assert_eq!(body, b"abcde");
        assert_eq!(
            all.last(),
            Some(&Arrived::End {
                code: 0,
                reason: String::new()
            })
        );
    }

    #[test]
    fn an_open_is_refused_on_the_lan_unless_allowed() {
        let _alone = alone();
        let session = Session::default();
        let mut conn = session.open(Open {
            inner: false,
            ..open("http://127.0.0.1:9/", None)
        });
        assert_eq!(
            arrivals(&mut *conn),
            [Arrived::Fail(Failure::Connect.code())]
        );
        let mut conn = session.open(Open {
            inner: false,
            ..open("ws://127.0.0.1:9/", Some(""))
        });
        assert_eq!(
            arrivals(&mut *conn),
            [Arrived::Fail(Failure::Connect.code())]
        );
    }

    #[test]
    // 応答の型は tungstenite が決める。
    #[expect(clippy::result_large_err)]
    fn a_websocket_carries_messages_both_ways_and_ends_with_the_close() {
        let _alone = alone();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let echo = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut protocol = None;
            let mut socket = tungstenite::accept_hdr(
                tcp,
                |asked: &tungstenite::handshake::server::Request,
                 mut answer: tungstenite::handshake::server::Response| {
                    protocol = asked
                        .headers()
                        .get("sec-websocket-protocol")
                        .map(|value| value.to_str().unwrap_or_default().to_owned());
                    if let Some(first) = protocol.as_deref().and_then(|all| all.split(',').next()) {
                        answer
                            .headers_mut()
                            .insert("sec-websocket-protocol", first.trim().parse().unwrap());
                    }
                    Ok(answer)
                },
            )
            .unwrap();
            while let Ok(message) = socket.read() {
                if message.is_close() {
                    let _ = socket.flush();
                    continue;
                }
                let _ = socket.send(message);
            }
            protocol
        });
        let session = Session::default();
        let mut conn = session.open(open(&format!("ws://127.0.0.1:{port}/"), Some("a, b")));
        let began = Instant::now();
        let head = loop {
            if let Some(arrived) = conn.next() {
                break arrived;
            }
            assert!(began.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(
            matches!(&head, Arrived::Head { status: 101, headers }
                if headers.iter().any(|(name, value)| name == "sec-websocket-protocol" && value == "a")),
            "{head:?}"
        );
        assert!(conn.send(Outgoing::Text(b"hello".to_vec())));
        assert!(conn.send(Outgoing::Binary(vec![1, 2])));
        assert!(conn.send(Outgoing::Close(1000, b"bye".to_vec())));
        let all = arrivals(&mut *conn);
        assert_eq!(
            all,
            [
                Arrived::Text(String::from("hello")),
                Arrived::Data(vec![1, 2]),
                Arrived::End {
                    code: 1000,
                    reason: String::from("bye")
                },
            ]
        );
        assert_eq!(echo.join().unwrap().as_deref(), Some("a, b"));
        assert!(!conn.send(Outgoing::Text(Vec::new())), "閉じた後は送れない");
    }

    #[test]
    fn a_connection_nobody_reads_holds_no_more_than_its_share() {
        let _alone = alone();
        let (sent, arrived) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicBool::new(false));
        let out = Sink {
            sent,
            dropped: Arc::clone(&dropped),
            queued: Arc::new((Mutex::new(0), Condvar::new())),
            generation: generation(),
        };
        let filling = std::thread::spawn(move || {
            let mut put = 0;
            while out.put(Arrived::Data(vec![0; 1 << 20])) {
                put += 1;
            }
            put
        });
        std::thread::sleep(Duration::from_millis(300));
        dropped.store(true, Ordering::SeqCst);
        let put = filling.join().unwrap();
        assert_eq!(put, QUEUE_BYTES >> 20, "読まれない間は上限で待つ");
        drop(arrived);
    }

    #[test]
    fn a_websocket_sender_that_floods_is_turned_away() {
        let _alone = alone();
        let (outgoing, _to_send) = sync_channel(OUTGOING);
        let mut conn = Conn {
            arrived: sync_channel(1).1,
            outgoing: Some(outgoing),
            dropped: Arc::new(AtomicBool::new(false)),
            queued: Arc::new((Mutex::new(0), Condvar::new())),
            failed: false,
        };
        let sent = (0..OUTGOING * 2)
            .filter(|_| zwsdk::net::Conn::send(&mut conn, Outgoing::Text(Vec::new())))
            .count();
        assert_eq!(sent, OUTGOING);
    }

    #[test]
    fn a_websocket_with_credentials_in_the_url_is_refused() {
        let _alone = alone();
        let mut conn = Session::default().open(open("wss://u:p@example.jp/", Some("")));
        assert_eq!(
            arrivals(&mut *conn),
            [Arrived::Fail(Failure::Argument.code())]
        );
    }
}

#[cfg(test)]
mod live_tests {
    use zwsdk::net::Session as _;

    #[test]
    fn https_is_reachable_when_asked() {
        if std::env::var_os("ZW_LIVE_TESTS").is_none() {
            return;
        }
        let reply = super::Session::default()
            .send("GET", "https://example.com/", &[], None, false)
            .expect("https で取れる");
        assert!(reply.status < 500, "{}", reply.status);
        assert!(!reply.body.is_empty());
    }
}
