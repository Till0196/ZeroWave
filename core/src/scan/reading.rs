// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::*;

/// テーブルだけ欲しいのでエンジンを通さない。
pub struct Reading {
    feed: Feed,
    guide: Guide,
    pub(super) si: Info,
    survey: isdb::tsmf::Survey,
}

impl std::fmt::Debug for Reading {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Reading")
            .field("services", &self.si.services().count())
            .field("identity", &self.si.identity())
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub network: u16,
    pub transport: u16,
    pub services: BTreeMap<u16, String>,
    pub slot: Option<(bool, u8)>,
    pub told: Vec<Told>,
    pub network_name: String,
    /// NIT の TS 情報記述子（TLV なら TLV-NIT のサービスごと）に載っていたもの。
    pub remote: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Told {
    pub id: u16,
    pub name: String,
    pub provider: String,
    pub kind: u8,
    pub scrambled: bool,
}

impl Reading {
    pub fn new(tlv: bool) -> Self {
        let container = if tlv {
            crate::filters::Container::Tlv
        } else {
            crate::filters::Container::Ts
        };
        Self {
            feed: Feed::new(container),
            guide: Guide::new(),
            si: Info::new(),
            survey: isdb::tsmf::Survey::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.survey.feed(bytes);
        let mut sink = Tables {
            guide: &mut self.guide,
            si: &mut self.si,
        };
        self.feed.push(bytes, &mut sink);
    }

    /// 現在のストリームの識別子、サービス、NIT が揃ったか。分割フレームでは NIT を待たない（1 つの周波数では揃わない）。
    pub fn settled(&self) -> bool {
        if self.si.identity().is_none() || self.si.services().next().is_none() {
            return false;
        }
        if self.survey.group().is_some_and(|group| group.carriers > 1) {
            return true;
        }
        self.si.networks().next().is_some()
    }

    /// 識別子が載っていなければ空（どのストリームか分からない行はチャンネルマップに書けない）。
    pub fn found(&self) -> Vec<Found> {
        let Some(identity) = self.si.identity() else {
            return Vec::new();
        };
        let mut services: BTreeMap<u16, String> = BTreeMap::new();
        for service in self.guide.channels() {
            if service.network == identity.network && service.transport == identity.transport {
                services.insert(service.service, service.name.clone());
            }
        }
        // 番組表はワンセグやデータを並べないので SI から。
        let told = self
            .si
            .services()
            .filter(|service| {
                service.network == identity.network && service.transport == identity.transport
            })
            .map(|service| Told {
                id: service.service,
                name: service.name.clone(),
                provider: service.provider.clone(),
                kind: service.kind,
                scrambled: service.scrambled,
            })
            .collect();
        let network_name = self
            .si
            .networks()
            .find(|network| network.network == identity.network)
            .map(|network| network.name.clone())
            .unwrap_or_default();
        let remote = self
            .si
            .networks()
            .flat_map(|network| network.multiplexes.iter())
            .find(|multiplex| {
                multiplex.transport == identity.transport
                    && multiplex.network == identity.network
                    && multiplex.remote_key != 0
            })
            .map(|multiplex| multiplex.remote_key);
        vec![Found {
            network: identity.network,
            transport: identity.transport,
            services,
            slot: None,
            told,
            network_name,
            remote,
        }]
    }

    pub fn relatives(&self) -> Vec<isdb::tsmf::Relative> {
        self.survey.streams()
    }

    pub fn group(&self) -> Option<isdb::tsmf::Group> {
        self.survey.group()
    }

    /// NIT と SDT の `other` に載っている別のストリームの分も。
    pub fn services(&self) -> Vec<Service> {
        self.guide.channels().cloned().collect()
    }
}

struct Tables<'a> {
    guide: &'a mut Guide,
    si: &'a mut Info,
}

impl Sink for Tables<'_> {
    fn on_section(&mut self, section: &Section) {
        crate::filters::tables::Tables {
            guide: self.guide,
            si: self.si,
        }
        .feed(section);
    }
}
