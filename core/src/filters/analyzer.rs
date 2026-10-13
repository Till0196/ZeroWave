// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組の構成を読むフィルター。[`Program`] を出すのは変わったときだけ。

use std::collections::BTreeMap;

use isdb::descriptors::{self as desc, Component, tag};
use isdb::{mmttables, tstables};
use mediadecoder::es::Framing;

use super::packet::{Origin, Section};
use super::tables::{TABLE_EIT_OWN, is_event_table};
use super::{Kind, Program, Stream};

/// TS は音声コンポーネント記述子を PMT に書かず EIT に書くので、この対応付けは TS だけに要る。
#[derive(Debug, Default, PartialEq, Eq)]
struct Described {
    sounds: BTreeMap<u16, Component>,
    pictures: BTreeMap<u16, String>,
}

type ProgramClock = (u16, Vec<(u16, u16)>);

#[derive(Debug, Default)]
pub struct Analyzer {
    /// EIT が足す前（二つのテーブルはどちらが先でもよいので別に持つ）。
    maps: BTreeMap<u16, (u16, Vec<Stream>)>,
    clocks: BTreeMap<u16, ProgramClock>,
    regions: BTreeMap<u16, Option<isdb::descriptors::TargetRegion>>,
    described: BTreeMap<u16, Described>,
    known: BTreeMap<u16, Program>,
    numbers: Vec<u16>,
    /// 空の PAT と、まだ届いていないのを分ける。
    listed: bool,
    kinds: BTreeMap<u16, Kind>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known(&self) -> &BTreeMap<u16, Program> {
        &self.known
    }

    pub fn kind(&self, id: u16) -> Option<Kind> {
        self.kinds.get(&id).copied()
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, program: Program) {
        if !self.numbers.contains(&program.number) {
            self.numbers.push(program.number);
        }
        self.remember(program);
    }

    pub fn shared(&self, id: u16) -> bool {
        self.known
            .values()
            .any(|program| program.streams.iter().any(|stream| stream.id == id))
    }

    pub fn listed(&self) -> Option<&[u16]> {
        self.listed.then_some(&self.numbers[..])
    }

    /// PAT から消えた番組を返す。
    pub fn programs(&mut self, numbers: &[u16]) -> Vec<Program> {
        self.listed = true;
        if self.numbers == numbers {
            return Vec::new();
        }
        self.numbers = numbers.to_vec();
        let gone: Vec<u16> = self
            .known
            .keys()
            .copied()
            .filter(|number| !numbers.contains(number))
            .collect();
        let mut left = Vec::with_capacity(gone.len());
        for number in gone {
            self.maps.remove(&number);
            self.clocks.remove(&number);
            self.regions.remove(&number);
            self.described.remove(&number);
            if let Some(program) = self.known.remove(&number) {
                left.push(program);
            }
        }
        self.rebuild_kinds();
        left
    }

    fn rebuild_kinds(&mut self) {
        self.kinds = self
            .known
            .values()
            .flat_map(|program| &program.streams)
            .map(|stream| (stream.id, stream.kind))
            .collect();
    }

    pub fn section(&mut self, section: &Section<'_>) -> Option<Program> {
        if section.origin != Origin::Ts {
            return None;
        }
        if section.program_map {
            return self.program_map(section);
        }
        if is_event_table(section.pid) {
            return self.event_table(section.data);
        }
        None
    }

