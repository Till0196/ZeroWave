// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 現在のストリームの SI に載っていた情報（deck の「ストリームの情報」用）。選局で空になる。

use std::collections::BTreeMap;

use isdb::descriptors::Logo;
use isdb::nit;

use crate::guide::Key;
use crate::guide::lines::contract_line;

const LOGO_PARTS_MOST: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BroadcasterInfo {
    pub network: u16,
    pub id: u8,
    pub name: String,
    pub extended: Option<isdb::descriptors::ExtendedBroadcaster>,
    pub related: Option<isdb::descriptors::RelatedBroadcaster>,
    pub services: Vec<(u16, u8)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Multiplex {
    pub transport: u16,
    pub network: u16,
    pub name: String,
    pub remote_key: u8,
    pub services: Vec<(u16, u8)>,
    pub carried: String,
    pub partial: Vec<u16>,
    pub terrestrial: Option<isdb::nit::Terrestrial>,
    pub satellite: Option<isdb::nit::Satellite>,
    pub cable: Option<isdb::nit::Cable>,
    pub bonding: Vec<isdb::nit::ChannelBondingCarrier>,
    pub advanced_cable: Option<isdb::nit::AdvancedCable>,
    pub cable_division: Vec<isdb::nit::CableDivisionCarrier>,
    pub si_parameters: Option<isdb::nit::SiParameters>,
    pub layers: Vec<(u8, Vec<u16>)>,
    pub system: Option<isdb::nit::SystemManagement>,
    pub links: Vec<isdb::nitdescriptors::Linkage>,
    pub emm: Vec<isdb::descriptors::CaEmmTs>,
    pub connected: Option<isdb::nitdescriptors::ConnectedTransmission>,
    pub service_groups: Vec<isdb::nitdescriptors::ServiceGroup>,
    pub area_broadcasting: Vec<isdb::nitdescriptors::AreaBroadcastingStation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkInfo {
    pub network: u16,
    pub name: String,
    pub own: bool,
    pub multiplexes: Vec<Multiplex>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub transport: u16,
    pub network: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceInfo {
    pub network: u16,
    pub transport: u16,
    pub service: u16,
    pub name: String,
    pub provider: String,
    /// サービス種別（STD-B10）。0 は載っていなかった。
    pub kind: u8,
    pub scrambled: bool,
    pub running: u8,
    pub eit_schedule: bool,
    pub eit_present_following: bool,
    pub logo: Option<Logo>,
    pub contracts: Vec<isdb::descriptors::CaContract>,
    pub broadcaster: Option<u8>,
}

impl ServiceInfo {
    pub fn key(&self) -> Key {
        Key {
            network: self.network,
            service: self.service,
        }
    }

    pub fn watchable(&self) -> bool {
        matches!(self.kind, 0x01 | 0x02 | 0xad)
    }
}

type Sections = BTreeMap<u8, Vec<u8>>;

#[derive(Debug, Clone, Default)]
pub struct Info {
    identity: Option<Identity>,
    frame_header: Option<isdb::tsmf::Info>,
    services: BTreeMap<Key, ServiceInfo>,
    networks: BTreeMap<u16, NetworkInfo>,
    broadcasters: BTreeMap<(u16, u8), BroadcasterInfo>,
    /// 緊急警報放送（STD-B10 6.2.24）。自ネットワークの NIT に載っている間だけ。終了信号も持つ。
    emergency: Vec<isdb::nitdescriptors::EmergencyService>,
    emm: Vec<(u16, u16)>,
    downloads: BTreeMap<(u8, u8), isdb::tstables::SoftwareDownload>,
    logos: BTreeMap<(u16, u8, u16), LogoImage>,
    logo_parts: BTreeMap<(u16, u8, u16, u16), Sections>,
    partial: Option<PartialInfo>,
    packages: Vec<isdb::mmttables::ListedPackage>,
    packages_told: bool,
    layouts: Vec<isdb::mmttables::Layout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogoImage {
    pub network: u16,
    pub logo_type: u8,
    pub logo_id: u16,
    pub version: u16,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartialInfo {
    pub network: Option<isdb::descriptors::NetworkIdentification>,
    pub time: Option<isdb::descriptors::PartialTsTime>,
    pub rate: Option<isdb::descriptors::PartialTs>,
    pub discontinuities: u32,
}

impl Info {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn identity(&self) -> Option<Identity> {
        self.identity
    }

    pub fn set_identity(&mut self, identity: Identity) {
        self.identity = Some(identity);
    }

    pub fn frame_header(&self) -> Option<&isdb::tsmf::Info> {
        self.frame_header.as_ref()
    }

    pub fn set_frame_header(&mut self, header: isdb::tsmf::Info) {
        self.frame_header = Some(header);
    }

    pub fn services(&self) -> impl Iterator<Item = &ServiceInfo> {
        self.services.values()
    }

    pub fn service(&self, key: Key) -> Option<&ServiceInfo> {
        self.services.get(&key)
    }

    /// BIT が先に付けた事業者は残す。
    pub fn tell_service(&mut self, told: ServiceInfo) {
        let entry = self.services.entry(told.key()).or_default();
        let broadcaster = entry.broadcaster.or(told.broadcaster);
        *entry = ServiceInfo {
            broadcaster,
            ..told
        };
    }

    pub fn tell_broadcaster(&mut self, told: BroadcasterInfo) {
        for (service, _) in &told.services {
            let key = Key {
                network: told.network,
                service: *service,
            };
            self.service_entry(key).broadcaster = Some(told.id);
        }
        self.broadcasters.insert((told.network, told.id), told);
    }

    fn service_entry(&mut self, key: Key) -> &mut ServiceInfo {
        self.services.entry(key).or_insert_with(|| ServiceInfo {
            network: key.network,
            service: key.service,
            ..ServiceInfo::default()
        })
    }

    pub fn broadcasters(&self) -> impl Iterator<Item = &BroadcasterInfo> {
        self.broadcasters.values()
    }

    pub fn broadcaster_of(&self, key: Key) -> Option<&BroadcasterInfo> {
        let id = self.services.get(&key)?.broadcaster?;
        self.broadcasters.get(&(key.network, id))
    }

    pub fn networks(&self) -> impl Iterator<Item = &NetworkInfo> {
        self.networks.values()
    }

    /// 終了信号も含む（通知バーを出すのは開始の間だけ）。
    pub fn emergency(&self) -> impl Iterator<Item = &isdb::nitdescriptors::EmergencyService> {
        self.emergency.iter()
    }

    pub fn emm(&self) -> &[(u16, u16)] {
        &self.emm
    }

    pub fn on_conditional_access(&mut self, systems: Vec<(u16, u16)>) {
        for entry in systems {
            if !self.emm.contains(&entry) {
                self.emm.push(entry);
            }
        }
    }

    pub fn downloads(&self) -> impl Iterator<Item = &isdb::tstables::SoftwareDownload> {
        self.downloads.values()
    }

    pub fn on_software_download(&mut self, table: isdb::tstables::SoftwareDownload) {
        self.downloads
            .insert((table.maker_id, table.model_id), table);
    }

    pub fn logos(&self) -> impl Iterator<Item = &LogoImage> {
        self.logos.values()
    }

    pub fn logo(&self, network: u16, logo_type: u8, logo_id: u16) -> Option<&LogoImage> {
        self.logos.get(&(network, logo_type, logo_id))
    }

    pub fn tell_logo_id(&mut self, network: u16, service: u16, logo_id: u16) {
        let entry = self.service_entry(Key { network, service });
        if entry.logo.is_none() {
            entry.logo = Some(Logo::Referenced { logo_id });
        }
    }

    /// MH-CDT のロゴの断片。PNG のシグネチャから IEND までのセクションが揃ったら繋いで返す。
    pub fn on_logo_part(
        &mut self,
        key: (u16, u8, u16, u16),
        number: u8,
        part: &[u8],
    ) -> Option<Vec<u8>> {
        // 別のバージョンの断片はもう揃わないので捨てる。数にも上限を設ける。
        let (network, logo_type, logo_id, version) = key;
        self.logo_parts
            .retain(|&(n, t, i, v), _| (n, t, i) != (network, logo_type, logo_id) || v == version);
        if !self.logo_parts.contains_key(&key) && self.logo_parts.len() >= LOGO_PARTS_MOST {
            self.logo_parts.clear();
        }
        let parts = self.logo_parts.entry(key).or_default();
        parts.insert(number, part.to_vec());
        let is_head = |bytes: &[u8]| bytes.starts_with(b"\x89PNG\r\n\x1a\n");
        let is_tail = |bytes: &[u8]| bytes.len() >= 8 && &bytes[bytes.len() - 8..][..4] == b"IEND";
        let start = parts
            .range(..=number)
            .rev()
            .find(|(_, bytes)| is_head(bytes))
            .map(|(n, _)| *n)?;
        let mut png = Vec::new();
        let mut expect = start;
        let mut end = None;
        for (n, bytes) in parts.range(start..) {
            if *n != expect {
                return None;
            }
            png.extend_from_slice(bytes);
            if is_tail(bytes) {
                end = Some(*n);
                break;
            }
            expect = expect.wrapping_add(1);
        }
        let end = end?;
        for n in start..=end {
            parts.remove(&n);
        }
        Some(png)
    }

    pub fn on_logo(&mut self, logo: LogoImage) {
        self.logos
            .insert((logo.network, logo.logo_type, logo.logo_id), logo);
    }

    pub fn partial(&self) -> Option<&PartialInfo> {
        self.partial.as_ref()
    }

    pub fn partial_mut(&mut self) -> &mut PartialInfo {
        self.partial.get_or_insert_with(PartialInfo::default)
    }

    pub fn packages(&self) -> &[isdb::mmttables::ListedPackage] {
        &self.packages
    }

    pub fn set_packages(&mut self, packages: Vec<isdb::mmttables::ListedPackage>) {
        self.packages = packages;
        self.packages_told = true;
    }

    pub fn listed_packages(&self) -> Option<Vec<u16>> {
        self.packages_told.then(|| {
            self.packages
                .iter()
                .filter_map(|package| match package.package_id[..] {
                    [high, low] => Some(u16::from_be_bytes([high, low])),
                    _ => None,
                })
                .collect()
        })
    }

    pub fn layouts(&self) -> &[isdb::mmttables::Layout] {
        &self.layouts
    }

    pub fn set_layouts(&mut self, layouts: Vec<isdb::mmttables::Layout>) {
        self.layouts = layouts;
    }

    /// 別の NID のテーブル（0x41）も入れる（自ネットワークの分だけだと別のストリームが無いように見える）。
    pub fn on_network_information(&mut self, read: &nit::Described) {
        if read.network.actual {
            self.emergency = read
                .transports
                .iter()
                .flat_map(|t| t.emergency.iter().cloned())
                .collect();
        }
        let entry = self
            .networks
            .entry(read.network.id)
            .or_insert_with(|| NetworkInfo {
                network: read.network.id,
                own: read.network.actual,
                ..NetworkInfo::default()
            });
        if entry.name.is_empty() && !read.network.name.is_empty() {
            entry.name = read.network.name.clone();
        }
        entry.own |= read.network.actual;
        for transport in &read.transports {
            let told = Multiplex {
                transport: transport.stream_id,
                network: transport.original_network_id,
                name: transport.name.clone(),
                remote_key: transport.remote_control_key_id,
                services: transport
                    .services
                    .iter()
                    .map(|service| (service.id, service.kind))
                    .collect(),
                carried: carried(transport),
                partial: transport.partial_reception.clone(),
                terrestrial: transport.terrestrial.clone(),
                satellite: transport.satellite,
                cable: transport.cable,
                bonding: transport.channel_bonding.clone(),
                advanced_cable: transport.advanced_cable.clone(),
                cable_division: transport.cable_division.clone(),
                si_parameters: transport.si_parameters.clone(),
                layers: transport.transmission_types.clone(),
                system: transport.system.clone(),
                links: transport.links.clone(),
                emm: transport.emm.clone(),
                connected: transport.connected.clone(),
                service_groups: transport.service_groups.clone(),
                area_broadcasting: transport.area_broadcasting.clone(),
            };
            match entry
                .multiplexes
                .iter_mut()
                .find(|known| known.transport == told.transport && known.network == told.network)
            {
                // 同じ TS の記述子は繰り返し届くので、空いているところだけ埋める。
                Some(known) => {
                    if known.name.is_empty() {
                        known.name = told.name;
                    }
                    if known.remote_key == 0 {
                        known.remote_key = told.remote_key;
                    }
                    if known.services.is_empty() {
                        known.services = told.services;
                    }
                    if known.carried.is_empty() {
                        known.carried = told.carried;
                    }
                    if known.partial.is_empty() {
                        known.partial = told.partial;
                    }
                    if known.terrestrial.is_none() {
                        known.terrestrial = told.terrestrial;
                    }
                    if known.satellite.is_none() {
                        known.satellite = told.satellite;
                    }
                    if known.cable.is_none() {
                        known.cable = told.cable;
                    }
                    if known.bonding.is_empty() {
                        known.bonding = told.bonding;
                    }
                    if known.advanced_cable.is_none() {
                        known.advanced_cable = told.advanced_cable;
                    }
                    if known.cable_division.is_empty() {
                        known.cable_division = told.cable_division;
                    }
                    if known.si_parameters.is_none() {
                        known.si_parameters = told.si_parameters;
                    }
                    if known.layers.is_empty() {
                        known.layers = told.layers;
                    }
                    if known.system.is_none() {
                        known.system = told.system;
                    }
                    if known.links.is_empty() {
                        known.links = told.links;
                    }
                    if known.emm.is_empty() {
                        known.emm = told.emm;
                    }
                    if known.connected.is_none() {
                        known.connected = told.connected;
                    }
                    if known.service_groups.is_empty() {
                        known.service_groups = told.service_groups;
                    }
                    if known.area_broadcasting.is_empty() {
                        known.area_broadcasting = told.area_broadcasting;
                    }
                }
                None => entry.multiplexes.push(told),
            }
        }
        if !read.remote_keys.is_empty() {
            for known in &mut entry.multiplexes {
                if known.remote_key != 0 {
                    continue;
                }
                known.remote_key = read
                    .remote_keys
                    .iter()
                    .find(|button| {
                        known
                            .services
                            .iter()
                            .any(|(service, _)| *service == button.service_id)
                    })
                    .map_or(0, |button| button.remote_control_key_id);
            }
        }
        entry.multiplexes.sort_by_key(|known| known.transport);
    }
}

fn carried(transport: &nit::Transport) -> String {
    if let Some(terrestrial) = &transport.terrestrial {
        let carriers: Vec<String> = terrestrial
            .carriers_hz
            .iter()
            .map(|hertz| format!("{:.6} MHz", f64::from(*hertz) / 1_000_000.0))
            .collect();
        if carriers.is_empty() {
            return format!("地上 エリア {}", terrestrial.area_code);
        }
        return format!(
            "地上 エリア {} / {}",
            terrestrial.area_code,
            carriers.join("、")
        );
    }
    if let Some(satellite) = &transport.satellite {
        return format!(
            "衛星 {}度{} {:.5} GHz {}",
            f64::from(satellite.orbital_position) / 10.0,
            if satellite.east { "東" } else { "西" },
            f64::from(satellite.downlink_khz) / 1_000_000.0,
            polarisation_name(satellite.polarisation),
        );
    }
    if let Some(cable) = &transport.cable {
        return format!(
            "有線 {:.4} MHz{}",
            f64::from(cable.frequency_hz) / 1_000_000.0,
            match cable.modulation_name() {
                Some(named) => format!(" {named}"),
                None => String::new(),
            }
        );
    }
    String::new()
}

pub fn service_lines(service: &ServiceInfo) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "running_status {}（{}） / EIT スケジュール {} / EIT 現在・次 {}",
        service.running,
        running_name(service.running),
        if service.eit_schedule {
            "あり"
        } else {
            "なし"
        },
        if service.eit_present_following {
            "あり"
        } else {
            "なし"
        },
    ));
    if let Some(logo) = &service.logo {
        lines.push(match logo {
            Logo::Downloaded {
                logo_id,
                version,
                download_data_id,
            } => format!(
                "ロゴ : CDT（logo ID {} / バージョン {version} / download data ID {}）",
                hex_id(*logo_id, 3),
                hex_id(*download_data_id, 4)
            ),
            Logo::Referenced { logo_id } => {
                format!("ロゴ : CDT（logo ID {}）", hex_id(*logo_id, 3))
            }
            Logo::Text(text) => format!("ロゴ : 簡易ロゴ「{text}」"),
            Logo::Other(kind) => format!("ロゴ : 伝送方式 0x{kind:02x}"),
        });
    }
    for contract in &service.contracts {
        lines.push(contract_line(contract));
    }
    if let Some(broadcaster) = service.broadcaster {
        lines.push(format!(
            "事業者 : broadcaster ID 0x{broadcaster:02x} ({broadcaster})"
        ));
    }
    lines
}

fn running_name(status: u8) -> &'static str {
    match status {
        0 => "未定義",
        1 => "非実行中",
        2 => "数秒以内に開始",
        3 => "停止中",
        4 => "実行中",
        _ => "予備",
    }
}

pub fn polarisation_name(polarisation: char) -> &'static str {
    match polarisation {
        'H' => "水平偏波",
        'V' => "垂直偏波",
        'L' => "左旋円偏波",
        'R' => "右旋円偏波",
        _ => "偏波不明",
    }
}

