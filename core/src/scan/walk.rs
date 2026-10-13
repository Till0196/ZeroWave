// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 つの `[[line]]` をスキャンする。衛星のトランスポンダの展開と、スキャン結果の書き方。

use coax::plan::{is_bs, transponder_of};

use super::sample::daemon_at;
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub network: u16,
    pub transport: u16,
    pub services: BTreeMap<u16, String>,
    pub slot: Option<(bool, u8)>,
    pub at: String,
    pub words: Vec<(String, String)>,
    pub cnr: Option<f32>,
    pub group: Option<u8>,
    /// 種別が無いと一覧に並べてよいサービスか分からないので、サービスの一覧に入れる。
    pub told: Vec<Told>,
    pub remote: Option<u8>,
}

/// 1 つの周波数が終わるまで `Lease` は解放しない（解放すると他のクライアントに渡る）。
/// `stop` が立ったら、そこまでに見つかった物を返す。
#[allow(clippy::too_many_arguments)]
pub fn walk(
    endpoint: &str,
    priority: i32,
    tuner: Option<&str>,
    spec: &Spec,
    options: &Options,
    stop: &std::sync::atomic::AtomicBool,
    progress: &mut impl FnMut(Step),
    #[cfg(feature = "plugins")] plugins: Option<&crate::plugins::Loaded>,
) -> Result<Vec<Hit>, String> {
    let targets = targets(spec)?;
    let mut targets = targets;
    let daemon = daemon_at(endpoint, priority);
    crate::input::source::coaxd::reach(&daemon).map_err(|why| why.to_string())?;
    let mut lease = daemon.hold(tuner);
    let mut hits: Vec<Hit> = Vec::new();
    let mut said_tuner = false;
    let mut widened: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
    let mut at = 0;
    while at < targets.len() {
        let target = targets[at].clone();
        let target = &target;
        let total = targets.len();
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        progress(Step {
            what: &target.name,
            done: at + 1,
            total,
            ..Step::default()
        });
        let mut take = || {
            sample(
                &mut lease,
                &daemon,
                target,
                options,
                stop,
                progress,
                #[cfg(feature = "plugins")]
                plugins,
            )
        };
        let mut taken = take();
        if matches!(taken, Err(Missed::Busy)) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            taken = take();
        }
        if !said_tuner && let Some(named) = lease.tuner() {
            said_tuner = true;
            progress(Step {
                what: &target.name,
                done: at + 1,
                total,
                notes: vec![format!("チューナー {named}")],
                ..Step::default()
            });
        }
        match taken {
            Ok(sampled) => {
                let mut notes = vec![heading(&target.name, "受信", &sampled)];
                if let Some(group) = &sampled.group
                    && group.carriers > 1
                {
                    notes.push(format!(
                        "    TSMF 搬送波 {}/{}　グループ{}",
                        group.sequence, group.carriers, group.id
                    ));
                }
                let found = split_found(&sampled);
                if found.is_empty() {
                    notes.push(format!(
                        "    内容を読み取れませんでした（{:.1}秒 / {:.1}MB）",
                        sampled.watched.as_secs_f32(),
                        sampled.bytes as f32 / (1024.0 * 1024.0)
                    ));
                }
                // 周波数だけで指定すると既定の TS しか出ないので、他の TS を `STREAM_ID` で足す。
                let more = satellite_places(target, &found, &sampled, &hits, &mut widened);
                if !more.is_empty() {
                    notes.push(format!(
                        "    同じトランスポンダのほかの TS もスキャンします（{} TS）",
                        more.len()
                    ));
                }
                let pinned = pin_stream(target, &found, &sampled);
                for found in &found {
                    if hits
                        .iter()
                        .any(|hit| hit.network == found.network && hit.transport == found.transport)
                    {
                        continue;
                    }
                    let mut network = beside(&found.network_name);
                    if let Some(remote) = found.remote {
                        network.push_str(&format!("　リモコン {remote}"));
                    }
                    match found.slot {
                        Some((tlv, number)) => notes.push(format!(
                            "    TSMF [#{number} {} TSID {} / ONID {}]{network}",
                            if tlv { "tlv" } else { "ts" },
                            crate::si::hex_id(found.transport, 4),
                            crate::si::hex_id(found.network, 4)
                        )),
                        None => notes.push(format!(
                            "    TSID {} / ONID {}{network}",
                            crate::si::hex_id(found.transport, 4),
                            crate::si::hex_id(found.network, 4)
                        )),
                    }
                    notes.extend(service_lines(found));
                    hits.push(Hit {
                        network: found.network,
                        transport: found.transport,
                        services: found.services.clone(),
                        slot: found.slot,
                        at: target.name.clone(),
                        words: pinned.clone(),
                        cnr: sampled.cnr,
                        group: sampled.group.as_ref().map(|group| group.id),
                        told: found.told.clone(),
                        remote: found.remote,
                    });
                }
                progress(Step {
                    what: &target.name,
                    done: at + 1,
                    total,
                    cnr: sampled.cnr,
                    strength: sampled.strength,
                    mbps: sampled.mbps,
                    notes,
                });
                targets.splice(at + 1..at + 1, more);
            }
            Err(Missed::Stopped) => break,
            Err(Missed::Gone(why)) => {
                return Err(format!(
                    "チューナーが取り外されました（{}）",
                    crate::input::source::coaxd::explain_apart(&why).0
                ));
            }
            Err(missed) => {
                let said = match missed {
                    Missed::NoLock(Some(strength)) => {
                        format!("{}: 信号なし　{strength:.1} dBm", target.name)
                    }
                    Missed::NoLock(None) => format!("{}: 信号なし", target.name),
                    Missed::NoData => {
                        format!("{}: 同期しましたが、データを受信できません", target.name)
                    }
                    Missed::Busy => format!("{}: チューナーが使用中", target.name),
                    Missed::Failed(why) => {
                        let (head, said) = crate::input::source::coaxd::explain_apart(&why);
                        if !said.is_empty() {
                            tracing::info!("{}: {head}（{said}）", target.name);
                        }
                        format!("{}: {head}", target.name)
                    }
                    Missed::Stopped | Missed::Gone(_) => String::new(),
                };
                progress(Step {
                    what: &target.name,
                    done: at + 1,
                    total,
                    notes: vec![said],
                    ..Step::default()
                });
            }
        }
        at += 1;
    }
    lease.release();
    Ok(hits)
}

