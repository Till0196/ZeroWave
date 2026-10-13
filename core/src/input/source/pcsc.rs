// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! coaxd のカードリーダーを使う PC/SC 互換ソケットを配置する。pcsc-lite のプロトコルはネットワーク越しに送れないので、
//! ソケットはこの PC に置き、カードとのやり取りは coaxd の HTTP で行う。

use std::path::{Path, PathBuf};

use coax::daemon::Endpoint;

use super::coaxd::endpoint_of;

/// 既定のパス：`PCSCLITE_CSOCK_NAME` か、coaxd のソケットと同じフォルダーの `pcscd.comm`。
pub fn default_socket() -> PathBuf {
    coax::pcscd::default_socket()
}

#[derive(Debug)]
pub struct Pcsc {
    endpoint: Endpoint,
    path: PathBuf,
    /// ほかのプロセスが既に配置していたなら無い。
    serving: Option<coax::pcscd::Serving>,
}

impl Pcsc {
    pub fn start(
        endpoint: &str,
        path: &Path,
        report: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let endpoint = endpoint_of(endpoint);
        let path = if path.as_os_str().is_empty() {
            default_socket()
        } else {
            path.to_owned()
        };
        let serving = match coax::pcscd::serve(endpoint.clone(), &path, report) {
            Ok(serving) => Some(serving),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => None,
            Err(error) => {
                return Err(format!(
                    "{} に PC/SC 互換ソケットを配置できない: {error}",
                    path.display()
                ));
            }
        };
        Ok(Self {
            endpoint,
            path,
            serving,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ours(&self) -> bool {
        self.serving.is_some()
    }

    pub fn serves(&self, endpoint: &str) -> bool {
        endpoint_of(endpoint) == self.endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_one_leaves_it_to_the_first() {
        let dir = std::env::temp_dir().join(format!("zw-pcsc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("pcscd.comm");
        let nowhere = dir.join("no-coaxd.sock");
        let nowhere = nowhere.to_string_lossy();
        let first = Pcsc::start(&nowhere, &path, |_| {}).expect("serves");
        assert!(first.ours());
        assert_eq!(first.path(), path);
        assert!(first.serves(&nowhere));
        assert!(!first.serves("127.0.0.1:40772"));
        let second = Pcsc::start(&nowhere, &path, |_| {}).expect("leaves it to the first");
        assert!(!second.ours());
        drop(second);
        assert!(
            path.exists(),
            "既存のソケットを使った側はソケットを消さない"
        );
        drop(first);
        assert!(!path.exists(), "配置した側は消す");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_path_is_the_usual_place() {
        assert!(
            default_socket().ends_with("pcscd.comm")
                || std::env::var_os("PCSCLITE_CSOCK_NAME").is_some()
        );
    }
}
