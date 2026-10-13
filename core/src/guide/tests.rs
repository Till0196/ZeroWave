// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::*;

use crate::filters::packet::{Origin, Section};
use crate::filters::tables::{self, EIT_LOW_PID, EIT_PID, NIT_PID, SDT_PID};
use crate::si::{Identity, Info};
use isdb::descriptors::{mh_tag, tag};
use isdb::mmttables;
use isdb::tstables::Descriptor;

const DESCRIPTOR_SHORT_EVENT: u8 = tag::SHORT_EVENT;
const DESCRIPTOR_SERVICE: u8 = tag::SERVICE;
const DESCRIPTOR_CONTENT: u8 = tag::CONTENT;
const DESCRIPTOR_COMPONENT: u8 = tag::COMPONENT;
const DESCRIPTOR_AUDIO_COMPONENT: u8 = tag::AUDIO_COMPONENT;
const DESCRIPTOR_DATA_CONTENT: u8 = tag::DATA_CONTENT;
const MH_DESCRIPTOR_SHORT_EVENT: u16 = mh_tag::SHORT_EVENT;
const MH_DESCRIPTOR_EXTENDED_EVENT: u16 = mh_tag::EXTENDED_EVENT;
const MH_DESCRIPTOR_SERVICE: u16 = mh_tag::SERVICE;
const MH_DESCRIPTOR_VIDEO_COMPONENT: u16 = mh_tag::VIDEO_COMPONENT;
const MH_DESCRIPTOR_AUDIO_COMPONENT: u16 = mh_tag::AUDIO_COMPONENT;

fn describe(descriptors: &[Descriptor], into: &mut Programme) {
    tables::describe(descriptors, into);
}

fn extended_event(body: &[u8], into: &mut Programme) {
    describe(
        &[Descriptor {
            tag: tag::EXTENDED_EVENT,
            body: body.to_vec(),
        }],
        into,
    );
}

struct Fed {
    guide: Guide,
    si: crate::si::Info,
}

impl Fed {
    fn new() -> Self {
        Self {
            guide: Guide::new(),
            si: crate::si::Info::new(),
        }
    }

    fn feed(&mut self, section: &Section<'_>) {
        tables::Tables {
            guide: &mut self.guide,
            si: &mut self.si,
        }
        .feed(section);
    }
}

impl std::ops::Deref for Fed {
    type Target = Guide;
    fn deref(&self) -> &Guide {
        &self.guide
    }
}

impl std::ops::DerefMut for Fed {
    fn deref_mut(&mut self) -> &mut Guide {
        &mut self.guide
    }
}

fn feed_as(guide: &mut Fed, origin: Origin, pid: u16, data: &[u8]) {
    guide.feed(&Section {
        origin,
        pid,
        program_map: false,
        data,
    });
}

fn feed(guide: &mut Fed, pid: u16, data: &[u8]) {
    feed_as(guide, Origin::Ts, pid, data);
}

fn feed_mmt(guide: &mut Fed, data: &[u8]) {
    feed_as(guide, Origin::MmtSi, 0, data);
}

fn feed_tlv(guide: &mut Fed, data: &[u8]) {
    feed_as(guide, Origin::TlvSi, 0, data);
}

#[test]
fn the_epoch_of_a_written_moment_is_the_unix_second_of_that_wall_clock() {
    let time = BroadcastTime {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };
    assert_eq!(epoch(time), 0);
    let time = BroadcastTime {
        year: 2026,
        month: 9,
        day: 8,
        hour: 12,
        minute: 30,
        second: 0,
    };
    assert_eq!(epoch(time), 1_788_870_600);
}

#[test]
fn a_programme_sent_again_replaces_the_one_at_that_moment() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7880,
        service: 0x0400,
    };
    let mut programme = Programme {
        title: "はじめ".to_owned(),
        ..Programme::scheduled(key, 1, Some(100), Some(60))
    };
    guide.tell(programme.clone());
    programme.title = "あと".to_owned();
    guide.tell(programme);
    let titles: Vec<&str> = guide
        .programmes(key)
        .map(|programme| programme.title.as_str())
        .collect();
    assert_eq!(titles, ["あと"]);
}

#[test]
fn what_mirakurun_said_is_not_overwritten_by_the_broadcast() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7880,
        service: 0x0400,
    };
    let make = |told, title: &str| Programme {
        service: key.service,
        network: key.network,
        event: 1,
        start: Some(100),
        seconds: Some(60),
        title: title.to_owned(),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told,
        details: Default::default(),
    };
    guide.tell(make(Told::Mirakurun, "Mirakurun"));
    guide.tell(make(Told::Broadcast, "放送波"));
    assert_eq!(guide.programmes(key).next().unwrap().title, "Mirakurun");
    guide.tell(make(Told::Mirakurun, "あとの Mirakurun"));
    assert_eq!(
        guide.programmes(key).next().unwrap().title,
        "あとの Mirakurun"
    );
}

#[test]
fn what_the_broadcast_said_is_only_filled_in_by_mirakurun() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7880,
        service: 0x0400,
    };
    let make = |told, title: &str, detail: &str| Programme {
        service: key.service,
        network: key.network,
        event: 1,
        start: Some(100),
        seconds: Some(60),
        title: title.to_owned(),
        detail: detail.to_owned(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told,
        details: Default::default(),
    };
    guide.tell(make(Told::Now, "放送波\u{3000}題", ""));
    guide.tell(make(Told::Mirakurun, "Mirakurun の題", "Mirakurun の説明"));
    let programme = guide.programmes(key).next().unwrap();
    assert_eq!(programme.title, "放送波\u{3000}題");
    assert_eq!(programme.detail, "Mirakurun の説明");
    assert_eq!(programme.told, Told::Now);
    guide.tell(make(Told::Broadcast, "予定表の題", "予定表の説明"));
    guide.tell(make(Told::Mirakurun, "Mirakurun の題", "Mirakurun の説明"));
    let programme = guide.programmes(key).next().unwrap();
    assert_eq!(programme.title, "放送波\u{3000}題");
    assert_eq!(programme.told, Told::Now);
}

