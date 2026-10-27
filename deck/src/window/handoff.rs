// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画のファイルを渡されて起動したとき、ほかの deck が動いていればそちらで開く（macOS では OS がそうする）。
//! 渡すのは絶対パス一つ。Windows は名前付きパイプ、Linux は Unix ドメインソケットで、名前は状態の置き場ごと。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::threads::link::{Act, Request, ToStage, Wake};

/// 動いている deck が受け口を開くまで待つ長さ（同時に起動したとき）。
const PATIENCE: Duration = Duration::from_secs(3);

/// 動いている deck にファイルを渡す。渡せたら真。
pub fn hand(path: &str) -> bool {
    let Ok(path) = std::path::absolute(path) else {
        return false;
    };
    let Some(name) = name() else {
        return false;
    };
    let until = Instant::now() + PATIENCE;
    loop {
        match send(&name, &path) {
            Ok(()) => {
                tracing::info!("動いている deck で開く: {}", path.display());
                return true;
            }
            Err(error) if Instant::now() >= until => {
                tracing::info!("動いている deck に渡せない: {error}");
                return false;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// 受け口を開き、届いたファイルを再生スレッドへ渡してウィンドウを前に出す。二重起動防止のロックを持つ deck だけが呼ぶ。
pub fn listen(
    to_stage: Sender<ToStage>,
    requests: Sender<Request>,
    proxy: winit::event_loop::EventLoopProxy<Wake>,
) {
    let Some(name) = name() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name(String::from("handoff"))
        .spawn(move || {
            let opened = move |path: PathBuf| {
                if !crate::window::opening::playable(&path) {
                    tracing::info!(
                        "{} は TS でも MMT/TLV でもない（内容で判定）",
                        path.display()
                    );
                    return;
                }
                tracing::info!("ほかの起動から開く: {}", path.display());
                let _ = to_stage.send(ToStage::Act(Act::Open(path)));
                let _ = requests.send(Request::Raise);
                let _ = proxy.send_event(Wake);
            };
            if let Err(error) = serve(&name, opened) {
                tracing::warn!("ほかの起動からファイルを受け取れない: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("ファイルを受け取るスレッドを起動できない: {error}");
    }
}

/// 状態の置き場ごとの名前（`ZW_STATE_DIR` やプロファイルの違う deck どうしは渡し合わない）。
fn name() -> Option<String> {
    let lock = crate::config::path_of("zerowave.instance")?;
    let text = lock.to_string_lossy();
    // Windows では大文字と小文字の違う二つの書き方が同じファイル。
    let text = if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    };
    Some(format!("zerowave-{:08x}", crc32fast::hash(text.as_bytes())))
}

fn read_all(from: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    from.take(64 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(windows)]
fn send(name: &str, path: &Path) -> std::io::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    let mut pipe = std::fs::OpenOptions::new()
        .write(true)
        .open(format!(r"\\.\pipe\{name}"))?;
    // 受け取った側がウィンドウを前に出せるように（前面のプロセスしか許されない）。
    // SAFETY: 引数を取らない許可。
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    pipe.write_all(path.to_string_lossy().as_bytes())
}

#[cfg(windows)]
fn serve(name: &str, opened: impl Fn(PathBuf)) -> std::io::Result<()> {
    use std::os::windows::io::FromRawHandle as _;
    use windows::Win32::Foundation::ERROR_PIPE_CONNECTED;
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    let wide: Vec<u16> = format!(r"\\.\pipe\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    loop {
        // SAFETY: 名前は終端付きのローカル。既定のセキュリティ記述子では同じユーザーと管理者だけが書ける。
        let pipe = unsafe {
            CreateNamedPipeW(
                windows::core::PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                4096,
                0,
                None,
            )
        };
        if pipe.is_invalid() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: いま作ったハンドル。File が閉じる。
        let file = unsafe { std::fs::File::from_raw_handle(pipe.0) };
        // SAFETY: 同上。待っている間に繋がれていれば ERROR_PIPE_CONNECTED。
        let connected = unsafe { ConnectNamedPipe(pipe, None) };
        if let Err(error) = connected
            && error.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            continue;
        }
        if let Ok(bytes) = read_all(&file) {
            opened(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
}

#[cfg(target_os = "linux")]
fn socket(name: &str) -> PathBuf {
    // 実行時の置き場（ユーザーだけが入れる）に。無ければ状態の置き場。
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .or_else(|| {
            crate::config::path_of("zerowave.instance")
                .and_then(|lock| lock.parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("{name}.sock"))
}

#[cfg(target_os = "linux")]
fn send(name: &str, path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut stream = std::os::unix::net::UnixStream::connect(socket(name))?;
    stream.write_all(path.as_os_str().as_bytes())
}

#[cfg(target_os = "linux")]
fn serve(name: &str, opened: impl Fn(PathBuf)) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let at = socket(name);
    // 前の deck が落ちて残したソケット（ロックを持つのはこの deck だけなので消してよい）。
    let _ = std::fs::remove_file(&at);
    let listener = std::os::unix::net::UnixListener::bind(&at)?;
    for stream in listener.incoming() {
        if let Ok(bytes) = stream.and_then(read_all) {
            opened(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)));
        }
    }
    Ok(())
}
