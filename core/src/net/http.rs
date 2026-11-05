// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! HTTP(S)。URL の `user:password@` は Basic 認証で送る。転送は五回まで追い、https から http へは移らない。

use std::future::Future;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};

use super::address::is_inner;
use super::{Error, Stop, never, tls};

const CONNECT_PATIENCE: Duration = Duration::from_secs(3);
/// 止める要求を確かめる間隔。
const LOOK: Duration = Duration::from_millis(50);
const MOST_REDIRECTS: usize = 5;

#[derive(Debug, Clone)]
pub struct Options {
    /// 内部ネットワーク（LAN など）へ繋いでよいか。
    pub inner: bool,
    /// 読み込みが一度に待つ上限。
    pub patience: Duration,
    /// 足すルート証明書（DER）。
    pub roots: Vec<Vec<u8>>,
    /// `sha1WithRSAEncryption` の証明書も受けるか（[`tls::config`]）。
    pub sha1: bool,
    pub agent: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            inner: false,
            patience: Duration::from_secs(10),
            roots: Vec::new(),
            sha1: false,
            agent: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    client: reqwest::Client,
    inner: bool,
}

pub struct Request<'a> {
    method: &'a str,
    url: &'a str,
    headers: Vec<(&'a str, &'a str)>,
    body: Option<&'a [u8]>,
    stop: Stop,
    deadline: Option<Instant>,
}

impl std::fmt::Debug for Request<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Request")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl<'a> Request<'a> {
    pub fn new(method: &'a str, url: &'a str) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
            stop: never(),
            deadline: None,
        }
    }

    pub fn get(url: &'a str) -> Self {
        Self::new("GET", url)
    }

    pub fn header(mut self, name: &'a str, value: &'a str) -> Self {
        self.headers.push((name, value));
        self
    }

    pub fn body(mut self, body: &'a [u8]) -> Self {
        self.body = Some(body);
        self
    }

    /// 応答の本文を読み終えるまで効く。
    pub fn stop(mut self, stop: Stop) -> Self {
        self.stop = stop;
        self
    }

    /// 応答の本文を読み終えるまで効く。
    pub fn deadline(mut self, at: Instant) -> Self {
        self.deadline = Some(at);
        self
    }
}

impl Client {
    pub fn new(options: &Options) -> Result<Self, Error> {
        let runtime = runtime()?;
        let _inside = runtime.enter();
        let inner = options.inner;
        let mut builder = reqwest::Client::builder()
            .http1_only()
            // 古いサーバーには、ヘッダーの名前を小文字だけで送ると読めない物がある。
            .http1_title_case_headers()
            .no_proxy()
            .connect_timeout(CONNECT_PATIENCE)
            .read_timeout(options.patience)
            .pool_idle_timeout(Duration::from_secs(30))
            .dns_resolver(Arc::new(Resolver { inner }))
            .redirect(redirects(inner))
            .tls_backend_preconfigured(rustls::ClientConfig::clone(&tls::config(
                &options.roots,
                options.sha1,
            )));
        if let Some(agent) = options.agent.as_deref() {
            builder = builder.user_agent(agent);
        }
        let client = builder
            .build()
            .map_err(|why| Error::Argument(why.to_string()))?;
        Ok(Self { client, inner })
    }

    pub fn send(&self, request: Request<'_>) -> Result<Response, Error> {
        // reqwest は要求を作るときに時計を使う。
        let _inside = runtime()?.enter();
        // URL にはパスワードが入り得るので、失敗の理由には書かない。
        let url =
            reqwest::Url::parse(request.url).map_err(|why| Error::Argument(why.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Argument(format!(
                "{}: http でも https でもない",
                url.scheme()
            )));
        }
        if !self.inner && literal_is_inner(&url) {
            return Err(Error::Inner);
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| Error::Argument(format!("メソッド {}", request.method)))?;
        let mut builder = self.client.request(method, url);
        for (name, value) in &request.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::Argument(format!("ヘッダー {name}")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| Error::Argument(format!("ヘッダー {name} の値")))?;
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body.to_vec());
        }
        let answer = run(builder.send(), &*request.stop, request.deadline)?;
        let status = answer.status();
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
        Ok(Response {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_owned(),
            headers,
            body: Arc::new(Mutex::new(Body {
                answer: Some(answer),
                chunk: Bytes::new(),
            })),
            closed: Arc::new(AtomicBool::new(false)),
            stop: request.stop,
            deadline: request.deadline,
        })
    }
}