#[test]
fn the_programme_at_a_moment_is_the_last_one_that_had_started() {
    let mut guide = Fed::new();
    let key = Key {
        network: 1,
        service: 2,
    };
    let make = |event, start, seconds, title: &str| Programme {
        service: key.service,
        network: key.network,
        event,
        start: Some(start),
        seconds,
        title: title.to_owned(),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told: Told::Broadcast,
        details: Default::default(),
    };
    guide.tell(make(1, 0, None, "さいしょ"));
    guide.tell(make(2, 100, Some(50), "つぎ"));
    assert_eq!(guide.at(key, 50).unwrap().title, "さいしょ");
    assert_eq!(guide.at(key, 120).unwrap().title, "つぎ");
    assert!(guide.at(key, 200).is_none());
}

#[test]
fn the_schedule_is_counted_by_table_and_segment_up_to_the_last_table() {
    let mut guide = Fed::new();
    let key = Key {
        network: 1,
        service: 2,
    };
    guide.tell_time(1_788_870_600);
    guide.tell_arrival(key, 3, 0x50, 32, 33, 0x51);
    assert_eq!(guide.filled(key), (1, 61));
    guide.tell_arrival(key, 3, 0x50, 33, 33, 0x51);
    assert_eq!(guide.filled(key), (2, 61));
    guide.tell_arrival(key, 7, 0x51, 0, 0, 0x51);
    assert_eq!(guide.filled(key), (3, 61));
    for segment in 5..32u8 {
        guide.tell_arrival(key, 3, 0x50, segment * 8, segment * 8, 0x51);
    }
    for segment in 1..32u8 {
        guide.tell_arrival(key, 7, 0x51, segment * 8, segment * 8, 0x51);
    }
    assert_eq!(guide.filled(key), (61, 61));
    assert!(guide.complete(key));
    guide.tell_arrival(key, 4, 0x50, 32, 33, 0x51);
    assert_eq!(guide.filled(key), (61, 61));
}

#[test]
fn an_extended_event_with_no_heading_continues_the_one_before() {
    let mut programme = Programme {
        ..Programme::scheduled(
            Key {
                service: 0,
                network: 0,
            },
            0,
            None,
            None,
        )
    };
    let latin = |text: &str| {
        let mut bytes = vec![0x1b, 0x28, 0x4a, 0x89];
        bytes.extend_from_slice(text.as_bytes());
        bytes
    };
    let item = |heading: &str, value: &str| {
        let (heading, value) = (latin(heading), latin(value));
        let mut bytes = vec![heading.len() as u8];
        bytes.extend_from_slice(&heading);
        bytes.push(value.len() as u8);
        bytes.extend_from_slice(&value);
        bytes
    };
    let items: Vec<u8> = [item("ab", "on"), vec![0, latin("e!").len() as u8]]
        .concat()
        .into_iter()
        .chain(latin("e!"))
        .collect();
    let mut body = vec![0x00, b'j', b'p', b'n', items.len() as u8];
    body.extend_from_slice(&items);
    body.push(0);
    extended_event(&body, &mut programme);
    assert_eq!(programme.extended, [("ab".to_owned(), "one!".to_owned())]);
}

#[test]
fn only_the_streams_own_table_says_what_the_stream_is() {
    let mut guide = Fed::new();
    assert_eq!(guide.si.identity(), None);
    feed(
        &mut guide,
        SDT_PID,
        &service_section_of(0x46, 0x0400, 1, 2, "隣"),
    );
    assert_eq!(
        guide.si.identity(),
        None,
        "別のストリームのテーブルで識別子を決めてはならない"
    );
    feed(
        &mut guide,
        SDT_PID,
        &service_section_of(0x42, 0x0400, 0x7880, 0x7fe1, "自分"),
    );
    assert_eq!(
        guide.si.identity(),
        Some(Identity {
            transport: 0x7fe1,
            network: 0x7880,
        })
    );
}

#[test]
fn what_the_broadcast_says_about_the_picture_and_the_sound_reads_as_words() {
    assert_eq!(
        video_name(0xb3).as_deref(),
        Some("1080i（1125i）、アスペクト比16:9 パンベクトルなし")
    );
    assert_eq!(audio_name(0x03), Some("2/0モード（ステレオ）"));
    assert_eq!(audio_name(0x02), Some("1/0＋1/0モード（デュアルモノ）"));
    assert_eq!(video_name(0x7f), None);
    assert_eq!(
        video_name(0x93).as_deref(),
        Some("2160p、アスペクト比16:9 パンベクトルなし")
    );
    assert_eq!(video_name(0x83).as_deref(), Some("4320p、アスペクト比16:9"));
    assert_eq!(video_name(0x80), None);
    assert_eq!(audio_name(0xff), None);
}

#[test]
fn a_moment_survives_going_to_the_calendar_and_back() {
    for moment in [0, 1_788_870_600, -86_400, 253_402_300_799] {
        let (year, month, day, hour, minute, second) = civil(moment);
        let back = epoch(BroadcastTime {
            year: year as i32,
            month,
            day,
            hour,
            minute,
            second,
        });
        assert_eq!(back, moment, "{moment}");
    }
}

/// 英数集合を G0 へ指定し、MSZ で半角にする。
fn latin(text: &str) -> Vec<u8> {
    let mut bytes = vec![0x1b, 0x28, 0x4a, 0x89];
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

fn long(table_id: u8, extension: u16, version: u8, number: u8, last: u8, body: &[u8]) -> Vec<u8> {
    let mut section = vec![table_id, 0x00, 0x00];
    section.extend_from_slice(&extension.to_be_bytes());
    section.extend_from_slice(&[0xc1 | (version << 1), number, last]);
    section.extend_from_slice(body);
    section.extend_from_slice(&[0; 4]);
    let length = section.len() - 3;
    section[1] = 0xb0 | ((length >> 8) as u8 & 0x0f);
    section[2] = length as u8;
    section
}

fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag, body.len() as u8];
    out.extend_from_slice(body);
    out
}

fn short(title: &str, detail: &str) -> Vec<u8> {
    let (title, detail) = (latin(title), latin(detail));
    let mut body = vec![b'j', b'p', b'n', title.len() as u8];
    body.extend_from_slice(&title);
    body.push(detail.len() as u8);
    body.extend_from_slice(&detail);
    descriptor(DESCRIPTOR_SHORT_EVENT, &body)
}

