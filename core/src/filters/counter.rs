// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 受信品質を数えるフィルター。見ているサービスのみを数える。

use std::collections::BTreeMap;

use super::packet::Parser;
use super::tables::SHARED_PIDS;
use super::{Container, Counts, Program};

#[derive(Debug, Default)]
pub struct Counter {
    /// 録画がどの番組のものかを、テーブルに載るだけの番組と分ける。
    arrived: BTreeMap<u16, u64>,
    before: Counts,
}

impl Counter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn unit_arrived(&mut self, id: u16) {
        *self.arrived.entry(id).or_default() += 1;
    }

    pub fn arrived(&self, id: u16) -> u64 {
        self.arrived.get(&id).copied().unwrap_or(0)
    }

    pub fn forget(&mut self, id: u16) {
        self.arrived.remove(&id);
    }

    /// 共有のテーブルの識別子は前の番組で落ちた数を抱えているので数え直す。
    pub fn rebase(&mut self, parser: &Parser, playing: Option<&Program>) {
        self.before = Self::since_start(parser, playing);
    }

    pub fn counts(&self, parser: &Parser, playing: Option<&Program>) -> Counts {
        let all = Self::since_start(parser, playing);
        let before = &self.before;
        Counts {
            packets: all.packets.saturating_sub(before.packets),
            dropped: all.dropped.saturating_sub(before.dropped),
            errors: all.errors.saturating_sub(before.errors),
            scrambled: all.scrambled.saturating_sub(before.scrambled),
        }
    }

    fn since_start(parser: &Parser, playing: Option<&Program>) -> Counts {
        let Some(program) = playing else {
            return parser.counts();
        };
        let shared: &[u16] = match parser.container() {
            Container::Ts => &SHARED_PIDS,
            Container::Tlv => &[],
        };
        let mut counts = Counts::default();
        let mut seen: Vec<u16> = Vec::new();
        for pid in shared
            .iter()
            .copied()
            .chain(std::iter::once(program.map))
            .chain(program.streams.iter().map(|stream| stream.id))
        {
            if seen.contains(&pid) {
                continue;
            }
            seen.push(pid);
            let Some(counted) = parser.counted(pid) else {
                continue;
            };
            counts.packets += counted.arrived;
            counts.dropped += counted.lost;
            counts.errors += counted.errors;
            counts.scrambled += counted.scrambled;
        }
        counts
    }
}
