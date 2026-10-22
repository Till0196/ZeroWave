// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 選局したら確かめ、外れていたら探す。チャンネルマップに書くのは一次の情報
//! （いま受けている TS の SDT actual と PAT）だけ。NIT とマップの他の行は探す候補にだけ使う。

use super::*;
use crate::config::channels::Carrier;
use crate::config::key::Stream;

/// SDT actual と PAT が揃うのを待つ長さ。揃わなければ確かめない。
const PATIENCE: Duration = Duration::from_secs(12);

/// 照合が済んだ選局。NIT は数秒おきにしか来ず、照合の時点では揃っていないことが多いので、NIT が示す
/// 局が変わるたびに一覧に無い局を足し直す。
#[derive(Debug, Clone)]
pub(super) struct WatchingNit {
    verifying: Verifying,
    seen: std::collections::BTreeSet<(u16, u16)>,
}

/// NIT が示す、自ネットワークの衛星の多重化ストリームとそのサービスの組。
fn nit_services(
    tables: &zwcore::player::Tables,
    network: u16,
) -> std::collections::BTreeSet<(u16, u16)> {
    tables
        .networks
        .iter()
        .filter(|known| known.network == network && known.own)
        .flat_map(|known| known.multiplexes.iter())
        .filter(|multiplex| multiplex.satellite.is_some() && multiplex.network == network)
        .flat_map(|multiplex| {
            multiplex
                .services
                .iter()
                .map(move |(service, _)| (multiplex.transport, *service))
        })
        .collect()
}

/// 局一覧（`state.toml` の streams）に入れる局か。チャンネルスキャンと同じく番組表の対象（テレビ・ラジオ）だけを
/// 入れ、ワンセグやデータは入れない（入れると番組表に種別の分からない局として並ぶ）。次のどれかなら入れない：
/// SDT のサービス記述子の種別が番組表の対象でない、NIT のサービスリスト記述子の種別が同じく対象でない、
/// NIT の部分受信記述子に載っている、PMT が部分受信の位置（0x1FC8〜0x1FCF）にある。
fn listable(tables: &zwcore::player::Tables, key: zwcore::guide::Key) -> bool {
    if let Some(service) = tables.services.iter().find(|service| service.key() == key) {
        return service.watchable();
    }
    let multiplexes = || {
        tables
            .networks
            .iter()
            .flat_map(|known| known.multiplexes.iter())
            .filter(|multiplex| multiplex.network == key.network)
    };
    let listed = multiplexes()
        .flat_map(|multiplex| multiplex.services.iter())
        .find(|(service, _)| *service == key.service)
        .map(|(_, kind)| *kind);
    if listed.is_some_and(|kind| !matches!(kind, 0x01 | 0x02 | 0xad)) {
        return false;
    }
    if multiplexes().any(|multiplex| multiplex.partial.contains(&key.service)) {
        return false;
    }
    !tables
        .programs
        .iter()
        .any(|program| program.number == key.service && program.one_seg())
}

#[derive(Debug, Clone)]
pub(super) struct Verifying {
    key: zwcore::guide::Key,
    name: String,
    container: zwcore::filters::Container,
    line: String,
    plan: String,
    expected: Stream,
    carrier: Carrier,
    began: Instant,
    left: Option<Vec<Carrier>>,
    tried: Vec<String>,
}

impl Verifying {
    fn how(&self, carrier: &Carrier) -> String {
        tuning::how_of(&carrier.tune_words(&self.plan))
    }
}

/// NIT が挙げたがまだ確かめていない局。確かめるまでチャンネルマップに書かない。
#[derive(Debug, Clone)]
pub(super) struct Unconfirmed {
    key: zwcore::guide::Key,
    how: String,
    line: String,
    plan: String,
    expected: Stream,
    carrier: Carrier,
    channel: zwcore::input::source::Channel,
}

/// チャンネルマップの中でこの選局パラメーターを作った行。マップの外（Mirakurun など）なら無し。
fn locate(how: &str) -> Option<(String, String, Stream, Carrier)> {
    let channels = crate::config::load_channels_used(&crate::config::load_state().profile);
    for (label, line) in &channels.lines {
        let plan = label.clone();
        for (stream, carriers) in &line.streams {
            for carrier in carriers {
                if tuning::how_of(&carrier.tune_words(&plan)) == how {
                    return Some((label.clone(), plan, *stream, carrier.clone()));
                }
            }
        }
    }
    None
}