fn event_section(
    table_id: u8,
    service: u16,
    network: u16,
    event: u16,
    described: &[u8],
) -> Vec<u8> {
    let mut body = vec![0x00, 0x01];
    body.extend_from_slice(&network.to_be_bytes());
    body.extend_from_slice(&[0x00, table_id]);
    body.extend_from_slice(&event.to_be_bytes());
    body.extend_from_slice(&[0xef, 0x6b, 0x12, 0x30, 0x00]);
    body.extend_from_slice(&[0x01, 0x00, 0x00]);
    body.push(0x80 | ((described.len() >> 8) as u8 & 0x0f));
    body.push(described.len() as u8);
    body.extend_from_slice(described);
    long(table_id, service, 0, 0, 0, &body)
}

fn service_section(service: u16, network: u16, transport: u16, name: &str) -> Vec<u8> {
    service_section_of(0x42, service, network, transport, name)
}

fn service_section_of(
    table_id: u8,
    service: u16,
    network: u16,
    transport: u16,
    name: &str,
) -> Vec<u8> {
    let named = latin(name);
    let mut service_descriptor = vec![0x01, 0x00, named.len() as u8];
    service_descriptor.extend_from_slice(&named);
    let described = descriptor(DESCRIPTOR_SERVICE, &service_descriptor);
    let mut body = Vec::new();
    body.extend_from_slice(&network.to_be_bytes());
    body.push(0xff);
    body.extend_from_slice(&service.to_be_bytes());
    body.push(0x03);
    body.push(0x80 | ((described.len() >> 8) as u8 & 0x0f));
    body.push(described.len() as u8);
    body.extend_from_slice(&described);
    long(table_id, transport, 0, 0, 0, &body)
}

/// MMT-SI の記述子。0xf000 以上のタグは長さが二バイト。
fn mh_descriptor(tag: u16, body: &[u8]) -> Vec<u8> {
    let mut out = tag.to_be_bytes().to_vec();
    if tag >= 0xf000 {
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    } else {
        out.push(body.len() as u8);
    }
    out.extend_from_slice(body);
    out
}

fn mh_short(title: &str, detail: &str) -> Vec<u8> {
    let mut body = vec![b'j', b'p', b'n', title.len() as u8];
    body.extend_from_slice(title.as_bytes());
    body.extend_from_slice(&(detail.len() as u16).to_be_bytes());
    body.extend_from_slice(detail.as_bytes());
    mh_descriptor(MH_DESCRIPTOR_SHORT_EVENT, &body)
}

fn mh_extended(items: &[(&str, &str)]) -> Vec<u8> {
    let mut listed = Vec::new();
    for (heading, value) in items {
        listed.push(heading.len() as u8);
        listed.extend_from_slice(heading.as_bytes());
        listed.extend_from_slice(&(value.len() as u16).to_be_bytes());
        listed.extend_from_slice(value.as_bytes());
    }
    let mut body = vec![0x00, b'j', b'p', b'n'];
    body.extend_from_slice(&(listed.len() as u16).to_be_bytes());
    body.extend_from_slice(&listed);
    body.extend_from_slice(&[0x00, 0x00]);
    mh_descriptor(MH_DESCRIPTOR_EXTENDED_EVENT, &body)
}

fn mh_event_section(
    table_id: u8,
    service: u16,
    network: u16,
    event: u16,
    described: &[u8],
) -> Vec<u8> {
    let mut body = vec![0x00, 0x01];
    body.extend_from_slice(&network.to_be_bytes());
    body.extend_from_slice(&[0x00, table_id]);
    body.extend_from_slice(&event.to_be_bytes());
    body.extend_from_slice(&[0xef, 0x6b, 0x12, 0x30, 0x00]);
    body.extend_from_slice(&[0x01, 0x00, 0x00]);
    body.push(0x80 | ((described.len() >> 8) as u8 & 0x0f));
    body.push(described.len() as u8);
    body.extend_from_slice(described);
    long(table_id, service, 0, 0, 0, &body)
}

#[test]
fn an_mh_event_table_is_read_with_its_own_lengths() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x000b,
        service: 0x0065,
    };
    let mut described = mh_short("うつわの旅", "千年の焼き物");
    described.extend_from_slice(&mh_extended(&[("出演者", "架空太郎")]));
    feed_mmt(
        &mut guide,
        &mh_event_section(
            mmttables::TABLE_MH_EIT_PRESENT,
            key.service,
            key.network,
            7,
            &described,
        ),
    );
    let programme = guide.programmes(key).next().expect("番組が無い");
    assert_eq!(programme.title, "うつわの旅");
    assert_eq!(programme.detail, "千年の焼き物");
    assert_eq!(
        programme.extended,
        vec![(String::from("出演者"), String::from("架空太郎"))]
    );
    assert_eq!(programme.event, 7);
    assert_eq!(programme.seconds, Some(3600));
}

#[test]
fn mmt_text_is_utf8_and_not_the_broadcast_coding() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x000b,
        service: 0x0065,
    };
    feed_mmt(
        &mut guide,
        &mh_event_section(
            mmttables::TABLE_MH_EIT_PRESENT,
            key.service,
            key.network,
            1,
            &mh_short("日曜のうた自慢", ""),
        ),
    );
    let programme = guide.programmes(key).next().expect("番組が無い");
    assert_eq!(programme.title, "日曜のうた自慢");
}

#[test]
fn an_mh_service_table_names_the_service() {
    let mut guide = Fed::new();
    let named = "ＢＳ　４Ｋ";
    let mut service_descriptor = vec![0x01, 0x00, named.len() as u8];
    service_descriptor.extend_from_slice(named.as_bytes());
    let described = mh_descriptor(MH_DESCRIPTOR_SERVICE, &service_descriptor);
    let mut body = vec![0x00, 0x0b, 0xff];
    body.extend_from_slice(&0x0065u16.to_be_bytes());
    body.push(0x03);
    body.push(0x80 | ((described.len() >> 8) as u8 & 0x0f));
    body.push(described.len() as u8);
    body.extend_from_slice(&described);
    let section = long(mmttables::TABLE_MH_SDT_OWN, 0xb110, 0, 0, 0, &body);
    feed_mmt(&mut guide, &section);
    let key = Key {
        network: 0x000b,
        service: 0x0065,
    };
    let service = guide.service(key).expect("サービスが無い");
    assert_eq!(service.name, named);
    assert_eq!(service.kind, 0x01);
    assert_eq!(
        guide.si.identity(),
        Some(Identity {
            transport: 0xb110,
            network: 0x000b,
        })
    );
}