/// 本文は [`Read`] で届いた分から読む。止めると（[`Stop`]・[`Closer`]）読み終わりになる。
pub struct Response {
    pub status: u16,
    pub reason: String,
    /// 名前は小文字。
    pub headers: Vec<(String, String)>,
    body: Arc<Mutex<Body>>,
    closed: Arc<AtomicBool>,
    stop: Stop,
    deadline: Option<Instant>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Response")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

struct Body {
    answer: Option<reqwest::Response>,
    chunk: Bytes,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(had, _)| had.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 別のスレッドから接続を切る。
    pub fn closer(&self) -> Closer {
        Closer {
            body: Arc::clone(&self.body),
            closed: Arc::clone(&self.closed),
        }
    }

    /// 本文を `most` バイトまで。その先は読まない。
    pub fn read_most(&mut self, most: usize) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        while out.len() < most {
            let Some(chunk) = self.next_chunk()? else {
                break;
            };
            let room = most - out.len();
            out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        Ok(out)
    }

    pub fn read_all(&mut self) -> Result<Vec<u8>, Error> {
        self.read_most(usize::MAX)
    }

    fn next_chunk(&mut self) -> Result<Option<Bytes>, Error> {
        let mut body = self.body.lock().unwrap_or_else(|e| e.into_inner());
        if !body.chunk.is_empty() {
            return Ok(Some(std::mem::take(&mut body.chunk)));
        }
        let Some(answer) = body.answer.as_mut() else {
            return Ok(None);
        };
        let _inside = runtime()?.enter();
        let closed = &self.closed;
        let stop = &self.stop;
        let stopped = || closed.load(Ordering::Relaxed) || stop();
        let next = run(answer.chunk(), &stopped, self.deadline);
        if !matches!(next, Ok(Some(_))) {
            body.answer = None;
        }
        next
    }
}

impl Read for Response {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        {
            let mut body = self.body.lock().unwrap_or_else(|e| e.into_inner());
            if !body.chunk.is_empty() {
                let read = body.chunk.len().min(into.len());
                into[..read].copy_from_slice(&body.chunk[..read]);
                body.chunk.advance(read);
                return Ok(read);
            }
        }
        match self.next_chunk() {
            Ok(Some(chunk)) => {
                let read = chunk.len().min(into.len());
                into[..read].copy_from_slice(&chunk[..read]);
                if read < chunk.len() {
                    self.body.lock().unwrap_or_else(|e| e.into_inner()).chunk = chunk.slice(read..);
                }
                Ok(read)
            }
            Ok(None) | Err(Error::Stopped) => Ok(0),
            Err(why) => Err(why.into()),
        }
    }
}

#[derive(Clone)]
pub struct Closer {
    body: Arc<Mutex<Body>>,
    closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for Closer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Closer")
    }
}

impl Closer {
    /// 読んでいる途中なら、それが戻るのを待ってから切る（戻ったときには接続は閉じている）。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.body.lock().unwrap_or_else(|e| e.into_inner()).answer = None;
    }
}

/// 通信だけの tokio。初めて使うときに作る。
fn runtime() -> Result<&'static tokio::runtime::Runtime, Error> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("net")
                .max_blocking_threads(8)
                .enable_all()
                .build()
                .inspect_err(|why| tracing::warn!("通信のスレッドを作れない: {why}"))
                .ok()
        })
        .as_ref()
        .ok_or_else(|| Error::Closed(String::from("通信のスレッドを作れない")))
}

pub(super) fn run<T>(
    work: impl Future<Output = Result<T, reqwest::Error>>,
    stop: &(dyn Fn() -> bool + Send + Sync),
    deadline: Option<Instant>,
) -> Result<T, Error> {
    if stop() {
        return Err(Error::Stopped);
    }
    runtime()?.block_on(async {
        let stopped = async {
            while !stop() {
                tokio::time::sleep(LOOK).await;
            }
        };
        let late = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            done = work => done.map_err(failure),
            () = stopped => Err(Error::Stopped),
            () = late => Err(Error::Timeout),
        }
    })
}