pub fn hex_id(value: impl Into<u64>, width: usize) -> String {
    let value = value.into();
    format!("0x{value:0width$x} ({value})")
}

pub fn broadcaster_lines(info: &BroadcasterInfo) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(extended) = &info.extended {
        lines.push(format!(
            "拡張ブロードキャスタ : 種別 {}（{}） / 地上事業者 ID {} / 系列 [{}] / 自ネットワーク外 [{}]",
            extended.kind,
            match extended.kind {
                1 => "テレビ",
                2 => "音声",
                _ => "予備",
            },
            hex_id(extended.terrestrial_broadcaster_id, 4),
            joined(&extended.affiliations, |affiliation| hex_id(*affiliation, 2)),
            joined(&extended.related, |(network, broadcaster)| format!(
                "NID {} / broadcaster ID {}",
                hex_id(*network, 4),
                hex_id(*broadcaster, 2)
            )),
        ));
    }
    if let Some(related) = &info.related {
        lines.push(format!(
            "関連ブロードキャスタ : [{}] / 系列 [{}] / ネットワークID [{}]",
            joined(&related.broadcasters, |(network, broadcaster)| format!(
                "NID {} / broadcaster ID {}",
                hex_id(*network, 4),
                hex_id(*broadcaster, 2)
            )),
            joined(&related.affiliations, |affiliation| hex_id(*affiliation, 2)),
            joined(&related.original_networks, |network| hex_id(*network, 4)),
        ));
    }
    for (service, kind) in &info.services {
        lines.push(format!(
            "サービス : SID 0x{service:04x} ({service}) / Type 0x{kind:02x}"
        ));
    }
    lines
}