#[test]
fn an_mh_time_table_sets_the_clock() {
    let mut guide = Fed::new();
    let mut section = vec![mmttables::TABLE_MH_TOT, 0x70, 0x00];
    section.extend_from_slice(&[0xef, 0x6b, 0x12, 0x30, 0x00]);
    section.extend_from_slice(&[0xf0, 0x00]);
    section.extend_from_slice(&[0; 4]);
    let length = section.len() - 3;
    section[1] = 0x70 | ((length >> 8) as u8 & 0x0f);
    section[2] = length as u8;
    feed_mmt(&mut guide, &section);
    assert_eq!(guide.now(), Some(1_788_870_600));
}

#[test]
fn a_section_of_each_table_lands_in_the_guide() {
    let mut guide = Fed::new();
    let (service, network, transport) = (0x0400u16, 0x7880u16, 0x7fe1u16);
    feed(
        &mut guide,
        SDT_PID,
        &service_section(service, network, transport, "ZWV"),
    );
    feed(
        &mut guide,
        EIT_PID,
        &event_section(0x4e, service, network, 7, &short("title", "detail")),
    );
    let key = Key { network, service };
    let named = guide.service(key).expect("サービスが入っていない");
    assert_eq!(named.name, "ZWV");
    assert_eq!(named.transport, transport);
    assert_eq!(named.kind, 0x01);
    let programme = guide.programmes(key).next().expect("番組が入っていない");
    assert_eq!(programme.title, "title");
    assert_eq!(programme.detail, "detail");
    assert_eq!(programme.event, 7);
    assert_eq!(programme.start, Some(1_788_870_600));
    assert_eq!(programme.seconds, Some(3600));
}

#[test]
fn a_thinner_account_of_the_same_programme_does_not_erase_the_title() {
    let mut guide = Fed::new();
    let (service, network) = (0x0400u16, 0x7880u16);
    let key = Key { network, service };
    feed(
        &mut guide,
        EIT_PID,
        &event_section(0x4e, service, network, 7, &short("title", "detail")),
    );
    let content = descriptor(DESCRIPTOR_CONTENT, &[0x80, 0xff]);
    feed(
        &mut guide,
        EIT_PID,
        &event_section(0x50, service, network, 7, &content),
    );
    let programme = guide.programmes(key).next().expect("番組が入っていない");
    assert_eq!(programme.title, "title");
    assert_eq!(programme.genres, [Genre::new(8, 0)]);
    assert_eq!(guide.filled(key).0, 1);
}

#[test]
fn the_first_day_of_the_epoch_was_a_thursday() {
    assert_eq!(weekday(0), "木");
    assert_eq!(weekday(1_788_870_600), "火");
}

#[test]
fn a_band_is_read_from_the_network_identifier() {
    assert_eq!(Band::of(0x7fe1).name(), "GR");
    assert_eq!(Band::of(0x7880), Band::of(0x7fe1));
    assert_eq!(Band::of(0x0004).name(), "BS");
    assert_eq!(Band::of(0x0007).name(), "CS");
    assert_eq!(Band::of(0x000b).name(), "BS4K");
    assert_eq!(Band::of(0x0001), Band::OTHER);
    assert_eq!(Band::OTHER.name(), "その他");
    assert!(Band::is_terrestrial(0x7fe1));
    assert!(!Band::is_terrestrial(0x0004));
    let all = Band::all();
    assert_eq!(all.len(), super::Network::defaults().len() + 1);
    assert_eq!(all.last(), Some(&Band::OTHER));
    assert_eq!(Band::of(0x0004).guide_title(), "BS番組表");
}

#[test]
fn merging_keeps_names_and_adds_programmes() {
    let mut mine = Fed::new();
    mine.tell_service(Service {
        network: 0x7fe1,
        service: 1032,
        name: "教育".to_owned(),
        kind: 1,
        ..Service::default()
    });
    let mut theirs = Fed::new();
    theirs.tell_service(Service {
        network: 0x7fe1,
        service: 1032,
        kind: 1,
        ..Service::default()
    });
    theirs.tell_service(Service {
        network: 0x0004,
        service: 101,
        name: "ＢＳ".to_owned(),
        kind: 1,
        ..Service::default()
    });
    theirs.tell(Programme {
        service: 101,
        network: 0x0004,
        event: 7,
        start: Some(100),
        seconds: Some(60),
        title: "番組".to_owned(),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told: Told::Mirakurun,
        details: Default::default(),
    });
    mine.merge(theirs.guide);
    let key = Key {
        network: 0x7fe1,
        service: 1032,
    };
    assert_eq!(mine.service(key).unwrap().name, "教育");
    assert_eq!(mine.services().count(), 2);
    assert_eq!(mine.programme_count(), 1);
}

#[test]
fn a_caption_is_read_from_the_data_content_descriptor_and_kept() {
    let mut programme = Programme {
        ..Programme::scheduled(
            Key {
                service: 0,
                network: 0,
            },
            0,
            Some(0),
            Some(60),
        )
    };
    describe(
        &[Descriptor {
            tag: DESCRIPTOR_DATA_CONTENT,
            body: vec![0x00, 0x08, 0x00, 0x00],
        }],
        &mut programme,
    );
    assert!(programme.caption);
    let mut without = programme.clone();
    without.caption = false;
    programme.absorb(without);
    assert!(programme.caption);
}

fn network_section(table_id: u8, network: u16, transports: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut name = vec![0x40];
    let named = b"\x0e\x7f\xe1TEST";
    name.push(named.len() as u8);
    name.extend_from_slice(named);
    let mut body = Vec::new();
    body.extend_from_slice(&[0xf0 | (name.len() >> 8) as u8, name.len() as u8]);
    body.extend_from_slice(&name);
    let mut loops = Vec::new();
    for (stream, original, descriptors) in transports {
        loops.extend_from_slice(&stream.to_be_bytes());
        loops.extend_from_slice(&original.to_be_bytes());
        loops.extend_from_slice(&[
            0xf0 | (descriptors.len() >> 8) as u8,
            descriptors.len() as u8,
        ]);
        loops.extend_from_slice(descriptors);
    }
    body.extend_from_slice(&[0xf0 | (loops.len() >> 8) as u8, loops.len() as u8]);
    body.extend_from_slice(&loops);
    let mut out = vec![table_id, 0xf0, 0x00];
    out.extend_from_slice(&network.to_be_bytes());
    out.extend_from_slice(&[0xc1, 0x00, 0x00]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0, 0, 0, 0]);
    let length = out.len() - 3;
    out[1] = 0xf0 | (length >> 8) as u8;
    out[2] = length as u8;
    out
}

