// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チューナーをもう一つ確保する（`zw_tuner_v1`）。インスタンスがクラッシュすれば確保したものも全部解放する。

use std::io::Read;
use std::sync::Arc;

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::pump::Inbox;
use crate::host::{HostState, Level, guest_text};
use crate::slots::Slots;

/// バッファが一杯になったら捨てる（ライブの放送は詰まらせるより捨てる）。
const MOST_HELD: usize = 512;

/// いまの局を開いたソースが同じ相手からもう一本開けるように。
pub trait Lender: Send + Sync {
    /// `NAME=VALUE&…`。
    fn words(&self) -> String;
    fn open(&self, words: &str) -> Result<Lent, String>;
}

pub struct Lent {
    pub bytes: Box<dyn Read + Send>,
    /// 接続を閉じて読んでいるスレッドを止める。何度呼んでもよい。
    pub stop: Box<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for Lent {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Lent").finish_non_exhaustive()
    }
}

pub type LenderOf = Arc<dyn Lender>;

pub(crate) type Borrowed = Slots<Held>;

pub(crate) struct Held {
    stop: Box<dyn Fn() + Send + Sync>,
    arrived: Inbox,
}

impl Drop for Held {
    fn drop(&mut self) {
        (self.stop)();
    }
}

impl Held {
    fn new(lent: Lent) -> std::io::Result<Self> {
        let Lent { bytes, stop } = lent;
        let arrived = Inbox::spawn("tuner-lent", bytes, Some(MOST_HELD))?;
        Ok(Self { stop, arrived })
    }
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::TUNER,
        "words",
        |mut caller: Caller<'_, HostState>| -> u32 {
            let state = caller.data_mut();
            match state.io.lender.clone() {
                Some(lender) => state.put_buf(lender.words().into_bytes()),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::TUNER,
        "open",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            let words = guest_text(&mut caller, ptr, len);
            let state = caller.data_mut();
            let Some(lender) = state.io.lender.clone() else {
                state.log(
                    Level::Warning,
                    "チューナーを確保できない（hooks に \"tuners\" が無いか、現在のソースが対応していない）",
                );
                return 0;
            };
            match lender.open(&words).and_then(|lent| Held::new(lent).map_err(|e| e.to_string())) {
                Ok(held) => {
                    state.log(Level::Info, &format!("チューナーをもう一つ確保した（{words}）"));
                    state.borrowed.put(held)
                }
                Err(why) => {
                    state.log(Level::Warning, &format!("チューナーを確保できない: {why}"));
                    0
                }
            }
        },
    )?;
    linker.func_wrap(
        abi::import::TUNER,
        "read",
        |mut caller: Caller<'_, HostState>, handle: u32| -> u32 {
            let state = caller.data_mut();
            match state
                .borrowed
                .get(handle)
                .and_then(|held| held.arrived.drain())
            {
                Some(bytes) => state.put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::TUNER,
        "close",
        |mut caller: Caller<'_, HostState>, handle: u32| {
            caller.data_mut().borrowed.take(handle);
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lent_stream_is_read_as_it_arrives_and_stopped_when_dropped() {
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&stopped);
        let held = Held::new(Lent {
            bytes: Box::new(std::io::Cursor::new(vec![7u8; 100_000])),
            stop: Box::new(move || seen.store(true, std::sync::atomic::Ordering::SeqCst)),
        })
        .expect("a thread");
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Some(bytes) = held.arrived.drain() {
            got.extend(bytes);
            assert!(std::time::Instant::now() < deadline, "終わらない");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(got.len(), 100_000);
        drop(held);
        assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
    }
}