    fn program_map(&mut self, section: &Section<'_>) -> Option<Program> {
        let map = tstables::program_map(section.data)?;
        let mut streams = Vec::with_capacity(map.streams.len());
        let mut ecm: Vec<(u16, u16)> = Vec::new();
        let mut take_ca = |descriptors: &[tstables::Descriptor]| {
            for found in descriptors
                .iter()
                .filter(|d| d.tag == tag::CONDITIONAL_ACCESS)
            {
                if let Some((system, pid)) = desc::conditional_access(&found.body)
                    && !ecm.contains(&(pid, system))
                {
                    ecm.push((pid, system));
                }
            }
        };
        take_ca(&map.descriptors);
        self.regions.insert(
            map.program_number,
            tstables::find(&map.descriptors, tag::TARGET_REGION)
                .and_then(|found| desc::target_region(&found.body)),
        );
        for stream in &map.streams {
            take_ca(&stream.descriptors);
            let kind = Kind::of_stream_type(stream.stream_type, &stream.descriptors);
            self.kinds.insert(stream.pid, kind);
            streams.push(Stream {
                id: stream.pid,
                kind,
                framing: Framing::StartCodes,
                declared: format!("0x{:02x}", stream.stream_type),
                tag: desc::component_tag(&stream.descriptors),
                component: tstables::find(&stream.descriptors, tag::AUDIO_COMPONENT)
                    .and_then(|found| desc::audio_component(&found.body)),
                name: None,
                hierarchy: tstables::find(&stream.descriptors, tag::HIERARCHICAL_TRANSMISSION)
                    .and_then(|found| desc::hierarchical_transmission(&found.body)),
                data: isdb::bxml::data_component(&stream.descriptors),
                decode: tstables::find(&stream.descriptors, tag::VIDEO_DECODE_CONTROL)
                    .and_then(|found| desc::video_decode_control(&found.body)),
            });
        }
        self.maps.insert(map.program_number, (section.pid, streams));
        self.clocks.insert(map.program_number, (map.pcr_pid, ecm));
        self.tell(map.program_number)
    }

    fn event_table(&mut self, data: &[u8]) -> Option<Program> {
        let table = tstables::event_table(data)?;
        if table.table_id != TABLE_EIT_OWN || table.number != 0 {
            return None;
        }
        let event = table.events.first()?;
        let mut described = Described::default();
        for found in &event.descriptors {
            match found.tag {
                tag::AUDIO_COMPONENT => {
                    if let Some(read) = desc::audio_component(&found.body)
                        && let Some(tag) = read.tag
                    {
                        described.sounds.insert(tag, read);
                    }
                }
                tag::COMPONENT => {
                    if let Some(read) = desc::picture_component(&found.body)
                        && let Some(name) = read.name
                    {
                        described.pictures.insert(read.tag, name);
                    }
                }
                _ => {}
            }
        }
        let known = self.described.entry(table.service_id).or_default();
        if *known == described {
            return None;
        }
        *known = described;
        self.maps
            .contains_key(&table.service_id)
            .then(|| self.tell(table.service_id))
            .flatten()
    }

    fn tell(&mut self, number: u16) -> Option<Program> {
        let (map, streams) = self.maps.get(&number)?;
        let map = *map;
        let described = self.described.get(&number);
        let streams: Vec<Stream> = streams
            .iter()
            .map(|stream| {
                let mut stream = stream.clone();
                let (Some(described), Some(tag)) = (described, stream.tag) else {
                    return stream;
                };
                match stream.kind {
                    Kind::Audio(_) if stream.component.is_none() => {
                        stream.component = described.sounds.get(&tag).cloned();
                    }
                    Kind::Video(_) => stream.name = described.pictures.get(&tag).cloned(),
                    _ => {}
                }
                stream
            })
            .collect();
        let (pcr, ecm) = self
            .clocks
            .get(&number)
            .cloned()
            .unwrap_or((0x1fff, Vec::new()));
        let program = Program {
            number,
            map,
            streams,
            pcr,
            ecm,
            application: None,
            region: self.regions.get(&number).cloned().flatten(),
            emergency_news: None,
            emergency: Vec::new(),
        };
        self.remember(program)
    }

    fn remember(&mut self, program: Program) -> Option<Program> {
        if self.known.get(&program.number) == Some(&program) {
            return None;
        }
        self.known.insert(program.number, program.clone());
        self.rebuild_kinds();
        Some(program)
    }

