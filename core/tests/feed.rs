// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The feed over transport streams ffmpeg made (`testdata/make.sh`).

use std::collections::BTreeMap;

use mediadecoder::es::Codec;
use zwcore::filters::{Audio, Container, Feed, Kind, Program, Sink, Unit};

mod common;

#[derive(Default)]
struct Count {
    streams: Vec<(u16, Kind)>,
    units: BTreeMap<u16, u64>,
    bytes: BTreeMap<u16, u64>,
    timed: u64,
}

impl Sink for Count {
    fn on_streams(&mut self, program: &Program) {
        for stream in &program.streams {
            if !self.streams.iter().any(|(id, _)| *id == stream.id) {
                self.streams.push((stream.id, stream.kind));
            }
        }
        self.streams.sort_by_key(|(id, _)| *id);
    }

    fn on_unit(&mut self, unit: &Unit<'_>) {
        *self.units.entry(unit.stream).or_default() += 1;
        *self.bytes.entry(unit.stream).or_default() += unit.data.len() as u64;
        if unit.pts.is_some() {
            self.timed += 1;
        }
    }
}

/// The last unit of each stream is not given: it ends at the start of the next, which never comes.
fn over(name: &str) -> Count {
    let bytes = common::testdata(name);
    let mut count = Count::default();
    Feed::new(Container::of_path(name).expect("a name that says which")).push(&bytes, &mut count);
    count
}

#[test]
fn a_transport_stream_of_h264_gives_its_pictures_and_its_sound() {
    let count = over("h264-baseline.ts");
    assert_eq!(
        count.streams,
        [
            (0x100, Kind::Video(Codec::H264)),
            (0x101, Kind::Audio(Audio::Adts))
        ]
    );
    assert_eq!(count.units.get(&0x100), Some(&29), "{:?}", count.units);
    assert!(count.units.get(&0x101).is_some_and(|units| *units > 0));
    assert_eq!(count.timed, count.units.values().sum::<u64>());
}

#[test]
fn a_transport_stream_of_mpeg2_gives_its_pictures_and_its_sound() {
    let count = over("mpeg2-interlaced.ts");
    assert_eq!(
        count.streams,
        [
            (0x100, Kind::Video(Codec::Mpeg2)),
            (0x101, Kind::Audio(Audio::Adts))
        ]
    );
    assert_eq!(count.units.get(&0x100), Some(&35), "{:?}", count.units);
    assert!(count.units.get(&0x101).is_some_and(|units| *units > 0));
    assert_eq!(count.timed, count.units.values().sum::<u64>());
}

#[test]
fn a_transport_stream_of_hevc_gives_its_pictures() {
    let count = over("hevc-main10.ts");
    assert_eq!(count.streams, [(0x100, Kind::Video(Codec::Hevc))]);
    assert_eq!(count.units.get(&0x100), Some(&26), "{:?}", count.units);
    assert_eq!(count.timed, count.units.values().sum::<u64>());
}
