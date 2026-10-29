// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! TS：同期バイトで 188 バイトに切り、`mpegts::Demuxer` に通す。セクションは読まずに
//! [`PacketSink::on_section`] へ、PES は [`PacketSink::on_unit`] へ。

use isdb::tstables;
use mpegts::PACKET;
use mpegts::demux::{
    Carries, Demuxer, Handler, Pat, Pcr, Pes, Section as TsSection, carries_sections,
};

use super::{Clock, Origin, PacketSink, Section};
use crate::filters::{Counted, Counts, Unit};

const TSMF_PID: u16 = 0x002f;

#[derive(Debug, Default)]
pub struct Parser {
    demuxer: Demuxer,
    held: Vec<u8>,
    frame_header: Option<isdb::tsmf::Info>,
    mixing: Detector,
    /// `None` はまだ PAT が来ていない（空の PAT も一度は通知する。放送休止の判定に要る）。
    numbers: Option<Vec<u16>>,
    kept: Vec<u16>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// これを含まない番組の PES は数えるだけで組み立てない（次に PMT が届いたときから）。
    pub fn keep(&mut self, pids: &[u16]) {
        if self.kept != pids {
            self.kept = pids.to_vec();
        }
    }

    pub fn push<S: PacketSink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) {
        let mut bytes = bytes;
        if !self.held.is_empty() {
            let wanted = PACKET - self.held.len();
            let take = wanted.min(bytes.len());
            self.held.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.held.len() < PACKET {
                return;
            }
            let packet: [u8; PACKET] = self.held[..].try_into().expect("a whole packet");
            self.held.clear();
            self.packet(&packet, sink);
        }
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] != mpegts::SYNC {
                // 録画の先頭はパケットの途中から始まることがある。
                at += 1;
                continue;
            }
            if at + PACKET > bytes.len() {
                break;
            }
            // 次のパケットも同期バイトで始まっていなければ、これは中身の 0x47。
            if at + PACKET < bytes.len() && bytes[at + PACKET] != mpegts::SYNC {
                at += 1;
                continue;
            }
            let packet: &[u8; PACKET] = bytes[at..at + PACKET].try_into().expect("a whole packet");
            at += PACKET;
            self.packet(packet, sink);
        }
        self.held.extend_from_slice(&bytes[at..]);
    }

    fn packet<S: PacketSink + ?Sized>(&mut self, packet: &[u8; PACKET], sink: &mut S) {
        if pid_of(packet) == TSMF_PID
            && let Some(header) = isdb::tsmf::info(packet)
            && self.frame_header.as_ref() != Some(&header)
        {
            sink.on_frame_header(&header);
            if let Some(mixing) = self.mixing.header(&header) {
                sink.on_mixing(mixing);
            }
            self.frame_header = Some(header);
        }
        let mut out = Output {
            sink,
            numbers: &mut self.numbers,
            mixing: &mut self.mixing,
            kept: &self.kept,
            named: Vec::new(),
        };
        self.demuxer.push(packet, &mut out);
        for (pid, carries) in out.named {
            self.demuxer.expect(pid, carries);
        }
    }

    pub fn scrambled(&self, pid: u16) -> u64 {
        self.demuxer.scrambled_on(pid)
    }

    pub fn identifiers(&self) -> Vec<u16> {
        self.demuxer.identifiers()
    }

    pub fn counted(&self, pid: u16) -> Option<Counted> {
        self.demuxer.counted_on(pid)
    }

    pub fn counts(&self) -> Counts {
        Counts {
            packets: self.demuxer.packets(),
            dropped: self.demuxer.lost(),
            errors: self.demuxer.errors(),
            scrambled: self.demuxer.scrambled(),
        }
    }
}

/// 一つのストリームに TS がいくつ載っているか。TSMF の多重フレームを分けずに受けると、複数の TS が混ざる。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mixing {
    #[default]
    One,
    /// 多重フレームのヘッダが複数の TS を示していて、PAT でまだ確かめていない。
    Undecided,
    Mixed,
}

/// PAT の `transport_stream_id` の移り変わりから [`Mixing`] を決める。
/// ヘッダがあれば別の TS の PAT が一度来れば混ざっている。無ければ A・B・A と戻ってきたら混ざっている。
#[derive(Debug, Default)]
struct Detector {
    state: Mixing,
    framed: bool,
    last: Option<u16>,
    before: Option<u16>,
    /// 同じ TS の PAT が続いた数。
    same: u32,
}

/// ヘッダが複数の TS を示すのに一つの TS の PAT だけが続いたとき、一つと決めるまでの数（PAT は 0.1 秒ごと）。
const ALONE_AFTER: u32 = 5;
/// 混ざっていたのが一つに戻ったと決めるまでの数。スロットの少ない TS の PAT はまばらに届く。
const SETTLED_AFTER: u32 = 100;

