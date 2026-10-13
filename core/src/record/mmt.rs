// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! MMT/TLV の録画を必要なサービスだけに絞る。テーブルを書き直すのは、IP ヘッダーを圧縮したパケットで
//! メッセージが分割も集約もされていないときだけ。

use std::collections::{BTreeMap, BTreeSet};

use super::{Options, Part};

const PIECES_MOST: usize = 4 << 20;

const PIECES_TOTAL: usize = 16 << 20;

const PIECES_IDS: usize = 256;

const SYNC: u8 = 0x7f;
const IPV4: u8 = 0x01;
const IPV6: u8 = 0x02;
const COMPRESSED: u8 = 0x03;
const CONTROL: u8 = 0xfe;
const NULL: u8 = 0xff;

const SIGNALLING: u8 = 0x02;
const PLT_PID: u16 = 0x0000;
const EIT_PID: u16 = 0x8000;
const MESSAGE_PA: u16 = 0x0000;
const MESSAGE_DATA: u16 = 0x8003;
const TABLE_PLT: u8 = 0x80;
const TABLE_MPT_FIRST: u8 = 0x11;
const TABLE_MPT_LAST: u8 = 0x20;

#[derive(Debug)]
pub(super) struct Filter {
    options: Options,
    held: Vec<u8>,
    plt_seen: bool,
    packages: BTreeMap<u16, u16>,
    maps: BTreeMap<u16, Vec<(u16, Part)>>,
    data: BTreeSet<u16>,
    drop: BTreeSet<u16>,
    pieces: BTreeMap<u16, (u8, Vec<u8>)>,
}