#[test]
fn a_network_table_fills_the_multiplexes_and_the_buttons() {
    let mut guide = Fed::new();
    let mut ts_information = vec![0xcd, 0x00, 0x01, (2 << 2) as u8];
    let name = b"\x41\x42";
    ts_information.extend_from_slice(name);
    ts_information[1] = (ts_information.len() - 2) as u8;
    let services = vec![0x41, 0x03, 0x04, 0x00, 0x01];
    let mut descriptors = ts_information;
    descriptors.extend_from_slice(&services);
    feed(
        &mut guide,
        NIT_PID,
        &network_section(0x40, 0x7fe1, &[(0x0400, 0x7fe1, descriptors)]),
    );
    let network = guide.si.networks().next().expect("ネットワークが無い");
    assert_eq!(network.network, 0x7fe1);
    assert!(network.own);
    assert_eq!(network.multiplexes.len(), 1);
    let multiplex = &network.multiplexes[0];
    assert_eq!(multiplex.transport, 0x0400);
    assert_eq!(multiplex.remote_key, 1);
    assert_eq!(multiplex.services, vec![(1024, 0x01)]);
    let listed = guide
        .service(Key {
            network: 0x7fe1,
            service: 1024,
        })
        .expect("NIT に載ったサービスが無い");
    assert_eq!(listed.kind, 0x01);
    assert_eq!(listed.transport, 0x0400);
    assert!(listed.name.is_empty(), "NIT にはサービス名が含まれません");
    assert!(listed.watchable());
    let key = Key {
        network: 0x7fe1,
        service: 1024,
    };
    assert_eq!(
        guide.service(key).and_then(|service| service.remote_key),
        Some(1)
    );
}

#[test]
fn a_table_describing_a_neighbour_is_kept_apart_from_our_own() {
    let mut guide = Fed::new();
    feed(&mut guide, NIT_PID, &network_section(0x40, 0x7fe1, &[]));
    feed(&mut guide, NIT_PID, &network_section(0x41, 0x0004, &[]));
    let networks: Vec<(u16, bool)> = guide
        .si
        .networks()
        .map(|network| (network.network, network.own))
        .collect();
    assert_eq!(networks, vec![(0x0004, false), (0x7fe1, true)]);
}

/// ARIB STD-B60 5.3.1.5、TR-B39 36.1.2.3。
#[test]
fn a_tlv_network_table_puts_the_four_k_services_on_their_buttons() {
    let mut guide = Fed::new();
    let mut about = vec![0x40];
    let name = "高度ＢＳデジタル放送".as_bytes();
    about.push(name.len() as u8);
    about.extend_from_slice(name);
    about.extend_from_slice(&[
        0xcd, 0x0b, 0x02, 0x01, 0x00, 0x65, 0xff, 0xff, 0x03, 0x00, 0xe7, 0xff, 0xff,
    ]);
    let mut streams = Vec::new();
    streams.extend_from_slice(&0xb110u16.to_be_bytes());
    streams.extend_from_slice(&0x000bu16.to_be_bytes());
    let listed = vec![0x41, 0x03, 0x00, 0x65, 0x01];
    streams.extend_from_slice(&[0xf0 | (listed.len() >> 8) as u8, listed.len() as u8]);
    streams.extend_from_slice(&listed);

    let mut section = vec![0x40, 0xf0, 0x00];
    section.extend_from_slice(&0x000bu16.to_be_bytes());
    section.extend_from_slice(&[0xc1, 0x00, 0x00]);
    section.extend_from_slice(&[0xf0 | (about.len() >> 8) as u8, about.len() as u8]);
    section.extend_from_slice(&about);
    section.extend_from_slice(&[0xf0 | (streams.len() >> 8) as u8, streams.len() as u8]);
    section.extend_from_slice(&streams);
    section.extend_from_slice(&[0, 0, 0, 0]);
    let length = section.len() - 3;
    section[1] = 0xf0 | (length >> 8) as u8;
    section[2] = length as u8;

    feed_tlv(&mut guide, &section);
    let network = guide.si.networks().next().expect("ネットワークが無い");
    assert_eq!(network.network, 0x000b);
    assert_eq!(
        network.name, "高度ＢＳデジタル放送",
        "TLV-SI の文字列は UTF-8 で、8 単位符号ではありません"
    );
    assert!(network.own);
    assert_eq!(network.multiplexes[0].transport, 0xb110);
    assert_eq!(network.multiplexes[0].remote_key, 1);
    let listed = guide
        .service(Key {
            network: 0x000b,
            service: 0x0065,
        })
        .expect("NIT に載ったサービスが無い");
    assert_eq!(listed.kind, 0x01);
    assert_eq!(listed.transport, 0xb110);

    for (service, button) in [(0x0065u16, 1u8), (0x00e7, 3)] {
        let key = Key {
            network: 0x000b,
            service,
        };
        guide.tell_service(Service {
            network: 0x000b,
            transport: 0xb110,
            service,
            name: String::from("四"),
            kind: 0x01,
            ..Service::default()
        });
        assert_eq!(guide.service(key).expect("サービスが無い").name, "四");
        assert_eq!(
            guide.service(key).expect("サービスが無い").remote_key,
            Some(button),
            "サービス {service:#06x} にリモコン番号が付いていない"
        );
    }

    let mut other = Fed::new();
    feed(&mut other, NIT_PID, &section);
    assert_eq!(
        other
            .si
            .networks()
            .next()
            .expect("ネットワークが無い")
            .multiplexes
            .iter()
            .filter(|multiplex| multiplex.remote_key != 0)
            .count(),
        0
    );
}

#[test]
fn a_programme_that_moved_leaves_its_old_moment() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7880,
        service: 0x0400,
    };
    let make = |event, start| Programme {
        service: key.service,
        network: key.network,
        event,
        start: Some(start),
        seconds: Some(1800),
        title: format!("番組{event}"),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told: Told::Broadcast,
        details: Default::default(),
    };
    guide.tell(make(1, 1000));
    guide.tell(make(2, 2800));
    guide.tell(make(2, 4600));
    let starts: Vec<Moment> = guide.programmes(key).filter_map(|p| p.start).collect();
    assert_eq!(starts, vec![1000, 4600]);
}

