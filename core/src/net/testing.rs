// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 試すための小さなサーバーと認証局。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;

/// 認証局（DER）と、それが 127.0.0.1 に出した証明書で応えるサーバーの設定。
pub fn authority() -> (Vec<u8>, Arc<rustls::ServerConfig>) {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![String::from("127.0.0.1")])
        .unwrap()
        .signed_by(&key, &ca)
        .unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone(), ca.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
    .unwrap();
    (ca.der().to_vec(), Arc::new(server))
}

pub trait Both: Read + Write {}
impl<T: Read + Write> Both for T {}

/// 接続ごとに `answer(要求の頭, 接続)` を呼ぶ。返るのは受けた要求の頭。
pub fn serve(
    connections: usize,
    tls: Option<Arc<rustls::ServerConfig>>,
    answer: impl Fn(&str, &mut dyn Both) + Send + 'static,
) -> (u16, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = std::thread::spawn(move || {
        let mut heads = Vec::new();
        for _ in 0..connections {
            let Ok((tcp, _)) = listener.accept() else {
                break;
            };
            let mut stream: Box<dyn Both> = match &tls {
                Some(config) => Box::new(rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::clone(config)).unwrap(),
                    tcp,
                )),
                None => Box::new(tcp),
            };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            answer(&head, &mut *stream);
            let _ = stream.flush();
            heads.push(head);
        }
        heads
    });
    (port, serving)
}
