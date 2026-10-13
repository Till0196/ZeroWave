// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! TS の録画を必要なサービスとストリームだけに絞る。PAT・PMT を書き直し、要らない PID のパケットを落とす。

use std::collections::{BTreeMap, BTreeSet};

use mpegts::{PACKET, SYNC, crc32};

use super::{Options, Part};

#[derive(Debug, Clone, Default)]
struct Map {
    pcr: Option<u16>,
    streams: Vec<(u16, Part)>,
}

#[derive(Debug)]
pub(super) struct Filter {
    options: Options,
    held: Vec<u8>,
    programs: BTreeMap<u16, u16>,
    nit: Option<u16>,
    pat_seen: bool,
    pat_sections: mpegts::sections::Sections,
    pmt_sections: BTreeMap<u16, mpegts::sections::Sections>,
    maps: BTreeMap<u16, Map>,
    drop: BTreeSet<u16>,
    rewritten: BTreeSet<u16>,
    counters: BTreeMap<u16, u8>,
}

impl Filter {
    pub(super) fn new(options: Options) -> Self {
        Self {
            options,
            held: Vec::new(),
            programs: BTreeMap::new(),
            nit: None,
            pat_seen: false,
            pat_sections: mpegts::sections::Sections::new(0),
            pmt_sections: BTreeMap::new(),
            maps: BTreeMap::new(),
            drop: BTreeSet::new(),
            rewritten: BTreeSet::new(),
            counters: BTreeMap::new(),
        }
    }

    fn passes_all(&self) -> bool {
        self.options.service.is_none()
            && self.options.caption
            && self.options.carousel
            && self.options.eit
    }