#[test]
fn programmes_that_ended_are_forgotten() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7880,
        service: 0x0400,
    };
    let make = |event, start, seconds| Programme {
        service: key.service,
        network: key.network,
        event,
        start: Some(start),
        seconds,
        title: String::new(),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told: Told::Broadcast,
        details: Default::default(),
    };
    guide.tell(make(1, 1000, Some(1800)));
    guide.tell(make(2, 2000, None));
    guide.tell(make(3, 100_000, Some(1800)));
    guide.forget_before(50_000);
    let events: Vec<u16> = guide.programmes(key).map(|p| p.event).collect();
    assert_eq!(events, vec![2, 3]);
    guide.forget_before(2000 + 86_400 + 1);
    let events: Vec<u16> = guide.programmes(key).map(|p| p.event).collect();
    assert_eq!(events, vec![3]);
}

#[test]
fn a_schedule_of_another_multiplex_is_kept() {
    use isdb::tstables::{Event, EventTable};
    let mut guide = Fed::new();
    let table = EventTable {
        table_id: 0x60,
        service_id: 0x0191,
        transport_stream_id: 0x40f2,
        original_network_id: 0x0004,
        version: 1,
        number: 0,
        last_number: 0,
        segment_last_number: 0,
        last_table_id: 0x60,
        events: vec![Event {
            event_id: 7,
            start: Some(isdb::tstables::BroadcastTime {
                year: 2026,
                month: 9,
                day: 14,
                hour: 1,
                minute: 0,
                second: 0,
            }),
            duration: Some(1800),
            running_status: 0,
            scrambled: false,
            descriptors: Vec::new(),
        }],
    };
    guide.on_event_table(&table);
    let key = Key {
        network: 0x0004,
        service: 0x0191,
    };
    assert_eq!(guide.programmes(key).count(), 1);
    assert_eq!(guide.filled(key).0, 1);
}

#[test]
fn an_event_group_names_the_services_sharing_the_programme() {
    let mut programme = Programme {
        ..Programme::scheduled(
            Key {
                service: 0x0400,
                network: 0x7880,
            },
            0x1234,
            Some(100),
            Some(60),
        )
    };
    let group = |body: &[u8], into: &mut Programme| {
        describe(
            &[Descriptor {
                tag: tag::EVENT_GROUP,
                body: body.to_vec(),
            }],
            into,
        );
    };
    group(
        &[0x12, 0x04, 0x01, 0x12, 0x34, 0x04, 0x02, 0x12, 0x34],
        &mut programme,
    );
    assert_eq!(programme.shared, vec![(0x0401, 0x1234), (0x0402, 0x1234)]);
    let mut other = programme.clone();
    other.shared.clear();
    group(&[0x21, 0x04, 0x03, 0x00, 0x01], &mut other);
    assert!(other.shared.is_empty());
}

#[test]
fn an_extended_programme_pushes_the_next_one_out_of_the_way() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7fe1,
        service: 1024,
    };
    let programme = |event: u16, start: i64, seconds: u32, told: Told| Programme {
        service: key.service,
        network: key.network,
        event,
        start: Some(start),
        seconds: Some(seconds),
        title: format!("番組 {event}"),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told,
        details: Default::default(),
    };
    guide.tell(programme(1, 0, 1800, Told::Mirakurun));
    guide.tell(programme(2, 1800, 1800, Told::Mirakurun));
    guide.tell(programme(1, 0, 2400, Told::Now));
    guide.tell(programme(2, 2400, 1800, Told::Now));
    let listed: Vec<(u16, i64, u32)> = guide
        .programmes(key)
        .map(|p| (p.event, p.start.unwrap(), p.seconds.unwrap()))
        .collect();
    assert_eq!(listed, vec![(1, 0, 2400), (2, 2400, 1800)]);
    let first = guide.programmes(key).next().unwrap();
    assert_eq!(first.told, Told::Now);
    assert_eq!(first.title, "番組 1");
    guide.tell(programme(1, 0, 1800, Told::Broadcast));
    guide.tell(programme(1, 0, 1800, Told::Mirakurun));
    guide.tell(programme(2, 1800, 1800, Told::Mirakurun));
    guide.tell(programme(2, 1800, 1800, Told::Broadcast));
    let listed: Vec<(u16, i64, u32)> = guide
        .programmes(key)
        .map(|p| (p.event, p.start.unwrap(), p.seconds.unwrap()))
        .collect();
    assert_eq!(listed, vec![(1, 0, 2400), (2, 2400, 1800)]);
}

#[test]
fn a_programme_the_present_table_said_is_pinned() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x7fe1,
        service: 1024,
    };
    let programme = |event: u16, start: i64, seconds: u32, told: Told| Programme {
        service: key.service,
        network: key.network,
        event,
        start: Some(start),
        seconds: Some(seconds),
        title: format!("番組 {event}"),
        detail: String::new(),
        extended: Vec::new(),
        genres: Vec::new(),
        video: None,
        audios: Vec::new(),
        free: true,
        caption: false,
        shared: Vec::new(),
        told,
        details: Default::default(),
    };
    guide.tell(programme(9, 0, 3600, Told::Now));
    for _ in 0..3 {
        guide.tell(programme(1, 0, 1800, Told::Mirakurun));
        guide.tell(programme(2, 1800, 1800, Told::Mirakurun));
        guide.tell(programme(1, 0, 1800, Told::Broadcast));
        guide.tell(programme(2, 1800, 1800, Told::Broadcast));
        assert_eq!(guide.at(key, 100).unwrap().event, 9);
        assert_eq!(guide.at(key, 2000).unwrap().event, 9);
    }
    guide.tell(programme(3, 3600, 1800, Told::Mirakurun));
    assert_eq!(guide.at(key, 4000).unwrap().event, 3);
}