impl Detector {
    /// 多重フレームのヘッダが届いた。状態が変わったら新しい状態を返す。
    fn header(&mut self, header: &isdb::tsmf::Info) -> Option<Mixing> {
        if self.framed || header.streams.len() < 2 {
            return None;
        }
        self.framed = true;
        self.same = 0;
        self.set(if self.state == Mixing::One {
            Mixing::Undecided
        } else {
            self.state
        })
    }

    /// PAT が届いた。状態が変わったら新しい状態を返す。
    fn pat(&mut self, stream_id: u16) -> Option<Mixing> {
        if self.last == Some(stream_id) {
            self.same += 1;
            return match self.state {
                Mixing::Undecided if self.same >= ALONE_AFTER => self.set(Mixing::One),
                Mixing::Mixed if self.same >= SETTLED_AFTER => self.set(Mixing::One),
                _ => None,
            };
        }
        let returned = self.before == Some(stream_id);
        let switched = self.last.is_some();
        self.before = self.last.replace(stream_id);
        self.same = 0;
        if switched && (self.framed || returned) {
            return self.set(Mixing::Mixed);
        }
        None
    }

    fn set(&mut self, state: Mixing) -> Option<Mixing> {
        (self.state != state).then(|| {
            self.state = state;
            state
        })
    }
}

pub fn pid_of(packet: &[u8; PACKET]) -> u16 {
    (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2])
}

struct Output<'a, S: PacketSink + ?Sized> {
    sink: &'a mut S,
    numbers: &'a mut Option<Vec<u16>>,
    mixing: &'a mut Detector,
    kept: &'a [u16],
    /// デマルチプレクサーを借りている間は通知できないので、パケットのあとで通知する。
    named: Vec<(u16, Carries)>,
}

impl<S: PacketSink + ?Sized> Handler for Output<'_, S> {
    fn on_pat(&mut self, pat: &Pat) {
        if let Some(mixing) = self.mixing.pat(pat.stream_id) {
            self.sink.on_mixing(mixing);
        }
        let numbers: Vec<u16> = pat.programs.iter().map(|program| program.number).collect();
        if self.numbers.as_ref() == Some(&numbers) {
            return;
        }
        self.sink.on_programs(&numbers);
        *self.numbers = Some(numbers);
    }

    fn on_section(&mut self, section: &TsSection<'_>) {
        if section.is_program_map
            && let Some(map) = tstables::program_map(section.data)
        {
            // どの識別子がセクションを伝送するかはストリームの型で決まる。テーブルを読むフィルターを待たない。
            let watched = self.kept.is_empty()
                || map
                    .streams
                    .iter()
                    .any(|stream| self.kept.contains(&stream.pid));
            for stream in &map.streams {
                self.named.push((
                    stream.pid,
                    if carries_sections(stream.stream_type) {
                        Carries::Sections
                    } else if watched {
                        Carries::Pes
                    } else {
                        Carries::Count
                    },
                ));
            }
        }
        self.sink.on_section(&Section {
            origin: Origin::Ts,
            pid: section.pid,
            program_map: section.is_program_map,
            data: section.data,
        });
    }

    fn on_pes(&mut self, pes: &Pes<'_>) {
        self.sink.on_unit(&Unit {
            stream: pes.pid,
            dts: pes.header.dts.map(|dts| dts as i64),
            pts: pes.header.pts.map(|pts| pts as i64),
            rap: pes.random_access,
            after_loss: pes.lost > 0,
            data: pes.payload,
        });
    }

    fn on_pcr(&mut self, pid: u16, pcr: Pcr) {
        self.sink.on_clock(Clock::Pcr {
            pid,
            ticks: pcr.to_90khz(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counting {
        units: usize,
        sections: usize,
        programs: Vec<u16>,
        clocks: usize,
    }

    impl PacketSink for Counting {
        fn on_programs(&mut self, numbers: &[u16]) {
            self.programs = numbers.to_vec();
        }
        fn on_section(&mut self, _: &Section<'_>) {
            self.sections += 1;
        }
        fn on_unit(&mut self, _: &Unit<'_>) {
            self.units += 1;
        }
        fn on_clock(&mut self, _: Clock) {
            self.clocks += 1;
        }
    }

    fn plain_packet() -> [u8; PACKET] {
        let mut packet = [0xffu8; PACKET];
        packet[0] = mpegts::SYNC;
        packet[1] = 0x01;
        packet[2] = 0x00;
        packet[3] = 0x10;
        packet
    }

    #[test]
    fn packets_are_found_wherever_the_recording_began() {
        let mut parser = Parser::new();
        let mut sink = Counting::default();
        let packet = plain_packet();
        let mut bytes = vec![0x47, 0x11, 0x22];
        bytes.extend_from_slice(&packet);
        bytes.extend_from_slice(&packet[..100]);
        parser.push(&bytes, &mut sink);
        parser.push(&packet[100..], &mut sink);
        assert_eq!(parser.counts().packets, 2);
        assert!(parser.held.is_empty());
    }
}
