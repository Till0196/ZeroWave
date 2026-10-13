// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ソケット（`zw_sock_v1`）。値はユーザーが入れたものだけを使い、宣言の既定値は見ない。
//! TCP はネットワーク接続なので `net = false` か `no_net` なら拒否する。

use std::io::{Read, Write};
use std::time::Duration;

use coax::daemon::{Connection, Endpoint};
use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::{HostState, Level, guest_bytes, guest_text};
use crate::slots::Slots;

/// 再生のスレッドを止めっぱなしにしないため。
const WRITE_PATIENCE: Duration = Duration::from_secs(3);

const READ_PATIENCE: Duration = Duration::from_millis(1);

const READ_SIZE: usize = 64 * 1024;

const READ_MOST: usize = 4 << 20;

pub(crate) type Socks = Slots<Sock>;

pub(crate) struct Sock {
    connection: Connection,
    ended: bool,
}

impl Sock {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        // ドメインソケットはタイムアウトを一つしか持てないので、書く前に書き込み用の長さにする。
        self.connection.set_read_timeout(WRITE_PATIENCE)?;
        self.connection.write_all(bytes)?;
        self.connection.flush()
    }

    fn read(&mut self) -> Option<Vec<u8>> {
        if self.ended {
            return None;
        }
        if self.connection.set_read_timeout(READ_PATIENCE).is_err() {
            return None;
        }
        let mut gathered = Vec::new();
        let mut chunk = vec![0u8; READ_SIZE];
        while gathered.len() < READ_MOST {
            match self.connection.read(&mut chunk) {
                Ok(0) => {
                    self.ended = true;
                    break;
                }
                Ok(read) => gathered.extend_from_slice(&chunk[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    break;
                }
                Err(_) => {
                    self.ended = true;
                    break;
                }
            }
        }
        (!self.ended || !gathered.is_empty()).then_some(gathered)
    }
}

fn connect_keyed(state: &mut HostState, key: &str) -> u32 {
    if !state.io.sockets.contains(key) {
        state.log(
            Level::Warning,
            &format!("zw_sock_v1: キー {key} は kind = \"socket\" と宣言されていない"),
        );
        return 0;
    }
    // 宣言の既定値は見ない（プラグインが自分で接続先を決められないように）。
    let written = state.io.chosen_value(key).unwrap_or_default();
    let endpoint = Endpoint::parse(written.trim());
    if matches!(endpoint, Endpoint::Network(_)) && state.io.net_closed() {
        state.log(
            Level::Warning,
            &format!("zw_sock_v1: ネットワークが無効なので {written} へは接続しない"),
        );
        return 0;
    }
    match Connection::to(&endpoint) {
        Ok(connection) => state.socks.put(Sock {
            connection,
            ended: false,
        }),
        Err(error) => {
            state.log(
                Level::Warning,
                &format!("zw_sock_v1: {key} へ接続できない: {error}"),
            );
            0
        }
    }
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::SOCK,
        "connect",
        |mut caller: Caller<'_, HostState>, key: u32, key_len: u32| -> u32 {
            let key = guest_text(&mut caller, key, key_len);
            connect_keyed(caller.data_mut(), &key)
        },
    )?;
    linker.func_wrap(
        abi::import::SOCK,
        "write",
        |mut caller: Caller<'_, HostState>, handle: u32, ptr: u32, len: u32| -> u32 {
            let Some(bytes) = guest_bytes(&mut caller, ptr, len) else {
                return 0;
            };
            let state = caller.data_mut();
            let Some(sock) = state.socks.get_mut(handle) else {
                return 0;
            };
            match sock.write(&bytes) {
                Ok(()) => 1,
                Err(error) => {
                    state.log(Level::Warning, &format!("zw_sock_v1: 書けない: {error}"));
                    0
                }
            }
        },
    )?;
    linker.func_wrap(
        abi::import::SOCK,
        "read",
        |mut caller: Caller<'_, HostState>, handle: u32| -> u32 {
            let state = caller.data_mut();
            match state.socks.get_mut(handle).and_then(Sock::read) {
                Some(bytes) => state.put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::SOCK,
        "close",
        |mut caller: Caller<'_, HostState>, handle: u32| {
            caller.data_mut().socks.take(handle);
        },
    )?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_socket_writes_reads_without_waiting_and_ends() {
        let directory = std::env::temp_dir().join(format!("zw-sock-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("echo.sock");
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("accept");
            let mut got = [0u8; 4];
            peer.read_exact(&mut got).expect("読む");
            peer.write_all(b"pong").expect("書く");
            got
        });
        let endpoint = Endpoint::parse(path.to_str().expect("パス"));
        let mut sock = Sock {
            connection: Connection::to(&endpoint).expect("接続する"),
            ended: false,
        };
        assert_eq!(sock.read(), Some(Vec::new()));
        sock.write(b"ping").expect("書ける");
        assert_eq!(&server.join().expect("相手"), b"ping");
        let mut heard = Vec::new();
        for _ in 0..200 {
            match sock.read() {
                Some(bytes) => heard.extend(bytes),
                None => break,
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(heard, b"pong");
        assert_eq!(sock.read(), None);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
