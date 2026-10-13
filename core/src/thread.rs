// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 期限付きで終了できるスレッド。終了を別のチャネルで受け取ってから `join` する。

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// deck のウォッチドッグ（5 秒）より短く。二つともタイムアウトしても超えないように。
pub const PATIENCE: Duration = Duration::from_secs(2);

/// drop されること（panic の巻き戻しでも）が終了の通知。
#[derive(Debug)]
pub struct Told(#[allow(dead_code)] Sender<()>);

#[derive(Debug)]
pub struct Thread {
    thread: Option<std::thread::JoinHandle<()>>,
    ends: Receiver<()>,
    called: &'static str,
}

impl Thread {
    /// `name` は OS に付ける英語の名前、`called` はログに書く名前。
    pub fn spawn(
        name: &'static str,
        called: &'static str,
        body: impl FnOnce(Told) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (told, ends) = channel();
        let thread = std::thread::Builder::new()
            .name(String::from(name))
            .spawn(move || body(Told(told)))?;
        Ok(Self {
            thread: Some(thread),
            ends,
            called,
        })
    }

    /// タイムアウトしたときは `join` しない（終了しないスレッドを待つことになる）。
    pub fn join_within(&mut self, by: Duration) -> bool {
        let Some(thread) = self.thread.take() else {
            return true;
        };
        match self.ends.recv_timeout(by) {
            Err(RecvTimeoutError::Disconnected) => {
                if thread.join().is_err() {
                    tracing::warn!("{}のスレッドが異常終了した", self.called);
                }
                true
            }
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                tracing::info!(
                    "{}のスレッドが {by:?} 以内に終了しないため、待たずに進む",
                    self.called
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_a_thread_that_ends() {
        let mut thread = Thread::spawn("done", "終了", |_told| {}).expect("スレッドを起動できる");
        assert!(thread.join_within(PATIENCE));
        assert!(thread.join_within(Duration::from_millis(0)));
    }

    #[test]
    fn gives_up_on_a_thread_that_does_not() {
        let (hold, held) = channel::<()>();
        let mut thread = Thread::spawn("stuck", "応答なし", move |_told| {
            let _ = held.recv();
        })
        .expect("スレッドを起動できる");
        let by = Duration::from_millis(50);
        let began = std::time::Instant::now();
        assert!(!thread.join_within(by));
        assert!(began.elapsed() >= by);
        drop(hold);
    }

    #[test]
    fn folds_a_thread_that_panics() {
        let mut thread = Thread::spawn("panics", "panic", |_told| panic!("わざと"))
            .expect("スレッドを起動できる");
        assert!(thread.join_within(PATIENCE));
    }
}
