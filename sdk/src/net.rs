// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 通信（STD-B24 第二分冊 (2/2) 付属 3 5.6.5、TR-B14 第三分冊 8）。同期で待つ（上限 `TIMEOUT`）。

use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(not(target_arch = "wasm32"))]
pub use client::*;

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

#[cfg(not(target_arch = "wasm32"))]
mod client {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
    use std::sync::{Arc, Mutex, RwLock};
    use std::time::{Duration, Instant};

    use super::{Failure, Reply, TIMEOUT, Url, join, parse_url};

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

    /// 選局で通信を全部止める。文書のスレッドは同期で待つので、止めないと前の局の文書が残る。
    static HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    pub fn hold() {
        HOLD.store(true, std::sync::atomic::Ordering::SeqCst);
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn release() {
        HOLD.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn held() -> bool {
        HOLD.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn generation() -> u64 {
        GENERATION.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// TCP の読み込みは途中で中断できないので、短い間隔で停止要求かタイムアウトかを確認する。
    struct Interruptible {
        inner: TcpStream,
        generation: u64,
        deadline: Instant,
    }

    impl Interruptible {
        fn new(inner: TcpStream) -> Self {
            let _ = inner.set_read_timeout(Some(Duration::from_millis(300)));
            let _ = inner.set_write_timeout(Some(Duration::from_millis(300)));
            Self {
                inner,
                generation: generation(),
                deadline: Instant::now() + TIMEOUT,
            }
        }

        fn connect(address: &std::net::SocketAddr, timeout: Duration) -> std::io::Result<Self> {
            TcpStream::connect_timeout(address, timeout.min(CONNECT_TIMEOUT)).map(Self::new)
        }

        fn check(&self) -> std::io::Result<()> {
            if held() || generation() != self.generation {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "選局で止めた",
                ));
            }
            if Instant::now() > self.deadline {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "期限"));
            }
            Ok(())
        }
    }

    fn is_tick(e: &std::io::Error) -> bool {
        matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    }