/// その行の方式が衛星か。衛星の行でケーブルの NIT を信じない（その逆も）。
fn satellite_line(plan: &str, carrier: &Carrier) -> bool {
    let system = carrier.system().unwrap_or(plan).to_ascii_lowercase();
    system == "isdbs"
}

/// 探索先。同じ周波数の他のスロット、NIT が示す場所、同じストリームの他の搬送波の順。
fn candidates(
    verifying: &Verifying,
    tables: &zwcore::player::Tables,
    table: Option<&crate::config::channels::LineTable>,
) -> Vec<Carrier> {
    let key = verifying.key;
    let base = &verifying.carrier;
    let satellite = satellite_line(&verifying.plan, base);
    let rows: Vec<(Stream, &Carrier)> = table
        .map(|table| {
            table
                .streams
                .iter()
                .flat_map(|(stream, carriers)| carriers.iter().map(move |one| (*stream, one)))
                .collect()
        })
        .unwrap_or_default();
    let listed: Vec<u16> = tables
        .networks
        .iter()
        .filter(|network| network.network == key.network)
        .flat_map(|network| network.multiplexes.iter())
        .filter(|multiplex| {
            if satellite {
                multiplex.satellite.is_some()
            } else {
                multiplex.cable.is_some()
            }
        })
        .filter(|multiplex| {
            multiplex
                .services
                .iter()
                .any(|(sid, _)| *sid == key.service)
        })
        .map(|multiplex| multiplex.transport)
        .collect();
    let mut out: Vec<Carrier> = Vec::new();
    let same_frequency =
        |one: &&(Stream, &Carrier)| one.1.order() == base.order() && !one.1.same_carrier(base);
    for (_, row) in rows
        .iter()
        .filter(same_frequency)
        .filter(|(stream, _)| *stream == verifying.expected || listed.contains(&stream.transport))
    {
        out.push((*row).clone());
    }
    if satellite {
        let transponder = |transport: u16| (transport >> 4) & 0x1f;
        for transport in std::iter::once(verifying.expected.transport).chain(listed.iter().copied())
        {
            if transponder(transport) == transponder(verifying.expected.transport) {
                out.push(base.with("stream_id", toml::Value::String(transport.to_string())));
            }
        }
    }
    for (_, row) in rows.iter().filter(same_frequency) {
        out.push((*row).clone());
    }
    for transport in &listed {
        for (_, row) in rows
            .iter()
            .filter(|(stream, _)| stream.transport == *transport)
        {
            out.push((*row).clone());
        }
        if satellite && let Some(khz) = zwcore::scan::satellite_frequency(key.network, *transport) {
            out.push(
                base.with("freq", toml::Value::Integer(i64::from(khz)))
                    .with("stream_id", toml::Value::String(transport.to_string())),
            );
        }
    }
    for (_, row) in rows
        .iter()
        .filter(|(stream, _)| *stream == verifying.expected)
    {
        out.push((*row).clone());
    }
    let mut kept: Vec<Carrier> = Vec::new();
    for one in out {
        if one.same_carrier(base)
            || kept.iter().any(|had| had.same_carrier(&one))
            || verifying.tried.contains(&verifying.how(&one))
        {
            continue;
        }
        kept.push(one);
    }
    kept
}

