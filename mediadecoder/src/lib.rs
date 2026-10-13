// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Video and sound: reading what an elementary stream says about itself and decoding it.
//! No GPU, platform, clock or device here; that side is `zwcore`.

pub mod audio;
pub mod bits;
pub mod es;
pub mod video;

/// 試験の素材。`sh testdata/make.sh` でワークスペースの `testdata/` に作る。
#[cfg(test)]
pub(crate) fn testdata(name: &str) -> Vec<u8> {
    let path = format!("{}/../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{path}: {error}（sh testdata/make.sh で作る）"))
}

/// Pulls one stream's PES payloads out of a transport stream, one unit per PES packet.
#[cfg(all(test, any(feature = "h264-decoder", feature = "hevc-decoder")))]
pub(crate) fn pes(bytes: &[u8], wanted: u16) -> Vec<Vec<u8>> {
    let mut payloads = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for packet in bytes.as_chunks::<188>().0 {
        if packet[0] != 0x47 {
            continue;
        }
        let pid = u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2]);
        if pid != wanted {
            continue;
        }
        let starts = packet[1] & 0x40 != 0;
        let control = packet[3] >> 4 & 3;
        let mut at = 4;
        if control & 2 != 0 {
            at += 1 + usize::from(packet[4]);
        }
        if control & 1 == 0 || at >= 188 {
            continue;
        }
        let payload = &packet[at..];
        if starts {
            if !current.is_empty() {
                payloads.push(std::mem::take(&mut current));
            }
            if payload.len() >= 9 {
                let header = 9 + usize::from(payload[8]);
                if payload.len() > header {
                    current.extend_from_slice(&payload[header..]);
                }
            }
        } else if !current.is_empty() {
            current.extend_from_slice(payload);
        }
    }
    if !current.is_empty() {
        payloads.push(current);
    }
    payloads
}

/// What went wrong that the caller can act on. The decoder keeps going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unsupported(&'static str),
    Damaged(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(out, "unsupported: {what}"),
            Self::Damaged(what) => write!(out, "damaged: {what}"),
        }
    }
}

impl std::error::Error for Error {}
