// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 放送の SI テーブルを読むフィルター。再生には要らない。読み分けはセクションの [`Origin`] で
//! （`table_id` は二つの伝送で別のものを指す）。

use isdb::descriptors::{self as desc, DATA_COMPONENT_CAPTION, mh_tag, tag};
use isdb::{mmttables, nit, tstables};

use super::packet::{Origin, Section};
use crate::guide::{Detailed, Genre, Guide, Key, Picture, Programme, Service, Told, epoch};
use crate::si::{BroadcasterInfo, Identity, Info, LogoImage, ServiceInfo};

pub const EIT_PID: u16 = 0x0012;
/// ワンセグの M-EIT と L-EIT（ARIB TR-B14）。区分の切り方が H-EIT と違う。
pub const EIT_MEDIUM_PID: u16 = 0x0026;
pub const EIT_LOW_PID: u16 = 0x0027;
pub const SDT_PID: u16 = 0x0011;
pub const NIT_PID: u16 = 0x0010;
pub const TOT_PID: u16 = 0x0014;
pub const BIT_PID: u16 = 0x0024;
pub const CAT_PID: u16 = 0x0001;
pub const SDTT_PID: u16 = 0x0023;
pub const SDTT_ALT_PID: u16 = 0x0028;
pub const CDT_PID: u16 = 0x0029;
/// 部分 TS（他の受信機の録画）が SDT/EIT の代わりに持つもの。
pub const SIT_PID: u16 = 0x001f;
pub const DIT_PID: u16 = 0x001e;
pub const TABLE_EIT_OWN: u8 = 0x4e;
const TABLE_TLV_NIT_OWN: u8 = 0x40;
const TABLE_TLV_NIT_OTHER: u8 = 0x41;

pub const fn is_event_table(pid: u16) -> bool {
    matches!(pid, EIT_PID | EIT_MEDIUM_PID | EIT_LOW_PID)
}

/// 落ちれば見ている局にも効くので受信の質に数える。
pub const SHARED_PIDS: [u16; 8] = [
    0x0000,
    0x0001,
    NIT_PID,
    SDT_PID,
    EIT_PID,
    TOT_PID,
    EIT_MEDIUM_PID,
    EIT_LOW_PID,
];

#[derive(Debug)]
pub struct Tables<'a> {
    pub guide: &'a mut Guide,
    pub si: &'a mut Info,
}