    pub(super) fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        if self.passes_all() {
            out.extend_from_slice(bytes);
            return;
        }
        self.held.extend_from_slice(bytes);
        let mut at = 0;
        while at + PACKET <= self.held.len() {
            if self.held[at] != SYNC {
                at += 1;
                continue;
            }
            let packet: [u8; PACKET] = self.held[at..at + PACKET]
                .try_into()
                .expect("188 バイトで切っている");
            self.packet(&packet, out);
            at += PACKET;
        }
        self.held.drain(..at);
    }

    fn packet(&mut self, packet: &[u8; PACKET], out: &mut Vec<u8>) {
        let pid = (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
        if pid == 0 {
            for section in self.pat_sections.feed(packet) {
                self.pat(&section, out);
            }
            if !self.rewrites_pat() {
                out.extend_from_slice(packet);
            }
            return;
        }
        if self.programs.values().any(|pmt| *pmt == pid) {
            let sections = self
                .pmt_sections
                .entry(pid)
                .or_insert_with(|| mpegts::sections::Sections::new(pid));
            for section in sections.feed(packet) {
                self.pmt(pid, &section, out);
            }
            if !self.rewritten.contains(&pid) && !self.drop.contains(&pid) && self.ready() {
                out.extend_from_slice(packet);
            }
            return;
        }
        if !self.options.eit && matches!(pid, 0x12 | 0x26 | 0x27) {
            return;
        }
        // テーブルを読み終えるまでは、どれを落とすか決まらない。
        if !self.ready() {
            if pid < 0x30 {
                out.extend_from_slice(packet);
            }
            return;
        }
        if self.drop.contains(&pid) {
            return;
        }
        out.extend_from_slice(packet);
    }

    fn rewrites_pat(&self) -> bool {
        self.options.service.is_some()
    }

    fn ready(&self) -> bool {
        if !self.pat_seen {
            return false;
        }
        match self.options.service {
            Some(service) => self
                .programs
                .get(&service)
                .is_some_and(|pmt| self.maps.contains_key(pmt)),
            None => self
                .programs
                .values()
                .all(|pmt| self.maps.contains_key(pmt)),
        }
    }

    fn pat(&mut self, section: &[u8], out: &mut Vec<u8>) {
        if section.first() != Some(&0x00) || !mpegts::sections::check(section) {
            return;
        }
        let Some(body) = section.get(8..section.len() - 4) else {
            return;
        };
        let mut programs = BTreeMap::new();
        let mut nit = None;
        for entry in body.as_chunks::<4>().0 {
            let number = u16::from_be_bytes([entry[0], entry[1]]);
            let pid = (u16::from(entry[2] & 0x1f) << 8) | u16::from(entry[3]);
            if number == 0 {
                nit = Some(pid);
            } else {
                programs.insert(number, pid);
            }
        }
        if programs != self.programs {
            self.maps
                .retain(|pmt, _| programs.values().any(|p| p == pmt));
            self.pmt_sections
                .retain(|pmt, _| programs.values().any(|p| p == pmt));
        }
        self.programs = programs;
        self.nit = nit;
        self.pat_seen = true;
        self.decide();
        if let Some(service) = self.options.service
            && let Some(pmt) = self.programs.get(&service).copied()
        {
            let stream = u16::from_be_bytes([section[3], section[4]]);
            let version = section[5];
            let mut entries = Vec::new();
            if let Some(nit) = self.nit {
                entries.extend_from_slice(&0u16.to_be_bytes());
                entries.extend_from_slice(&(0xe000 | nit).to_be_bytes());
            }
            entries.extend_from_slice(&service.to_be_bytes());
            entries.extend_from_slice(&(0xe000 | pmt).to_be_bytes());
            let rebuilt = table(0x00, stream, version, &entries);
            self.emit(0, &rebuilt, out);
        }
    }

    fn pmt(&mut self, pid: u16, section: &[u8], out: &mut Vec<u8>) {
        if section.first() != Some(&0x02) || !mpegts::sections::check(section) {
            return;
        }
        let Some(number) = section.get(3..5).map(|b| u16::from_be_bytes([b[0], b[1]])) else {
            return;
        };
        let Some(info_length) = section
            .get(10..12)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]) & 0x0fff))
        else {
            return;
        };
        let pcr = (u16::from(section[8] & 0x1f) << 8) | u16::from(section[9]);
        let loop_start = 12 + info_length;
        let end = section.len() - 4;
        let mut map = Map {
            pcr: (pcr != 0x1fff).then_some(pcr),
            streams: Vec::new(),
        };
        let mut kept = Vec::new();
        let mut at = loop_start;
        while at + 5 <= end {
            let kind = section[at];
            let es = (u16::from(section[at + 1] & 0x1f) << 8) | u16::from(section[at + 2]);
            let length =
                usize::from(u16::from_be_bytes([section[at + 3], section[at + 4]]) & 0x0fff);
            let next = (at + 5 + length).min(end);
            let part = part_of(kind, &section[at + 5..next]);
            map.streams.push((es, part));
            let wanted = match part {
                Part::Caption => self.options.caption,
                Part::Carousel => self.options.carousel,
                Part::Other => true,
            };
            if wanted {
                kept.extend_from_slice(&section[at..next]);
            }
            at = next;
        }
        let removed = map.streams.iter().any(|(_, part)| match part {
            Part::Caption => !self.options.caption,
            Part::Carousel => !self.options.carousel,
            Part::Other => false,
        });
        self.maps.insert(pid, map);
        self.decide();
        let ours = self
            .options
            .service
            .is_none_or(|service| self.programs.get(&service) == Some(&pid));
        if removed && ours {
            self.rewritten.insert(pid);
            let version = section[5];
            let mut body = Vec::new();
            body.extend_from_slice(&section[8..loop_start]);
            body.extend_from_slice(&kept);
            let rebuilt = table(0x02, number, version, &body);
            self.emit(pid, &rebuilt, out);
        } else {
            self.rewritten.remove(&pid);
        }
    }

    fn decide(&mut self) {
        let mut keep: BTreeSet<u16> = BTreeSet::new();
        let mut drop: BTreeSet<u16> = BTreeSet::new();
        for (number, pmt) in &self.programs {
            let ours = self
                .options
                .service
                .is_none_or(|service| service == *number);
            let map = self.maps.get(pmt);
            let mut pids: Vec<(u16, bool)> = vec![(*pmt, true)];
            if let Some(map) = map {
                if let Some(pcr) = map.pcr {
                    pids.push((pcr, true));
                }
                for (es, part) in &map.streams {
                    let wanted = match part {
                        Part::Caption => self.options.caption,
                        Part::Carousel => self.options.carousel,
                        Part::Other => true,
                    };
                    pids.push((*es, wanted));
                }
            }
            for (pid, wanted) in pids {
                if ours && wanted {
                    keep.insert(pid);
                } else {
                    drop.insert(pid);
                }
            }
        }
        // 二つの番組が共有する PID は、残す番組があれば残す。
        self.drop = drop.difference(&keep).copied().collect();
    }

    fn emit(&mut self, pid: u16, section: &[u8], out: &mut Vec<u8>) {
        let counter = self.counters.entry(pid).or_insert(0);
        let mut payload = Vec::with_capacity(section.len() + 1);
        payload.push(0);
        payload.extend_from_slice(section);
        for (nth, chunk) in payload.chunks(PACKET - 4).enumerate() {
            let mut packet = [0xffu8; PACKET];
            packet[0] = SYNC;
            packet[1] = (if nth == 0 { 0x40 } else { 0 }) | ((pid >> 8) as u8 & 0x1f);
            packet[2] = pid as u8;
            packet[3] = 0x10 | (*counter & 0x0f);
            *counter = counter.wrapping_add(1);
            packet[4..4 + chunk.len()].copy_from_slice(chunk);
            out.extend_from_slice(&packet);
        }
    }
}

