// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一度しか流れてこないバイト列を複数のリーダーに配るリングバッファ。放送では満杯なら一番古いブロックを捨て、
//! 録画ではいちばん遅いリーダーを待つ。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Block {
    at: u64,
    bytes: Vec<u8>,
}

impl Block {
    pub fn at(&self) -> u64 {
        self.at
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug)]
pub struct Buffer {
    inner: Mutex<Inner>,
    woke: Condvar,
    blocks: usize,
    waits: bool,
}

#[derive(Debug)]
struct Inner {
    held: VecDeque<Arc<Block>>,
    first: u64,
    next: u64,
    readers: BTreeMap<usize, u64>,
    ids: usize,
    ended: bool,
    starved: Option<Instant>,
}

impl Buffer {
    /// `waits` は録画なら真（待たないと、リーダーが触る前に上書きしてしまう）。
    pub fn new(blocks: usize, waits: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                held: VecDeque::with_capacity(blocks.max(1)),
                first: 0,
                next: 0,
                readers: BTreeMap::new(),
                ids: 0,
                ended: false,
                starved: None,
            }),
            woke: Condvar::new(),
            blocks: blocks.max(1),
            waits,
        })
    }

    pub fn reader(self: &Arc<Self>) -> Reader {
        let mut inner = self.lock();
        let id = inner.ids;
        inner.ids += 1;
        let at = inner.first;
        inner.readers.insert(id, at);
        drop(inner);
        Reader {
            buffer: Arc::clone(self),
            id,
            at,
            missed: 0,
        }
    }

    pub fn push(&self, bytes: &[u8]) -> bool {
        let mut inner = self.lock();
        loop {
            if inner.readers.is_empty() {
                return false;
            }
            if inner.held.len() < self.blocks {
                break;
            }
            if !self.waits || inner.readers.values().all(|at| *at > inner.first) {
                inner.held.pop_front();
                inner.first += 1;
                break;
            }
            inner = self.woke.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
        let at = inner.next;
        inner.held.push_back(Arc::new(Block {
            at,
            bytes: bytes.to_vec(),
        }));
        inner.next += 1;
        inner.starved = None;
        drop(inner);
        self.woke.notify_all();
        true
    }

    pub fn end(&self) {
        let mut inner = self.lock();
        inner.ended = true;
        drop(inner);
        self.woke.notify_all();
    }

    pub fn starved(&self) -> Option<Duration> {
        self.lock().starved.map(|since| since.elapsed())
    }

    pub fn ended(&self) -> bool {
        self.lock().ended
    }

    pub fn held(&self) -> usize {
        self.lock().held.iter().map(|block| block.bytes.len()).sum()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// drop するとリングバッファに伝わる（ライターが待ち続けないように）。
#[derive(Debug)]
pub struct Reader {
    buffer: Arc<Buffer>,
    id: usize,
    at: u64,
    missed: u64,
}

impl Reader {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Arc<Block>> {
        let mut inner = self.buffer.lock();
        loop {
            if self.at < inner.first {
                self.missed += inner.first - self.at;
                self.at = inner.first;
                inner.readers.insert(self.id, self.at);
            }
            if self.at < inner.next {
                let index = (self.at - inner.first) as usize;
                let block = Arc::clone(&inner.held[index]);
                self.at += 1;
                inner.readers.insert(self.id, self.at);
                drop(inner);
                self.buffer.woke.notify_all();
                return Some(block);
            }
            if inner.ended {
                return None;
            }
            inner.starved.get_or_insert_with(Instant::now);
            inner = self
                .buffer
                .woke
                .wait(inner)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn missed(&self) -> u64 {
        self.missed
    }

    pub fn reading(self) -> Reading {
        Reading {
            reader: self,
            held: None,
            offset: 0,
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let mut inner = self.buffer.lock();
        inner.readers.remove(&self.id);
        drop(inner);
        // 待つリングバッファは、居なくなったリーダーを待ち続けてはいけない。
        self.buffer.woke.notify_all();
    }
}

#[derive(Debug)]
pub struct Reading {
    reader: Reader,
    held: Option<Arc<Block>>,
    offset: usize,
}

impl Reading {
    pub fn missed(&self) -> u64 {
        self.reader.missed()
    }
}

impl std::io::Read for Reading {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        if into.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(block) = self.held.as_ref() {
                let left = &block.bytes[self.offset..];
                if !left.is_empty() {
                    let took = left.len().min(into.len());
                    into[..took].copy_from_slice(&left[..took]);
                    self.offset += took;
                    return Ok(took);
                }
                self.held = None;
                self.offset = 0;
            }
            let Some(block) = self.reader.next() else {
                return Ok(0);
            };
            self.held = Some(block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_reader_left_waiting_is_starved_until_something_arrives() {
        let buffer = Buffer::new(4, true);
        let mut reader = buffer.reader();
        assert_eq!(buffer.starved(), None, "まだ誰も待っていない");
        let writer = Arc::clone(&buffer);
        let pushing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            assert!(
                writer
                    .starved()
                    .is_some_and(|waited| waited >= Duration::from_millis(100)),
                "リーダーが待たされている"
            );
            writer.push(b"a");
        });
        assert!(reader.next().is_some());
        pushing.join().unwrap();
        assert_eq!(buffer.starved(), None, "書き込まれたら消える");
        buffer.end();
        assert!(reader.next().is_none());
        assert!(buffer.ended());
    }

    #[test]
    fn one_reader_gets_everything_in_order() {
        let buffer = Buffer::new(8, false);
        let mut reader = buffer.reader();
        for i in 0..5u8 {
            assert!(buffer.push(&[i, i, i]));
        }
        buffer.end();
        let mut got = Vec::new();
        while let Some(block) = reader.next() {
            got.extend_from_slice(block.bytes());
        }
        assert_eq!(got, vec![0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4]);
        assert_eq!(reader.missed(), 0);
    }

    #[test]
    fn two_readers_each_see_all_of_it() {
        let buffer = Buffer::new(8, false);
        let mut one = buffer.reader();
        let mut two = buffer.reader();
        for i in 0..4u8 {
            assert!(buffer.push(&[i]));
        }
        buffer.end();
        let drain = |reader: &mut Reader| {
            let mut got = Vec::new();
            while let Some(block) = reader.next() {
                got.extend_from_slice(block.bytes());
            }
            got
        };
        assert_eq!(drain(&mut one), vec![0, 1, 2, 3]);
        assert_eq!(drain(&mut two), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_slow_reader_is_left_behind_and_told_so() {
        let buffer = Buffer::new(2, false);
        let mut reader = buffer.reader();
        for i in 0..4u8 {
            assert!(buffer.push(&[i]));
        }
        buffer.end();
        let mut got = Vec::new();
        while let Some(block) = reader.next() {
            got.extend_from_slice(block.bytes());
        }
        assert_eq!(got, vec![2, 3]);
        assert_eq!(reader.missed(), 2);
    }

    #[test]
    fn a_fast_reader_beside_a_slow_one_misses_nothing() {
        let buffer = Buffer::new(2, false);
        let mut quick = buffer.reader();
        let mut slow = buffer.reader();
        for i in 0..4u8 {
            assert!(buffer.push(&[i]));
            let block = quick.next().expect("書き込んだものは読める");
            assert_eq!(block.bytes(), [i]);
        }
        buffer.end();
        assert_eq!(quick.missed(), 0);
        let mut got = Vec::new();
        while let Some(block) = slow.next() {
            got.extend_from_slice(block.bytes());
        }
        assert_eq!(got, vec![2, 3]);
        assert_eq!(slow.missed(), 2);
    }

    #[test]
    fn what_is_left_is_read_after_the_end() {
        let buffer = Buffer::new(8, false);
        let mut reader = buffer.reader();
        buffer.push(b"nokori");
        buffer.end();
        assert_eq!(reader.next().expect("残っている").bytes(), b"nokori");
        assert!(reader.next().is_none());
    }

    #[test]
    fn the_writer_is_told_when_the_last_reader_goes() {
        let buffer = Buffer::new(4, false);
        let reader = buffer.reader();
        assert!(buffer.push(b"a"));
        drop(reader);
        assert!(!buffer.push(b"b"));
    }

    #[test]
    fn a_reader_waits_for_what_has_not_arrived() {
        let buffer = Buffer::new(4, false);
        let mut reader = buffer.reader();
        let writing = Arc::clone(&buffer);
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            writing.push(b"osoi");
            writing.end();
        });
        assert_eq!(reader.next().expect("待てば来る").bytes(), b"osoi");
        assert!(reader.next().is_none());
        writer.join().expect("ライターのスレッドが終わる");
    }

    #[test]
    fn the_reading_end_hides_the_blocks() {
        let buffer = Buffer::new(8, false);
        let mut reading = buffer.reader().reading();
        buffer.push(b"abc");
        buffer.push(b"de");
        buffer.end();
        let mut got = Vec::new();
        reading.read_to_end(&mut got).expect("読み切れる");
        assert_eq!(got, b"abcde");
        assert_eq!(reading.missed(), 0);
    }

    #[test]
    fn a_waiting_ring_leaves_nobody_behind() {
        let buffer = Buffer::new(2, true);
        let mut reader = buffer.reader();
        let writing = Arc::clone(&buffer);
        let writer = std::thread::spawn(move || {
            for i in 0..8u8 {
                assert!(writing.push(&[i]));
            }
            writing.end();
        });
        let mut got = Vec::new();
        while let Some(block) = reader.next() {
            got.extend_from_slice(block.bytes());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(got, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(reader.missed(), 0);
        writer.join().expect("ライターのスレッドが終わる");
    }

    #[test]
    fn a_waiting_ring_lets_the_writer_go_when_the_reader_does() {
        let buffer = Buffer::new(1, true);
        let reader = buffer.reader();
        assert!(buffer.push(b"a"));
        let writing = Arc::clone(&buffer);
        let writer = std::thread::spawn(move || writing.push(b"b"));
        std::thread::sleep(std::time::Duration::from_millis(30));
        drop(reader);
        assert!(
            !writer.join().expect("ライターのスレッドが終わる"),
            "リーダーが居なければ偽"
        );
    }

    #[test]
    fn it_says_how_much_it_holds() {
        let buffer = Buffer::new(2, false);
        let _reader = buffer.reader();
        assert_eq!(buffer.held(), 0);
        buffer.push(b"abc");
        assert_eq!(buffer.held(), 3);
        buffer.push(b"de");
        assert_eq!(buffer.held(), 5);
        buffer.push(b"f");
        assert_eq!(buffer.held(), 3);
    }
}