pub fn multiplex_lines(multiplex: &Multiplex) -> Vec<String> {
    let mut lines = Vec::new();
    for (kind, services) in &multiplex.layers {
        lines.push(format!(
            "伝送階層 0x{kind:02x} : [{}]",
            joined(services, |service| format!("SID {}", hex_id(*service, 4)))
        ));
    }
    if let Some(system) = &multiplex.system {
        lines.push(format!(
            "システム管理 : {} / 放送規格 0x{:02x} / 詳細 0x{:02x}",
            if system.broadcasting {
                "放送"
            } else {
                "放送休止"
            },
            system.broadcasting_identifier,
            system.additional_identification
        ));
    }
    for link in &multiplex.links {
        lines.push(format!(
            "リンク : 種別 0x{:02x} → NID {} / TSID {} / SID {}",
            link.linkage_type,
            hex_id(link.original_network_id, 4),
            hex_id(link.transport_stream_id, 4),
            hex_id(link.service_id, 4)
        ));
    }
    for emm in &multiplex.emm {
        lines.push(format!(
            "CA_EMM_TS : CA system ID {} → NID {} / TSID {} / 電源保持 {} 分",
            hex_id(emm.ca_system_id, 4),
            hex_id(emm.original_network_id, 4),
            hex_id(emm.transport_stream_id, 4),
            emm.power_supply_period
        ));
    }
    if let Some(connected) = &multiplex.connected {
        lines.push(format!(
            "接続送信 : グループ 0x{:04x} / セグメント種別 {} / 変調 A {} B {} C {}",
            connected.group_id,
            connected.segment_type,
            connected.modulation_a,
            connected.modulation_b,
            connected.modulation_c
        ));
    }
    for group in &multiplex.service_groups {
        lines.push(format!(
            "サービスグループ : 種別 {} / [{}]",
            group.group_type,
            joined(&group.pairs, |pair| format!(
                "{}→{}",
                pair.primary_service_id, pair.secondary_service_id
            ))
        ));
    }
    if let Some(parameters) = &multiplex.si_parameters {
        lines.push(format!(
            "SI 伝送パラメータ : バージョン {} / 更新 MJD 0x{:04x} / テーブル [{}]",
            parameters.version,
            parameters.update_mjd,
            joined(&parameters.tables, |(table, description)| format!(
                "0x{table:02x}:{}",
                description
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ))
        ));
    }
    for station in &multiplex.area_broadcasting {
        lines.push(format!(
            "エリア放送 : 局 0x{:06x} / 地域 0x{:04x} / 信号形式 {}",
            station.station_id, station.location_code, station.signal_format
        ));
    }
    lines
}