/// 字幕・文字スーパーはコンポーネントタグ 0x30〜0x3F、カルーセルは 0x40〜0x7F か stream_type 0x0D。
fn part_of(kind: u8, descriptors: &[u8]) -> Part {
    let mut at = 0;
    while at + 2 <= descriptors.len() {
        let tag = descriptors[at];
        let length = usize::from(descriptors[at + 1]);
        if tag == 0x52 && length >= 1 && at + 2 < descriptors.len() {
            return match descriptors[at + 2] {
                0x30..=0x3f => Part::Caption,
                0x40..=0x7f => Part::Carousel,
                _ => Part::Other,
            };
        }
        at += 2 + length;
    }
    if kind == 0x0d {
        Part::Carousel
    } else {
        Part::Other
    }
}

fn table(id: u8, extension: u16, version: u8, body: &[u8]) -> Vec<u8> {
    let length = 5 + body.len() + 4;
    let mut section = Vec::with_capacity(3 + length);
    section.push(id);
    section.push(0xb0 | ((length >> 8) as u8 & 0x0f));
    section.push(length as u8);
    section.extend_from_slice(&extension.to_be_bytes());
    section.push(version);
    section.push(0);
    section.push(0);
    section.extend_from_slice(body);
    let check = crc32(&section);
    section.extend_from_slice(&check.to_be_bytes());
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packets(pid: u16, section: &[u8], counter: &mut u8) -> Vec<u8> {
        let mut filter = Filter::new(Options {
            service: None,
            caption: true,
            carousel: true,
            eit: true,
        });
        filter.counters.insert(pid, *counter);
        let mut out = Vec::new();
        filter.emit(pid, section, &mut out);
        *counter = filter.counters[&pid];
        out
    }

    fn pat() -> Vec<u8> {
        let mut body = Vec::new();
        for (number, pid) in [(0u16, 0x10u16), (101, 0x1f0), (102, 0x1f8)] {
            body.extend_from_slice(&number.to_be_bytes());
            body.extend_from_slice(&(0xe000 | pid).to_be_bytes());
        }
        table(0x00, 0x4321, 0xc1, &body)
    }

    fn pmt(number: u16, base: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(0xe000 | base).to_be_bytes());
        body.extend_from_slice(&0xf000u16.to_be_bytes());
        for (kind, pid, tag) in [
            (0x02u8, base, 0x00u8),
            (0x0f, base + 0x10, 0x10),
            (0x06, base + 0x30, 0x30),
            (0x0d, base + 0x40, 0x40),
        ] {
            body.push(kind);
            body.extend_from_slice(&(0xe000 | pid).to_be_bytes());
            body.extend_from_slice(&(0xf000u16 | 3).to_be_bytes());
            body.extend_from_slice(&[0x52, 1, tag]);
        }
        table(0x02, number, 0xc1, &body)
    }

    fn payload(pid: u16) -> Vec<u8> {
        let mut packet = vec![0u8; PACKET];
        packet[0] = SYNC;
        packet[1] = (pid >> 8) as u8;
        packet[2] = pid as u8;
        packet[3] = 0x10;
        packet
    }

    fn pids(out: &[u8]) -> Vec<u16> {
        out.chunks(PACKET)
            .map(|p| (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]))
            .collect()
    }

    fn stream() -> Vec<u8> {
        let mut counter = 0;
        let mut input = packets(0, &pat(), &mut counter);
        input.extend(packets(0x1f0, &pmt(101, 0x100), &mut 0));
        input.extend(packets(0x1f8, &pmt(102, 0x200), &mut 0));
        for pid in [0x100, 0x110, 0x130, 0x140, 0x200, 0x210, 0x12, 0x11] {
            input.extend(payload(pid));
        }
        input
    }

    #[test]
    fn rebuilt_tables_carry_a_good_check() {
        assert!(mpegts::sections::check(&pat()));
        assert!(mpegts::sections::check(&pmt(101, 0x100)));
    }

    #[test]
    fn only_the_watched_service_is_kept() {
        let mut filter = Filter::new(Options {
            service: Some(101),
            caption: true,
            carousel: true,
            eit: true,
        });
        let mut out = Vec::new();
        filter.push(&stream(), &mut out);
        let seen = pids(&out);
        assert!(seen.contains(&0x100) && seen.contains(&0x110));
        assert!(seen.contains(&0x130) && seen.contains(&0x140));
        assert!(!seen.contains(&0x1f8) && !seen.contains(&0x200) && !seen.contains(&0x210));
        assert!(seen.contains(&0x12) && seen.contains(&0x11));
        let pat_packet = out
            .chunks(PACKET)
            .find(|p| p[1] & 0x1f == 0 && p[2] == 0)
            .unwrap();
        let section = &pat_packet[5..5 + 3 + usize::from(pat_packet[7])];
        assert!(mpegts::sections::check(section));
        assert_eq!(section[1] & 0x0f, 0);
        assert_eq!(usize::from(section[2]), 5 + 8 + 4);
    }

    #[test]
    fn captions_carousel_and_eit_can_be_left_out() {
        let mut filter = Filter::new(Options {
            service: None,
            caption: false,
            carousel: false,
            eit: false,
        });
        let mut out = Vec::new();
        filter.push(&stream(), &mut out);
        let seen = pids(&out);
        for gone in [0x130, 0x140, 0x230, 0x240, 0x12] {
            assert!(!seen.contains(&gone), "{gone:#x}");
        }
        for kept in [0x100, 0x110, 0x200, 0x210, 0x11] {
            assert!(seen.contains(&kept), "{kept:#x}");
        }
        let pmt_packet = out
            .chunks(PACKET)
            .find(|p| (u16::from(p[1] & 0x1f) << 8 | u16::from(p[2])) == 0x1f0)
            .unwrap();
        let length = usize::from(u16::from_be_bytes([pmt_packet[6], pmt_packet[7]]) & 0x0fff);
        let section = &pmt_packet[5..5 + 3 + length];
        assert!(mpegts::sections::check(section));
        assert_eq!(length, 9 + 2 * 8 + 4);
    }

    #[test]
    fn nothing_is_touched_when_everything_is_wanted() {
        let mut filter = Filter::new(Options {
            service: None,
            caption: true,
            carousel: true,
            eit: true,
        });
        let input = stream();
        let mut out = Vec::new();
        filter.push(&input, &mut out);
        assert_eq!(out, input);
    }
}
