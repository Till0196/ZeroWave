// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一番下の層：バイト列をパケットに切り、イベント（[`PacketSink`]）にする。テーブルは読まない。
//! MMT の単位に時刻を付けるのはこの層、TS は PES ヘッダーのまま。

use super::{Container, Counted, Counts, Unit};

pub mod tlv;
pub mod ts;

/// 識別子では見分けられない（タグの幅と文字コードが違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Ts,
    MmtSi,
    TlvSi,
}

#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    pub origin: Origin,
    /// TS は PID、MMT は packet_id、TLV-SI は 0。
    pub pid: u16,
    pub program_map: bool,
    pub data: &'a [u8],
}

impl Section<'_> {
    pub fn table_id(&self) -> u8 {
        self.data.first().copied().unwrap_or(0xff)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    Pcr { pid: u16, ticks: u64 },
    Ntp(u64),
}

pub trait PacketSink {
    fn on_frame_header(&mut self, _header: &isdb::tsmf::Info) {}

    /// 一つのストリームに載っている TS の数の見立てが変わった。
    fn on_mixing(&mut self, _mixing: ts::Mixing) {}

    fn on_programs(&mut self, _numbers: &[u16]) {}

    fn on_clock(&mut self, _clock: Clock) {}

    fn on_section(&mut self, _section: &Section<'_>) {}

    fn on_package(&mut self, _packet_id: u16, _table: &isdb::mmttables::PackageTable) {}

    /// PA メッセージに乗る、MPT 以外のテーブル。先頭のバイトがテーブルの番号。
    fn on_mmt_table(&mut self, _packet_id: u16, _table: &[u8]) {}

    fn on_unit(&mut self, _unit: &Unit<'_>) {}

    fn on_caption(&mut self, _packet_id: u16, _mpu: u32, _at: Option<i64>, _data: &[u8]) {}

    fn on_item(
        &mut self,
        _packet_id: u16,
        _mpu: u32,
        _item: u32,
        _fragmentation: u8,
        _data: &[u8],
    ) {
    }
}

#[derive(Debug)]
pub enum Parser {
    Ts(ts::Parser),
    Tlv(tlv::Parser),
}

impl Parser {
    pub fn new(container: Container) -> Self {
        match container {
            Container::Ts => Self::Ts(ts::Parser::new()),
            Container::Tlv => Self::Tlv(tlv::Parser::new()),
        }
    }

    pub fn container(&self) -> Container {
        match self {
            Self::Ts(_) => Container::Ts,
            Self::Tlv(_) => Container::Tlv,
        }
    }

    pub fn keep(&mut self, pids: &[u16]) {
        match self {
            Self::Ts(parser) => parser.keep(pids),
            Self::Tlv(parser) => parser.keep(pids),
        }
    }

    pub fn push<S: PacketSink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) {
        match self {
            Self::Ts(parser) => parser.push(bytes, sink),
            Self::Tlv(parser) => parser.push(bytes, sink),
        }
    }

    pub fn counted(&self, id: u16) -> Option<Counted> {
        match self {
            Self::Ts(parser) => parser.counted(id),
            Self::Tlv(parser) => parser.counted(id),
        }
    }

    pub fn identifiers(&self) -> Vec<u16> {
        match self {
            Self::Ts(parser) => parser.identifiers(),
            Self::Tlv(parser) => parser.identifiers(),
        }
    }

    pub fn counts(&self) -> Counts {
        match self {
            Self::Ts(parser) => parser.counts(),
            Self::Tlv(parser) => parser.counts(),
        }
    }

    pub fn scrambled(&self, id: u16) -> u64 {
        match self {
            Self::Ts(parser) => parser.scrambled(id),
            Self::Tlv(parser) => parser.scrambled(id),
        }
    }

    pub fn scrambled_packets(&self) -> u64 {
        match self {
            Self::Ts(parser) => parser.counts().scrambled,
            Self::Tlv(parser) => parser.scrambled_packets(),
        }
    }
}