fn joined<'a, T: 'a>(
    items: impl IntoIterator<Item = &'a T>,
    say: impl Fn(&'a T) -> String,
) -> String {
    items.into_iter().map(say).collect::<Vec<_>>().join(" ")
}

pub fn emergency_line(warning: &isdb::nitdescriptors::EmergencyService) -> String {
    let signal = match (warning.started, warning.signal_level) {
        (false, _) => "終了信号",
        (true, 0) => "第1種開始信号",
        (true, _) => "第2種開始信号",
    };
    let mut line = format!(
        "緊急警報放送 : SID {} / {signal}",
        hex_id(warning.service_id, 4)
    );
    if !warning.area_codes.is_empty() {
        let areas: Vec<String> = warning
            .area_codes
            .iter()
            .map(|area| hex_id(*area, 3))
            .collect();
        line.push_str(&format!(" / 地域 {}", areas.join("・")));
    }
    line
}

pub fn emm_lines(emm: &[(u16, u16)]) -> Vec<String> {
    emm.iter()
        .map(|(system, pid)| {
            format!(
                "EMM : CA system ID {} {} / PID {}",
                hex_id(*system, 4),
                ca_system_name(*system),
                hex_id(*pid, 4)
            )
        })
        .collect()
}

pub fn ca_system_name(system: u16) -> &'static str {
    match system {
        0x0005 => "ARIB STD-B25",
        0x000a => "ARIB STD-B25（CS）",
        0x000d => "ARIB STD-B61",
        0x000e => "ARIB STD-B61（地上）",
        _ => "—",
    }
}