fn literal_is_inner(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(v4)) => is_inner(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => is_inner(IpAddr::V6(v6)),
        _ => false,
    }
}

fn redirects(inner: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let downgraded = attempt
            .previous()
            .last()
            .is_some_and(|was| was.scheme() == "https")
            && attempt.url().scheme() != "https";
        if attempt.previous().len() > MOST_REDIRECTS {
            attempt.error(Refusal::Redirects)
        } else if downgraded {
            attempt.error(Refusal::Downgrade)
        } else if !inner && literal_is_inner(attempt.url()) {
            attempt.error(Refusal::Inner)
        } else {
            attempt.follow()
        }
    })
}

/// 名前を引いたうえで、内部ネットワークのアドレスを除く。
struct Resolver {
    inner: bool,
}

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let inner = self.inner;
        Box::pin(async move {
            let found = tokio::task::spawn_blocking(move || {
                (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(Iterator::collect::<Vec<_>>)
            })
            .await
            .map_err(|_| Refusal::Name)?
            .map_err(|_| Refusal::Name)?;
            if found.is_empty() {
                return Err(Refusal::Name.into());
            }
            let kept: Vec<_> = found
                .into_iter()
                .filter(|address| inner || !is_inner(address.ip()))
                .collect();
            if kept.is_empty() {
                return Err(Refusal::Inner.into());
            }
            Ok(Box::new(kept.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Refusal {
    Name,
    Inner,
    Downgrade,
    Redirects,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Name => "名前を引けない",
            Self::Inner => "内部ネットワークの宛先",
            Self::Downgrade => "https から http へは移らない",
            Self::Redirects => "転送が多すぎる",
        })
    }
}

impl std::error::Error for Refusal {}

fn failure(error: reqwest::Error) -> Error {
    // URL にはパスワードが入り得る。
    let error = error.without_url();
    if error.is_timeout() {
        return Error::Timeout;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(at) = source {
        // hyper は io::Error を io::Error で包む。
        let mut seen = Some(at);
        while let Some(this) = seen {
            if let Some(refusal) = this.downcast_ref::<Refusal>() {
                return match refusal {
                    Refusal::Name => Error::Name,
                    Refusal::Inner => Error::Inner,
                    Refusal::Downgrade => Error::Tls,
                    Refusal::Redirects => Error::Connect,
                };
            }
            if this.is::<rustls::Error>() {
                return Error::Tls;
            }
            let io = this.downcast_ref::<std::io::Error>();
            if io.is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut) {
                return Error::Timeout;
            }
            seen = io
                .and_then(std::io::Error::get_ref)
                .map(|inside| inside as &(dyn std::error::Error + 'static));
        }
        source = at.source();
    }
    if error.is_builder() {
        Error::Argument(error.to_string())
    } else if error.is_connect() {
        Error::Connect
    } else {
        Error::Closed(error.to_string())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::net::testing::{authority, serve};

    fn lan() -> Client {
        Client::new(&Options {
            inner: true,
            ..Options::default()
        })
        .unwrap()
    }

    fn ok(body: &'static [u8]) -> impl Fn(&str, &mut dyn crate::net::testing::Both) + Send {
        move |_, stream| {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
    }

    #[test]
    fn credentials_in_the_url_are_sent_as_basic_and_headers_keep_their_case() {
        let (port, heads) = serve(1, None, ok(b"hi"));
        let url = format!("http://user:pa%3Ass@127.0.0.1:{port}/version");
        let mut answer = lan()
            .send(Request::get(&url).header("X-Priority", "3"))
            .unwrap();
        assert_eq!(
            (answer.status, answer.read_all().unwrap()),
            (200, b"hi".to_vec())
        );
        let head = heads.join().unwrap().remove(0);
        assert!(head.starts_with("GET /version HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("X-Priority: 3\r\n"), "{head}");
        assert!(
            head.contains(&format!(
                "Authorization: {}\r\n",
                crate::net::basic("user", Some("pa:ss"))
            )),
            "{head}"
        );
    }

    #[test]
    fn credentials_do_not_show_in_what_went_wrong() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        for url in [
            format!("http://user:secret@127.0.0.1:{port}/"),
            String::from("http://user:secret@[::1"),
        ] {
            let why = lan().send(Request::get(&url)).err().unwrap();
            assert!(!why.to_string().contains("secret"), "{why}");
            assert!(!format!("{why:?}").contains("secret"), "{why:?}");
        }
    }

    #[test]
    fn sha1_is_not_trusted_unless_asked() {
        assert!(!Options::default().sha1);
    }

    #[test]
    fn a_body_is_read_no_further_than_asked() {
        let (port, _) = serve(1, None, ok(b"abcdefgh"));
        let url = format!("http://127.0.0.1:{port}/");
        let mut answer = lan().send(Request::get(&url)).unwrap();
        assert_eq!(answer.read_most(3).unwrap(), b"abc");
    }

    #[test]
    fn the_lan_is_refused_unless_allowed() {
        let client = Client::new(&Options::default()).unwrap();
        assert_eq!(
            client.send(Request::get("http://127.0.0.1:9/")).err(),
            Some(Error::Inner)
        );
        assert_eq!(
            client.send(Request::get("http://localhost:9/")).err(),
            Some(Error::Inner),
            "名前を引いた先も"
        );
    }

    #[test]
    fn a_stream_that_stops_coming_is_cut_from_another_thread() {
        let (port, _) = serve(1, None, |_, stream| {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\r\nab");
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(5));
        });
        let url = format!("http://127.0.0.1:{port}/stream");
        let mut answer = lan().send(Request::get(&url)).unwrap();
        let closer = answer.closer();
        let reading = std::thread::spawn(move || {
            let mut got = Vec::new();
            let _ = answer.read_to_end(&mut got);
            got
        });
        std::thread::sleep(Duration::from_millis(200));
        let began = Instant::now();
        closer.close();
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(reading.join().unwrap(), b"ab");
    }

    #[test]
    fn waiting_for_an_answer_can_be_stopped() {
        let (port, _) = serve(1, None, |_, _| std::thread::sleep(Duration::from_secs(5)));
        let url = format!("http://127.0.0.1:{port}/");
        let flag = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&flag);
        let raising = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let began = Instant::now();
        let got =
            lan().send(Request::get(&url).stop(Arc::new(move || seen.load(Ordering::Relaxed))));
        raising.join().unwrap();
        assert_eq!(got.err(), Some(Error::Stopped));
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn https_is_checked_against_the_roots_given() {
        let (ca, server) = authority();
        let (port, _) = serve(2, Some(server), ok(b"secure"));
        let url = format!("https://127.0.0.1:{port}/");
        let trusting = Client::new(&Options {
            inner: true,
            roots: vec![ca],
            ..Options::default()
        })
        .unwrap();
        let mut answer = trusting.send(Request::get(&url)).unwrap();
        assert_eq!(answer.read_all().unwrap(), b"secure");
        assert_eq!(lan().send(Request::get(&url)).err(), Some(Error::Tls));
    }

    #[test]
    fn https_is_not_followed_down_to_http() {
        let (ca, server) = authority();
        let (port, _) = serve(1, Some(server), |_, stream| {
            let _ = stream.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/\r\nContent-Length: 0\r\n\r\n",
            );
        });
        let client = Client::new(&Options {
            inner: true,
            roots: vec![ca],
            ..Options::default()
        })
        .unwrap();
        let url = format!("https://127.0.0.1:{port}/");
        assert_eq!(client.send(Request::get(&url)).err(), Some(Error::Tls));
    }

    #[test]
    fn a_post_turned_away_with_303_is_followed_as_a_get() {
        let (port, heads) = serve(2, None, |head, stream| {
            if head.starts_with("POST") {
                let _ = stream.write_all(
                    b"HTTP/1.1 303 See Other\r\nLocation: /done\r\nContent-Length: 0\r\n\r\n",
                );
            } else {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        let url = format!("http://127.0.0.1:{port}/form");
        let mut answer = lan().send(Request::new("POST", &url).body(b"a=1")).unwrap();
        assert_eq!(answer.read_all().unwrap(), b"ok");
        let heads = heads.join().unwrap();
        assert!(heads[1].starts_with("GET /done "), "{heads:?}");
    }
}
