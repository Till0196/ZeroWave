// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ログの出し先。端末から起動したらその端末へ。そうでなければ `[log] file` が真のときだけ
//! fd 2 を `deck.log` に付け替える。`ZW_LOG=パス` なら設定に関わらずそのファイルへ。

use std::path::PathBuf;

/// 設定の `[log]`。ログは何より先に決めるので、この節だけを先に読む。
fn settings() -> &'static crate::config::file::Log {
    static READ: std::sync::OnceLock<crate::config::file::Log> = std::sync::OnceLock::new();
    READ.get_or_init(|| {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Only {
            log: crate::config::file::Log,
        }
        crate::config::path_of("zerowave.toml")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str::<Only>(&text).ok())
            .map(|only| only.log)
            .unwrap_or_default()
    })
}

fn path() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("ZW_LOG") {
        return Some(PathBuf::from(named));
    }
    crate::state::file("zerowave.log")
}

pub fn init() {
    use tracing_subscriber::{EnvFilter, fmt};
    // 外のライブラリは警告から。プラグインをコンパイルするとき wasmtime と cranelift は TRACE を数万行出し、
    // 書き込みに追い付かず起動が分単位で遅れる。対象の名前は前方一致（`wasmtime` で `wasmtime_environ` も）。
    const QUIET: &str = ",wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui=warn,\
        egui_wgpu=warn,egui_winit=warn,winit=warn,cpal=warn,wasmtime=warn,cranelift=warn";
    let filter = std::env::var("ZW_LOG_LEVEL")
        .ok()
        .and_then(|asked| EnvFilter::try_new(format!("{asked}{QUIET}")).ok())
        .or_else(|| EnvFilter::try_new(format!("{}{QUIET}", settings().level)).ok())
        .unwrap_or_else(|| EnvFilter::new(format!("info{QUIET}")));
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(ansi)
        .with_timer(fmt::time::uptime())
        .try_init();
}

pub fn attach() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // 親の端末に繋ぎ直す。繋げたら stdout も（`--check` の結果はそちら）。
        if console::attach_parent() && std::env::var_os("ZW_LOG").is_none() {
            return None;
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() && std::env::var_os("ZW_LOG").is_none() {
            return None;
        }
    }
    if std::env::var_os("ZW_LOG").is_none() && !settings().file {
        return None;
    }
    let mut path = path()?;
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    // 同時に動くもう一つの deck のログを消さない。
    if !own(&path) {
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        path.set_file_name(format!("{stem}-{}.log", std::process::id()));
    }
    let file = create(&path).ok()?;
    redirect(file).then_some(path)
}

fn own(path: &std::path::Path) -> bool {
    match crate::config::lock(path, false) {
        Ok(Some(held)) => {
            std::mem::forget(held);
            true
        }
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Windows では一行ずつディスクまで書く（OS ごと落ちたとき最後の行が消えないように）。
fn create(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    if settings().keep {
        options.append(true).create(true);
    } else {
        options.write(true).create(true).truncate(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options.custom_flags(FILE_FLAG_WRITE_THROUGH);
    }
    options.open(path)
}

#[cfg(unix)]
fn redirect(file: std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: `dup2` は生の fd を受けるだけ。複製された 2 は独立に残る。
    unsafe { libc::dup2(file.as_raw_fd(), 2) == 2 }
}

/// Windows では書くのはログのスレッド（レンダリングのスレッドが書き込みで止まってフレームを捨てないように）。
#[cfg(windows)]
fn redirect(file: std::fs::File) -> bool {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let Ok((reader, writer)) = std::io::pipe() else {
        return false;
    };
    let handle = HANDLE(writer.into_raw_handle());
    // SAFETY: 有効なパイプの書き込み側を標準ハンドルに据えるだけ。
    let set = unsafe {
        SetStdHandle(STD_ERROR_HANDLE, handle).is_ok()
            && SetStdHandle(STD_OUTPUT_HANDLE, handle).is_ok()
    };
    set && writing::start(reader, file)
}

#[cfg(windows)]
mod writing {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{TrySendError, sync_channel};
    use std::time::{Duration, Instant};

    /// 待ちのチャンクの数の上限（一つ 64 KiB まで）。溢れた分は捨てて、その数を後で書く。
    const HELD: usize = 1024;

    static PENDING: AtomicUsize = AtomicUsize::new(0);
    static DROPPED: AtomicUsize = AtomicUsize::new(0);
    static LAST: AtomicU64 = AtomicU64::new(0);
    static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

    fn now() -> u64 {
        STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    pub fn start(mut reader: std::io::PipeReader, mut file: std::fs::File) -> bool {
        let (send, receive) = sync_channel::<Vec<u8>>(HELD);
        let read = std::thread::Builder::new()
            .name(String::from("log read"))
            .spawn(move || {
                let mut chunk = vec![0u8; 64 << 10];
                while let Ok(read) = reader.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    LAST.store(now(), Ordering::Release);
                    PENDING.fetch_add(1, Ordering::AcqRel);
                    match send.try_send(chunk[..read].to_vec()) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            PENDING.fetch_sub(1, Ordering::AcqRel);
                            DROPPED.fetch_add(1, Ordering::AcqRel);
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
            });
        let write = std::thread::Builder::new()
            .name(String::from("log write"))
            .spawn(move || {
                for bytes in receive {
                    let dropped = DROPPED.swap(0, Ordering::AcqRel);
                    if dropped > 0 {
                        let _ = writeln!(
                            file,
                            "（書き込みが追い付かず、ログを {dropped} 回分捨てました）"
                        );
                    }
                    let _ = file.write_all(&bytes);
                    PENDING.fetch_sub(1, Ordering::AcqRel);
                    LAST.store(now(), Ordering::Release);
                }
            });
        read.is_ok() && write.is_ok()
    }

    /// 溜まっている分を書き切るのを待つ（長くて一秒）。待たないと panic のメッセージが消える。
    pub fn finish() {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(1) {
            let idle = now().saturating_sub(LAST.load(Ordering::Acquire));
            if PENDING.load(Ordering::Acquire) == 0 && idle >= 60 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn finish() {
    #[cfg(windows)]
    writing::finish();
}

#[cfg(not(any(unix, windows)))]
fn redirect(_file: std::fs::File) -> bool {
    false
}

#[cfg(windows)]
mod console {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    pub fn attach_parent() -> bool {
        // SAFETY: 引数は定数で、失敗は戻り値で返す。
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS).is_ok() }
    }
}

/// 致命的なエラー：ログに残し、ファイラーから起動されたならウィンドウにも出す。
pub fn fatal(text: &str) {
    tracing::info!("{text}");
    #[cfg(windows)]
    if !attached_to_terminal() {
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};
        crate::window::win32::message_box("ZeroWave", text, MB_OK | MB_ICONERROR);
    }
}

#[cfg(windows)]
fn attached_to_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}
