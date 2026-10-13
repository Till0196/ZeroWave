// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルスキャンの手順。周波数の回り方は `zwcore::scan`。受信方式は一つずつ（並列だと同じチューナーを取り合う）。

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::config::channels::{Carrier, Channels, Swept};
use crate::config::key::Stream;

#[derive(Debug, Clone)]
pub struct Leg {
    pub label: String,
    pub endpoint: String,
    pub priority: i32,
    pub spec: zwcore::scan::Spec,
    pub tuner: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub legs: Vec<Leg>,
    pub profile: String,
    pub options: zwcore::scan::Options,
    pub stop: Arc<AtomicBool>,
    /// 止めたときにそこまでを残すか（最後までスキャンした方式は常に残す）。
    pub keep: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default)]
pub struct Scanning {
    pub line: String,
    pub what: String,
    pub measured: String,
    pub done: usize,
    pub total: usize,
    pub log: Vec<String>,
    pub done_with: Option<Result<Summary, String>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub streams: usize,
    pub services: usize,
}

pub type Progress = Arc<Mutex<Scanning>>;

impl Scanning {
    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    pub fn running(&self) -> bool {
        self.done_with.is_none() && (self.total > 0 || !self.what.is_empty())
    }
}

pub struct Hooks<'a> {
    pub wake: &'a dyn Fn(),
    /// 視聴中のチューナーを空けてもらう（真）・戻してもらう（偽）。空きが無いときだけ。
    pub yield_tuner: &'a dyn Fn(bool),
    pub finished: &'a dyn Fn(Vec<zwcore::guide::Service>),
}

fn services_of(hits: &[zwcore::scan::Hit]) -> Vec<zwcore::guide::Service> {
    let mut out = Vec::new();
    for hit in hits {
        if hit.told.is_empty() {
            out.extend(
                hit.services
                    .iter()
                    .map(|(service, name)| zwcore::guide::Service {
                        network: hit.network,
                        transport: hit.transport,
                        service: *service,
                        name: name.clone(),
                        kind: 0x01,
                        ..zwcore::guide::Service::default()
                    }),
            );
        } else {
            out.extend(hit.told.iter().map(|told| zwcore::guide::Service {
                network: hit.network,
                transport: hit.transport,
                service: told.id,
                name: told.name.clone(),
                provider: told.provider.clone(),
                kind: told.kind,
                scrambled: told.scrambled,
                ..zwcore::guide::Service::default()
            }));
        }
    }
    out
}

fn no_tuner_free(legs: &[Leg]) -> bool {
    let Some(first) = legs.first() else {
        return false;
    };
    let Ok(boards) = zwcore::input::source::coaxd::boards(&first.endpoint, first.priority) else {
        return false;
    };
    let tuners: Vec<&zwcore::input::source::coaxd::DaemonTuner> = boards
        .iter()
        .flat_map(|board| &board.tuners)
        .map(|held| &held.tuner)
        .collect();
    legs.iter().any(|leg| {
        let (_, system) = crate::ui::panes::settings::offered(&leg.spec.plan);
        let wanted = |tuner: &zwcore::input::source::coaxd::DaemonTuner| {
            (system.is_empty() || tuner.receives(system))
                && leg.tuner.as_ref().is_none_or(|id| tuner.id == *id)
        };
        tuners.iter().any(|tuner| wanted(tuner))
            && !tuners.iter().any(|tuner| wanted(tuner) && !tuner.leased)
    })
}

fn shown(label: &str) -> &'static str {
    crate::ui::panes::settings::offered(label).0
}

