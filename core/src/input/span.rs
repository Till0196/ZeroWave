// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画の長さと、その中の再生位置。TS は PCR、TLV は NTP をティックとして使う。
//! TLV の位置は「開いた位置の NTP − 先頭の NTP + 時計」。

use std::io::{Read, Seek, SeekFrom};

use crate::filters::Container;
use crate::filters::packet::{self, Clock, PacketSink};

pub const LOOK: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub container: Container,
    pub first: i64,
    pub last: i64,
}

impl Span {
    pub fn measure(container: Container, file: &mut (impl Read + Seek)) -> Option<Self> {
        // ログを出さないと、たまにシークバーが出ないだけに見える。
        fn read(file: &mut (impl Read + Seek)) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
            let length = file.seek(SeekFrom::End(0))?;
            file.seek(SeekFrom::Start(0))?;
            let mut head = vec![0; LOOK.min(length as usize)];
            file.read_exact(&mut head)?;
            let tail_from = length.saturating_sub(LOOK as u64);
            file.seek(SeekFrom::Start(tail_from))?;
            let mut tail = vec![0; (length - tail_from) as usize];
            file.read_exact(&mut tail)?;
            Ok((head, tail))
        }
        let (head, tail) = match read(file) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!("録画の先頭と末尾を読み込めない: {error}");
                return None;
            }
        };
        let first = stamps(container, &head);
        let Some(first_stamp) = first.first().map(|s| s.1) else {
            tracing::info!("録画の先頭に時刻情報がない");
            return None;
        };
        let of = first.first().map(|s| s.0);
        let last = stamps(container, &tail);
        let Some(last_stamp) = last
            .iter()
            .rev()
            .find(|s| Some(s.0) == of)
            .or_else(|| last.last())
            .map(|s| s.1)
        else {
            tracing::info!("録画の末尾に時刻情報がない");
            return None;
        };
        let span = Self {
            container,
            first: first_stamp,
            last: first_stamp,
        };
        // PCR は 33 ビットで一周する（約 26.5 時間）。
        let last_stamp = span.unwrapped(last_stamp);
        Some(Self {
            last: last_stamp,
            ..span
        })
    }

    pub fn unwrapped(&self, stamp: i64) -> i64 {
        match self.container {
            Container::Ts if stamp < self.first => stamp + PCR_WRAP,
            _ => stamp,
        }
    }

    pub fn length(&self) -> i64 {
        self.last - self.first
    }

    /// 90 kHz。`opened_at` は開いた位置のティック、`now` は再生の時計。
    pub fn position(&self, opened_at: i64, now: i64) -> i64 {
        let at = match self.container {
            Container::Ts => match now - self.first {
                at if at < -PCR_WRAP / 2 => at + PCR_WRAP,
                at => at,
            },
            Container::Tlv => opened_at - self.first + now,
        };
        at.clamp(0, self.length())
    }

    pub fn offset(&self, fraction: f32, bytes: u64) -> u64 {
        let fraction = f64::from(fraction.clamp(0.0, 1.0));
        ((bytes as f64) * fraction) as u64
    }
}

const PCR_WRAP: i64 = 1 << 33;

/// TS は一番多い PID の分だけ。
pub fn stamps(container: Container, bytes: &[u8]) -> Vec<(u16, i64)> {
    let found = clocks(container, bytes);
    match container {
        Container::Ts => of_commonest_pid(found),
        Container::Tlv => found,
    }
}

pub fn first_stamp(container: Container, bytes: &[u8]) -> Option<i64> {
    stamps(container, bytes).first().map(|s| s.1)
}

#[derive(Default)]
struct Clocks(Vec<(u16, i64)>);

impl PacketSink for Clocks {
    fn on_clock(&mut self, clock: Clock) {
        self.0.push(match clock {
            Clock::Pcr { pid, ticks } => (pid, ticks as i64),
            Clock::Ntp(ntp) => (0, ntp_to_90k(ntp)),
        });
    }
}

/// 同期はパーサーが取る（録画の先頭がパケットの途中のことがある）。
fn clocks(container: Container, bytes: &[u8]) -> Vec<(u16, i64)> {
    let mut clocks = Clocks::default();
    packet::Parser::new(container).push(bytes, &mut clocks);
    clocks.0
}