fn bare_satellite(target: &Target) -> bool {
    let said = |name: &str| target.words.iter().find(|(key, _)| key == name);
    said("DELIVERY_SYSTEM").is_some_and(|(_, system)| {
        system.eq_ignore_ascii_case("ISDBS") || system.eq_ignore_ascii_case("ISDBS3")
    }) && said("STREAM_ID").is_none()
}

pub(super) fn pin_stream(
    target: &Target,
    found: &[Found],
    sampled: &Sampled,
) -> Vec<(String, String)> {
    let mut words = target.words.clone();
    if !bare_satellite(target) || sampled.relative_streams.is_some() {
        return words;
    }
    if let [only] = found {
        words.push((String::from("STREAM_ID"), only.transport.to_string()));
    }
    words
}

/// NIT が同じトランスポンダにあるとする TSID を指定して選局し直す。書くのは SDT actual に載った
/// 物だけ（NIT や SDT other は正しくないことがある）。
pub(super) fn satellite_places(
    target: &Target,
    found: &[Found],
    sampled: &Sampled,
    hits: &[Hit],
    widened: &mut std::collections::BTreeSet<u16>,
) -> Vec<Target> {
    if !bare_satellite(target) {
        return Vec::new();
    }
    let Some(transponder) = found
        .iter()
        .find_map(|found| transponder_of(found.network, found.transport))
    else {
        return Vec::new();
    };
    let network = found.first().map_or(0, |found| found.network);
    let mark = transponder | (network << 8);
    if !widened.insert(mark) {
        return Vec::new();
    }
    let seen = |transport: u16| {
        found.iter().any(|found| found.transport == transport)
            || hits.iter().any(|hit| hit.transport == transport)
    };
    let mut others: Vec<u16> = sampled
        .listed
        .iter()
        .filter(|(listed, transport)| {
            is_bs(*listed) == is_bs(network)
                && transponder_of(*listed, *transport) == Some(transponder)
        })
        .map(|(_, transport)| *transport)
        .filter(|transport| !seen(*transport))
        .collect();
    others.sort_unstable();
    others.dedup();
    if others.is_empty() {
        return Vec::new();
    }
    let with = |value: String| {
        let mut words = target.words.clone();
        words.push((String::from("STREAM_ID"), value));
        words
    };
    match sampled.relative_streams {
        Some(places) => (0..places)
            .map(|place| Target {
                name: format!("{} 相対TS番号 {place}", target.name),
                tlv: false,
                words: with(place.to_string()),
            })
            .collect(),
        None => others
            .into_iter()
            .map(|transport| Target {
                name: format!("{} TSID {}", target.name, crate::si::hex_id(transport, 4)),
                tlv: false,
                words: with(transport.to_string()),
            })
            .collect(),
    }
}