pub fn run(job: Job, into: &Progress, hooks: &Hooks<'_>) {
    let wake = hooks.wake;
    let mut streams = 0usize;
    let mut services = 0usize;
    let mut found: Vec<zwcore::guide::Service> = Vec::new();
    let yielded = no_tuner_free(&job.legs);
    if yielded {
        if let Ok(mut scanning) = into.lock() {
            scanning.log.push(String::from(
                "チャンネルスキャンのため、視聴中のチューナーを閉じます",
            ));
        }
        wake();
        (hooks.yield_tuner)(true);
        let began = std::time::Instant::now();
        while began.elapsed() < std::time::Duration::from_secs(8) && no_tuner_free(&job.legs) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    for leg in &job.legs {
        if job.stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if let Ok(mut scanning) = into.lock() {
            scanning.line = crate::ui::panes::settings::offered(&leg.label).0.to_owned();
            scanning.what = String::new();
            scanning
                .log
                .push(format!("{} をスキャンします", shown(&leg.label)));
        }
        wake();
        let told = into.clone();
        let waking = wake;
        let mut progress = |step: zwcore::scan::Step<'_>| {
            if let Ok(mut scanning) = told.lock() {
                if !step.what.is_empty() {
                    scanning.what = step.what.to_owned();
                }
                if step.total > 0 {
                    scanning.done = step.done;
                    scanning.total = step.total;
                }
                let mut measured = String::new();
                if let Some(cnr) = step.cnr {
                    measured.push_str(&format!("CNR {cnr:.1} dB"));
                } else if let Some(strength) = step.strength {
                    measured.push_str(&format!("{strength:.1} dBm"));
                }
                if let Some(mbps) = step.mbps {
                    if !measured.is_empty() {
                        measured.push_str("　　");
                    }
                    measured.push_str(&format!("{mbps:.1} Mbps"));
                }
                scanning.measured = measured;
                for note in step.notes {
                    if !note.is_empty() {
                        scanning.log.push(note);
                    }
                }
            }
            waking();
        };
        let walked = zwcore::scan::walk(
            &leg.endpoint,
            leg.priority,
            leg.tuner.as_deref(),
            &leg.spec,
            &job.options,
            &job.stop,
            &mut progress,
            Some(&crate::plugins::loaded()),
        );
        match walked {
            Ok(hits) => {
                let stopped = job.stop.load(std::sync::atomic::Ordering::Relaxed);
                let keep = !stopped || job.keep.load(std::sync::atomic::Ordering::Relaxed);
                if keep {
                    streams += hits.len();
                    services += hits.iter().map(|hit| hit.services.len()).sum::<usize>();
                    write_line(&job.profile, &leg.label, &hits, swept_of(&leg.spec));
                    found.extend(services_of(&hits));
                }
                if let Ok(mut scanning) = into.lock() {
                    scanning.log.push(if keep {
                        format!("{}：多重化ストリーム数 {}", shown(&leg.label), hits.len())
                    } else {
                        format!("{} のスキャン結果を破棄しました", shown(&leg.label))
                    });
                }
            }
            Err(why) => {
                if let Ok(mut scanning) = into.lock() {
                    scanning.done_with = Some(Err(format!(
                        "{} をスキャンできません。{why}",
                        shown(&leg.label)
                    )));
                }
                (hooks.finished)(found);
                if yielded {
                    (hooks.yield_tuner)(false);
                }
                wake();
                return;
            }
        }
        wake();
    }
    if let Ok(mut scanning) = into.lock() {
        scanning.done_with = Some(Ok(Summary { streams, services }));
    }
    // 視聴を戻す前にチャンネルマップへ（戻した局が一覧にあるように）。
    (hooks.finished)(found);
    if yielded {
        (hooks.yield_tuner)(false);
    }
    wake();
}

/// 今回スキャンした範囲。`only` で絞ったならその搬送波だけ（スキャンしていない行を消さないため）。
fn swept_of(spec: &zwcore::scan::Spec) -> Swept {
    if spec.only.is_empty() {
        return Swept::Whole;
    }
    match zwcore::scan::targets(spec) {
        Ok(targets) => Swept::Some(
            targets
                .iter()
                .map(|target| frequency_of(&target.words))
                .collect(),
        ),
        Err(_) => Swept::Some(Vec::new()),
    }
}

