// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 読み込みスレッドと、そのバッファ。guest の `read` は待たず、終了してすべて読み終えていれば `None`。

use std::io::Read;
use std::sync::mpsc::{Receiver, TryRecvError, TrySendError, channel, sync_channel};

const READ_SIZE: usize = 64 * 1024;

pub(crate) struct Inbox(Receiver<Vec<u8>>);

enum Full {
    Drop,
    Wait,
}

impl Inbox {
    /// 溢れたら新しい方を捨てる。`None` は全部貯める。
    pub(crate) fn spawn(
        name: &str,
        reader: impl Read + Send + 'static,
        most: Option<usize>,
    ) -> std::io::Result<Self> {
        match most {
            Some(most) => Self::spawn_with(name, reader, most, Full::Drop),
            None => {
                let (sent, arrived) = channel();
                Self::run(
                    name,
                    reader,
                    Box::new(move |bytes| sent.send(bytes).is_ok()),
                    arrived,
                )
            }
        }
    }

    /// 溢れたら読まれるのを待つ。捨てると内容が壊れるもの（子プロセスの標準出力）のため。
    pub(crate) fn spawn_waiting(
        name: &str,
        reader: impl Read + Send + 'static,
        most: usize,
    ) -> std::io::Result<Self> {
        Self::spawn_with(name, reader, most, Full::Wait)
    }

    fn spawn_with(
        name: &str,
        reader: impl Read + Send + 'static,
        most: usize,
        full: Full,
    ) -> std::io::Result<Self> {
        let (sent, arrived) = sync_channel(most);
        let send: Box<dyn FnMut(Vec<u8>) -> bool + Send> = match full {
            Full::Drop => Box::new(move |bytes| {
                !matches!(sent.try_send(bytes), Err(TrySendError::Disconnected(_)))
            }),
            Full::Wait => Box::new(move |bytes| sent.send(bytes).is_ok()),
        };
        Self::run(name, reader, send, arrived)
    }

    fn run(
        name: &str,
        mut reader: impl Read + Send + 'static,
        mut send: Box<dyn FnMut(Vec<u8>) -> bool + Send>,
        arrived: Receiver<Vec<u8>>,
    ) -> std::io::Result<Self> {
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let mut buffer = vec![0u8; READ_SIZE];
                while let Ok(read @ 1..) = reader.read(&mut buffer) {
                    if !send(buffer[..read].to_vec()) {
                        break;
                    }
                }
            })?;
        Ok(Self(arrived))
    }

    pub(crate) fn drain(&self) -> Option<Vec<u8>> {
        let mut gathered = Vec::new();
        loop {
            match self.0.try_recv() {
                Ok(bytes) => gathered.extend_from_slice(&bytes),
                Err(TryRecvError::Empty) => return Some(gathered),
                Err(TryRecvError::Disconnected) => {
                    return (!gathered.is_empty()).then_some(gathered);
                }
            }
        }
    }
}