impl Filter {
    pub(super) fn new(options: Options) -> Self {
        Self {
            options,
            held: Vec::new(),
            plt_seen: false,
            packages: BTreeMap::new(),
            maps: BTreeMap::new(),
            data: BTreeSet::new(),
            drop: BTreeSet::new(),
            pieces: BTreeMap::new(),
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
        let mut held = std::mem::take(&mut self.held);
        held.extend_from_slice(bytes);
        let mut at = 0;
        while at + 4 <= held.len() {
            if !starts_packet(&held, at) {
                at += 1;
                continue;
            }
            let length = usize::from(u16::from_be_bytes([held[at + 2], held[at + 3]]));
            let end = at + 4 + length;
            if end > held.len() {
                break;
            }
            self.packet(&held[at..end], out);
            at = end;
        }
        held.drain(..at);
        self.held = held;
    }

    fn packet(&mut self, packet: &[u8], out: &mut Vec<u8>) {
        let payload = &packet[4..];
        let (mmtp_at, rewritable) = match packet[1] {
            COMPRESSED if payload.len() >= 3 => match payload[2] {
                0x21 | 0x61 => (3, true),
                0x20 => (23, false),
                0x60 => (45, false),
                _ => {
                    out.extend_from_slice(packet);
                    return;
                }
            },
            IPV4 => match udp_body(payload, false) {
                Some(at) => (at, false),
                None => {
                    out.extend_from_slice(packet);
                    return;
                }
            },
            IPV6 => match udp_body(payload, true) {
                Some(at) => (at, false),
                None => {
                    out.extend_from_slice(packet);
                    return;
                }
            },
            _ => {
                out.extend_from_slice(packet);
                return;
            }
        };
        let Some(mmtp) = payload.get(mmtp_at..) else {
            out.extend_from_slice(packet);
            return;
        };
        let Some(read) = mmttlv::mmtp::read(mmtp) else {
            out.extend_from_slice(packet);
            return;
        };
        let pid = read.packet_id;
        let signalling = read.payload_type == SIGNALLING;
        let body_at = mmtp.len() - read.payload.len();
        if signalling {
            self.observe(pid, read.payload);
        }
        if !self.options.eit && pid == EIT_PID {
            return;
        }
        if self.drop.contains(&pid) {
            return;
        }
        if !signalling && !self.ready() {
            return;
        }
        if signalling
            && rewritable
            && let Some(rewritten) = self.rewrite(pid, read.payload)
        {
            let mut new = Vec::with_capacity(packet.len());
            new.extend_from_slice(&packet[..4]);
            new.extend_from_slice(&payload[..mmtp_at]);
            new.extend_from_slice(&mmtp[..body_at]);
            new.extend_from_slice(&rewritten);
            let length = (new.len() - 4) as u16;
            new[2..4].copy_from_slice(&length.to_be_bytes());
            out.extend_from_slice(&new);
            return;
        }
        out.extend_from_slice(packet);
    }

    fn pieces_len(&self) -> usize {
        self.pieces.values().map(|(_, held)| held.len()).sum()
    }

    /// MPT などは分割されて届くこともあるので、組み立ててから読む。
    fn observe(&mut self, pid: u16, payload: &[u8]) {
        // 大きなカルーセルを組み立て直さない。
        if self.data.contains(&pid) {
            return;
        }
        let Some(&head) = payload.first() else {
            return;
        };
        let body = payload.get(2..).unwrap_or_default();
        let assembled;
        let whole: &[u8] = match head >> 6 {
            0 => {
                self.pieces.remove(&pid);
                payload
            }
            1 => {
                self.pieces.remove(&pid);
                if self.pieces.len() >= PIECES_IDS || self.pieces_len() + body.len() > PIECES_TOTAL
                {
                    return;
                }
                self.pieces.insert(pid, (head & 0x3f, body.to_vec()));
                return;
            }
            fragment => {
                let Some(held_len) = self.pieces.get(&pid).map(|(_, held)| held.len()) else {
                    return;
                };
                // 壊れたストリームで中間の断片だけが続いてもバッファを伸ばし続けない。
                if held_len + body.len() > PIECES_MOST
                    || self.pieces_len() + body.len() > PIECES_TOTAL
                {
                    self.pieces.remove(&pid);
                    return;
                }
                let Some((flags, held)) = self.pieces.get_mut(&pid) else {
                    return;
                };
                held.extend_from_slice(body);
                if fragment == 2 {
                    return;
                }
                let (flags, held) = (*flags, std::mem::take(held));
                self.pieces.remove(&pid);
                let mut whole = vec![flags, 0];
                whole.extend(held);
                assembled = whole;
                &assembled
            }
        };
        let mut changed = false;
        for (id, message) in messages(whole) {
            match id {
                MESSAGE_PA => {
                    for (table_id, table) in isdb::mh::package_message_tables(message) {
                        if table_id == TABLE_PLT && pid == PLT_PID {
                            changed |= self.plt(table);
                        } else if (TABLE_MPT_FIRST..=TABLE_MPT_LAST).contains(&table_id) {
                            changed |= self.mpt(pid, table);
                        }
                    }
                }
                MESSAGE_DATA => {
                    changed |= self.data.insert(pid);
                }
                _ => {}
            }
        }
        if changed {
            self.decide();
        }
    }

    fn plt(&mut self, table: &[u8]) -> bool {
        let Some(read) = isdb::mmttables::package_list(table) else {
            return false;
        };
        let packages: BTreeMap<u16, u16> = read
            .packages
            .iter()
            .filter_map(|package| {
                let service = service_of(&package.package_id)?;
                match package.location {
                    isdb::mmttables::PackageLocation::Here { packet_id } => {
                        Some((service, packet_id))
                    }
                    _ => None,
                }
            })
            .collect();
        if self.plt_seen && self.packages == packages {
            return false;
        }
        self.plt_seen = true;
        self.packages = packages;
        true
    }

    fn mpt(&mut self, pid: u16, table: &[u8]) -> bool {
        let Some(read) = isdb::mmttables::package_table(table) else {
            return false;
        };
        let assets: Vec<(u16, Part)> = read
            .assets
            .iter()
            .filter_map(|asset| Some((asset.packet_id()?, part_of(&asset.kind))))
            .collect();
        if self.maps.get(&pid) == Some(&assets) {
            return false;
        }
        self.maps.insert(pid, assets);
        true
    }

    fn ready(&self) -> bool {
        if !self.plt_seen {
            return false;
        }
        match self.options.service {
            Some(service) => self
                .packages
                .get(&service)
                .is_some_and(|mpt| self.maps.contains_key(mpt)),
            None => self
                .packages
                .values()
                .all(|mpt| self.maps.contains_key(mpt)),
        }
    }

    fn wanted(&self, part: Part) -> bool {
        match part {
            Part::Caption => self.options.caption,
            Part::Carousel => self.options.carousel,
            Part::Other => true,
        }
    }

    fn decide(&mut self) {
        let mut keep = BTreeSet::new();
        let mut drop = BTreeSet::new();
        for (service, mpt) in &self.packages {
            let ours = self.options.service.is_none_or(|wanted| wanted == *service);
            let mut pids = vec![(*mpt, true)];
            if let Some(assets) = self.maps.get(mpt) {
                pids.extend(assets.iter().map(|(pid, part)| (*pid, self.wanted(*part))));
            }
            for (pid, wanted) in pids {
                if ours && wanted {
                    keep.insert(pid);
                } else {
                    drop.insert(pid);
                }
            }
        }
        if !self.options.carousel {
            drop.extend(self.data.iter().copied());
        }
        // PLT は落とさず書き直す。二つのサービスが共有する識別子は残す。
        drop.remove(&PLT_PID);
        self.drop = drop.difference(&keep).copied().collect();
    }

    fn rewrite(&self, pid: u16, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 2 || payload[0] >> 6 != 0 || payload[0] & 1 != 0 {
            return None;
        }
        let message = &payload[2..];
        if message.len() < 8 || u16::from_be_bytes([message[0], message[1]]) != MESSAGE_PA {
            return None;
        }
        let length = u32::from_be_bytes([message[3], message[4], message[5], message[6]]) as usize;
        let extension = message.get(7..7 + length)?;
        let count = usize::from(*extension.first()?);
        let head = extension.get(..1 + 4 * count)?;
        let mut tables = Vec::new();
        let mut at = 1 + 4 * count;
        let mut changed = false;
        while at + 4 <= extension.len() {
            let table_length =
                usize::from(u16::from_be_bytes([extension[at + 2], extension[at + 3]]));
            let table = extension.get(at..at + 4 + table_length)?;
            let rebuilt = match table[0] {
                TABLE_PLT if pid == PLT_PID => self
                    .options
                    .service
                    .and_then(|service| plt_only(table, service)),
                id if (TABLE_MPT_FIRST..=TABLE_MPT_LAST).contains(&id) => self.mpt_without(table),
                _ => None,
            };
            match rebuilt {
                Some(rebuilt) => {
                    changed = true;
                    tables.push(rebuilt);
                }
                None => tables.push(table.to_vec()),
            }
            at += 4 + table_length;
        }
        if !changed {
            return None;
        }
        let mut new_extension = head.to_vec();
        for (nth, table) in tables.iter().enumerate().take(count) {
            let entry = 1 + 4 * nth;
            let length = (table.len() - 4) as u16;
            new_extension[entry + 2..entry + 4].copy_from_slice(&length.to_be_bytes());
        }
        for table in &tables {
            new_extension.extend_from_slice(table);
        }
        new_extension.extend_from_slice(extension.get(at..)?);
        let mut out = Vec::with_capacity(2 + 7 + new_extension.len());
        out.extend_from_slice(&payload[..2]);
        out.extend_from_slice(&message[..3]);
        out.extend_from_slice(&(new_extension.len() as u32).to_be_bytes());
        out.extend_from_slice(&new_extension);
        Some(out)
    }

    fn mpt_without(&self, table: &[u8]) -> Option<Vec<u8>> {
        let length = usize::from(u16::from_be_bytes([*table.get(2)?, *table.get(3)?]));
        let body = table.get(4..4 + length)?;
        let identifier = usize::from(*body.get(1)?);
        let mut at = 2 + identifier;
        let descriptors = usize::from(u16::from_be_bytes([*body.get(at)?, *body.get(at + 1)?]));
        at += 2 + descriptors;
        let count = usize::from(*body.get(at)?);
        let head = body.get(..at)?;
        at += 1;
        let mut kept = Vec::new();
        let mut kept_count = 0u8;
        let mut removed = false;
        for _ in 0..count {
            let size = asset_size(body.get(at..)?)?;
            let entry = &body[at..at + size];
            let part = asset_kind(entry).map_or(Part::Other, |kind| part_of(&kind));
            if self.wanted(part) {
                kept.extend_from_slice(entry);
                kept_count += 1;
            } else {
                removed = true;
            }
            at += size;
        }
        if !removed {
            return None;
        }
        let mut new_body = head.to_vec();
        new_body.push(kept_count);
        new_body.extend_from_slice(&kept);
        new_body.extend_from_slice(body.get(at..)?);
        let mut out = vec![table[0], table[1]];
        out.extend_from_slice(&(new_body.len() as u16).to_be_bytes());
        out.extend_from_slice(&new_body);
        Some(out)
    }
}

fn starts_packet(bytes: &[u8], at: usize) -> bool {
    if bytes[at] != SYNC || !matches!(bytes[at + 1], IPV4 | IPV6 | COMPRESSED | CONTROL | NULL) {
        return false;
    }
    let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
    let next = at + 4 + length;
    next >= bytes.len() || bytes[next] == SYNC
}

/// NTP（123 番）なら `None`（通す）。
fn udp_body(payload: &[u8], v6: bool) -> Option<usize> {
    let header = if v6 {
        40
    } else {
        usize::from(*payload.first()? & 0x0f) * 4
    };
    let port = u16::from_be_bytes([*payload.get(header + 2)?, *payload.get(header + 3)?]);
    (port != 123).then_some(header + 8)
}

/// 分割されたメッセージは読まない（分割された PA は書き直さないので）。
fn messages(payload: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    if payload.len() < 2 || payload[0] >> 6 != 0 {
        return out;
    }
    let aggregated = payload[0] & 1 != 0;
    let long_lengths = payload[0] & 2 != 0;
    let mut body = &payload[2..];
    if !aggregated {
        if let Some(found) = message(body) {
            out.push(found);
        }
        return out;
    }
    let width = if long_lengths { 4 } else { 2 };
    while body.len() >= width {
        let length = if width == 4 {
            u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize
        } else {
            usize::from(u16::from_be_bytes([body[0], body[1]]))
        };
        body = &body[width..];
        if length > body.len() {
            break;
        }
        if let Some(found) = message(&body[..length]) {
            out.push(found);
        }
        body = &body[length..];
    }
    out
}

fn message(buffer: &[u8]) -> Option<(u16, &[u8])> {
    let id = u16::from_be_bytes([*buffer.first()?, *buffer.get(1)?]);
    let long = id == 0 || id == 0x8003 || id >= 0xf000 || (0x7000..=0x7fff).contains(&id);
    let header = if long { 7 } else { 5 };
    let length = if long {
        u32::from_be_bytes([
            *buffer.get(3)?,
            *buffer.get(4)?,
            *buffer.get(5)?,
            *buffer.get(6)?,
        ]) as usize
    } else {
        usize::from(u16::from_be_bytes([*buffer.get(3)?, *buffer.get(4)?]))
    };
    Some((id, buffer.get(header..header + length)?))
}

fn service_of(package_id: &[u8]) -> Option<u16> {
    match package_id {
        [high, low] => Some(u16::from_be_bytes([*high, *low])),
        _ => None,
    }
}

fn plt_only(table: &[u8], service: u16) -> Option<Vec<u8>> {
    let length = usize::from(u16::from_be_bytes([*table.get(2)?, *table.get(3)?]));
    let body = table.get(4..4 + length)?;
    let count = usize::from(*body.first()?);
    let mut at = 1;
    let mut kept = None;
    for _ in 0..count {
        let identifier = usize::from(*body.get(at)?);
        let id = body.get(at + 1..at + 1 + identifier)?;
        let (_, used) = isdb::mmttables::general_location(body.get(at + 1 + identifier..)?)?;
        let end = at + 1 + identifier + used;
        if service_of(id) == Some(service) {
            kept = Some(body.get(at..end)?.to_vec());
        }
        at = end;
    }
    let kept = kept?;
    let mut new_body = vec![1u8];
    new_body.extend_from_slice(&kept);
    new_body.extend_from_slice(body.get(at..)?);
    let mut out = vec![table[0], table[1]];
    out.extend_from_slice(&(new_body.len() as u16).to_be_bytes());
    out.extend_from_slice(&new_body);
    Some(out)
}

fn asset_size(buffer: &[u8]) -> Option<usize> {
    let identifier = usize::from(*buffer.get(5)?);
    let mut at = 6 + identifier + 4;
    let clock = *buffer.get(at)? & 0x01 != 0;
    at += 1;
    if clock {
        let extended = *buffer.get(at + 1)? & 0x01 != 0;
        at += 2;
        if extended {
            at += 4;
        }
    }
    let locations = usize::from(*buffer.get(at)?);
    at += 1;
    for _ in 0..locations {
        at += match *buffer.get(at)? {
            0x00 => 3,
            0x01 => 13,
            0x02 | 0x04 => 37,
            0x03 => 7,
            0x05 => 2 + usize::from(*buffer.get(at + 1)?),
            _ => return None,
        };
    }
    let descriptors = usize::from(u16::from_be_bytes([*buffer.get(at)?, *buffer.get(at + 1)?]));
    at += 2 + descriptors;
    (at <= buffer.len()).then_some(at)
}

fn asset_kind(entry: &[u8]) -> Option<String> {
    let identifier = usize::from(*entry.get(5)?);
    let at = 6 + identifier;
    Some(String::from_utf8_lossy(entry.get(at..at + 4)?).into_owned())
}

fn part_of(kind: &str) -> Part {
    match kind {
        "stpp" => Part::Caption,
        "aapp" => Part::Carousel,
        _ => Part::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(kind: &[u8; 4], pid: u16) -> Vec<u8> {
        let mut out = vec![0x00, 0, 0, 0, 0, 2, 0x00, 0x01];
        out.extend_from_slice(kind);
        out.push(0x00); // 時計との関係なし
        out.push(1); // ロケーションが一つ
        out.push(0x00);
        out.extend_from_slice(&pid.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // 記述子なし
        out
    }

    fn mpt(service: u16, assets: &[(&[u8; 4], u16)]) -> Vec<u8> {
        let mut body = vec![0xfc, 2];
        body.extend_from_slice(&service.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.push(assets.len() as u8);
        for (kind, pid) in assets {
            body.extend(asset(kind, *pid));
        }
        let mut table = vec![TABLE_MPT_LAST, 0];
        table.extend_from_slice(&(body.len() as u16).to_be_bytes());
        table.extend(body);
        table
    }

    fn plt(packages: &[(u16, u16)]) -> Vec<u8> {
        let mut body = vec![packages.len() as u8];
        for (service, pid) in packages {
            body.push(2);
            body.extend_from_slice(&service.to_be_bytes());
            body.push(0x00);
            body.extend_from_slice(&pid.to_be_bytes());
        }
        body.push(0); // IP の配信の数
        let mut table = vec![TABLE_PLT, 0];
        table.extend_from_slice(&(body.len() as u16).to_be_bytes());
        table.extend(body);
        table
    }

    fn pa(pid: u16, tables: &[Vec<u8>]) -> Vec<u8> {
        let mut extension = vec![0u8];
        for table in tables {
            extension.extend_from_slice(table);
        }
        let mut message = vec![0x00, 0x00, 0x00];
        message.extend_from_slice(&(extension.len() as u32).to_be_bytes());
        message.extend(extension);
        let mut signalling = vec![0x00, 0x00];
        signalling.extend(message);
        packet(pid, SIGNALLING, &signalling)
    }

    fn packet(pid: u16, kind: u8, body: &[u8]) -> Vec<u8> {
        let mut mmtp = vec![0x40, kind];
        mmtp.extend_from_slice(&pid.to_be_bytes());
        mmtp.extend_from_slice(&[0; 8]);
        mmtp.extend_from_slice(body);
        let mut payload = vec![0x00, 0x10, 0x61];
        payload.extend(mmtp);
        let mut out = vec![SYNC, COMPRESSED];
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend(payload);
        out
    }

    fn pids(stream: &[u8]) -> Vec<u16> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 4 <= stream.len() {
            let length = usize::from(u16::from_be_bytes([stream[at + 2], stream[at + 3]]));
            let payload = &stream[at + 4..at + 4 + length];
            out.push(u16::from_be_bytes([payload[5], payload[6]]));
            at += 4 + length;
        }
        out
    }

    fn stream() -> Vec<u8> {
        let mut input = pa(PLT_PID, &[plt(&[(101, 0xff01), (102, 0xff02)])]);
        input.extend(pa(
            0xff01,
            &[mpt(
                101,
                &[
                    (b"hev1", 0xf100),
                    (b"mp4a", 0xf110),
                    (b"stpp", 0xf130),
                    (b"aapp", 0xf140),
                ],
            )],
        ));
        input.extend(pa(
            0xff02,
            &[mpt(102, &[(b"hev1", 0xf200), (b"mp4a", 0xf210)])],
        ));
        for pid in [0xf100, 0xf110, 0xf130, 0xf140, 0xf200, 0xf210, EIT_PID] {
            input.extend(packet(pid, 0x00, &[0; 16]));
        }
        input
    }

    #[test]
    fn a_fragmented_mpt_is_put_together() {
        let mut filter = Filter::new(Options {
            service: Some(101),
            caption: true,
            carousel: true,
            eit: true,
        });
        let mut input = pa(PLT_PID, &[plt(&[(101, 0xff01)])]);
        let whole = pa(0xff01, &[mpt(101, &[(b"hev1", 0xf100), (b"mp4a", 0xf110)])]);
        let message = &whole[4 + 3 + 12 + 2..];
        let third = message.len() / 3;
        for (fragment, part) in [
            (1u8, &message[..third]),
            (2, &message[third..2 * third]),
            (3, &message[2 * third..]),
        ] {
            let mut signalling = vec![fragment << 6, 0];
            signalling.extend_from_slice(part);
            input.extend(packet(0xff01, SIGNALLING, &signalling));
        }
        for pid in [0xf100, 0xf110] {
            input.extend(packet(pid, 0x00, &[0; 16]));
        }
        let mut out = Vec::new();
        filter.push(&input, &mut out);
        let kept = pids(&out);
        assert!(
            kept.contains(&0xf100) && kept.contains(&0xf110),
            "{kept:x?}"
        );
    }

    #[test]
    fn only_the_watched_package_is_kept() {
        let mut filter = Filter::new(Options {
            service: Some(101),
            caption: true,
            carousel: true,
            eit: true,
        });
        let mut out = Vec::new();
        filter.push(&stream(), &mut out);
        let seen = pids(&out);
        for kept in [PLT_PID, 0xff01, 0xf100, 0xf110, 0xf130, 0xf140, EIT_PID] {
            assert!(seen.contains(&kept), "{kept:#x}");
        }
        for gone in [0xff02, 0xf200, 0xf210] {
            assert!(!seen.contains(&gone), "{gone:#x}");
        }
        let mut check = Filter::new(Options {
            service: None,
            caption: false,
            carousel: true,
            eit: true,
        });
        check.observe(
            PLT_PID,
            &out[4 + 3 + 12..4 + usize::from(u16::from_be_bytes([out[2], out[3]]))],
        );
        assert_eq!(check.packages.len(), 1);
        assert_eq!(check.packages.get(&101), Some(&0xff01));
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
        for gone in [0xf130, 0xf140, EIT_PID] {
            assert!(!seen.contains(&gone), "{gone:#x}");
        }
        for kept in [0xf100, 0xf110, 0xf200, 0xf210, 0xff01, 0xff02] {
            assert!(seen.contains(&kept), "{kept:#x}");
        }
        let mut check = Filter::new(filter.options);
        let mut at = 0;
        while at < out.len() {
            let length = usize::from(u16::from_be_bytes([out[at + 2], out[at + 3]]));
            let packet = &out[at..at + 4 + length];
            let pid = u16::from_be_bytes([packet[9], packet[10]]);
            if pid == 0xff01 {
                check.observe(pid, &packet[4 + 3 + 12..]);
            }
            at += 4 + length;
        }
        assert_eq!(check.maps.get(&0xff01).map(Vec::len), Some(2));
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

    /// `ZW_MMTS` にパスを設定したときだけ実行する。
    #[test]
    fn a_real_recording_stays_well_formed() {
        let Ok(path) = std::env::var("ZW_MMTS") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let bytes = &bytes[..bytes.len().min(64 << 20)];
        let mut filter = Filter::new(Options {
            service: None,
            caption: false,
            carousel: false,
            eit: false,
        });
        let mut out = Vec::new();
        for chunk in bytes.chunks(48 * 1024) {
            filter.push(chunk, &mut out);
        }
        let mut at = 0;
        let mut count = 0;
        while at + 4 <= out.len() {
            assert_eq!(out[at], SYNC, "{at}");
            at += 4 + usize::from(u16::from_be_bytes([out[at + 2], out[at + 3]]));
            count += 1;
        }
        tracing::info!(
            "{} → {} バイト、{count} パケット、MPT {:?}",
            bytes.len(),
            out.len(),
            filter.maps
        );
        assert!(count > 0 && out.len() < bytes.len());
        let mut again = Filter::new(Options {
            service: Some(u16::MAX),
            caption: true,
            carousel: true,
            eit: true,
        });
        let mut ignored = Vec::new();
        again.push(&out, &mut ignored);
        tracing::info!("読み直した MPT {:?}", again.maps);
        assert!(
            again
                .maps
                .values()
                .all(|assets| assets.iter().all(|(_, part)| *part == Part::Other))
        );
    }
}
