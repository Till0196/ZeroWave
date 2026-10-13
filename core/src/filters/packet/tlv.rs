// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MMT/TLV：TLV パケットから MMTP、MMTP から MPU、MPU からアクセス単位。
//! 処理単位のアクセス単位は読まずに数える（放送は `sample_number` を 0 のままにする）。

use std::collections::BTreeMap;

use isdb::mh::{MESSAGE_CA, MESSAGE_M2_SECTION, MESSAGE_M2_SHORT_SECTION, MESSAGE_PA};
use isdb::mhdata::MESSAGE_DATA_TRANSMISSION;
use isdb::mmttables::{
    self, ClockBase, DESCRIPTOR_MPU_EXTENDED_TIMESTAMP, DESCRIPTOR_MPU_TIMESTAMP,
    ExtendedTimestampEntry, TABLE_MPT_COMPLETE, TABLE_MPT_SUBSET_FIRST,
};
use mediadecoder::es::Codec;
use mmttlv::mmtp::{self, Mmtp};
use mmttlv::mpu::{self, Reassembler};
use mmttlv::signalling::Signalling;
use mmttlv::tlv::{self, Piece};

use super::{Clock, Origin, PacketSink, Section};
use crate::filters::{Counted, Counts, Kind, Unit};

#[derive(Debug)]
pub struct Parser {
    packets: tlv::Feed,
    signalling: Signalling,
    units: Reassembler,
    assets: BTreeMap<u16, Asset>,
    clock: ClockBase,
    latest_ntp: Option<u64>,
    /// 読めないことを示すフラグが立ったまま届いたパケット。組み立てると長さだけ正しい壊れた単位が出来る。
    scrambled: u64,
    scrambled_on: BTreeMap<u16, u64>,
    kept: Vec<u16>,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            packets: tlv::Feed::new(),
            signalling: Signalling::new(),
            units: Reassembler::new(),
            assets: BTreeMap::new(),
            clock: ClockBase::new(),
            latest_ntp: None,
            scrambled: 0,
            scrambled_on: BTreeMap::new(),
            kept: Vec::new(),
        }
    }

    pub fn scrambled(&self, packet_id: u16) -> u64 {
        self.scrambled_on.get(&packet_id).copied().unwrap_or(0)
    }

    pub fn scrambled_packets(&self) -> u64 {
        self.scrambled
    }

    pub fn identifiers(&self) -> Vec<u16> {
        self.units.packet_ids().collect()
    }

    /// カウンターは MMTP のパケット番号。二度来た物、CM の継ぎ目で付け直された番号は異常ではない。
    pub fn counted(&self, packet_id: u16) -> Option<Counted> {
        let tally = self.units.tally(packet_id)?;
        Some(Counted {
            arrived: tally.arrived,
            passed: tally.arrived - tally.scrambled - tally.out_of_order,
            lost: tally.lost,
            errors: tally.errors,
            scrambled: tally.scrambled,
        })
    }

    pub fn counts(&self) -> Counts {
        let packets = self.packets.stats();
        let units = self.units.stats();
        Counts {
            packets: packets.packets,
            dropped: units.lost_packets,
            errors: packets.invalid + units.parse_errors + units.fragment_errors,
            scrambled: self.scrambled,
        }
    }

    pub fn push<S: PacketSink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) {
        for piece in self.packets.feed(bytes) {
            match piece {
                Piece::Ntp(ntp) => {
                    self.clock.set(ntp);
                    self.latest_ntp = Some(ntp);
                    sink.on_clock(Clock::Ntp(ntp));
                }
                Piece::Control(payload) => {
                    if let Some(section) = isdb::mh::control_section(&payload) {
                        sink.on_section(&Section {
                            origin: Origin::TlvSi,
                            pid: 0,
                            program_map: false,
                            data: section,
                        });
                    }
                }
                Piece::Mmtp {
                    packet_id,
                    payload_type,
                    timestamp,
                    sequence_number,
                    rap,
                    payload,
                    scrambled,
                    ..
                } => {
                    if scrambled {
                        self.scrambled += 1;
                        *self.scrambled_on.entry(packet_id).or_default() += 1;
                    }
                    let packet = Mmtp {
                        packet_id,
                        payload_type,
                        timestamp,
                        sequence_number,
                        rap,
                        payload: &payload,
                        scrambled,
                    };
                    for unit in self.units.push(&packet) {
                        self.mpu(&unit, sink);
                    }
                    if !scrambled && payload_type == mmtp::SIGNALLING {
                        self.signalled(packet_id, &payload, sink);
                    }
                }
            }
        }
    }

    fn signalled<S: PacketSink + ?Sized>(&mut self, packet_id: u16, payload: &[u8], sink: &mut S) {
        for message in self.signalling.push(packet_id, payload) {
            match message.id {
                MESSAGE_PA => {
                    for (table_id, bytes) in isdb::mh::package_message_tables(&message.payload) {
                        if (TABLE_MPT_SUBSET_FIRST..=TABLE_MPT_COMPLETE).contains(&table_id)
                            && let Some(table) = mmttables::package_table(bytes)
                        {
                            self.package(&table);
                            sink.on_package(packet_id, &table);
                        } else {
                            sink.on_mmt_table(packet_id, bytes);
                        }
                    }
                }
                MESSAGE_CA => sink.on_mmt_table(packet_id, &message.payload),
                MESSAGE_M2_SECTION | MESSAGE_M2_SHORT_SECTION | MESSAGE_DATA_TRANSMISSION => {
                    for section in isdb::mh::sections(&message.payload) {
                        sink.on_section(&Section {
                            origin: Origin::MmtSi,
                            pid: packet_id,
                            program_map: false,
                            data: section,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn package(&mut self, table: &mmttables::PackageTable) {
        let mut named: Vec<u16> = Vec::new();
        for entry in &table.assets {
            let Some(packet_id) = entry.packet_id() else {
                continue;
            };
            named.push(packet_id);
            let asset = self.assets.entry(packet_id).or_default();
            asset.packet_id = packet_id;
            asset.kind = Kind::of_asset(&entry.kind);
            if let Some(found) = mmttables::find(&entry.descriptors, DESCRIPTOR_MPU_TIMESTAMP) {
                let mut newest = None;
                for stamp in mmttables::mpu_timestamps(&found.body) {
                    asset.times.insert(stamp.sequence, stamp.ntp);
                    newest = Some(stamp.sequence);
                }
                if let Some(newest) = newest {
                    keep_near(&mut asset.times, newest);
                }
            }
            if let Some(found) =
                mmttables::find(&entry.descriptors, DESCRIPTOR_MPU_EXTENDED_TIMESTAMP)
            {
                let read = mmttables::extended_timestamp(&found.body);
                if !read.truncated {
                    if let Some(timescale) = read.timescale {
                        asset.timescale = timescale;
                    }
                    asset.offset_kind = read.pts_offset_type;
                    asset.default_pts = read.default_pts_offset;
                    let mut newest = None;
                    for entry in read.entries {
                        newest = Some(entry.sequence);
                        asset.extended.insert(entry.sequence, entry);
                    }
                    if let Some(newest) = newest {
                        keep_near(&mut asset.extended, newest);
                    }
                }
            }
            if asset.kind == Kind::Caption
                && let Some(found) =
                    mmttables::find(&entry.descriptors, isdb::descriptors::mh_tag::DATA_CODING)
            {
                asset.caption = isdb::mhcaption::subtitle_info(&found.body);
            }
        }
        // 同じ ID を次に使うアセットが古いタイムスタンプを拾わないよう、MPT から消えたアセットは時刻の状態ごと消す。
        self.assets.retain(|id, _| named.contains(id));
    }

    pub fn keep(&mut self, pids: &[u16]) {
        if self.kept != pids {
            self.kept = pids.to_vec();
        }
    }

    fn mpu<S: PacketSink + ?Sized>(&mut self, unit: &mpu::AccessUnit, sink: &mut S) {
        if !unit.timed {
            sink.on_item(
                unit.packet_id,
                unit.mpu_sequence_number,
                unit.item_id,
                unit.fragmentation,
                &unit.data,
            );
            return;
        }
        let Some(asset) = self.assets.get_mut(&unit.packet_id) else {
            return;
        };
        // 字幕の時刻は文書の中。MPU の提示時刻（STD-B60 9.3.2）を添える。
        if asset.kind == Kind::Caption {
            let at = caption_ntp(asset, unit.mpu_sequence_number, self.latest_ntp, &unit.data)
                .and_then(|ntp| self.clock.to_90k(ntp));
            sink.on_caption(unit.packet_id, unit.mpu_sequence_number, at, &unit.data);
            return;
        }
        if matches!(asset.kind, Kind::Video(_) | Kind::Audio(_))
            && !self.kept.is_empty()
            && !self.kept.contains(&unit.packet_id)
        {
            asset.have_mpu = false;
            return;
        }
        if !asset.have_mpu || asset.current != unit.mpu_sequence_number {
            // 最後のアクセスユニットは次の処理単位の先頭で終わる（音声は一 MPU 一単位なので捨てない）。
            let ending = asset.current;
            if let Some(finished) = asset.finish()
                && let Some(times) = asset.times_90k(&mut self.clock, ending, finished.index)
            {
                sink.on_unit(&finished.unit(unit.packet_id, &times));
            }
            asset.trusted = asset.have_mpu;
            asset.current = unit.mpu_sequence_number;
            asset.have_mpu = true;
            asset.index = 0;
            asset.unit.clear();
        }
        let Some(finished) = asset.data_unit(&unit.data, unit.rap, unit.after_loss) else {
            return;
        };
        let Some(times) =
            asset.times_90k(&mut self.clock, unit.mpu_sequence_number, finished.index)
        else {
            return;
        };
        sink.on_unit(&finished.unit(unit.packet_id, &times));
    }
}

#[derive(Debug, Default)]
struct Asset {
    packet_id: u16,
    kind: Kind,
    timescale: u32,
    offset_kind: u8,
    default_pts: u16,
    times: BTreeMap<u32, u64>,
    extended: BTreeMap<u32, ExtendedTimestampEntry>,
    caption: Option<isdb::mhcaption::SubtitleInfo>,

    current: u32,
    have_mpu: bool,
    trusted: bool,
    index: u32,

    unit: Vec<u8>,
    open: bool,
    rap: bool,
    /// The loss belongs to the picture, not to the data unit that noticed it.
    after_loss: bool,
}

struct Finished {
    index: u32,
    rap: bool,
    after_loss: bool,
    data: Vec<u8>,
}

impl Finished {
    fn unit(&self, stream: u16, times: &mmttables::AccessUnitTimes) -> Unit<'_> {
        Unit {
            stream,
            dts: Some(times.dts),
            pts: Some(times.pts),
            rap: self.rap,
            after_loss: self.after_loss,
            data: &self.data,
        }
    }
}

const UNIT_MOST: usize = 64 << 20;

impl Asset {
    fn data_unit(&mut self, data: &[u8], rap: bool, after_loss: bool) -> Option<Finished> {
        if !self.trusted {
            self.after_loss |= after_loss;
            return None;
        }
        if matches!(self.kind, Kind::Video(_)) {
            let mut done = None;
            if starts_access_unit(self.kind, data) {
                done = self.finish();
                self.start(rap);
            }
            if self.open {
                // AUD の来ない壊れたストリームで伸び続けないようにする。超えたらこの単位は捨てる。
                if self.unit.len() + data.len() > UNIT_MOST {
                    self.unit.clear();
                    self.open = false;
                    self.after_loss = true;
                    return done;
                }
                self.unit.extend_from_slice(data);
                self.rap |= rap;
            }
            self.after_loss |= after_loss;
            return done;
        }
        let done = self.finish();
        self.start(rap);
        self.unit.extend_from_slice(data);
        self.after_loss |= after_loss;
        done
    }

    fn start(&mut self, rap: bool) {
        self.index += 1;
        self.open = true;
        self.rap = rap;
        self.unit.clear();
    }

    fn finish(&mut self) -> Option<Finished> {
        let done = (self.open && !self.unit.is_empty()).then(|| Finished {
            index: self.index,
            rap: self.rap,
            after_loss: std::mem::take(&mut self.after_loss),
            data: {
                let room = self.unit.capacity();
                std::mem::replace(&mut self.unit, Vec::with_capacity(room))
            },
        });
        self.open = false;
        done
    }

    fn times_90k(
        &self,
        clock: &mut ClockBase,
        sequence: u32,
        index: u32,
    ) -> Option<mmttables::AccessUnitTimes> {
        let ntp = *self.times.get(&sequence)?;
        let entry = self.extended.get(&sequence)?;
        mmttables::access_unit_times(
            clock,
            entry,
            self.timescale,
            self.offset_kind,
            self.default_pts,
            ntp,
            index,
        )
    }
}

const MPU_KEPT: u32 = 256;

/// 捨てないと見続ける間伸びる。番号は一周するので差で比べる。
fn keep_near<T>(times: &mut BTreeMap<u32, T>, newest: u32) {
    times.retain(|&sequence, _| {
        newest.wrapping_sub(sequence) <= MPU_KEPT || sequence.wrapping_sub(newest) <= MPU_KEPT
    });
}

fn starts_access_unit(kind: Kind, unit: &[u8]) -> bool {
    if unit.len() < 6 || u32::from_be_bytes([unit[0], unit[1], unit[2], unit[3]]) < 2 {
        return false;
    }
    match kind {
        Kind::Video(Codec::Hevc) => (unit[4] >> 1) & 0x3f == 35,
        Kind::Video(Codec::H264) => unit[4] & 0x1f == 9,
        _ => false,
    }
}

/// ARIB STD-B60 9.3. The time control mode says what a TTML `begin` counts from.
fn caption_ntp(asset: &Asset, sequence: u32, latest: Option<u64>, data: &[u8]) -> Option<u64> {
    let mpu = asset.times.get(&sequence).copied().or(latest)?;
    let Some(info) = &asset.caption else {
        return Some(mpu);
    };
    let begin = isdb::mhcaption::mfu(data)
        .filter(|mfu| mfu.data_type == isdb::mhcaption::DATA_TTML)
        .and_then(|mfu| ttml_begin_millis(mfu.data));
    let Some(begin) = begin else {
        return Some(mpu);
    };
    let origin = match info.tmd {
        0b0000 => ((mpu >> 32) / 86_400 * 86_400) << 32,
        0b0010 => info.reference_start?,
        0b0011 => mpu,
        _ => return Some(mpu),
    };
    Some(ntp_add_millis(origin, begin))
}

fn ntp_add_millis(ntp: u64, millis: i64) -> u64 {
    let seconds = i64::try_from(ntp >> 32).unwrap_or(0) + millis.div_euclid(1000);
    let fraction = (ntp & 0xffff_ffff) + ((millis.rem_euclid(1000) as u64) << 32) / 1000;
    let seconds = seconds + (fraction >> 32) as i64;
    (seconds.max(0) as u64) << 32 | (fraction & 0xffff_ffff)
}

fn ttml_begin_millis(ttml: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(ttml).ok()?;
    let mut earliest: Option<i64> = None;
    let mut rest = text;
    while let Some(at) = rest.find("begin=\"") {
        rest = &rest[at + 7..];
        let end = rest.find('"')?;
        if let Some(millis) = ttml_time_millis(&rest[..end]) {
            earliest = Some(earliest.map_or(millis, |e| e.min(millis)));
        }
        rest = &rest[end..];
    }
    earliest
}

fn ttml_time_millis(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.contains(':') {
        let mut parts = text.splitn(3, ':');
        let hours: i64 = parts.next()?.parse().ok()?;
        let minutes: i64 = parts.next()?.parse().ok()?;
        let seconds = parts.next()?;
        let (whole, fraction) = match seconds.find(['.', ':']) {
            Some(dot) if seconds.as_bytes()[dot] == b':' => {
                let frames: i64 = seconds[dot + 1..].parse().ok()?;
                (&seconds[..dot], frames * 1000 / 30)
            }
            Some(dot) => {
                let digits = &seconds[dot + 1..];
                let mut millis = 0i64;
                for (i, c) in digits.chars().take(3).enumerate() {
                    millis += i64::from(c.to_digit(10)?) * 10i64.pow(2 - i as u32);
                }
                (&seconds[..dot], millis)
            }
            None => (seconds, 0),
        };
        let whole: i64 = whole.parse().ok()?;
        return Some(((hours * 60 + minutes) * 60 + whole) * 1000 + fraction);
    }
    let unit_at = text.find(|c: char| c.is_ascii_alphabetic())?;
    let value: f64 = text[..unit_at].parse().ok()?;
    let scale = match &text[unit_at..] {
        "h" => 3_600_000.0,
        "m" => 60_000.0,
        "s" => 1000.0,
        "ms" => 1.0,
        _ => return None,
    };
    Some((value * scale).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpu_times_keep_only_the_near_ones() {
        let mut times = std::collections::BTreeMap::new();
        for sequence in 0..1000u32 {
            times.insert(sequence, ());
            super::keep_near(&mut times, sequence);
        }
        assert_eq!(times.len() as u32, super::MPU_KEPT + 1);
        assert!(times.contains_key(&999) && times.contains_key(&(999 - super::MPU_KEPT)));

        let mut wrapped = std::collections::BTreeMap::new();
        for sequence in [u32::MAX - 2, u32::MAX - 1, u32::MAX, 0, 1] {
            wrapped.insert(sequence, ());
        }
        wrapped.insert(5000, ());
        super::keep_near(&mut wrapped, 1);
        assert_eq!(wrapped.len(), 5, "折り返しの前後は残り、遠いものだけ消える");
    }

    #[derive(Default)]
    struct Counting {
        units: usize,
    }

    impl PacketSink for Counting {
        fn on_unit(&mut self, _unit: &Unit<'_>) {
            self.units += 1;
        }
    }

    fn unreadable_packet() -> Vec<u8> {
        let mut mmtp = vec![0x02, 0x00, 0xa1, 0x41];
        mmtp.extend_from_slice(&[0; 4]); // timestamp
        mmtp.extend_from_slice(&[0; 4]); // sequence number
        mmtp.extend_from_slice(&[0x00, 0x00, 0x00, 0x05]);
        mmtp.extend_from_slice(&[0x80, 0x01, 0x00, 0x01, 0x04]);
        mmtp.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let mut body = vec![0x00, 0x10, 0x21];
        body.extend(mmtp);
        let mut packet = vec![0x7f, 0x03];
        packet.extend_from_slice(&(body.len() as u16).to_be_bytes());
        packet.extend(body);
        packet
    }

    #[test]
    fn an_unreadable_packet_is_counted_and_never_reassembled() {
        let mut feed = Parser::new();
        let mut sink = Counting::default();
        let packet = unreadable_packet();
        for _ in 0..3 {
            feed.push(&packet, &mut sink);
        }
        assert_eq!(feed.scrambled_packets(), 3);
        assert_eq!(feed.scrambled(0xa141), 3);
        assert_eq!(feed.scrambled(0x1000), 0, "他のアセットまで数えている");
        assert_eq!(sink.units, 0, "デコードしていないものを組み立てた");
    }

    #[test]
    fn a_caption_is_timed_from_its_reference_time() {
        assert_eq!(ttml_time_millis("00:12:32.000"), Some(752_000));
        assert_eq!(ttml_time_millis("01:00:00.5"), Some(3_600_500));
        assert_eq!(ttml_time_millis("00:00:01:15"), Some(1_500));
        assert_eq!(ttml_time_millis("2.5s"), Some(2_500));
        assert_eq!(ttml_time_millis("300ms"), Some(300));
        assert_eq!(ttml_time_millis("nonsense"), None);
        let ttml = br#"<tt><body><div begin="00:00:02.000" end="00:00:05.000"><p begin="00:00:01.500">x</p></div></body></tt>"#;
        assert_eq!(ttml_begin_millis(ttml), Some(1_500));
        assert_eq!(ntp_add_millis(10 << 32, 1_500), 11 << 32 | 1 << 31);
        assert_eq!(ntp_add_millis(10 << 32 | 1 << 31, 500), 11 << 32);
        let mut mfu = vec![0x30, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
        let body = br#"<tt><body><div begin="00:00:10.000"><p>x</p></div></body></tt>"#;
        mfu[5] = (body.len() >> 8) as u8;
        mfu[6] = body.len() as u8;
        mfu.extend_from_slice(body);
        let mut asset = Asset {
            kind: Kind::Caption,
            ..Asset::default()
        };
        let reference = 1_000u64 << 32;
        asset.caption = Some(isdb::mhcaption::SubtitleInfo {
            tag: 0x30,
            version: 0,
            language: String::from("jpn"),
            kind: 0,
            format: 0,
            opm: 0,
            tmd: 0b0010,
            dmf: 0,
            resolution: 1,
            compression: 0,
            start_mpu_sequence: None,
            reference_start: Some(reference),
        });
        assert_eq!(
            caption_ntp(&asset, 7, Some(5_000 << 32), &mfu),
            Some(1_010 << 32)
        );
        asset.caption.as_mut().unwrap().reference_start = None;
        assert_eq!(caption_ntp(&asset, 7, Some(5_000 << 32), &mfu), None);
        mfu[4] = 0x10; // a PNG
        assert_eq!(
            caption_ntp(&asset, 7, Some(5_000 << 32), &mfu),
            Some(5_000 << 32)
        );
    }
}