fn of_commonest_pid(mut found: Vec<(u16, i64)>) -> Vec<(u16, i64)> {
    let mut counts: Vec<(u16, usize)> = Vec::new();
    for (pid, _) in &found {
        match counts.iter_mut().find(|(p, _)| p == pid) {
            Some((_, n)) => *n += 1,
            None => counts.push((*pid, 1)),
        }
    }
    let Some(&(most, _)) = counts.iter().max_by_key(|(_, n)| *n) else {
        return found;
    };
    found.retain(|(pid, _)| *pid == most);
    found
}

fn ntp_to_90k(ntp: u64) -> i64 {
    ((u128::from(ntp) * 90_000) >> 32) as i64
}

pub fn clock_text(ticks: i64) -> String {
    let seconds = (ticks.max(0) / 90_000) as u64;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpegts::{PACKET, SYNC};
    use std::io::Cursor;

    fn pcr_packet(pid: u16, base: u64) -> [u8; PACKET] {
        let mut packet = [0xffu8; PACKET];
        packet[0] = SYNC;
        packet[1] = (pid >> 8) as u8;
        packet[2] = pid as u8;
        packet[3] = 0x20;
        packet[4] = (PACKET - 5) as u8;
        packet[5] = 0x10;
        packet[6] = (base >> 25) as u8;
        packet[7] = (base >> 17) as u8;
        packet[8] = (base >> 9) as u8;
        packet[9] = (base >> 1) as u8;
        packet[10] = ((base & 1) << 7) as u8 | 0x7e;
        packet[11] = 0;
        packet
    }

    fn plain_packet(pid: u16) -> [u8; PACKET] {
        let mut packet = [0u8; PACKET];
        packet[0] = SYNC;
        packet[1] = (pid >> 8) as u8;
        packet[2] = pid as u8;
        packet[3] = 0x10;
        packet
    }

    #[test]
    fn pcrs_are_read_from_the_commonest_pid() {
        let mut bytes = vec![0x47, 0x00, 0x11]; // パケットの途中
        for i in 0..10u64 {
            bytes.extend_from_slice(&pcr_packet(0x1ff, 90_000 * i));
            bytes.extend_from_slice(&plain_packet(0x100));
            if i % 5 == 0 {
                bytes.extend_from_slice(&pcr_packet(0x2ff, 5_000_000 + i));
            }
        }
        let found = stamps(Container::Ts, &bytes);
        assert_eq!(found.len(), 10);
        assert!(found.iter().all(|(pid, _)| *pid == 0x1ff));
        assert_eq!(found[0].1, 0);
        assert_eq!(found[9].1, 90_000 * 9);
    }

    #[test]
    fn a_span_across_the_pcr_wrap_is_measured() {
        let start = (PCR_WRAP - 900_000) as u64;
        let mut bytes = Vec::new();
        for i in 0..200u64 {
            let base = (start + 9_000 * i) % PCR_WRAP as u64;
            bytes.extend_from_slice(&pcr_packet(0x1ff, base));
        }
        let span = Span::measure(Container::Ts, &mut Cursor::new(bytes)).unwrap();
        assert_eq!(span.length(), 9_000 * 199);
        assert_eq!(span.position(span.first, 90_000), 990_000);
        assert_eq!(span.unwrapped(90_000), PCR_WRAP + 90_000);
    }

    #[test]
    fn a_span_is_measured_and_positioned() {
        let mut bytes = Vec::new();
        for i in 0..100u64 {
            bytes.extend_from_slice(&pcr_packet(0x1ff, 1_000_000 + 9_000 * i));
        }
        let mut file = Cursor::new(bytes.clone());
        let span = Span::measure(Container::Ts, &mut file).unwrap();
        assert_eq!(span.first, 1_000_000);
        assert_eq!(span.last, 1_000_000 + 9_000 * 99);
        assert_eq!(span.position(span.first, 1_000_000 + 450_000), 450_000);
        assert_eq!(span.position(span.first, 0), 0);
        assert_eq!(span.position(span.first, i64::MAX / 2), span.length());
        assert_eq!(span.offset(0.5, bytes.len() as u64), bytes.len() as u64 / 2);
    }

    #[test]
    fn tlv_counts_from_where_it_opened() {
        let span = Span {
            container: Container::Tlv,
            first: 1000,
            last: 91_000,
        };
        assert_eq!(span.position(46_000, 9_000), 54_000);
    }

    #[test]
    fn clock_text_reads_like_a_player() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(90_000 * 65), "1:05");
        assert_eq!(clock_text(90_000 * 3725), "1:02:05");
    }

    #[test]
    fn ntp_is_scaled_to_90k() {
        assert_eq!(ntp_to_90k(1 << 32), 90_000);
        assert_eq!(ntp_to_90k(1 << 31), 45_000);
    }
}
