// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ソケット（`zw_sock_v1`）。値はユーザーが入れたものだけを使い、宣言の既定値は見ない。
//! TCP はネットワーク接続なので `net = false` か `no_net` なら拒否する。
//! 値の `$名前`・`${名前}` は本体の環境変数で展開する（`$XDG_RUNTIME_DIR` や macOS の `$TMPDIR` の下のソケット）。
//! Windows では `\\.\pipe\…` を名前付きパイプとして開く。

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
    link: Link,
    ended: bool,
}

enum Link {
    Daemon(Connection),
    #[cfg(windows)]
    Pipe(pipe::Pipe),
}

impl Sock {
    fn of(link: Link) -> Self {
        Self { link, ended: false }
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match &mut self.link {
            Link::Daemon(connection) => {
                // ドメインソケットはタイムアウトを一つしか持てないので、書く前に書き込み用の長さにする。
                connection.set_read_timeout(WRITE_PATIENCE)?;
                connection.write_all(bytes)?;
                connection.flush()
            }
            #[cfg(windows)]
            Link::Pipe(pipe) => pipe.write(bytes),
        }
    }

    /// 待たずに読める分だけ。無ければ `WouldBlock`。
    fn read_some(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.link {
            Link::Daemon(connection) => connection.read(into),
            #[cfg(windows)]
            Link::Pipe(pipe) => pipe.read_ready(into),
        }
    }