    impl Read for Interruptible {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            loop {
                self.check()?;
                match self.inner.read(buf) {
                    Err(e) if is_tick(&e) => continue,
                    other => return other,
                }
            }
        }
    }

    impl Write for Interruptible {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            loop {
                self.check()?;
                match self.inner.write(buf) {
                    Err(e) if is_tick(&e) => continue,
                    other => return other,
                }
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    /// UDP ソケットを外向きに connect して経路があるかを見るだけ。ローカルアドレスは見ない（Windows は未定のまま返す）。
    pub fn has_ip() -> bool {
        if std::env::var_os("ZW_NO_NET").is_some() {
            return false;
        }
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

    pub fn resolve(host: &str) -> Option<String> {
        (host, 0u16)
            .to_socket_addrs()
            .ok()?
            .next()
            .map(|address| address.ip().to_string())
    }

    /// 放送の文書が内部ネットワーク（loopback・LAN・CGNAT・マルチキャストなど）へ送れないよう、既定では断る。
    pub fn is_inner(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let [a, b, ..] = v4.octets();
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || v4.is_multicast()
                    || a == 0
                    // 100.64.0.0/10（CGNAT）
                    || (a == 100 && (b & 0xc0) == 64)
                    || (a == 198 && (b & 0xfe) == 18)
            }
            IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return is_inner(IpAddr::V4(v4));
                }
                let first = v6.segments()[0];
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80
                    || (first & 0xffc0) == 0xfec0
            }
        }
    }

    /// 名前解決したアドレスにそのまま接続するので、確かめた後で替えられることは無い。
    pub(crate) fn address_of(host: &str, port: u16, inner: bool) -> Result<SocketAddr, Failure> {
        let mut found = false;
        for address in (host, port).to_socket_addrs().map_err(|_| Failure::Name)? {
            found = true;
            if inner || !is_inner(address.ip()) {
                return Ok(address);
            }
        }
        if found {
            tracing::debug!("内部ネットワークの宛先なので接続しない: {host}");
            Err(Failure::Connect)
        } else {
            Err(Failure::Name)
        }
    }

    /// ICMP は特権が要るので TCP で。返るのは (相手のアドレス, 往復のミリ秒)。
    pub fn reach(host: &str, port: u16, timeout: Duration) -> Option<(String, u32)> {
        reach_from(host, port, timeout, false)
    }

    pub fn reach_from(
        host: &str,
        port: u16,
        timeout: Duration,
        inner: bool,
    ) -> Option<(String, u32)> {
        if held() {
            return None;
        }
        let address = address_of(host, port, inner).ok()?;
        let began = Instant::now();
        Interruptible::connect(&address, timeout).ok()?;
        Some((
            address.ip().to_string(),
            began.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
        ))
    }

    /// 本体の中から使う通信。内部ネットワークの宛先は拒否する。
    static SHARED: Session = Session::new();

    pub fn post(uri: &str, form: &[u8]) -> Result<Reply, Failure> {
        // TR-B14 は 4096 バイト。それより先は実装依存。
        SHARED.request("POST", uri, &[], Some(form), 65_536, false)
    }

    pub fn get(uri: &str) -> Result<Reply, Failure> {
        send("GET", uri, &[], None)
    }

    /// 本文があり `Content-Type` が無ければ form。転送は五回まで（POST の 303 は GET に）。
    pub fn send(
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) -> Result<Reply, Failure> {
        SHARED.send(method, uri, headers, body, false)
    }

    pub fn add_root_certificate(der: &[u8]) -> bool {
        SHARED.add_root_certificate(der)
    }

    /// 一つのプラグインの通信。放送で届いたものはこの中だけで使い、選局で破棄する。
    #[derive(Debug, Default)]
    pub struct Session {
        roots: Mutex<Vec<(u64, Vec<u8>)>>,
        config: Mutex<Option<(u64, usize, Arc<rustls::ClientConfig>)>>,
    }

    const MOST_ROOTS: usize = 8;

    impl Session {
        pub const fn new() -> Self {
            Self {
                roots: Mutex::new(Vec::new()),
                config: Mutex::new(None),
            }
        }

        /// 放送で届いたものを追加する（TR-B14 第三分冊）。この通信だけで使い、選局で破棄する。
        pub fn add_root_certificate(&self, der: &[u8]) -> bool {
            let now = generation();
            let mut roots = self.roots.lock().unwrap_or_else(|e| e.into_inner());
            roots.retain(|(at, _)| *at == now);
            if roots.iter().any(|(_, had)| had == der) || roots.len() >= MOST_ROOTS {
                return false;
            }
            let parsed = rustls::pki_types::CertificateDer::from(der.to_vec());
            if rustls::RootCertStore::empty().add(parsed).is_err() {
                return false;
            }
            roots.push((now, der.to_vec()));
            true
        }

        fn tls_config(&self) -> Arc<rustls::ClientConfig> {
            let now = generation();
            let roots: Vec<Vec<u8>> = {
                let mut roots = self.roots.lock().unwrap_or_else(|e| e.into_inner());
                roots.retain(|(at, _)| *at == now);
                roots.iter().map(|(_, der)| der.clone()).collect()
            };
            if roots.is_empty() {
                return base_config();
            }
            let mut cached = self.config.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, count, config)) = cached.as_ref()
                && *at == now
                && *count == roots.len()
            {
                return config.clone();
            }
            let built = build_config(&roots);
            *cached = Some((now, roots.len(), built.clone()));
            built
        }

        pub fn send(
            &self,
            method: &str,
            uri: &str,
            headers: &[(String, String)],
            body: Option<&[u8]>,
            inner: bool,
        ) -> Result<Reply, Failure> {
            let mut at = uri.to_owned();
            let mut method = method.to_owned();
            let mut body = body;
            for _ in 0..5 {
                let reply = self.request(&method, &at, headers, body, 16 << 20, inner)?;
                let moved = matches!(reply.status, 301 | 302 | 303 | 307 | 308);
                let Some(location) = reply.location.as_deref().filter(|_| moved) else {
                    return Ok(reply);
                };
                if reply.status == 303 || (matches!(reply.status, 301 | 302) && method == "POST") {
                    method = String::from("GET");
                    body = None;
                }
                let next = join(&at, location).unwrap_or_else(|| location.to_owned());
                // https から http へはリダイレクトしない。
                if at.starts_with("https://") && !next.starts_with("https://") {
                    return Err(Failure::Tls);
                }
                at = next;
            }
            Err(Failure::Connect)
        }

        fn request(
            &self,
            method: &str,
            uri: &str,
            headers: &[(String, String)],
            body: Option<&[u8]>,
            most: usize,
            inner: bool,
        ) -> Result<Reply, Failure> {
            let url = parse_url(uri).ok_or(Failure::Argument)?;
            let head = request_head(method, &url, headers, body.map(<[u8]>::len))?;
            if held() {
                return Err(Failure::Disconnected);
            }
            let address = address_of(&url.host, url.port, inner)?;
            let tcp = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    Failure::Timeout
                } else {
                    Failure::Connect
                }
            })?;
            let tcp = Interruptible::new(tcp);
            let mut stream = if url.secure {
                wrap_tls(tcp, &url.host, self.tls_config())?
            } else {
                Stream::Plain(tcp)
            };
            stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(body.unwrap_or(&[])))
                .and_then(|()| stream.flush())
                .map_err(io_failure)?;
            read_reply(BufReader::new(stream), most)
        }
    }

    fn is_token(text: &str) -> bool {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    }

    /// `@` や空白・改行でリクエストヘッダーを壊させない。
    fn is_host(text: &str) -> bool {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._:".contains(&b))
    }

    /// リクエスト行を壊させないよう、メソッドは token の文字だけ、パスは空白・制御文字・ASCII 外を `%XX` に。
    pub(crate) fn request_head(
        method: &str,
        url: &Url,
        headers: &[(String, String)],
        body: Option<usize>,
    ) -> Result<String, Failure> {
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
        let host = url.authority();
        let has = |name: &str| headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
        let mut head =
            format!("{method} {escaped} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        if !has("accept") {
            head.push_str("Accept: */*\r\n");
        }
        for (name, value) in headers {
            if !is_token(name)
                || name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("transfer-encoding")
                || name.eq_ignore_ascii_case("connection")
                || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
            {
                continue;
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(length) = body {
            if !has("content-type") {
                head.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
            }
            head.push_str(&format!("Content-Length: {length}\r\n"));
        }
        head.push_str("\r\n");
        Ok(head)
    }

    enum Stream {
        Plain(Interruptible),
        Tls(Box<rustls::StreamOwned<rustls::ClientConnection, Interruptible>>),
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self {
                Self::Plain(s) => s.read(buf),
                Self::Tls(s) => s.read(buf),
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            match self {
                Self::Plain(s) => s.write(buf),
                Self::Tls(s) => s.write(buf),
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            match self {
                Self::Plain(s) => s.flush(),
                Self::Tls(s) => s.flush(),
            }
        }
    }

    static BASE: RwLock<Option<Arc<rustls::ClientConfig>>> = RwLock::new(None);

    fn base_config() -> Arc<rustls::ClientConfig> {
        if let Some(config) = BASE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return config.clone();
        }
        let built = build_config(&[]);
        *BASE.write().unwrap_or_else(|e| e.into_inner()) = Some(built.clone());
        built
    }

    /// `ZW_CA_DIR` の PEM も足す。`ZW_TLS_INSECURE=1` は証明書を検証しない（テスト用）。
    fn build_config(broadcast: &[Vec<u8>]) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let certs: Vec<_> = broadcast
            .iter()
            .map(|der| rustls::pki_types::CertificateDer::from(der.clone()))
            .collect();
        roots.add_parsable_certificates(certs);
        if let Some(dir) = std::env::var_os("ZW_CA_DIR") {
            for (name, pem) in std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read(e.path()).ok().map(|b| (e.path(), b)))
            {
                use rustls::pki_types::pem::PemObject;
                let certs: Vec<_> = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
                    .flatten()
                    .collect();
                let (added, _) = roots.add_parsable_certificates(certs);
                if added == 0 {
                    tracing::warn!("ルート証明書として読み込めません: {}", name.display());
                }
            }
        }
        let provider = Arc::new(provider_with_sha1());
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("rustls の既定のバージョン");
        let config = if std::env::var_os("ZW_TLS_INSECURE").is_some() {
            tracing::warn!("ZW_TLS_INSECURE が設定されているので、https の証明書を確かめません");
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
                .with_no_client_auth()
        } else {
            builder.with_root_certificates(roots).with_no_client_auth()
        };
        Arc::new(config)
    }

    /// ring の既定に `sha1WithRSAEncryption` の検証を足したもの（証明書チェーンの検証にだけ使う）。
    fn provider_with_sha1() -> rustls::crypto::CryptoProvider {
        let mut provider = rustls::crypto::ring::default_provider();
        let mut all: Vec<&'static dyn rustls::pki_types::SignatureVerificationAlgorithm> =
            provider.signature_verification_algorithms.all.to_vec();
        all.push(&RSA_SHA1);
        provider.signature_verification_algorithms.all = Box::leak(all.into_boxed_slice());
        provider
    }

    #[derive(Debug)]
    struct RsaSha1;

    static RSA_SHA1: RsaSha1 = RsaSha1;

    impl rustls::pki_types::SignatureVerificationAlgorithm for RsaSha1 {
        fn public_key_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
            rustls::pki_types::alg_id::RSA_ENCRYPTION
        }

        fn signature_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
            rustls::pki_types::AlgorithmIdentifier::from_slice(&[
                0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05, 0x05, 0x00,
            ])
        }

        fn verify_signature(
            &self,
            public_key: &[u8],
            message: &[u8],
            signature: &[u8],
        ) -> Result<(), rustls::pki_types::InvalidSignature> {
            ring::signature::UnparsedPublicKey::new(
                &ring::signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
                public_key,
            )
            .verify(message, signature)
            .map_err(|_| rustls::pki_types::InvalidSignature)
        }
    }

    #[derive(Debug)]
    struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

    impl rustls::client::danger::ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    fn wrap_tls(
        stream: Interruptible,
        host: &str,
        config: Arc<rustls::ClientConfig>,
    ) -> Result<Stream, Failure> {
        let name =
            rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|_| Failure::Tls)?;
        let connection = rustls::ClientConnection::new(config, name).map_err(|_| Failure::Tls)?;
        let mut tls = rustls::StreamOwned::new(connection, stream);
        while tls.conn.is_handshaking() {
            if let Err(e) = tls.conn.complete_io(&mut tls.sock) {
                return Err(match e.kind() {
                    std::io::ErrorKind::TimedOut => Failure::Timeout,
                    std::io::ErrorKind::Interrupted => Failure::Disconnected,
                    _ => Failure::Tls,
                });
            }
        }
        Ok(Stream::Tls(Box::new(tls)))
    }

    fn io_failure(e: std::io::Error) -> Failure {
        match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Failure::Timeout,
            _ => Failure::Disconnected,
        }
    }

    const LINE_MOST: usize = 8 << 10;
    const HEAD_MOST: usize = 64 << 10;

    fn read_line_within<T: Read>(
        reader: &mut BufReader<T>,
        line: &mut String,
        budget: &mut usize,
    ) -> Result<(), Failure> {
        let most = LINE_MOST.min(*budget);
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(most as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(io_failure)?;
        if bytes.len() > most {
            return Err(Failure::Disconnected);
        }
        *budget -= bytes.len();
        line.clear();
        line.push_str(&String::from_utf8_lossy(&bytes));
        Ok(())
    }

    pub(crate) fn read_reply<T: Read>(
        mut reader: BufReader<T>,
        most: usize,
    ) -> Result<Reply, Failure> {
        let mut budget = HEAD_MOST;
        let mut line = String::new();
        read_line_within(&mut reader, &mut line, &mut budget)?;
        let status = line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or(Failure::Disconnected)?;
        let mut length: Option<usize> = None;
        let mut chunked = false;
        let mut content_type = None;
        let mut location = None;
        let mut headers = Vec::new();
        loop {
            read_line_within(&mut reader, &mut line, &mut budget)?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                headers.push((name.trim().to_owned(), value.trim().to_owned()));
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim();
                if name == "content-length" {
                    length = value.parse().ok();
                } else if name == "content-type" {
                    content_type = Some(value.to_owned());
                } else if name == "location" {
                    location = Some(value.to_owned());
                } else if name == "transfer-encoding"
                    && value.to_ascii_lowercase().contains("chunked")
                {
                    chunked = true;
                }
            }
        }
        let mut body = Vec::new();
        if chunked {
            loop {
                // 短いチャンクを際限なく送らせない。
                let mut size_budget = LINE_MOST;
                read_line_within(&mut reader, &mut line, &mut size_budget)?;
                let size = u64::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)
                    .map_err(|_| Failure::Disconnected)?;
                if size == 0 {
                    break;
                }
                // 相手が示したサイズのまま確保しない。
                let room = most.saturating_sub(body.len()) as u64;
                let kept = size.min(room);
                let before = body.len();
                (&mut reader)
                    .take(kept)
                    .read_to_end(&mut body)
                    .map_err(io_failure)?;
                if (body.len() - before) as u64 != kept {
                    return Err(Failure::Disconnected);
                }
                let rest = size - kept;
                if rest > 0
                    && std::io::copy(&mut (&mut reader).take(rest), &mut std::io::sink())
                        .map_err(io_failure)?
                        != rest
                {
                    return Err(Failure::Disconnected);
                }
                read_line_within(&mut reader, &mut line, &mut size_budget)?;
            }
        } else if let Some(length) = length {
            body.resize(length.min(most), 0);
            reader.read_exact(&mut body).map_err(io_failure)?;
        } else {
            reader
                .take(most as u64)
                .read_to_end(&mut body)
                .map_err(io_failure)?;
        }
        body.truncate(most);
        Ok(Reply {
            status,
            body,
            content_type,
            location,
            headers,
        })
    }
}

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
    fn holding_fails_requests_at_once() {
        super::hold();
        assert_eq!(
            super::get("http://192.0.2.1/").err(),
            Some(super::Failure::Disconnected)
        );
        super::release();
    }

    #[test]
    fn the_form_is_escaped_the_rfc_2396_way() {
        assert_eq!(escape(b"a b-1"), "a%20b-1");
        assert_eq!(escape(&[0xa4, 0xa2]), "%A4%A2");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_reply_is_read_with_a_length_or_in_chunks() {
        use crate::net::client::read_reply;
        use std::io::BufReader;
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let reply = read_reply(BufReader::new(&plain[..]), 1 << 20).unwrap();
        assert_eq!((reply.status, reply.body.as_slice()), (200, &b"hello"[..]));
        let chunked = b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let reply = read_reply(BufReader::new(&chunked[..]), 1 << 20).unwrap();
        assert_eq!((reply.status, reply.body.as_slice()), (201, &b"abcde"[..]));
        let until_close = b"HTTP/1.1 404 Not Found\r\n\r\nnope";
        let reply = read_reply(BufReader::new(&until_close[..]), 1 << 20).unwrap();
        assert_eq!((reply.status, reply.body.as_slice()), (404, &b"nope"[..]));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_huge_chunk_size_is_not_allocated() {
        use crate::net::client::read_reply;
        use std::io::BufReader;
        let huge = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffff\r\nabc";
        assert!(read_reply(BufReader::new(&huge[..]), 1 << 20).is_err());
        let over = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n2\r\ngh\r\n0\r\n\r\n";
        let reply = read_reply(BufReader::new(&over[..]), 4).unwrap();
        assert_eq!(reply.body, b"abcd");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn the_head_of_a_reply_has_a_limit() {
        use crate::net::client::read_reply;
        use std::io::BufReader;
        let mut long = b"HTTP/1.1 200 OK\r\nX: ".to_vec();
        long.extend(std::iter::repeat_n(b'a', 9000));
        long.extend_from_slice(b"\r\n\r\n");
        assert!(
            read_reply(BufReader::new(&long[..]), 1 << 20).is_err(),
            "一行が長すぎる"
        );
        let mut many = b"HTTP/1.1 200 OK\r\n".to_vec();
        for _ in 0..20_000 {
            many.extend_from_slice(b"X: y\r\n");
        }
        many.extend_from_slice(b"\r\n");
        assert!(
            read_reply(BufReader::new(&many[..]), 1 << 20).is_err(),
            "ヘッダーが多すぎる"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn the_request_line_cannot_be_broken() {
        use crate::net::client::request_head;
        let url = parse_url("http://example.jp/a b\r\nX-Evil: 1#frag").unwrap();
        let head = request_head("GET", &url, &[], None).unwrap();
        assert!(
            head.starts_with("GET /a%20b%0D%0AX-Evil:%201 HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(request_head("GET /x HTTP/1.1\r\nA:", &url, &[], None).is_err());
        let url = parse_url("http://a.jp\r\nX/").unwrap();
        assert!(request_head("GET", &url, &[], None).is_err(), "名前に改行");
        let url = parse_url("http://u@a.jp/").unwrap();
        assert!(
            request_head("GET", &url, &[], None).is_err(),
            "ユーザー情報"
        );
        let url = parse_url("http://a.jp/").unwrap();
        let headers = [
            (String::from("Content-Length"), String::from("0")),
            (String::from("X-Ok"), String::from("1")),
        ];
        let head = request_head("POST", &url, &headers, Some(3)).unwrap();
        assert!(head.contains("X-Ok: 1\r\n") && head.contains("Content-Length: 3\r\n"));
        assert_eq!(head.matches("Content-Length").count(), 1, "{head}");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn inner_addresses_are_told_apart() {
        use crate::net::client::is_inner;
        for inner in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(is_inner(inner.parse().unwrap()), "{inner}");
        }
        for outer in [
            "8.8.8.8",
            "203.0.113.1",
            "2001:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_inner(outer.parse().unwrap()), "{outer}");
        }
        use crate::net::client::address_of;
        assert_eq!(
            address_of("127.0.0.1", 9, false).err(),
            Some(Failure::Connect),
            "既定では内部ネットワークへ接続しない"
        );
        assert!(address_of("127.0.0.1", 9, true).is_ok(), "許せば繋ぐ");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn broadcast_roots_are_kept_per_session_and_bounded() {
        let session = super::client::Session::new();
        assert!(!session.add_root_certificate(b"not a certificate"));
        assert!(!super::client::Session::new().add_root_certificate(&[0x30, 0x82, 0, 0]));
    }
}

#[cfg(test)]
mod live_tests {
    #[test]
    fn https_is_reachable_when_asked() {
        if std::env::var_os("ZW_LIVE_TESTS").is_none() {
            return;
        }
        let reply = super::get("https://example.com/").expect("https で取れる");
        assert!(reply.status < 500, "{}", reply.status);
        assert!(!reply.body.is_empty());
    }
}