/// 配信の表の見出し（メーカーと機種）。
pub fn download_title(table: &isdb::tstables::SoftwareDownload) -> String {
    format!(
        "メーカー ID {} / 機種 ID {} / NID {} / TSID {} / SID {} / バージョン {}",
        hex_id(table.maker_id, 2),
        hex_id(table.model_id, 2),
        hex_id(table.original_network_id, 4),
        hex_id(table.transport_stream_id, 4),
        hex_id(table.service_id, 4),
        table.version
    )
}

/// 配信の表のグループごとの行と、その告知の文。
pub fn download_contents(table: &isdb::tstables::SoftwareDownload) -> Vec<(String, Vec<String>)> {
    let mut contents = Vec::new();
    for content in &table.contents {
        let mut said = format!(
            "グループ {} : 対象バージョン 0x{:03x} → 新バージョン 0x{:03x} / {} / {}",
            content.group,
            content.target_version,
            content.new_version,
            if content.download_level == 1 {
                "強制"
            } else {
                "任意"
            },
            match content.version_indicator {
                0 => "全バージョン",
                1 => "対象バージョン以下",
                2 => "対象バージョン以上",
                _ => "対象バージョンのみ",
            }
        );
        for (start, seconds) in &content.schedules {
            said.push_str(&format!(
                " / {:04}/{:02}/{:02} {:02}:{:02} から {} 秒",
                start.year, start.month, start.day, start.hour, start.minute, seconds
            ));
        }
        let mut notice: Vec<String> = Vec::new();
        // 同じ配信の記述子が何度も届くので、配信の情報は一度だけ書く。
        let mut described: Vec<(u32, u32, u8)> = Vec::new();
        for found in &content.descriptors {
            if found.tag == isdb::descriptors::tag::DOWNLOAD_CONTENT
                && let Some(read) = isdb::descriptors::download_content(&found.body)
            {
                let what = (read.download_id, read.component_size, read.component_tag);
                if !described.contains(&what) {
                    described.push(what);
                    said.push_str(&format!(
                        " / download ID {} / {} バイト / tag 0x{:02x}{}",
                        hex_id(read.download_id, 8),
                        read.component_size,
                        read.component_tag,
                        if read.reboot { " / 再起動" } else { "" },
                    ));
                }
                if let Some((_, text)) = &read.text {
                    notice.extend(
                        text.lines()
                            .map(str::trim_end)
                            .filter(|line| !line.is_empty())
                            .map(str::to_owned),
                    );
                }
            }
        }
        contents.push((said, notice));
    }
    contents
}