/// スキャンで見つかった物をチャンネルマップの行へ。分割されたフレーム（TSMF）はストリームごとに一行。
fn mapped(hits: &[zwcore::scan::Hit]) -> BTreeMap<Stream, Vec<Carrier>> {
    let mut found: BTreeMap<Stream, Vec<Carrier>> = BTreeMap::new();
    let mut group_carriers: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    for hit in hits {
        if let Some(group) = hit.group {
            let at = frequency_of(&hit.words);
            let list = group_carriers.entry(group).or_default();
            if !list.contains(&at) {
                list.push(at);
            }
        }
    }
    let mut written_split: std::collections::BTreeSet<(u8, Stream)> =
        std::collections::BTreeSet::new();
    for hit in hits {
        let stream = Stream {
            network: hit.network,
            transport: hit.transport,
        };
        if let Some(group) = hit.group
            && group_carriers
                .get(&group)
                .is_some_and(|list| list.len() > 1)
            && !written_split.insert((group, stream))
        {
            continue;
        }
        let mut fields = toml::Table::new();
        for (name, value) in &hit.words {
            let number = |value: &str| {
                value.parse::<i64>().map_or_else(
                    |_| toml::Value::String(value.to_owned()),
                    toml::Value::Integer,
                )
            };
            let (name, value): (String, toml::Value) = match name.as_str() {
                "DELIVERY_SYSTEM" => (
                    String::from("system"),
                    toml::Value::String(value.to_ascii_lowercase()),
                ),
                "FREQUENCY" => (String::from("freq"), number(value)),
                "CHANNEL" => (String::from("ch"), number(value)),
                // プラグイン向けの語（`multiplex.part` など）は書かない。
                other if other.contains('.') => continue,
                other => (
                    other.to_ascii_lowercase(),
                    toml::Value::String(value.clone()),
                ),
            };
            fields.insert(name, value);
        }
        if let Some((tlv, number)) = hit.slot {
            let kind = if tlv { "tlv" } else { "ts" };
            fields.insert(
                String::from("slot"),
                toml::Value::String(format!("{kind}:{number}")),
            );
        }
        if let Some(cnr) = hit.cnr {
            fields.insert(String::from("cnr"), toml::Value::Float(f64::from(cnr)));
        }
        if let Some(group) = hit.group
            && let Some(list) = group_carriers.get(&group)
            && list.len() > 1
        {
            fields.insert(
                String::from("carriers"),
                toml::Value::Array(
                    list.iter()
                        .map(|at| {
                            at.parse::<i64>().map_or_else(
                                |_| toml::Value::String(at.clone()),
                                toml::Value::Integer,
                            )
                        })
                        .collect(),
                ),
            );
        }
        found.entry(stream).or_default().push(Carrier { fields });
    }
    found
}

fn write_line(profile: &str, label: &str, hits: &[zwcore::scan::Hit], swept: Swept) {
    let found = mapped(hits);
    let at = now();
    let label = label.to_owned();
    crate::config::update_channels_of(profile, move |channels: &mut Channels| {
        let line = channels.lines.entry(label).or_default();
        line.merge(found, &swept, at);
    });
    let known: Vec<(Stream, BTreeMap<u16, String>, Option<u8>)> = hits
        .iter()
        .map(|hit| {
            (
                Stream {
                    network: hit.network,
                    transport: hit.transport,
                },
                hit.services.clone(),
                hit.remote,
            )
        })
        .collect();
    crate::config::update_state(move |state| {
        for (stream, services, remote) in known {
            let known = state.streams.entry(stream).or_default();
            // 示されなかったときは前の番号を残す（NIT は来ないことがある）。
            if let Some(remote) = remote {
                known.remote = Some(u32::from(remote));
            }
            for (service, name) in services {
                if name.is_empty() {
                    continue;
                }
                known.services.insert(service, name);
            }
        }
    });
}

fn frequency_of(words: &[(String, String)]) -> String {
    for name in ["FREQUENCY", "CHANNEL"] {
        if let Some((_, value)) = words.iter().find(|(had, _)| had == name) {
            return value.clone();
        }
    }
    String::new()
}

fn now() -> Option<toml::value::Datetime> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let secs = now.as_secs();
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (year, month, day) = civil_of(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
    .parse()
    .ok()
}