#[test]
fn an_mh_event_says_whether_it_is_hdr_and_names_every_sound() {
    let mut guide = Fed::new();
    let key = Key {
        network: 0x000b,
        service: 0x0065,
    };
    let mut described = mh_short("題", "");
    described.extend_from_slice(&mh_descriptor(
        MH_DESCRIPTOR_VIDEO_COMPONENT,
        &[0x63, 0x88, 0x01, 0x00, 0x50, b'j', b'p', b'n'],
    ));
    let mut sound = vec![0x03, 0x09, 0x01, 0x10, 0x11, 0xff, 0x40 | (7 << 1)];
    sound.extend_from_slice(b"jpn");
    sound.extend_from_slice("日本語".as_bytes());
    described.extend_from_slice(&mh_descriptor(MH_DESCRIPTOR_AUDIO_COMPONENT, &sound));
    let mut second = vec![0x03, 0x03, 0x01, 0x11, 0x11, 0xff, 7 << 1];
    second.extend_from_slice(b"eng");
    described.extend_from_slice(&mh_descriptor(MH_DESCRIPTOR_AUDIO_COMPONENT, &second));
    feed_mmt(
        &mut guide,
        &mh_event_section(
            mmttables::TABLE_MH_EIT_PRESENT,
            key.service,
            key.network,
            7,
            &described,
        ),
    );
    let programme = guide.programmes(key).next().expect("番組が無い");
    let picture = programme.video.expect("映像の情報が無い");
    assert_eq!(picture.hdr(), Some(true));
    assert_eq!(
        picture.name().as_deref(),
        Some("2160p 59.94fps、アスペクト比16:9、HDR（HLG）")
    );
    assert_eq!(programme.audios.len(), 2);
    assert_eq!(
        programme.audios[0].describe(),
        "日本語　3/2＋LFEモード（5.1ch）、48kHz、主音声"
    );
    assert_eq!(
        programme.audios[1].describe(),
        "英語　2/0モード（ステレオ）、48kHz"
    );
    assert_eq!(programme.audio().map(|sound| sound.kind), Some(0x09));
}

#[test]
fn a_ts_event_keeps_every_sound_with_who_it_is_for() {
    let mut programme = Programme {
        ..Programme::scheduled(
            Key {
                service: 0,
                network: 0,
            },
            0,
            Some(0),
            Some(60),
        )
    };
    let mut main = vec![0x02, 0x03, 0x10, 0x0f, 0xff, 0x40 | (7 << 1)];
    main.extend_from_slice(b"jpn");
    let mut assist = vec![0x02, 0x23, 0x11, 0x0f, 0xff, 7 << 1];
    assist.extend_from_slice(b"jpn");
    describe(
        &[
            Descriptor {
                tag: DESCRIPTOR_COMPONENT,
                body: vec![0x01, 0xb3, 0x00, b'j', b'p', b'n'],
            },
            Descriptor {
                tag: DESCRIPTOR_AUDIO_COMPONENT,
                body: main,
            },
            Descriptor {
                tag: DESCRIPTOR_AUDIO_COMPONENT,
                body: assist,
            },
        ],
        &mut programme,
    );
    let picture = programme.video.expect("映像の情報が無い");
    assert_eq!(picture.hdr(), None);
    assert_eq!(
        picture.name().as_deref(),
        Some("1080i（1125i）、アスペクト比16:9 パンベクトルなし")
    );
    assert_eq!(
        programme.audios[0].describe(),
        "日本語　2/0モード（ステレオ）、48kHz、主音声"
    );
    assert_eq!(
        programme.audios[1].describe(),
        "日本語　2/0モード（ステレオ）（解説）、48kHz"
    );
}

#[test]
fn a_picture_with_an_unnamed_component_type_falls_back_to_its_fields() {
    let picture = Picture {
        component_type: Some(0x7f),
        resolution: 6,
        aspect: 0,
        progressive: true,
        frame_rate: 0,
        transfer: 0,
    };
    assert_eq!(picture.name().as_deref(), Some("2160p"));
    assert_eq!(picture.hdr(), None);
}

#[test]
fn h264_and_hevc_component_descriptors_describe_the_video_too() {
    for (stream_content, component_type, named) in [
        (0x05u8, 0xf3u8, "180p、アスペクト比16:9 パンベクトルなし"),
        (0x09, 0x93, "2160p、アスペクト比16:9 パンベクトルなし"),
    ] {
        let mut programme = Programme::scheduled(
            Key {
                service: 0,
                network: 0,
            },
            0,
            Some(0),
            Some(60),
        );
        describe(
            &[Descriptor {
                tag: DESCRIPTOR_COMPONENT,
                body: vec![stream_content, component_type, 0x00, b'j', b'p', b'n'],
            }],
            &mut programme,
        );
        let picture = programme.video.expect("映像の情報が無い");
        assert_eq!(picture.name().as_deref(), Some(named));
    }
}

#[test]
fn a_4k_programme_from_the_broadcast_names_its_resolution() {
    let picture = Picture::of_component_type(0x93);
    assert_eq!(
        picture.name().as_deref(),
        Some("2160p、アスペクト比16:9 パンベクトルなし")
    );
}

#[test]
fn a_low_event_table_for_partial_reception_lands_in_the_guide() {
    let mut guide = Fed::new();
    let (service, network) = (0xf180u16, 0x7c30u16);
    feed(
        &mut guide,
        EIT_LOW_PID,
        &event_section(0x4e, service, network, 3, &short("oneseg", "")),
    );
    let key = Key { network, service };
    let programme = guide.programmes(key).next().expect("番組が入っていない");
    assert_eq!(programme.title, "oneseg");
    assert_eq!(programme.event, 3);
}

#[test]
fn a_moment_is_written_the_same_way_everywhere() {
    let start = epoch(BroadcastTime {
        year: 2026,
        month: 9,
        day: 8,
        hour: 12,
        minute: 30,
        second: 0,
    });
    assert_eq!(hhmm(start), "12:30");
    assert_eq!(date_label(start), "9/8（火）");
    assert_eq!(airtime(start, Some(start + 90 * 60)), "12:30〜14:00");
    assert_eq!(airtime(start, None), "12:30〜");
}

