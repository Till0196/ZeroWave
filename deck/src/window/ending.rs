// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 終了処理。正常に終了できなければ強制的に終了する。順序：ウォッチドッグ → ソース → 別ウィンドウ → 再生スレッド → 音声出力 → ウォッチドッグを止める。
//! 固まりうるスレッドは join せず置いて行く（[`Terminator`]）。

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const WATCHDOG: Duration = Duration::from_secs(5);
/// 終了するときの番組表の保存を待つ長さ。ウィンドウを閉じた後なので、長くても目に見えない。
pub const GUIDE_SAVING: Duration = Duration::from_secs(30);

pub use zwcore::thread::{PATIENCE, Thread};

/// プロセスを終える。Linux では `atexit` を通らない（置いて行ったスレッドが EGL や VA-API を
/// 使っている間に Mesa や libva の後始末が走ると落ちる）。
pub fn exit_now(code: i32) -> ! {
    crate::log::finish();
    // 二重起動防止のロックは先に解放する（Linux の `_exit` は Drop を実行しないので、ロックファイルが残る）。
    crate::config::release_single();
    // stdout・stderr のロックは取らない（止まったスレッドが持っていると戻らない）。
    #[cfg(target_os = "linux")]
    // SAFETY: 戻らない。
    unsafe {
        libc::_exit(code)
    }
    #[cfg(not(target_os = "linux"))]
    std::process::exit(code)
}

#[derive(Debug)]
pub struct Terminator {
    stand: Option<Sender<()>>,
    at: Arc<Mutex<&'static str>>,
}

pub fn watch(by: Duration) -> Terminator {
    let (stand, down) = channel::<()>();
    let at = Arc::new(Mutex::new("終了の準備"));
    let saying = Arc::clone(&at);
    let watching = std::thread::Builder::new()
        .name(String::from("ending"))
        .spawn(move || {
            if let Err(RecvTimeoutError::Timeout) = down.recv_timeout(by) {
                let at = saying.lock().map_or("不明", |at| *at);
                tracing::info!("{by:?} 以内に終了しない（{at}）。プロセスを強制終了する");
                if crate::config::restart_requested() {
                    crate::restart();
                }
                crate::window::ending::exit_now(1);
            }
        });
    if let Err(error) = &watching {
        tracing::warn!("終了を監視するスレッドを起動できない: {error}");
    }
    Terminator {
        stand: Some(stand),
        at,
    }
}

impl Terminator {
    pub fn at(&self, what: &'static str) {
        if let Ok(mut at) = self.at.lock() {
            *at = what;
        }
    }

    pub fn stood_down(mut self) {
        self.stand = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminator_stood_down_does_not_fire() {
        let by = Duration::from_millis(50);
        watch(by).stood_down();
        std::thread::sleep(by * 4);
        drop(watch(by));
        std::thread::sleep(by * 4);
    }
}
