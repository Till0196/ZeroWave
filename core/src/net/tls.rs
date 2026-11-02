// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! TLS の設定。証明書は OS のストア（rustls-platform-verifier）で確かめ、通らなければ同梱のルート
//! （webpki-roots）で確かめ直す。どちらにも、渡されたルートと `ZW_CA_DIR` の PEM を足す。
//! `sha1` のときだけ `sha1WithRSAEncryption` の証明書も受ける（古い局のサーバー）。
//! デバッグビルドでは `ZW_TLS_INSECURE=1` で確かめない（試すため）。

use std::sync::{Arc, OnceLock};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

/// `extra` は DER。無ければ一つを使い回す。
pub fn config(extra: &[Vec<u8>], sha1: bool) -> Arc<rustls::ClientConfig> {
    static PLAIN: [OnceLock<Arc<rustls::ClientConfig>>; 2] = [OnceLock::new(), OnceLock::new()];
    if extra.is_empty() {
        return Arc::clone(PLAIN[usize::from(sha1)].get_or_init(|| build(&[], sha1)));
    }
    build(extra, sha1)
}

/// ルート証明書として使えるか。
pub fn is_certificate(der: &[u8]) -> bool {
    rustls::RootCertStore::empty()
        .add(CertificateDer::from(der.to_vec()))
        .is_ok()
}

fn build(extra: &[Vec<u8>], sha1: bool) -> Arc<rustls::ClientConfig> {
    let provider = provider(sha1);
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("rustls の既定のバージョン");
    let verifier: Arc<dyn ServerCertVerifier> =
        if cfg!(debug_assertions) && std::env::var_os("ZW_TLS_INSECURE").is_some() {
            tracing::warn!("ZW_TLS_INSECURE が設定されているので、https の証明書を確かめません");
            Arc::new(Unchecked(provider))
        } else {
            let mut roots: Vec<CertificateDer<'static>> = extra
                .iter()
                .map(|der| CertificateDer::from(der.clone()))
                .collect();
            roots.extend(local_roots().iter().cloned());
            Arc::new(Either::new(roots, provider))
        };
    Arc::new(
        builder
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth(),
    )
}

/// `sha1` は証明書の署名にだけ効く（ハンドシェイクの署名の方式には足さない）。
fn provider(sha1: bool) -> Arc<CryptoProvider> {
    static PROVIDER: [OnceLock<Arc<CryptoProvider>>; 2] = [OnceLock::new(), OnceLock::new()];
    Arc::clone(PROVIDER[usize::from(sha1)].get_or_init(|| {
        let mut provider = rustls::crypto::ring::default_provider();
        if sha1 {
            let mut all = provider.signature_verification_algorithms.all.to_vec();
            all.push(&RSA_SHA1);
            provider.signature_verification_algorithms.all = Box::leak(all.into_boxed_slice());
        }
        Arc::new(provider)
    }))
}

/// `ZW_CA_DIR` の PEM。
fn local_roots() -> &'static [CertificateDer<'static>] {
    static ROOTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        use rustls::pki_types::pem::PemObject;
        let Some(dir) = std::env::var_os("ZW_CA_DIR") else {
            return Vec::new();
        };
        let mut roots = Vec::new();
        for path in std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
        {
            let Ok(pem) = std::fs::read(&path) else {
                continue;
            };
            let before = roots.len();
            roots.extend(CertificateDer::pem_slice_iter(&pem).flatten());
            if roots.len() == before {
                tracing::warn!("ルート証明書として読み込めません: {}", path.display());
            }
        }
        roots
    })
}

/// OS のストアで確かめ、通らなければ同梱のルートで。
#[derive(Debug)]
struct Either {
    os: Option<rustls_platform_verifier::Verifier>,
    bundled: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl Either {
    fn new(extra: Vec<CertificateDer<'static>>, provider: Arc<CryptoProvider>) -> Self {
        let os = rustls_platform_verifier::Verifier::new_with_extra_roots(
            extra.iter().cloned(),
            Arc::clone(&provider),
        )
        .inspect_err(|why| tracing::warn!("OS の証明書ストアを使えない: {why}"))
        .ok();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        roots.add_parsable_certificates(extra);
        let bundled =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
                .build()
                .ok();
        Self {
            os,
            bundled,
            provider,
        }
    }
}

impl ServerCertVerifier for Either {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verify = |verifier: &dyn ServerCertVerifier| {
            verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        };
        let by_os = self.os.as_ref().map(|os| verify(os));
        if let Some(Ok(verified)) = by_os {
            return Ok(verified);
        }
        let failed = || rustls::Error::General(String::from("証明書を確かめる手段が無い"));
        match self.bundled.as_deref().map(|bundled| verify(bundled)) {
            Some(Ok(verified)) => Ok(verified),
            // OS の答えの方が分かりやすい（期限切れ・名前違いなど）。
            Some(Err(why)) => Err(by_os.and_then(Result::err).unwrap_or(why)),
            None => Err(by_os.and_then(Result::err).unwrap_or_else(failed)),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct Unchecked(Arc<CryptoProvider>);

impl ServerCertVerifier for Unchecked {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
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
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
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

#[cfg(test)]
mod tests {
    use rustls::pki_types::SignatureVerificationAlgorithm as _;

    use super::*;

    #[test]
    fn sha1_is_accepted_only_where_asked() {
        let has = |sha1: bool| {
            provider(sha1)
                .signature_verification_algorithms
                .all
                .iter()
                .any(|alg| alg.signature_alg_id() == RSA_SHA1.signature_alg_id())
        };
        assert!(has(true));
        assert!(!has(false));
    }
}