pub(super) fn heading(name: &str, note: &str, sampled: &Sampled) -> String {
    let mut said = format!("{name}: {note}");
    if let Some(cnr) = sampled.cnr {
        said.push_str(&format!("　CNR {cnr:.1} dB"));
    } else if let Some(strength) = sampled.strength {
        said.push_str(&format!("　{strength:.1} dBm"));
    }
    if let Some(mbps) = sampled.mbps {
        said.push_str(&format!("　{mbps:.1} Mbps"));
    }
    said
}

fn beside(name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else {
        format!("　{}", isdb::arib::substitute(name))
    }
}

pub(super) fn service_lines(found: &Found) -> Vec<String> {
    if found.told.is_empty() {
        return found
            .services
            .iter()
            .map(|(id, name)| {
                format!(
                    "        SID {} {}",
                    crate::si::hex_id(*id, 4),
                    shown_name(name)
                )
            })
            .collect();
    }
    found
        .told
        .iter()
        .map(|service| {
            let mut line = format!(
                "        SID {} {}",
                crate::si::hex_id(service.id, 4),
                shown_name(&service.name)
            );
            if !service.provider.is_empty() {
                line.push_str(&format!("　{}", isdb::arib::substitute(&service.provider)));
            }
            let mut what: Vec<String> = Vec::new();
            match isdb::sdt::kind_name(service.kind) {
                Some(kind) => what.push(kind.to_owned()),
                None if service.kind != 0 => what.push(format!("他({:02x})", service.kind)),
                None => {}
            }
            if service.scrambled {
                what.push(String::from("有料"));
            }
            if !what.is_empty() {
                line.push_str(&format!("　({})", what.join(", ")));
            }
            line
        })
        .collect()
}

fn shown_name(name: &str) -> String {
    if name.is_empty() {
        String::from("???")
    } else {
        isdb::arib::substitute(name)
    }
}

/// 分割された TS は TSMF ヘッダに載っている所まで（サービス名は選局して結合してから SI に載る）。
pub(super) fn split_found(sampled: &Sampled) -> Vec<Found> {
    // 内容を読めなかった TS が消えては困るので、TSMF ヘッダに載った TS を全部書く。
    if sampled.relatives.is_empty() {
        return sampled.found.clone();
    }
    sampled
        .relatives
        .iter()
        .filter(|relative| relative.stream_id != 0)
        .map(|relative| {
            let named = sampled.found.iter().find(|found| {
                found.network == relative.original_network_id
                    && found.transport == relative.stream_id
            });
            Found {
                network: relative.original_network_id,
                transport: relative.stream_id,
                services: named
                    .map(|found| found.services.clone())
                    .unwrap_or_default(),
                slot: Some((relative.tlv, relative.number)),
                told: named.map(|found| found.told.clone()).unwrap_or_default(),
                network_name: named
                    .map(|found| found.network_name.clone())
                    .unwrap_or_default(),
                remote: named.and_then(|found| found.remote),
            }
        })
        .collect()
}