    fn read(&mut self) -> Option<Vec<u8>> {
        if self.ended {
            return None;
        }
        let ready = match &self.link {
            Link::Daemon(connection) => connection.set_read_timeout(READ_PATIENCE),
            #[cfg(windows)]
            Link::Pipe(_) => Ok(()),
        };
        if ready.is_err() {
            return None;
        }
        let mut gathered = Vec::new();
        let mut chunk = vec![0u8; READ_SIZE];
        while gathered.len() < READ_MOST {
            match self.read_some(&mut chunk) {
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
    let written = expanded(&state.io.chosen_value(key).unwrap_or_default(), |name| {
        std::env::var(name).ok()
    });
    #[cfg(windows)]
    if pipe::is_pipe(&written) {
        return match pipe::Pipe::open(&written) {
            Ok(pipe) => state.socks.put(Sock::of(Link::Pipe(pipe))),
            Err(error) => {
                state.log(
                    Level::Warning,
                    &format!("zw_sock_v1: {key} へ接続できない: {error}"),
                );
                0
            }
        };
    }
    let endpoint = Endpoint::parse(&written);
    if matches!(endpoint, Endpoint::Network(_)) && state.io.net_closed() {
        state.log(
            Level::Warning,
            &format!("zw_sock_v1: ネットワークが無効なので {written} へは接続しない"),
        );
        return 0;
    }
    match Connection::to(&endpoint) {
        Ok(connection) => state.socks.put(Sock::of(Link::Daemon(connection))),
        Err(error) => {
            state.log(
                Level::Warning,
                &format!("zw_sock_v1: {key} へ接続できない: {error}"),
            );
            0
        }
    }
}

/// ユーザーが入れた値の前後の空白を落とし、環境変数を展開する。
fn expanded(written: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    crate::host::proc::expand(written.trim(), &lookup)
}

/// Windows の名前付きパイプ。待たずに読むため、`PeekNamedPipe` でたまっている分だけを読む。
#[cfg(windows)]
mod pipe {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;

    /// `\\.\pipe\名前`（`\\?\pipe\`、`/` 区切りも）。
    pub(super) fn is_pipe(written: &str) -> bool {
        let head: String = written
            .chars()
            .take(9)
            .map(|c| {
                if c == '/' {
                    '\\'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        head == r"\\.\pipe\" || head == r"\\?\pipe\"
    }

    pub(super) struct Pipe(File);

    impl Pipe {
        pub(super) fn open(path: &str) -> std::io::Result<Self> {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map(Self)
        }

        pub(super) fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.0.write_all(bytes)?;
            self.0.flush()
        }

        /// 相手が閉じていれば `PeekNamedPipe` が失敗する（`ERROR_BROKEN_PIPE`）。
        pub(super) fn read_ready(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
            let mut ready = 0u32;
            // SAFETY: ハンドルは `File` が持っていて生きている。書かせるのは `ready` だけ。
            unsafe {
                PeekNamedPipe(
                    HANDLE(self.0.as_raw_handle()),
                    None,
                    0,
                    None,
                    Some(&mut ready),
                    None,
                )
            }
            .map_err(|_| std::io::Error::last_os_error())?;
            if ready == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
            }
            let n = into.len().min(ready as usize);
            self.0.read(&mut into[..n])
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
        let mut sock = Sock::of(Link::Daemon(Connection::to(&endpoint).expect("接続する")));
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

#[cfg(test)]
mod written_tests {
    use super::*;

    #[test]
    fn the_value_expands_the_environment() {
        let lookup = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some(String::from("/run/user/1000")),
            "TMPDIR" => Some(String::from("/var/folders/ab/T/")),
            _ => None,
        };
        assert_eq!(
            expanded(" $XDG_RUNTIME_DIR/discord-ipc-0 ", lookup),
            "/run/user/1000/discord-ipc-0"
        );
        assert_eq!(
            expanded("${TMPDIR}discord-ipc-0", lookup),
            "/var/folders/ab/T/discord-ipc-0"
        );
        assert_eq!(expanded("localhost:40772", lookup), "localhost:40772");
        assert_eq!(expanded("", lookup), "", "空は coaxd のまま");
    }
}

#[cfg(all(test, windows))]
mod pipe_tests {
    use super::*;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::PCWSTR;

    #[test]
    fn only_pipe_paths_are_pipes() {
        assert!(pipe::is_pipe(r"\\.\pipe\discord-ipc-0"));
        assert!(pipe::is_pipe(r"\\?\PIPE\x"));
        assert!(pipe::is_pipe("//./pipe/x"));
        assert!(!pipe::is_pipe(r"C:\Users\me\coaxd.sock"));
        assert!(!pipe::is_pipe("localhost:40772"));
    }

    /// 偽の相手が最初の四バイトを読んで "pong" を返し、閉じる。
    #[test]
    fn a_pipe_writes_reads_without_waiting_and_ends() {
        let name = format!(r"\\.\pipe\zw-sock-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` は終端されていて呼び出しより長生きする。ほかは値だけ。
        let server = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                None,
            )
        };
        assert!(!server.is_invalid(), "パイプが作れる");
        let raw = server.0 as usize;
        let peer = std::thread::spawn(move || {
            use std::os::windows::io::FromRawHandle;
            let server = HANDLE(raw as *mut _);
            // 先につながれていれば ERROR_PIPE_CONNECTED が返るが、それもつながっている。
            // SAFETY: 作ったばかりのハンドルで、同期で待つ。
            let _ = unsafe { ConnectNamedPipe(server, None) };
            // SAFETY: ハンドルの所有を File に移す（落ちたら閉じて、相手に終わりが伝わる）。
            let mut file = unsafe { std::fs::File::from_raw_handle(server.0) };
            let mut got = [0u8; 4];
            file.read_exact(&mut got).expect("読む");
            file.write_all(b"pong").expect("書く");
            got
        });
        let mut sock = Sock::of(Link::Pipe(pipe::Pipe::open(&name).expect("開ける")));
        assert_eq!(
            sock.read(),
            Some(Vec::new()),
            "何も来ていなければ待たずに空"
        );
        sock.write(b"ping").expect("書ける");
        assert_eq!(&peer.join().expect("相手"), b"ping");
        let mut heard = Vec::new();
        for _ in 0..200 {
            match sock.read() {
                Some(bytes) => heard.extend(bytes),
                None => break,
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(heard, b"pong");
        assert_eq!(sock.read(), None, "相手が閉じたら終わり");
    }
}