/// 確かめた一次の情報をチャンネルマップに書く。同じ搬送波を別のストリームの行に置いていたら外す。
/// 変わらなければ書かない（`seen` は日付が変わったときだけ）。
/// `seen` は受けて確かめたか。NIT から書くだけの物は確かめた日を付けない。
fn write_carrier(line: &str, carrier: &Carrier, actual: Stream, seen: bool) -> bool {
    // 書くのはその方式の結果を持っている所：いまのプロファイル自身か、無ければ借りている all。
    let own = crate::config::load_channels();
    let mut channels = if own.lines.contains_key(line) {
        own
    } else {
        crate::config::load_channels_of("")
    };
    let before = channels.clone();
    let table = channels.lines.entry(line.to_owned()).or_default();
    for (stream, carriers) in &mut table.streams {
        if *stream != actual {
            carriers.retain(|one| one.keep() || !one.same_carrier(carrier));
        }
    }
    table.streams.retain(|_, carriers| !carriers.is_empty());
    let list = table.streams.entry(actual).or_default();
    let had = list
        .iter()
        .position(|one| one.same_carrier(carrier))
        .map(|at| list.remove(at));
    let mut row = had.clone().unwrap_or_else(|| carrier.clone());
    for (name, value) in &carrier.fields {
        if name != "seen" {
            row.fields.insert(name.clone(), value.clone());
        }
    }
    let today = crate::config::state::now();
    let same_day = |one: Option<toml::value::Datetime>| {
        one.and_then(|at| at.date)
            .zip(today.and_then(|at| at.date))
            .is_some_and(|(a, b)| a == b)
    };
    let stale = !same_day(had.as_ref().and_then(Carrier::seen));
    if seen
        && let Some(today) = today
        && stale
    {
        row.fields
            .insert(String::from("seen"), toml::Value::Datetime(today));
    }
    list.insert(0, row);
    if channels == before {
        return false;
    }
    crate::config::save_channels(&channels);
    true
}

impl Stage {
    pub(super) fn verify_opened(
        &mut self,
        key: zwcore::guide::Key,
        how: &str,
        name: &str,
        container: zwcore::filters::Container,
    ) {
        self.watching_nit = None;
        if let Some(verifying) = self.verifying.as_mut()
            && verifying.key == key
            && verifying.left.is_some()
            && verifying.how(&verifying.carrier) == how
        {
            verifying.began = Instant::now();
            return;
        }
        let placed = locate(how).or_else(|| {
            self.unconfirmed
                .iter()
                .find(|one| one.how == how)
                .map(|one| {
                    (
                        one.line.clone(),
                        one.plan.clone(),
                        one.expected,
                        one.carrier.clone(),
                    )
                })
        });
        self.verifying = placed.map(|(line, plan, expected, carrier)| Verifying {
            key,
            name: name.to_owned(),
            container,
            line,
            plan,
            expected,
            carrier,
            began: Instant::now(),
            left: None,
            tried: vec![how.to_owned()],
        });
    }

    pub(super) fn verify_refused(&mut self) -> bool {
        if !self
            .verifying
            .as_ref()
            .is_some_and(|verifying| verifying.left.is_some())
        {
            return false;
        }
        self.next_candidate();
        true
    }