    /// 一つのテーブルに一つのサービスなので、これだけで番組が決まる。
    pub fn package(&mut self, table: &mmttables::PackageTable) -> Option<Program> {
        let mut streams = Vec::with_capacity(table.assets.len());
        for entry in &table.assets {
            let Some(packet_id) = entry.packet_id() else {
                continue;
            };
            let kind = Kind::of_asset(&entry.kind);
            streams.push(Stream {
                id: packet_id,
                kind,
                framing: Framing::Lengths,
                declared: entry.kind.clone(),
                tag: desc::mh_component_tag(&entry.descriptors),
                component: mmttables::find(&entry.descriptors, desc::mh_tag::AUDIO_COMPONENT)
                    .and_then(|found| desc::mh_audio_component(&found.body)),
                // 字幕のアセットはデータのコンポーネントとして記述される。
                data: mmttables::find(&entry.descriptors, desc::mh_tag::DATA_CODING)
                    .filter(|_| kind == Kind::Caption)
                    .and_then(|found| {
                        Some(isdb::bxml::DataComponent {
                            id: u16::from_be_bytes([*found.body.first()?, *found.body.get(1)?]),
                            additional: found.body.get(2..)?.to_vec(),
                        })
                    }),
                hierarchy: mmttables::find(&entry.descriptors, desc::mh_tag::ASSET_GROUP)
                    .and_then(|found| desc::asset_group(&found.body)),
                // 運用されない（TR-B39 第四分冊 表 34.1-42）。
                name: None,
                decode: None,
            });
        }
        streams.sort_by_key(|stream| stream.id);
        let number = table.service_id();
        if !self.numbers.contains(&number) {
            self.numbers.push(number);
        }
        self.remember(Program {
            number,
            map: 0,
            streams,
            pcr: 0x1fff,
            ecm: Vec::new(),
            application: mmttables::find(&table.descriptors, desc::mh_tag::APPLICATION_SERVICE)
                .and_then(|found| desc::application_service(&found.body)),
            region: mmttables::find(&table.descriptors, desc::mh_tag::TARGET_REGION)
                .and_then(|found| desc::target_region(&found.body)),
            emergency_news: mmttables::find(&table.descriptors, desc::mh_tag::EMERGENCY_NEWS)
                .and_then(|found| desc::emergency_news(&found.body)),
            emergency: mmttables::find(&table.descriptors, desc::mh_tag::EMERGENCY_INFORMATION)
                .map(|found| isdb::nitdescriptors::emergency_information(&found.body))
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::Audio;

    fn program_map(number: u16, streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = vec![
            0x02,
            0xb0,
            0x00,
            (number >> 8) as u8,
            number as u8,
            0xc1,
            0x00,
            0x00,
            0xe1,
            0x00,
            0xf0,
            0x00,
        ];
        for (stream_type, pid, descriptors) in streams {
            body.push(*stream_type);
            body.push(0xe0 | (pid >> 8) as u8);
            body.push(*pid as u8);
            body.push(0xf0 | (descriptors.len() >> 8) as u8);
            body.push(descriptors.len() as u8);
            body.extend_from_slice(descriptors);
        }
        body.extend_from_slice(&[0, 0, 0, 0]);
        let length = body.len() - 3;
        body[1] = 0xb0 | (length >> 8) as u8;
        body[2] = length as u8;
        body
    }

    fn section(pid: u16, program_map: bool, data: &[u8]) -> Section<'_> {
        Section {
            origin: Origin::Ts,
            pid,
            program_map,
            data,
        }
    }

    #[test]
    fn a_repeated_map_is_not_news() {
        let mut analyzer = Analyzer::new();
        let map = program_map(
            0x0400,
            &[(0x02, 0x100, vec![0x52, 0x01, 0x00]), (0x0f, 0x110, vec![])],
        );
        let first = analyzer
            .section(&section(0x1f0, true, &map))
            .expect("a programme");
        assert_eq!(first.streams.len(), 2);
        assert_eq!(first.streams[0].tag, Some(0));
        assert_eq!(first.streams[1].kind, Kind::Audio(Audio::Adts));
        assert!(analyzer.section(&section(0x1f0, true, &map)).is_none());
        assert_eq!(analyzer.kind(0x110), Some(Kind::Audio(Audio::Adts)));
        let changed = program_map(0x0400, &[(0x02, 0x100, vec![0x52, 0x01, 0x00])]);
        let second = analyzer
            .section(&section(0x1f0, true, &changed))
            .expect("changed");
        assert_eq!(second.streams.len(), 1);
    }

    #[test]
    fn a_programme_the_table_stops_naming_leaves() {
        let mut analyzer = Analyzer::new();
        let map = program_map(0x0400, &[(0x02, 0x100, vec![])]);
        analyzer.programs(&[0x0400]);
        analyzer.section(&section(0x1f0, true, &map));
        assert!(analyzer.known().contains_key(&0x0400));
        let gone = analyzer.programs(&[0x0401]);
        assert_eq!(gone.len(), 1);
        assert!(analyzer.known().is_empty());
        assert_eq!(analyzer.kind(0x100), None);
    }
}
