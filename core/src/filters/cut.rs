// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use mediadecoder::es::{self, Access, Codec, Format, Framing, Picture};

use super::analyzer::Analyzer;
use super::packet::{self, PacketSink, Section};
use super::{Container, Kind, Unit};

#[derive(Debug, Clone)]
pub struct Cutout {
    pub tag: u64,
    pub picture: Picture,
    pub format: Format,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Cutting {
    pub codec: Codec,
    pub framing: Framing,
    pub units: Vec<Cutout>,
}

#[derive(Default)]
struct Collect {
    stream: Option<u16>,
    analyzer: Analyzer,
    kinds: std::collections::BTreeMap<u16, (Kind, Framing)>,
    counts: std::collections::BTreeMap<u16, u64>,
    payloads: Vec<Vec<u8>>,
}

impl Collect {
    fn told(&mut self, program: &super::Program) {
        for stream in &program.streams {
            self.kinds.insert(stream.id, (stream.kind, stream.framing));
        }
    }
}

impl PacketSink for Collect {
    fn on_programs(&mut self, numbers: &[u16]) {
        self.analyzer.programs(numbers);
    }

    fn on_section(&mut self, section: &Section<'_>) {
        if let Some(program) = self.analyzer.section(section) {
            self.told(&program);
        }
    }

    fn on_package(&mut self, _packet_id: u16, table: &isdb::mmttables::PackageTable) {
        if let Some(program) = self.analyzer.package(table) {
            self.told(&program);
        }
    }

    fn on_unit(&mut self, unit: &Unit<'_>) {
        *self.counts.entry(unit.stream).or_default() += 1;
        if Some(unit.stream) == self.stream {
            self.payloads.push(unit.data.to_vec());
        }
    }
}

impl Cutting {
    /// データの多い順に、完全な単位が出るまで試す。
    pub fn of(bytes: &[u8], container: Container, most: usize) -> Option<Self> {
        let mut looking = Collect::default();
        packet::Parser::new(container).push(bytes, &mut looking);
        let mut candidates: Vec<(u16, Codec, Framing)> = looking
            .kinds
            .iter()
            .filter_map(|(id, (kind, framing))| match kind {
                Kind::Video(codec) => Some((*id, *codec, *framing)),
                _ => None,
            })
            .collect();
        candidates.sort_by_key(|(id, _, _)| {
            std::cmp::Reverse(looking.counts.get(id).copied().unwrap_or(0))
        });
        for (stream, codec, framing) in candidates {
            if let Some(units) = cut(bytes, container, stream, codec, framing, most) {
                return Some(Self {
                    codec,
                    framing,
                    units,
                });
            }
        }
        None
    }

    /// 無いファイルは `None`（テストが飛ばす理由に使う）。
    pub fn of_path(path: &str, most: usize) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        Self::of(&bytes, Container::of_path(path)?, most)
    }

    pub fn format(&self) -> Option<Format> {
        self.units.first().map(|unit| unit.format)
    }
}

fn cut(
    bytes: &[u8],
    container: Container,
    stream: u16,
    codec: Codec,
    framing: Framing,
    most: usize,
) -> Option<Vec<Cutout>> {
    let mut keeping = Collect {
        stream: Some(stream),
        ..Collect::default()
    };
    packet::Parser::new(container).push(bytes, &mut keeping);

    let mut splitter = es::Splitter::new(codec, framing);
    let mut out = Vec::new();
    let mut format: Option<Format> = None;
    let mut entered = false;
    let mut tag = 0u64;
    for payload in &keeping.payloads {
        let mut found: Vec<Access> = Vec::new();
        splitter.push(payload, &mut found);
        for access in found {
            if let Some(declared) = access.format {
                format = Some(declared);
            }
            let Some(now) = format else {
                continue;
            };
            entered = entered || (access.picture.random_access && access.picture.carries_format);
            if !entered {
                continue;
            }
            out.push(Cutout {
                tag,
                picture: access.picture,
                format: now,
                data: payload[access.range.clone()].to_vec(),
            });
            tag += 1;
            if out.len() >= most {
                return Some(out);
            }
        }
    }
    (!out.is_empty()).then_some(out)
}