#[test]
fn genres_are_named_from_the_standards_tables() {
    assert_eq!(Genre::new(0x0, 0x1).describe(), "ニュース／報道（天気）");
    assert_eq!(
        Genre::new(0x9, 0x4).describe(),
        "劇場／公演（歌舞伎・古典）"
    );
    assert_eq!(
        Genre::new(0xa, 0xb).describe(),
        "趣味／教育（生涯教育・資格）"
    );
    assert_eq!(Genre::new(0x3, 0xf).describe(), "ドラマ（その他）");
    assert_eq!(Genre::new(0x3, 0x7).describe(), "ドラマ");
    assert_eq!(Genre::new(0xf, 0xf).describe(), "その他");
    let cs = Genre {
        large: 0xe,
        middle: 0x1,
        user: 0x12,
    };
    assert_eq!(cs.describe(), "洋画（CS）（コメディー）");
    assert_eq!(cs.family(), Some(0x6));
    let sport = Genre {
        large: 0xe,
        middle: 0x1,
        user: 0x03,
    };
    assert_eq!(sport.describe(), "スポーツ（CS）（アメリカンフットボール）");
    assert_eq!(sport.family(), Some(0x1));
    let extend = Genre {
        large: 0xe,
        middle: 0x0,
        user: 0x01,
    };
    assert_eq!(extend.describe(), "番組付属情報（延長の可能性あり）");
    assert_eq!(extend.family(), None);
    let programme = Programme {
        genres: vec![extend, Genre::new(0x1, 0x1)],
        ..Programme::scheduled(
            Key {
                network: 0x7fef,
                service: 0x400,
            },
            1,
            None,
            None,
        )
    };
    assert_eq!(programme.genre(), Some(&Genre::new(0x1, 0x1)));
}

#[test]
fn bs_services_get_their_remote_key_from_the_table() {
    assert_eq!(default_remote_key(0x0004, 101), Some(1));
    assert_eq!(default_remote_key(0x0004, 102), Some(1));
    assert_eq!(default_remote_key(0x0004, 103), None);
    assert_eq!(default_remote_key(0x0004, 231), Some(3));
    assert_eq!(default_remote_key(0x0004, 232), Some(3));
    assert_eq!(default_remote_key(0x0004, 143), Some(4));
    assert_eq!(default_remote_key(0x0004, 193), Some(9));
    assert_eq!(default_remote_key(0x0004, 202), Some(10));
    assert_eq!(default_remote_key(0x0004, 222), Some(12));
    assert_eq!(default_remote_key(0x0004, 929), None);
    assert_eq!(default_remote_key(0x000b, 101), None);
    assert_eq!(default_remote_key(0x7fe5, 1064), None);
    let mut guide = Guide::new();
    guide.tell_listed(
        Key {
            network: 0x0004,
            service: 161,
        },
        1,
        0x4011,
    );
    assert_eq!(
        guide
            .service(Key {
                network: 0x0004,
                service: 161
            })
            .unwrap()
            .remote_key,
        Some(6)
    );
}

#[test]
fn mh_cdt_logo_parts_are_joined_in_order() {
    let mut info = Info::new();
    let key = (0x000b, 7, 0x65, 1);
    let head = b"\x89PNG\r\n\x1a\nAAAA".to_vec();
    let middle = b"BBBB".to_vec();
    let tail = b"CCCC\0\0\0\0IEND\0\0\0\0".to_vec();
    assert!(info.on_logo_part(key, 3, &head).is_none());
    assert!(info.on_logo_part(key, 5, &tail).is_none());
    let png = info.on_logo_part(key, 4, &middle).expect("揃った");
    assert_eq!(png, [head.clone(), middle, tail.clone()].concat());
    let whole = [head, tail].concat();
    assert_eq!(info.on_logo_part(key, 0, &whole), Some(whole));
}

#[test]
fn details_are_read_only_for_the_networks_the_setting_names() {
    let item = |heading: &str, value: &str| {
        let (heading, value) = (latin(heading), latin(value));
        let mut bytes = vec![heading.len() as u8];
        bytes.extend_from_slice(&heading);
        bytes.push(value.len() as u8);
        bytes.extend_from_slice(&value);
        bytes
    };
    let items = item("ab", "one");
    let mut extended_body = vec![0x00, b'j', b'p', b'n', items.len() as u8];
    extended_body.extend_from_slice(&items);
    extended_body.push(0);
    let title = latin("title");
    let detail = latin("detail");
    let mut short_body = vec![b'j', b'p', b'n', title.len() as u8];
    short_body.extend_from_slice(&title);
    short_body.push(detail.len() as u8);
    short_body.extend_from_slice(&detail);

    let told = |network: u16| {
        let mut programme = Programme::scheduled(
            Key {
                service: 1,
                network,
            },
            0,
            None,
            None,
        );
        describe(
            &[
                Descriptor {
                    tag: tag::SHORT_EVENT,
                    body: short_body.clone(),
                },
                Descriptor {
                    tag: tag::EXTENDED_EVENT,
                    body: extended_body.clone(),
                },
            ],
            &mut programme,
        );
        programme
    };

    assert!(!told(0x0004).extended.is_empty());
    Detailed::set(Some(vec![(0x0004, 0x0004)]));
    assert_eq!(told(0x0004).extended, [("ab".to_owned(), "one".to_owned())]);
    let cs = told(0x0006);
    assert!(
        cs.extended.is_empty(),
        "無効にしたネットワークでは拡張形式イベント記述子を読まない"
    );
    assert_eq!(cs.title, "title", "タイトルは今までどおり届く");
    assert_eq!(cs.detail, "detail", "一行の説明も今までどおり");
    Detailed::set(Some(Vec::new()));
    assert!(told(0x0004).extended.is_empty());
    // この設定はプロセス全体で共有なので戻す。
    Detailed::set(None);
}

#[test]
fn a_band_tells_the_ranges_it_holds() {
    Band::set_networks(Vec::new());
    assert_eq!(Band::of(0x0004).ranges(), [(0x0004, 0x0004)]);
    assert_eq!(Band::of(0x0006).ranges(), [(0x0006, 0x0007)]);
    assert_eq!(Band::of(0x7fe6).ranges(), [(0x7880, 0x7fef)]);
    assert!(Band::OTHER.ranges().is_empty());
}

#[test]
fn sdt_other_does_not_move_a_confirmed_transport() {
    let mut guide = Guide::new();
    let key = Key {
        network: 4,
        service: 200,
    };
    let told = |transport, confirmed| Service {
        network: 4,
        transport,
        transport_confirmed: confirmed,
        service: 200,
        name: String::from("ゼロウェーブシネマ"),
        kind: 1,
        ..Service::default()
    };
    guide.tell_service(told(0x40f2, false));
    assert_eq!(guide.service(key).unwrap().transport, 0x40f2);
    guide.tell_service(told(0x48f3, true));
    assert_eq!(guide.service(key).unwrap().transport, 0x48f3);
    guide.tell_service(told(0x40f2, false));
    let service = guide.service(key).unwrap();
    assert_eq!(service.transport, 0x48f3);
    assert!(service.transport_confirmed);
}