pub fn logo_line(logo: &LogoImage) -> String {
    format!(
        "NID {} / 種類 {}（{}） / logo ID {} / バージョン {} / PNG {} バイト",
        hex_id(logo.network, 4),
        logo.logo_type,
        crate::guide::logo::name(logo.logo_type),
        hex_id(logo.logo_id, 3),
        logo.version,
        logo.png.len()
    )
}

pub fn partial_lines(partial: &PartialInfo) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(network) = &partial.network {
        lines.push(format!(
            "ネットワークID : {} / 種別 0x{:04x} / NID {}",
            network.country,
            network.media_type,
            hex_id(network.network_id, 4)
        ));
    }
    if let Some(time) = &partial.time {
        let mut said = format!("番組の時刻 : バージョン {}", time.event_version);
        if let Some(start) = time.event_start {
            said.push_str(&format!(
                " / 開始 {:04}/{:02}/{:02} {:02}:{:02}:{:02}",
                start.year, start.month, start.day, start.hour, start.minute, start.second
            ));
        }
        if let Some(seconds) = time.duration {
            said.push_str(&format!(" / 長さ {seconds} 秒"));
        }
        if let Some(offset) = time.offset {
            said.push_str(&format!(
                " / オフセット {offset} 秒（フラグ {}）",
                u8::from(time.offset_flag)
            ));
        }
        if let Some(jst) = time.jst_time {
            said.push_str(&format!(
                " / 録画開始 {:04}/{:02}/{:02} {:02}:{:02}:{:02}",
                jst.year, jst.month, jst.day, jst.hour, jst.minute, jst.second
            ));
        }
        lines.push(said);
    }
    if let Some(rate) = &partial.rate {
        lines.push(format!(
            "レート : 最大 {:.3} Mbps / 平滑化の最小 {:.3} Mbps / バッファ {} バイト",
            f64::from(rate.peak_rate) * 400.0 / 1e6,
            f64::from(rate.minimum_smoothing_rate) * 400.0 / 1e6,
            u32::from(rate.maximum_smoothing_buffer) * 4096
        ));
    }
    lines.push(format!("不連続（DIT） : {} 回", partial.discontinuities));
    lines
}