impl Tables<'_> {
    pub fn feed(&mut self, section: &Section<'_>) {
        match section.origin {
            Origin::Ts => self.feed_ts(section.pid, section.data),
            Origin::MmtSi => self.feed_mmt(section.pid, section.data),
            Origin::TlvSi => self.feed_tlv(section.data),
        }
    }

    pub fn mmt_table(&mut self, table: &[u8]) {
        match table.first() {
            Some(&mmttables::TABLE_PLT) => {
                if let Some(read) = mmttables::package_list(table) {
                    self.si.set_packages(read.packages);
                }
            }
            Some(&mmttables::TABLE_LCT) => {
                if let Some(layouts) = mmttables::layout_configuration(table) {
                    self.si.set_layouts(layouts);
                }
            }
            Some(&mmttables::TABLE_MH_CAT) => {
                if let Some(read) = isdb::datatables::conditional_access(table) {
                    let systems = read
                        .descriptors
                        .iter()
                        .filter(|found| found.tag == mh_tag::ACCESS_CONTROL)
                        .filter_map(|found| desc::mh_access_control(&found.body))
                        .map(|(system, location)| {
                            let pid = match location {
                                mmttables::PackageLocation::Here { packet_id } => packet_id,
                                _ => 0xffff,
                            };
                            (system, pid)
                        })
                        .collect();
                    self.si.on_conditional_access(systems);
                }
            }
            _ => {}
        }
    }

    fn feed_ts(&mut self, pid: u16, section: &[u8]) {
        match pid {
            EIT_PID => {
                if let Some(table) = tstables::event_table(section) {
                    self.guide.on_event_table(&table);
                }
            }
            // 番組は入れるが届き具合は数えない（区分の切り方が H-EIT と別）。
            EIT_MEDIUM_PID | EIT_LOW_PID => {
                if let Some(table) = tstables::event_table(section) {
                    self.guide.on_event_table_counting(&table, false);
                }
            }
            SDT_PID => {
                if let Some(table) = tstables::service_description(section) {
                    self.on_service_description(&table);
                }
            }
            TOT_PID => {
                if let Some(table) = tstables::time_table(section) {
                    self.guide.on_time_table(&table);
                }
            }
            NIT_PID => {
                if let Some(read) = nit::described(section) {
                    self.si.on_network_information(&read);
                    self.guide.on_network_information(&read);
                }
            }
            BIT_PID => {
                if let Some(table) = tstables::broadcaster_information(section) {
                    self.on_broadcaster_information(&table);
                }
            }
            CAT_PID => {
                if let Some(table) = tstables::conditional_access(section) {
                    self.si
                        .on_conditional_access(ca_systems(&table.descriptors));
                }
            }
            SDTT_PID | SDTT_ALT_PID => {
                if let Some(table) = tstables::software_download(section) {
                    self.si.on_software_download(table);
                }
            }
            CDT_PID => {
                if let Some(table) = tstables::common_data(section)
                    && table.data_type == 0x01
                {
                    self.on_logo(LogoImage {
                        network: table.original_network_id,
                        logo_type: table.logo_type,
                        logo_id: table.logo_id,
                        version: table.logo_version,
                        png: table.data,
                    });
                }
            }
            SIT_PID => {
                if let Some(table) = tstables::selection_information(section) {
                    self.on_selection_information(&table);
                }
            }
            DIT_PID if tstables::discontinuity_information(section).is_some() => {
                self.si.partial_mut().discontinuities += 1;
            }
            _ => {}
        }
    }

    fn feed_mmt(&mut self, packet_id: u16, section: &[u8]) {
        let Some(&table_id) = section.first() else {
            return;
        };
        match table_id {
            mmttables::TABLE_MH_AIT => {
                if let Some(table) = mmttables::application_information(section) {
                    self.si
                        .on_application_information(packet_id, table.applications.len());
                }
            }
            mmttables::TABLE_MH_EIT_PRESENT..=mmttables::TABLE_MH_EIT_SCHEDULE_LAST => {
                if let Some(table) = mmttables::event_table(section) {
                    self.guide.on_mh_event_table(&table);
                }
            }
            mmttables::TABLE_MH_SDT_OWN | mmttables::TABLE_MH_SDT_OTHER => {
                if let Some(table) = mmttables::service_description(section) {
                    self.on_mh_service_description(&table);
                }
            }
            mmttables::TABLE_MH_TOT => {
                if let Some(table) = mmttables::time_table(section) {
                    self.guide.tell_time(epoch(table.time));
                }
            }
            mmttables::TABLE_MH_BIT => {
                if let Some(table) = mmttables::broadcaster_information(section) {
                    self.on_mh_broadcaster_information(&table);
                }
            }
            mmttables::TABLE_MH_SDTT => {
                if let Some(table) = mmttables::software_download(section) {
                    self.si.on_software_download(table);
                }
            }
            mmttables::TABLE_MH_CDT => {
                if let Some(table) = mmttables::common_data(section)
                    && table.data_type == 0x01
                    && let Some(head) = table.data.get(..7)
                {
                    // STD-B60 6.7.1。一枚が複数のセクションに割れる（TR-B39 5.2.5）ので揃うまでバッファする。
                    let size = usize::from(u16::from_be_bytes([head[5], head[6]]));
                    let part = table.data.get(7..7 + size).unwrap_or(&[]);
                    let network = table.original_network_id;
                    let logo_type = head[0];
                    let logo_id = u16::from_be_bytes([head[1] & 0x01, head[2]]);
                    let version = u16::from_be_bytes([head[3] & 0x0f, head[4]]);
                    if let Some(png) = self.si.on_logo_part(
                        (network, logo_type, logo_id, version),
                        table.number,
                        part,
                    ) {
                        self.on_logo(LogoImage {
                            network,
                            logo_type,
                            logo_id,
                            version,
                            png,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// MMT/TLV のリモコン番号は TLV-NIT のリモコンキー記述子にしか無い（ARIB STD-B60 5.3.1.5）。
    fn feed_tlv(&mut self, section: &[u8]) {
        let Some(&table_id) = section.first() else {
            return;
        };
        if matches!(table_id, TABLE_TLV_NIT_OWN | TABLE_TLV_NIT_OTHER)
            && let Some(read) = nit::described_tlv(section)
        {
            self.si.on_network_information(&read);
            self.guide.on_network_information(&read);
        }
    }

    fn on_service_description(&mut self, table: &tstables::ServiceDescription) {
        if table.own() {
            self.si.set_identity(Identity {
                transport: table.transport_stream_id,
                network: table.original_network_id,
            });
        }
        for described in &table.services {
            let naming = tstables::find(&described.descriptors, tag::SERVICE)
                .and_then(|found| desc::service(&found.body));
            let mut service = ServiceInfo {
                running: described.running_status,
                eit_schedule: described.eit_schedule,
                eit_present_following: described.eit_present_following,
                ..ServiceInfo::default()
            };
            for found in &described.descriptors {
                match found.tag {
                    tag::LOGO_TRANSMISSION => {
                        service.logo = desc::logo_transmission(&found.body);
                    }
                    tag::CA_CONTRACT_INFO => {
                        service
                            .contracts
                            .extend(desc::ca_contract_info(&found.body));
                    }
                    _ => {}
                }
            }
            self.service_told(
                (
                    table.original_network_id,
                    table.transport_stream_id,
                    table.own(),
                ),
                described.service_id,
                described.scrambled,
                naming,
                service,
            );
        }
    }

    fn on_mh_service_description(&mut self, table: &mmttables::ServiceDescription) {
        if table.own() {
            self.si.set_identity(Identity {
                transport: table.tlv_stream_id,
                network: table.original_network_id,
            });
        }
        for described in &table.services {
            let naming = mmttables::find(&described.descriptors, mh_tag::SERVICE)
                .and_then(|found| desc::mh_service(&found.body));
            let mut service = ServiceInfo {
                running: described.running_status,
                eit_schedule: described.eit_schedule,
                eit_present_following: described.eit_present_following,
                ..ServiceInfo::default()
            };
            for found in &described.descriptors {
                match found.tag {
                    mh_tag::LOGO_TRANSMISSION => {
                        service.logo = desc::mh_logo_transmission(&found.body);
                    }
                    mh_tag::CA_CONTRACT_INFO => {
                        service
                            .contracts
                            .extend(desc::mh_ca_contract_info(&found.body));
                    }
                    _ => {}
                }
            }
            self.service_told(
                (table.original_network_id, table.tlv_stream_id, table.own()),
                described.service_id,
                described.scrambled,
                naming,
                service,
            );
        }
    }

    pub fn on_logo(&mut self, logo: LogoImage) {
        self.guide.tell_logo(
            logo.network,
            crate::guide::Logo {
                logo_type: logo.logo_type,
                logo_id: logo.logo_id,
                version: logo.version,
                png: logo.png.clone(),
            },
        );
        self.si.on_logo(logo);
    }

    /// 一つの項目が別の NID の局を並べることがある（BS のストリームに載る CS のロゴ）。
    pub fn on_logo_module(&mut self, module: &isdb::logo::LogoModule) {
        for entry in &module.entries {
            let mut networks: Vec<u16> = entry.services.iter().map(|s| s.0).collect();
            networks.sort_unstable();
            networks.dedup();
            for (network, _, service) in &entry.services {
                self.guide.tell_logo_id(
                    Key {
                        network: *network,
                        service: *service,
                    },
                    entry.logo_id,
                );
                self.si.tell_logo_id(*network, *service, entry.logo_id);
            }
            for network in networks {
                self.on_logo(LogoImage {
                    network,
                    logo_type: module.logo_type,
                    logo_id: entry.logo_id,
                    version: 0,
                    png: entry.png.clone(),
                });
            }
        }
    }

    fn service_told(
        &mut self,
        (network, transport, own): (u16, u16, bool),
        service: u16,
        scrambled: bool,
        naming: Option<desc::ServiceNaming>,
        rest: ServiceInfo,
    ) {
        let Some(naming) = naming.filter(|n| !n.name.is_empty() || n.kind != 0) else {
            return;
        };
        let logo_id = match rest.logo {
            Some(desc::Logo::Downloaded { logo_id, .. } | desc::Logo::Referenced { logo_id }) => {
                Some(logo_id)
            }
            _ => None,
        };
        let simple_logo = match &rest.logo {
            Some(desc::Logo::Text(text)) if !text.is_empty() => Some(text.clone()),
            _ => None,
        };
        self.guide.tell_service(Service {
            network,
            transport,
            transport_confirmed: own,
            service,
            scrambled,
            kind: naming.kind,
            provider: naming.provider.clone(),
            name: naming.name.clone(),
            logo_id,
            simple_logo,
            ..Service::default()
        });
        self.si.tell_service(ServiceInfo {
            network,
            transport,
            service,
            scrambled,
            kind: naming.kind,
            provider: naming.provider,
            name: naming.name,
            ..rest
        });
    }

    fn on_selection_information(&mut self, table: &tstables::SelectionInformation) {
        let partial = self.si.partial_mut();
        for found in &table.descriptors {
            match found.tag {
                tag::NETWORK_IDENTIFICATION => {
                    partial.network = desc::network_identification(&found.body);
                }
                tag::PARTIAL_TS_TIME => partial.time = desc::partial_ts_time(&found.body),
                tag::PARTIAL_TS => partial.rate = desc::partial_ts(&found.body),
                _ => {}
            }
        }
        let network = partial
            .network
            .as_ref()
            .map_or(0, |network| network.network_id);
        if self.si.identity().is_none() && network != 0 {
            self.si.set_identity(Identity {
                transport: 0,
                network,
            });
        }
        let time = self.si.partial().and_then(|p| p.time);
        let start = time.and_then(|t| t.event_start);
        let seconds = time.and_then(|t| t.duration);
        for service in &table.services {
            let key = Key {
                network,
                service: service.service_id,
            };
            let naming = tstables::find(&service.descriptors, tag::SERVICE)
                .and_then(|found| desc::service(&found.body));
            self.service_told(
                (network, 0, true),
                service.service_id,
                false,
                naming,
                ServiceInfo {
                    running: service.running_status,
                    ..ServiceInfo::default()
                },
            );
            let mut programme = Programme::scheduled(key, 0, start.map(epoch), seconds);
            programme.told = Told::Now;
            describe(&service.descriptors, &mut programme);
            if !programme.title.is_empty() {
                self.guide.tell(programme);
            }
        }
    }

    fn on_broadcaster_information(&mut self, table: &tstables::BroadcasterInformation) {
        for broadcaster in &table.broadcasters {
            let mut info = BroadcasterInfo {
                network: table.original_network_id,
                id: broadcaster.broadcaster_id,
                ..BroadcasterInfo::default()
            };
            for found in &broadcaster.descriptors {
                match found.tag {
                    tag::BROADCASTER_NAME => info.name = desc::broadcaster_name(&found.body),
                    tag::EXTENDED_BROADCASTER => {
                        info.extended = desc::extended_broadcaster(&found.body);
                    }
                    tag::SERVICE_LIST => info.services.extend(service_list(&found.body)),
                    _ => {}
                }
            }
            self.si.tell_broadcaster(info);
        }
    }

    fn on_mh_broadcaster_information(&mut self, table: &mmttables::BroadcasterInformation) {
        for broadcaster in &table.broadcasters {
            let mut info = BroadcasterInfo {
                network: table.original_network_id,
                id: broadcaster.broadcaster_id,
                ..BroadcasterInfo::default()
            };
            for found in &broadcaster.descriptors {
                match found.tag {
                    mh_tag::BROADCASTER_NAME => {
                        info.name = desc::mh_broadcaster_name(&found.body);
                    }
                    mh_tag::RELATED_BROADCASTER => {
                        info.related = desc::related_broadcaster(&found.body);
                    }
                    mh_tag::SERVICE_LIST => info.services.extend(service_list(&found.body)),
                    _ => {}
                }
            }
            self.si.tell_broadcaster(info);
        }
    }
}

impl Guide {
    /// 別のストリームのテーブルも入れる（BS と CS は一つのストリームで全局分を送る。ARIB TR-B15）。
    pub fn on_event_table(&mut self, table: &tstables::EventTable) {
        self.on_event_table_counting(table, true);
    }

    pub(crate) fn on_event_table_counting(&mut self, table: &tstables::EventTable, counting: bool) {
        let key = Key {
            network: table.original_network_id,
            service: table.service_id,
        };
        let which = u16::from(table.table_id) | if counting { 0 } else { 0x100 };
        if self.seen_section(key, which, table.number, table.version) {
            return;
        }
        if table.schedule() && counting {
            self.tell_arrival(
                key,
                table.version,
                table.table_id,
                table.number,
                table.segment_last_number,
                table.last_table_id,
            );
        }
        for event in &table.events {
            let mut programme = programme(key, event);
            if !table.schedule() {
                programme.told = Told::Now;
            }
            self.tell(programme);
        }
    }

    /// 別のストリームを示すテーブルは無い（STD-B60 表 4-8）。
    pub fn on_mh_event_table(&mut self, table: &mmttables::EventTable) {
        let key = Key {
            network: table.original_network_id,
            service: table.service_id,
        };
        if self.seen_section(key, u16::from(table.table_id), table.number, table.version) {
            return;
        }
        if table.schedule() {
            self.tell_arrival(
                key,
                table.version,
                table.table_id,
                table.number,
                table.segment_last_number,
                table.last_table_id,
            );
        }
        for event in &table.events {
            let mut programme =
                Programme::scheduled(key, event.event_id, event.start.map(epoch), event.duration);
            programme.free = !event.scrambled;
            programme.told = if table.schedule() {
                Told::Broadcast
            } else {
                Told::Now
            };
            describe_mmt(&event.descriptors, &mut programme);
            self.tell(programme);
        }
    }

    pub fn on_time_table(&mut self, table: &tstables::TimeTable) {
        self.tell_time(epoch(table.time));
    }
}

pub(crate) fn programme(key: Key, event: &tstables::Event) -> Programme {
    let mut programme =
        Programme::scheduled(key, event.event_id, event.start.map(epoch), event.duration);
    programme.free = !event.scrambled;
    describe(&event.descriptors, &mut programme);
    programme
}

pub(crate) fn describe(descriptors: &[tstables::Descriptor], into: &mut Programme) {
    // 区切りは文字コードの途中なのでバイトのまま繋ぐ。
    let mut extended: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let detailed = Detailed::reads(into.network);
    for found in descriptors {
        match found.tag {
            tag::SHORT_EVENT => {
                if let Some(read) = desc::short_event(&found.body) {
                    into.title = read.title;
                    into.detail = read.text;
                }
            }
            tag::EXTENDED_EVENT if detailed => {
                for (heading, value) in desc::extended_event_bytes(&found.body) {
                    match extended.last_mut() {
                        Some(last) if heading.is_empty() => last.1.extend_from_slice(value),
                        _ => extended.push((heading.to_vec(), value.to_vec())),
                    }
                }
            }
            tag::CONTENT => genres(desc::content(&found.body), into),
            tag::COMPONENT => {
                if let Some(read) = desc::picture_component(&found.body)
                    && desc::STREAM_CONTENT_VIDEO.contains(&read.stream_content)
                {
                    into.video = into
                        .video
                        .or(Some(Picture::of_component_type(read.component_type)));
                }
            }
            tag::AUDIO_COMPONENT => {
                if let Some(sound) = desc::audio_component(&found.body) {
                    into.audios.push(sound);
                }
            }
            // `data_component_id` 0x0008 なら字幕（ARIB STD-B10 第2部 6.2.28）。
            tag::DATA_CONTENT => {
                if desc::data_content_id(&found.body) == Some(DATA_COMPONENT_CAPTION) {
                    into.caption = true;
                }
            }
            tag::EVENT_GROUP => shared(desc::event_group(&found.body), into),
            tag::PARENTAL_RATING => {
                into.details
                    .ratings
                    .extend(desc::parental_rating(&found.body));
            }
            tag::CA_CONTRACT_INFO => {
                into.details
                    .contracts
                    .extend(desc::ca_contract_info(&found.body));
            }
            tag::SERIES => into.details.series = desc::series(&found.body),
            tag::COMPONENT_GROUP => {
                into.details.groups = desc::component_group(&found.body);
            }
            tag::TARGET_REGION => into.details.region = desc::target_region(&found.body),
            tag::HYPERLINK => into.details.links.extend(desc::hyperlink(&found.body)),
            _ => {}
        }
    }
    extend(
        extended
            .into_iter()
            .map(|(heading, value)| {
                (
                    isdb::descriptors::text(&heading),
                    isdb::descriptors::text(&value),
                )
            })
            .collect(),
        into,
    );
}

/// 字幕は番組ごとのフラグにならないので入れない。
pub(crate) fn describe_mmt(descriptors: &[mmttables::Descriptor], into: &mut Programme) {
    for found in descriptors {
        match found.tag {
            mh_tag::SHORT_EVENT => {
                if let Some(read) = desc::mh_short_event(&found.body) {
                    into.title = read.title;
                    into.detail = read.text;
                }
            }
            mh_tag::EXTENDED_EVENT if Detailed::reads(into.network) => {
                extend(desc::mh_extended_event(&found.body), into);
            }
            mh_tag::CONTENT => genres(desc::content(&found.body), into),
            mh_tag::VIDEO_COMPONENT => {
                into.video = into.video.or_else(|| {
                    desc::mh_video_component(&found.body).map(|read| Picture {
                        component_type: None,
                        resolution: read.resolution,
                        aspect: read.aspect,
                        progressive: read.progressive,
                        frame_rate: read.frame_rate,
                        transfer: read.transfer,
                    })
                });
            }
            mh_tag::AUDIO_COMPONENT => {
                if let Some(sound) = desc::mh_audio_component(&found.body) {
                    into.audios.push(sound);
                }
            }
            mh_tag::EVENT_GROUP => shared(desc::event_group(&found.body), into),
            mh_tag::PARENTAL_RATING => {
                into.details
                    .ratings
                    .extend(desc::parental_rating(&found.body));
            }
            mh_tag::CA_CONTRACT_INFO => {
                into.details
                    .contracts
                    .extend(desc::mh_ca_contract_info(&found.body));
            }
            mh_tag::SERIES => into.details.series = desc::mh_series(&found.body),
            mh_tag::COMPONENT_GROUP => {
                into.details.groups = desc::mh_component_group(&found.body);
            }
            mh_tag::MULTIMEDIA_SERVICE_INFO => {
                into.details
                    .multimedia
                    .extend(desc::multimedia_service_info(&found.body));
            }
            _ => {}
        }
    }
}

/// サービスリスト記述子（0x41、MH 0x800D も同じ並び）。
fn service_list(body: &[u8]) -> impl Iterator<Item = (u16, u8)> + '_ {
    body.as_chunks::<3>()
        .0
        .iter()
        .map(|entry| (u16::from_be_bytes([entry[0], entry[1]]), entry[2]))
}

fn extend(items: Vec<(String, String)>, into: &mut Programme) {
    for (heading, value) in items {
        match into.extended.last_mut() {
            Some(last) if heading.is_empty() => last.1.push_str(&value),
            _ => into.extended.push((heading, value)),
        }
    }
}

fn genres(read: Vec<desc::Content>, into: &mut Programme) {
    for content in read {
        let genre = Genre {
            large: content.large,
            middle: content.middle,
            user: content.user,
        };
        if !into.genres.contains(&genre) {
            into.genres.push(genre);
        }
    }
}

fn shared(read: Option<(u8, Vec<(u16, u16)>)>, into: &mut Programme) {
    let Some((group, pairs)) = read else {
        return;
    };
    if group != desc::EVENT_GROUP_COMMON {
        return;
    }
    for pair in pairs {
        if !into.shared.contains(&pair) {
            into.shared.push(pair);
        }
    }
}

pub(crate) fn ca_systems(descriptors: &[tstables::Descriptor]) -> Vec<(u16, u16)> {
    descriptors
        .iter()
        .filter_map(|found| match found.tag {
            tag::CONDITIONAL_ACCESS => desc::conditional_access(&found.body),
            tag::ACCESS_CONTROL => {
                desc::access_control(&found.body).map(|(system, pid, _)| (system, pid))
            }
            _ => None,
        })
        .collect()
}
