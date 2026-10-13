// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The queue between the player and the thread or callback feeding the wire.
//! Padding written to keep the link from underrunning is not counted.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub const FRAME: usize = 4;

/// It has to give up: the device's delay is only learnt by asking.
const PATIENCE: Duration = Duration::from_millis(50);

/// HBR（192 kHz の 8 チャネル）で 2 秒ほど。
const QUEUE_MOST: usize = 6 << 20;

#[derive(Debug, Default)]
pub struct Pump {
    queue: Mutex<VecDeque<u8>>,
    woke: Condvar,
    handed: AtomicU64,
    delay: AtomicU64,
    stop: AtomicBool,
    dead: AtomicBool,
}

impl Pump {
    pub fn new() -> Self {
        Self::default()
    }

    /// ブロックしない（捨てたバーストはレシーバーで音の途切れになる）。書き込み側が終わっていれば偽、
    /// 上限を超えたら古い方から捨てる。
    pub fn push(&self, burst: &[u8]) -> bool {
        if self.dead.load(Ordering::Relaxed) {
            return false;
        }
        let mut queue = self.lock();
        queue.extend(burst);
        let held = queue.len();
        if held > QUEUE_MOST {
            let over = (held - QUEUE_MOST).div_ceil(FRAME) * FRAME;
            queue.drain(..over.min(held));
        }
        drop(queue);
        self.woke.notify_one();
        true
    }

    /// 出力先が無くなった。止まるのを待たずに捨てる。
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn die(&self) {
        self.dead.store(true, Ordering::Relaxed);
        self.lock().clear();
    }

    /// Gives back whole frames only.
    pub fn take(&self, most: usize, out: &mut Vec<u8>) {
        out.clear();
        let mut queue = self.lock();
        if queue.is_empty() && !self.stop.load(Ordering::Relaxed) {
            queue = match self.woke.wait_timeout(queue, PATIENCE) {
                Ok((queue, _)) => queue,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        let want = (most.min(queue.len()) / FRAME) * FRAME;
        out.extend(queue.drain(..want));
    }

    /// PipeWire 用：グラフのスレッドが止まると他のストリームも止まるので待たない。
    #[cfg(target_os = "linux")]
    pub fn try_take(&self, most: usize, out: &mut Vec<u8>) {
        out.clear();
        let mut queue = self.lock();
        let want = (most.min(queue.len()) / FRAME) * FRAME;
        out.extend(queue.drain(..want));
    }

    pub fn handed(&self, frames: u64) {
        self.handed.fetch_add(frames, Ordering::Relaxed);
    }

    pub fn holding(&self, frames: u64) {
        self.delay.store(frames, Ordering::Relaxed);
    }

    /// Saturating: at first the device holds only the padding that opened the link.
    pub fn played(&self) -> u64 {
        self.handed
            .load(Ordering::Relaxed)
            .saturating_sub(self.delay.load(Ordering::Relaxed))
    }

    pub fn queued(&self) -> usize {
        let waiting = self.lock().len() / FRAME;
        waiting + self.delay.load(Ordering::Relaxed) as usize
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.woke.notify_all();
    }

    pub fn closing(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<u8>> {
        match self.queue.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FRAME, Pump};

    #[test]
    fn what_goes_in_comes_out_in_whole_frames() {
        let pump = Pump::new();
        pump.push(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut out = Vec::new();
        pump.take(6, &mut out);
        assert_eq!(out, [1, 2, 3, 4]);
        pump.take(64, &mut out);
        assert_eq!(out, [5, 6, 7, 8]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn taking_without_waiting_gives_what_is_there() {
        let pump = Pump::new();
        let mut out = Vec::new();
        pump.try_take(64, &mut out);
        assert!(out.is_empty());
        pump.push(&[1, 2, 3, 4]);
        pump.try_take(64, &mut out);
        assert_eq!(out, [1, 2, 3, 4]);
    }

    #[test]
    fn a_closing_link_gives_nothing_rather_than_waiting() {
        let pump = Pump::new();
        pump.close();
        let mut out = Vec::new();
        pump.take(64, &mut out);
        assert!(out.is_empty());
        assert!(pump.closing());
    }

    #[test]
    fn the_clock_follows_content_and_not_padding() {
        let pump = Pump::new();
        pump.holding(1024);
        assert_eq!(pump.played(), 0);
        pump.handed(1024);
        assert_eq!(pump.played(), 0);
        pump.handed(1024);
        assert_eq!(pump.played(), 1024);
    }

    #[test]
    fn what_is_queued_counts_the_device_as_well() {
        let pump = Pump::new();
        pump.push(&vec![0; 400 * FRAME]);
        pump.holding(600);
        assert_eq!(pump.queued(), 1000);
    }
}