/// 1970 年からの日数を年月日へ（Howard Hinnant の `civil_from_days`）。
fn civil_of(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(freq: &str, transport: u16, slot: (bool, u8), group: Option<u8>) -> zwcore::scan::Hit {
        zwcore::scan::Hit {
            network: 4,
            transport,
            services: BTreeMap::new(),
            slot: Some(slot),
            at: format!("C{freq}"),
            words: vec![
                (String::from("DELIVERY_SYSTEM"), String::from("ISDBC")),
                (String::from("FREQUENCY"), freq.to_owned()),
            ],
            cnr: Some(30.0),
            group,
            told: Vec::new(),
            remote: None,
        }
    }

    #[test]
    fn what_a_scan_found_goes_into_the_roster() {
        let mut told = hit("93000000", 0x40f1, (false, 1), None);
        told.told = vec![
            zwcore::scan::Told {
                id: 1024,
                name: String::from("ゼロウェーブ総合"),
                kind: 0x01,
                ..zwcore::scan::Told::default()
            },
            zwcore::scan::Told {
                id: 1432,
                name: String::from("ゼロウェーブ携帯"),
                kind: 0xc0,
                ..zwcore::scan::Told::default()
            },
        ];
        let mut named = hit("99000000", 0x4031, (true, 1), Some(8));
        named.services.insert(102, String::from("ゼロウェーブ８Ｋ"));
        let roster = services_of(&[told, named]);
        assert_eq!(roster.len(), 3);
        assert_eq!((roster[0].service, roster[0].kind), (1024, 0x01));
        assert!(roster[0].watchable());
        assert!(!roster[1].watchable(), "ワンセグは並べない（種別のまま）");
        assert_eq!(
            (roster[2].service, roster[2].kind),
            (102, 0x01),
            "名前だけなら TV"
        );
        assert_eq!(roster[2].transport, 0x4031);
    }

    #[test]
    fn a_split_frame_is_written_once_with_its_carriers() {
        let hits = vec![
            hit("471000000", 0x4031, (true, 1), Some(8)),
            hit("471000000", 0x4032, (true, 2), Some(8)),
            hit("477000000", 0x4031, (true, 1), Some(8)),
            hit("477000000", 0x4032, (true, 2), Some(8)),
        ];
        let found = mapped(&hits);
        assert_eq!(found.len(), 2, "ストリームは二つ");
        let carriers = &found[&Stream {
            network: 4,
            transport: 0x4031,
        }];
        assert_eq!(carriers.len(), 1, "ストリームごとに一行");
        let carrier = &carriers[0];
        assert_eq!(carrier.order(), "471000000", "代表は最初に見つかった周波数");
        assert_eq!(carrier.carriers(), ["471000000", "477000000"]);
        assert_eq!(carrier.slot(), Some("tlv:1"));
        let words = carrier.tune_words("isdbc");
        assert!(
            words.contains(&(
                String::from("multiplex.carriers"),
                String::from("471000000,477000000")
            )),
            "{words:?}"
        );
        assert!(words.contains(&(String::from("multiplex.part"), String::from("tlv:1"))));
        assert!(
            !words.iter().any(|(name, _)| name == "carriers"),
            "配列をそのまま渡さない: {words:?}"
        );
    }

    #[test]
    fn carriers_of_one_multiplex_are_still_listed_separately() {
        let hits = vec![
            hit("471000000", 0x7fe6, (false, 1), None),
            hit("477000000", 0x7fe6, (false, 1), None),
        ];
        let found = mapped(&hits);
        let carriers = &found[&Stream {
            network: 4,
            transport: 0x7fe6,
        }];
        assert_eq!(carriers.len(), 2, "良い方を選べるように両方残す");
        assert!(carriers[0].carriers().is_empty());
    }

    #[test]
    fn the_ratio_stays_between_none_and_all() {
        let mut scanning = Scanning::default();
        assert_eq!(scanning.ratio(), 0.0, "まだ何も見つけていない");
        scanning.total = 40;
        scanning.done = 10;
        assert!((scanning.ratio() - 0.25).abs() < f32::EPSILON);
        scanning.done = 99;
        assert_eq!(scanning.ratio(), 1.0, "はみ出さない");
    }

    #[test]
    fn the_time_is_written_as_a_toml_datetime() {
        let at = now().expect("作れる");
        let written = at.to_string();
        assert!(written.contains('T'), "{written}");
        assert!(
            written.parse::<toml::value::Datetime>().is_ok(),
            "{written}"
        );
    }

    #[test]
    fn the_days_become_a_date() {
        assert_eq!(civil_of(0), (1970, 1, 1));
        assert_eq!(civil_of(59), (1970, 3, 1));
        assert_eq!(civil_of(19_782), (2024, 2, 29));
    }
}