    pub(super) fn check_verifying(&mut self) {
        let Some(verifying) = self.verifying.as_ref() else {
            self.watch_nit();
            return;
        };
        // 録画の再生へ移った。テーブルは録画の物なので、どこにも書かない。
        if self.player.is_recording() {
            self.verifying = None;
            return;
        }
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.supply.tuning().is_some())
        {
            return;
        }
        let ready = {
            let tables = self.player.tables();
            (tables.identity, !tables.programs.is_empty())
        };
        let (Some(identity), true) = ready else {
            if verifying.began.elapsed() >= PATIENCE {
                tracing::info!(
                    "{} の識別情報が揃わないため、チャンネル一覧との照合をやめる",
                    verifying.name
                );
                self.verifying = None;
            }
            return;
        };
        let tables = self.player.tables().clone();
        let actual = Stream {
            network: identity.network,
            transport: identity.transport,
        };
        let key = verifying.key;
        let here = actual.network == key.network
            && tables
                .programs
                .iter()
                .any(|program| program.number == key.service);
        let verifying = verifying.clone();
        let mut changed = write_carrier(&verifying.line, &verifying.carrier, actual, true);
        if here {
            let listed_here = crate::config::load_state()
                .streams
                .get(&actual)
                .is_some_and(|known| known.services.contains_key(&key.service));
            let was_unconfirmed = self.unconfirmed.iter().any(|one| one.key == key);
            self.unconfirmed.retain(|one| one.key != key);
            if !listable(&tables, key) {
                // ワンセグなどは選局できるが局一覧には入れない。前に入ってしまった物は外す。
                if listed_here {
                    crate::config::update_state(|state| {
                        for (stream, known) in &mut state.streams {
                            if stream.network == key.network {
                                known.services.remove(&key.service);
                            }
                        }
                    });
                    changed = true;
                    tracing::info!(
                        "{} は番組表の対象でないため、局一覧から外した",
                        verifying.name
                    );
                }
            } else if actual != verifying.expected || !listed_here {
                // 局はチャンネルマップと違うストリームにあった。局一覧（`state.toml`）の局も移す。
                let service_name = self
                    .service(key)
                    .map(|service| service.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| verifying.name.clone());
                crate::config::update_state(|state| {
                    for (stream, known) in &mut state.streams {
                        if stream.network == key.network && *stream != actual {
                            known.services.remove(&key.service);
                        }
                    }
                    state
                        .streams
                        .entry(actual)
                        .or_default()
                        .services
                        .insert(key.service, service_name);
                });
                changed = true;
                let said = if was_unconfirmed {
                    format!("{} を確認し、チャンネル一覧に追加した", verifying.name)
                } else {
                    format!(
                        "{} が別の多重化ストリーム（{actual}）にあったため、チャンネル一覧を更新した",
                        verifying.name
                    )
                };
                tracing::info!("{said}");
            } else if verifying.left.is_some() {
                tracing::info!(
                    "{} が別の場所で見つかったため、チャンネル一覧を更新した",
                    verifying.name
                );
            }
            self.verifying = None;
            let offered = self.offer_unconfirmed(&verifying, &tables);
            if changed {
                self.remap_coaxd();
            } else if offered {
                self.unconfirmed_offered();
            }
            self.watching_nit = Some(WatchingNit {
                seen: nit_services(&tables, verifying.key.network),
                verifying,
            });
            return;
        }
        tracing::info!(
            "{} がチャンネル一覧の位置（{} の {}）に無い。現在の多重化ストリームは {actual}",
            verifying.name,
            verifying.line,
            verifying.expected
        );
        if changed {
            self.remap_coaxd();
        }
        if let Some(verifying) = self.verifying.as_mut()
            && verifying.left.is_none()
        {
            let channels = crate::config::load_channels_used(&crate::config::load_state().profile);
            let found = candidates(verifying, &tables, channels.lines.get(&verifying.line));
            tracing::info!("  探索先 {} 件", found.len());
            verifying.left = Some(found);
        }
        self.next_candidate();
    }

    fn next_candidate(&mut self) {
        let Some(verifying) = self.verifying.as_mut() else {
            return;
        };
        let next = verifying.left.as_mut().and_then(|left| {
            if left.is_empty() {
                None
            } else {
                Some(left.remove(0))
            }
        });
        let Some(carrier) = next else {
            let verifying = self.verifying.take();
            let Some(verifying) = verifying else {
                return;
            };
            let first = verifying.tried.first().cloned();
            if let Some(first) = first
                && !self.tried_ways.contains(&first)
            {
                self.tried_ways.push(first);
            }
            let another = self.live.as_ref().is_some_and(|live| {
                live.channels.iter().any(|channel| {
                    channel.key == Some(verifying.key) && !self.tried_ways.contains(&channel.how)
                })
            });
            if another {
                tracing::info!("{} を別の経路で再試行する", verifying.name);
                self.tune(verifying.key);
                return;
            }
            tracing::info!("{} がチャンネル一覧の位置に見つからない", verifying.name);
            self.toast(&format!(
                "{} を受信できません。\nチャンネルスキャンをやり直してください。",
                verifying.name
            ));
            return;
        };
        let how = verifying.how(&carrier);
        verifying.tried.push(how.clone());
        verifying.carrier = carrier;
        verifying.began = Instant::now();
        let channel = zwcore::input::source::Channel {
            name: verifying.name.clone(),
            key: Some(verifying.key),
            order: None,
            transport: None,
            how,
            container: verifying.container,
        };
        let key = verifying.key;
        tracing::info!("  探索先 {}", channel.how);
        self.send_tune(channel, key);
    }

    /// 照合が済んだ選局で NIT の示す局が変わったら、一覧に無い局を足す。
    fn watch_nit(&mut self) {
        let Some(watching) = self.watching_nit.as_ref() else {
            return;
        };
        if self.player.is_recording() {
            self.watching_nit = None;
            return;
        }
        if self
            .live
            .as_ref()
            .is_none_or(|live| live.supply.tuning().is_some())
        {
            return;
        }
        let network = watching.verifying.key.network;
        let tables = self.player.tables();
        if tables
            .identity
            .is_none_or(|identity| identity.network != network)
        {
            return;
        }
        let seen = nit_services(tables, network);
        if seen == watching.seen {
            return;
        }
        let tables = tables.clone();
        let verifying = watching.verifying.clone();
        if let Some(watching) = self.watching_nit.as_mut() {
            watching.seen = seen;
        }
        if self.offer_unconfirmed(&verifying, &tables) {
            self.unconfirmed_offered();
        }
    }

    /// 未確認の局を足したあと、チャンネルマップを読み直したときと同じく番組表とソースの一覧を作り直す。
    fn unconfirmed_offered(&mut self) {
        self.catalog_due = true;
        self.roster_seeded = false;
        self.listed_mark = None;
    }

    /// NIT が挙げる局のうち一覧に無い物を未確認として足す。衛星の方式だけ。
    fn offer_unconfirmed(
        &mut self,
        verifying: &Verifying,
        tables: &zwcore::player::Tables,
    ) -> bool {
        if !satellite_line(&verifying.plan, &verifying.carrier) {
            return false;
        }
        let Some(live) = self.live.as_mut() else {
            return false;
        };
        let network = verifying.key.network;
        let mut added = 0;
        for multiplex in tables
            .networks
            .iter()
            .filter(|known| known.network == network && known.own)
            .flat_map(|known| known.multiplexes.iter())
            .filter(|multiplex| multiplex.satellite.is_some() && multiplex.network == network)
        {
            let Some(khz) = zwcore::scan::satellite_frequency(network, multiplex.transport) else {
                continue;
            };
            for (service, kind) in &multiplex.services {
                if !matches!(kind, 0x01 | 0x02 | 0xad) {
                    continue;
                }
                let key = zwcore::guide::Key {
                    network,
                    service: *service,
                };
                if live.channels.iter().any(|channel| channel.key == Some(key))
                    || self.unconfirmed.iter().any(|one| one.key == key)
                {
                    continue;
                }
                let carrier = verifying
                    .carrier
                    .with("freq", toml::Value::Integer(i64::from(khz)))
                    .with(
                        "stream_id",
                        toml::Value::String(multiplex.transport.to_string()),
                    );
                let how = verifying.how(&carrier);
                let stations = if self.recording_guide {
                    &self.guide
                } else {
                    self.stations.shown()
                };
                let name = super::stations::service_in(stations, &self.guide, key)
                    .map(|service| service.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("サービス {service}"));
                let channel = zwcore::input::source::Channel {
                    name: format!("{name}（未確認）"),
                    key: Some(key),
                    order: None,
                    transport: None,
                    how: how.clone(),
                    container: verifying.container,
                };
                live.channels.push(channel.clone());
                // チャンネルマップと state にも書く（書かないと起動のたびに消え、選局すると増える）。
                // 自局の NIT の周波数と TS なので確か。違えば選局したときの確かめで直る。
                let expected = Stream {
                    network,
                    transport: multiplex.transport,
                };
                write_carrier(&verifying.line, &carrier, expected, false);
                crate::config::update_state(|state| {
                    state
                        .streams
                        .entry(expected)
                        .or_default()
                        .services
                        .entry(*service)
                        .or_insert(name);
                });
                self.unconfirmed.push(Unconfirmed {
                    key,
                    how,
                    line: verifying.line.clone(),
                    plan: verifying.plan.clone(),
                    expected: Stream {
                        network,
                        transport: multiplex.transport,
                    },
                    carrier,
                    channel,
                });
                added += 1;
            }
        }
        if added > 0 {
            tracing::info!("NIT にある {added} 局を未確認として一覧に追加した");
        }
        added > 0
    }

    pub(super) fn remap_coaxd(&mut self) {
        let Some(url) = self.live.as_ref().and_then(|live| live.url.clone()) else {
            return;
        };
        if !url.starts_with("coaxd:") {
            return;
        }
        // チューナーで絞らずにビルドし直すと、受けられない方式の all の局まで選べるようになる。
        let file = crate::config::load_plain();
        let profile = crate::config::load_state().profile;
        let Some((endpoint, priority, tuners, scanned)) = super::openings(&file, &profile)
            .into_iter()
            .find_map(|(_, opening)| match opening {
                Opening::Coaxd {
                    endpoint,
                    priority,
                    tuners,
                    profile,
                } if tuning::came_of_coaxd(&endpoint, &tuners, &profile) == url => {
                    Some((endpoint, priority, tuners, profile))
                }
                _ => None,
            })
        else {
            return;
        };
        let mapped = tuning::mapped_for(&endpoint, priority, &tuners, &scanned);
        self.catalog_due = true;
        if !mapped.is_empty()
            && let Some(live) = self.live.as_mut()
        {
            tracing::info!("チャンネル一覧の {} 局を選局可能にした", mapped.len());
            live.channels = mapped;
            for one in &self.unconfirmed {
                if !live
                    .channels
                    .iter()
                    .any(|channel| channel.key == Some(one.key))
                {
                    live.channels.push(one.channel.clone());
                }
            }
        }
        self.roster_seeded = false;
        self.listed_mark = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nit_services_are_the_own_satellite_multiplexes() {
        let multiplex = |transport, services: &[u16], satellite: bool| zwcore::si::Multiplex {
            transport,
            network: 4,
            services: services.iter().map(|service| (*service, 0x01)).collect(),
            satellite: satellite.then(isdb::nit::Satellite::default),
            ..zwcore::si::Multiplex::default()
        };
        let mut tables = zwcore::player::Tables {
            networks: vec![
                zwcore::si::NetworkInfo {
                    network: 4,
                    own: true,
                    multiplexes: vec![multiplex(16625, &[101, 102], true)],
                    ..zwcore::si::NetworkInfo::default()
                },
                zwcore::si::NetworkInfo {
                    network: 6,
                    own: false,
                    multiplexes: vec![multiplex(24608, &[55], true)],
                    ..zwcore::si::NetworkInfo::default()
                },
            ],
            ..zwcore::player::Tables::default()
        };
        let first = nit_services(&tables, 4);
        assert_eq!(
            first.into_iter().collect::<Vec<_>>(),
            [(16625, 101), (16625, 102)]
        );
        // 残りの多重化ストリームが後から届くと変わったことになる（そこで一覧に無い局を足す）。
        tables.networks[0]
            .multiplexes
            .push(multiplex(16626, &[201], true));
        assert!(nit_services(&tables, 4).contains(&(16626, 201)));
        assert_ne!(
            nit_services(&tables, 4),
            nit_services(&zwcore::player::Tables::default(), 4)
        );
    }

    #[test]
    fn a_one_segment_service_is_not_listed_as_a_station() {
        let tokyo = 0x7fe0;
        let key = |service| zwcore::guide::Key {
            network: tokyo,
            service,
        };
        let program = |number, map| zwcore::filters::Program {
            number,
            map,
            streams: Vec::new(),
            pcr: 0,
            ecm: Vec::new(),
            application: None,
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        };
        let service = |id, kind| zwcore::guide::Service {
            network: tokyo,
            service: id,
            kind,
            ..zwcore::guide::Service::default()
        };
        // SDT が届く前でも、PMT が部分受信の位置にあればワンセグ。
        let mut tables = zwcore::player::Tables {
            programs: vec![program(1024, 0x1f0), program(1408, 0x1fc8)],
            ..zwcore::player::Tables::default()
        };
        assert!(listable(&tables, key(1024)));
        assert!(!listable(&tables, key(1408)));
        // NIT の部分受信記述子に載っていればワンセグ。
        tables.programs.clear();
        tables.networks = vec![zwcore::si::NetworkInfo {
            network: tokyo,
            own: true,
            multiplexes: vec![zwcore::si::Multiplex {
                transport: 0x7fe0,
                network: tokyo,
                services: vec![(1024, 0x01), (1408, 0xc0)],
                partial: vec![1408],
                ..zwcore::si::Multiplex::default()
            }],
            ..zwcore::si::NetworkInfo::default()
        }];
        assert!(listable(&tables, key(1024)));
        assert!(!listable(&tables, key(1408)));
        // SDT の種別があればそれで決める。
        tables.services = vec![service(1024, 0x01), service(1408, 0xc0)];
        assert!(listable(&tables, key(1024)));
        assert!(!listable(&tables, key(1408)));
        // 何も分からない局は今までどおり入れる。
        assert!(listable(&zwcore::player::Tables::default(), key(1032)));
    }

    fn carrier(text: &str) -> Carrier {
        let table: toml::Table = toml::from_str(text).unwrap();
        Carrier { fields: table }
    }

    #[test]
    fn a_satellite_miss_looks_on_the_same_transponder_then_where_the_nit_says() {
        let base = carrier(
            r#"system = "isdbs"
freq = 1318460
stream_id = "16626""#,
        );
        let verifying = Verifying {
            key: zwcore::guide::Key {
                network: 4,
                service: 201,
            },
            name: String::from("ゼロウェーブプレミアム"),
            container: zwcore::filters::Container::Ts,
            line: String::from("sat"),
            plan: String::from("isdbs"),
            expected: Stream {
                network: 4,
                transport: 0x40f2,
            },
            carrier: base.clone(),
            began: Instant::now(),
            left: None,
            tried: Vec::new(),
        };
        let multiplex = |transport: u16, satellite: bool, service: u16| zwcore::si::Multiplex {
            transport,
            network: 4,
            services: vec![(service, 1)],
            satellite: satellite.then(isdb::nit::Satellite::default),
            ..zwcore::si::Multiplex::default()
        };
        let tables = zwcore::player::Tables {
            networks: vec![zwcore::si::NetworkInfo {
                network: 4,
                name: String::new(),
                own: true,
                multiplexes: vec![multiplex(0x4011, true, 201), multiplex(0x4631, false, 201)],
            }],
            ..zwcore::player::Tables::default()
        };
        let found = candidates(&verifying, &tables, None);
        let named: Vec<(Option<String>, String)> = found
            .iter()
            .map(|one| (one.stream_id(), one.order()))
            .collect();
        assert_eq!(
            named,
            [(
                Some(String::from("16401")),
                zwcore::scan::satellite_frequency(4, 0x4011)
                    .unwrap()
                    .to_string()
            )],
            "同じトランスポンダに 201 を置くストリームは無いので、NIT が示す 0x4011 だけ"
        );
    }

    #[test]
    fn swapped_slots_are_found_on_the_same_carrier() {
        let slot = |n: u8| {
            carrier(&format!(
                "system = \"isdbc\"\nfreq = 465000000\nslot = \"ts:{n}\""
            ))
        };
        let premium = Stream {
            network: 4,
            transport: 0x40f2,
        };
        let bs10 = Stream {
            network: 4,
            transport: 0x48f3,
        };
        let mut table = crate::config::channels::LineTable::default();
        table.streams.insert(bs10, vec![slot(1), slot(2)]);
        let verifying = Verifying {
            key: zwcore::guide::Key {
                network: 4,
                service: 201,
            },
            name: String::from("ゼロウェーブプレミアム"),
            container: zwcore::filters::Container::Ts,
            line: String::from("cable"),
            plan: String::from("isdbc"),
            expected: premium,
            carrier: slot(1),
            began: Instant::now(),
            left: None,
            tried: Vec::new(),
        };
        let found = candidates(&verifying, &zwcore::player::Tables::default(), Some(&table));
        assert_eq!(
            found.iter().map(Carrier::slot).collect::<Vec<_>>(),
            [Some("ts:2")]
        );
    }

    #[test]
    fn a_satellite_row_is_told_apart_by_its_stream() {
        let row = carrier("freq = 1318460\nstream_id = \"18675\"");
        let other = carrier("freq = 1318460\nstream_id = \"16626\"");
        assert!(!row.same_carrier(&other), "衛星は stream_id も比べる");
        assert!(row.same_carrier(&row.with("seen", toml::Value::Boolean(true))));
    }
}
