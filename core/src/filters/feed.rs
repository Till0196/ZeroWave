// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 下の二つ（[`packet`] と [`analyzer`]）をまとめたインターフェース。ツールとテストのため。エンジンは通らない。

use super::analyzer::Analyzer;
use super::packet::{self, PacketSink, Section};
use super::{Container, Counted, Counts, Program, Unit};

pub trait Sink {
    fn on_frame_header(&mut self, _header: &isdb::tsmf::Info) {}
    fn on_mixing(&mut self, _mixing: packet::ts::Mixing) {}
    fn on_programs(&mut self, _numbers: &[u16]) {}
    fn on_streams(&mut self, _program: &Program) {}
    fn on_section(&mut self, _section: &Section<'_>) {}
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
pub struct Feed {
    parser: packet::Parser,
    analyzer: Analyzer,
}

impl Feed {
    pub fn new(container: Container) -> Self {
        Self {
            parser: packet::Parser::new(container),
            analyzer: Analyzer::new(),
        }
    }

    pub fn container(&self) -> Container {
        self.parser.container()
    }

    pub fn programs(&self) -> &std::collections::BTreeMap<u16, Program> {
        self.analyzer.known()
    }

    pub fn push<S: Sink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) {
        let mut through = Through {
            analyzer: &mut self.analyzer,
            sink,
        };
        self.parser.push(bytes, &mut through);
    }

    pub fn counts(&self) -> Counts {
        self.parser.counts()
    }

    pub fn counted(&self, id: u16) -> Option<Counted> {
        self.parser.counted(id)
    }

    pub fn identifiers(&self) -> Vec<u16> {
        self.parser.identifiers()
    }

    pub fn scrambled(&self, id: u16) -> u64 {
        self.parser.scrambled(id)
    }
}

struct Through<'a, S: Sink + ?Sized> {
    analyzer: &'a mut Analyzer,
    sink: &'a mut S,
}

impl<S: Sink + ?Sized> PacketSink for Through<'_, S> {
    fn on_frame_header(&mut self, header: &isdb::tsmf::Info) {
        self.sink.on_frame_header(header);
    }

    fn on_mixing(&mut self, mixing: packet::ts::Mixing) {
        self.sink.on_mixing(mixing);
    }

    fn on_programs(&mut self, numbers: &[u16]) {
        self.analyzer.programs(numbers);
        self.sink.on_programs(numbers);
    }

    fn on_section(&mut self, section: &Section<'_>) {
        if let Some(program) = self.analyzer.section(section) {
            self.sink.on_streams(&program);
        }
        self.sink.on_section(section);
    }

    fn on_package(&mut self, _packet_id: u16, table: &isdb::mmttables::PackageTable) {
        if let Some(program) = self.analyzer.package(table) {
            self.sink.on_streams(&program);
        }
    }

    fn on_unit(&mut self, unit: &Unit<'_>) {
        if self.analyzer.kind(unit.stream).is_some() {
            self.sink.on_unit(unit);
        }
    }

    fn on_caption(&mut self, packet_id: u16, mpu: u32, at: Option<i64>, data: &[u8]) {
        self.sink.on_caption(packet_id, mpu, at, data);
    }

    fn on_item(&mut self, packet_id: u16, mpu: u32, item: u32, fragmentation: u8, data: &[u8]) {
        self.sink.on_item(packet_id, mpu, item, fragmentation, data);
    }
}