pub fn package_line(package: &isdb::mmttables::ListedPackage) -> String {
    use isdb::mmttables::PackageLocation;
    let id = package
        .package_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let location = match &package.location {
        PackageLocation::Here { packet_id } => format!("packet_id {}", hex_id(*packet_id, 4)),
        PackageLocation::Ip { port, packet_id } => {
            format!("IP ポート {port} / packet_id {}", hex_id(*packet_id, 4))
        }
        PackageLocation::Ts {
            network_id,
            transport_stream_id,
            pid,
        } => format!(
            "TS NID {} / TSID {} / PID {}",
            hex_id(*network_id, 4),
            hex_id(*transport_stream_id, 4),
            hex_id(*pid, 4)
        ),
        PackageLocation::Url(url) => url.clone(),
        PackageLocation::Other(kind) => format!("場所の種別 0x{kind:02x}"),
    };
    format!("パッケージ {id} : {location}")
}

pub fn layout_line(layout: &isdb::mmttables::Layout) -> String {
    format!(
        "レイアウト {} / デバイス {} : {}",
        layout.number,
        layout.device,
        layout
            .regions
            .iter()
            .map(|r| format!(
                "領域{} ({},{})-({},{}) レイヤー{}",
                r.number, r.left, r.top, r.right, r.bottom, r.layer
            ))
            .collect::<Vec<_>>()
            .join(" / ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_emergency_warning_is_written_as_the_standard_names_it() {
        let mut warning = isdb::nitdescriptors::EmergencyService {
            service_id: 0x0400,
            started: true,
            signal_level: 0,
            area_codes: vec![0x5a5],
        };
        assert_eq!(
            emergency_line(&warning),
            "緊急警報放送 : SID 0x0400 (1024) / 第1種開始信号 / 地域 0x5a5 (1445)"
        );
        warning.signal_level = 1;
        warning.area_codes.push(0x00f);
        assert_eq!(
            emergency_line(&warning),
            "緊急警報放送 : SID 0x0400 (1024) / 第2種開始信号 / 地域 0x5a5 (1445)・0x00f (15)"
        );
        warning.started = false;
        warning.area_codes.clear();
        assert_eq!(
            emergency_line(&warning),
            "緊急警報放送 : SID 0x0400 (1024) / 終了信号"
        );
    }

    #[test]
    fn the_package_list_is_unknown_until_it_arrives() {
        let mut info = Info::new();
        assert_eq!(info.listed_packages(), None);
        info.set_packages(Vec::new());
        assert_eq!(info.listed_packages(), Some(Vec::new()));
        info.set_packages(vec![isdb::mmttables::ListedPackage {
            package_id: vec![0x00, 0x65],
            location: isdb::mmttables::PackageLocation::Here { packet_id: 0x0010 },
        }]);
        assert_eq!(info.listed_packages(), Some(vec![0x0065]));
    }
}
