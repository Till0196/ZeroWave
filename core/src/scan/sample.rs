// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 つの周波数を選局してロックを待ち、決まった時間だけ読む。

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub lock_timeout: std::time::Duration,
    pub data_timeout: std::time::Duration,
    pub analysis_time: std::time::Duration,
    /// 分割フレームのとき。1 つの周波数には一部しか含まれないので短く。
    pub hold_analysis_time: std::time::Duration,
    pub poll: std::time::Duration,
    pub minimum_bytes: usize,
    pub maximum_bytes: usize,
}

impl Default for Options {
    fn default() -> Self {
        use std::time::Duration;
        Self {
            lock_timeout: Duration::from_secs(3),
            data_timeout: Duration::from_secs(5),
            analysis_time: Duration::from_secs(12),
            hold_analysis_time: Duration::from_secs(4),
            poll: Duration::from_millis(250),
            minimum_bytes: 2 * 1024 * 1024,
            maximum_bytes: 24 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Missed {
    NoLock(Option<f32>),
    NoData,
    /// 前に使っていた側が解放している途中のことがある。
    Busy,
    Stopped,
    Gone(String),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct Step<'a> {
    pub what: &'a str,
    pub done: usize,
    pub total: usize,
    pub cnr: Option<f32>,
    pub strength: Option<f32>,
    pub mbps: Option<f32>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sampled {
    pub found: Vec<Found>,
    pub relatives: Vec<isdb::tsmf::Relative>,
    pub group: Option<isdb::tsmf::Group>,
    pub cnr: Option<f32>,
    pub strength: Option<f32>,
    pub mbps: Option<f32>,
    pub bytes: usize,
    pub watched: std::time::Duration,
    /// NIT に載っている衛星の TS。選局先の候補にだけ使う（NIT は正しくないことがある）。
    pub listed: Vec<(u16, u16)>,
    pub relative_streams: Option<u32>,
}

/// `Lease::tune` は待たずに戻るのでこちらで待つ（`tune` を繰り返すと選局のやり直しになる）。
pub fn wait_for_lock(
    lease: &mut coax::daemon::Lease,
    daemon: &coax::daemon::Daemon,
    target: &Target,
    options: &Options,
    stop: &std::sync::atomic::AtomicBool,
    progress: &mut impl FnMut(Step),
) -> Result<coax::daemon::Tuned, Missed> {
    let tuner = lease.tuner().map(str::to_owned);
    let asked = crate::input::source::coaxd::with_setting(&target.asked(), tuner.as_deref());
    let tuned = lease.tune(&asked).map_err(|why| {
        let said = why.to_string();
        if said.contains("503") || said.to_ascii_lowercase().contains("busy") {
            Missed::Busy
        } else {
            Missed::Failed(said)
        }
    })?;
    if tuned.locked {
        return Ok(tuned);
    }
    let began = std::time::Instant::now();
    let mut last = tuned.clone();
    while began.elapsed() < options.lock_timeout {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Missed::Stopped);
        }
        std::thread::sleep(options.poll);
        let Some(tuner) = lease.tuner() else {
            continue;
        };
        let seen = daemon
            .seen(tuner)
            .map_err(|why| Missed::Gone(why.to_string()))?;
        last.cnr = seen.cnr.or(last.cnr);
        last.strength = seen.strength.or(last.strength);
        progress(Step {
            what: &format!(
                "{}　同期待ち {:.1}秒",
                target.name,
                began.elapsed().as_secs_f32()
            ),
            cnr: last.cnr,
            strength: last.strength,
            ..Step::default()
        });
        // まだストリームを開いていないので、ロックしていてもデータは届いていない。
        if seen.locked == Some(true) {
            last.locked = true;
            return Ok(last);
        }
        if seen.locked.is_none() {
            return Ok(last);
        }
    }
    Err(Missed::NoLock(last.strength))
}

/// `plugins` を渡すと TSMF を分離してサービス名まで、渡さなければ相対TS番号と TSID まで。
pub fn sample(
    lease: &mut coax::daemon::Lease,
    daemon: &coax::daemon::Daemon,
    target: &Target,
    options: &Options,
    stop: &std::sync::atomic::AtomicBool,
    progress: &mut impl FnMut(Step),
    #[cfg(feature = "plugins")] plugins: Option<&crate::plugins::Loaded>,
) -> Result<Sampled, Missed> {
    let tuned = wait_for_lock(lease, daemon, target, options, stop, progress)?;
    progress(Step {
        what: &format!("{}　ストリームを開いています", target.name),
        cnr: tuned.cnr,
        strength: tuned.strength,
        ..Step::default()
    });
    let tuner = lease.tuner().map(str::to_owned);
    let asked = crate::input::source::coaxd::with_setting(&target.asked(), tuner.as_deref());
    let mut answer = lease
        .stream(
            &asked,
            Some(options.maximum_bytes),
            Some(options.analysis_time),
            Some(options.data_timeout),
        )
        .map_err(|why| Missed::Failed(why.to_string()))?;
    let mut reading = Reading::new(target.tlv);
    #[cfg(feature = "plugins")]
    let mut slots = Slots::new(plugins);
    let began = std::time::Instant::now();
    let mut read = 0usize;
    let mut buffer = vec![0u8; 188 * 348];
    let mut told = std::time::Instant::now();
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let taken = match answer.read_body(&mut buffer) {
            Ok(0) => break,
            Ok(taken) => taken,
            Err(_) => break,
        };
        read += taken;
        reading.push(&buffer[..taken]);
        #[cfg(feature = "plugins")]
        slots.push(&buffer[..taken], &reading);
        if told.elapsed() >= options.poll {
            told = std::time::Instant::now();
            progress(Step {
                what: &format!(
                    "{}　ストリーム取得中 {:.1}秒　{:.1}MB",
                    target.name,
                    began.elapsed().as_secs_f32(),
                    read as f32 / (1024.0 * 1024.0)
                ),
                cnr: tuned.cnr,
                strength: tuned.strength,
                mbps: Some(mbps(read, began.elapsed())),
                ..Step::default()
            });
        }
        let split = reading.group().is_some_and(|group| group.carriers > 1);
        let patience = if split {
            options.hold_analysis_time
        } else {
            options.analysis_time
        };
        // 分割された TS は 1 つの周波数ではテーブルが揃わないので、TSMF ヘッダが読めたら切り上げる。
        if split && read >= options.minimum_bytes && !reading.relatives().is_empty() {
            break;
        }
        if read >= options.minimum_bytes && reading.settled() {
            break;
        }
        if began.elapsed() >= patience || read >= options.maximum_bytes {
            break;
        }
    }
    // 読み残すと接続ごと切られてチューナーを失う。
    let _ = answer.read_to_end();
    lease.recover(answer);
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(Missed::Stopped);
    }
    if read == 0 {
        return Err(Missed::NoData);
    }
    #[cfg_attr(not(feature = "plugins"), allow(unused_mut))]
    let mut found = reading.found();
    #[cfg(feature = "plugins")]
    {
        let named = slots.found();
        if !named.is_empty() {
            found = named;
        }
    }
    let watched = began.elapsed();
    let listed = reading
        .si
        .networks()
        .flat_map(|network| network.multiplexes.iter())
        .filter(|multiplex| multiplex.satellite.is_some())
        .map(|multiplex| (multiplex.network, multiplex.transport))
        .collect();
    Ok(Sampled {
        found,
        relatives: reading.relatives(),
        group: reading.group(),
        cnr: tuned.cnr,
        strength: tuned.strength,
        mbps: Some(mbps(read, watched)),
        bytes: read,
        watched,
        listed,
        relative_streams: tuned.relative_streams,
    })
}

pub(super) fn mbps(bytes: usize, over: std::time::Duration) -> f32 {
    (bytes as f32 * 8.0) / over.as_secs_f32().max(0.001) / 1_000_000.0
}

pub(super) fn daemon_at(endpoint: &str, priority: i32) -> coax::daemon::Daemon {
    coax::daemon::Daemon::new(crate::input::source::coaxd::endpoint_of(endpoint))
        .with_priority(priority)
}

pub fn tuner_matching(endpoint: &str, priority: i32, pattern: &str) -> Option<String> {
    if pattern.trim().is_empty() {
        return None;
    }
    let daemon = daemon_at(endpoint, priority);
    crate::input::source::coaxd::reach(&daemon).ok()?;
    let mut boards = daemon.listing().ok()?;
    crate::input::source::coaxd::declare_boards(&mut boards);
    boards
        .iter()
        .flat_map(|board| &board.tuners)
        .find(|held| held.tuner.matches(pattern) && !held.tuner.leased)
        .map(|held| held.tuner.id.clone())
}

pub fn serving_tuner(
    endpoint: &str,
    priority: i32,
    system: &str,
    patterns: &[String],
) -> Option<String> {
    let daemon = daemon_at(endpoint, priority);
    crate::input::source::coaxd::reach(&daemon).ok()?;
    let mut boards = daemon.listing().ok()?;
    crate::input::source::coaxd::declare_boards(&mut boards);
    let serving: Vec<&crate::input::source::coaxd::DaemonTuner> = boards
        .iter()
        .flat_map(|board| &board.tuners)
        .map(|held| &held.tuner)
        .filter(|tuner| {
            !tuner.deliveries.is_empty()
                && tuner.receives(system)
                && crate::input::source::coaxd::tuner_setting(&tuner.id).allows(system)
                && (patterns.is_empty() || patterns.iter().any(|pattern| tuner.matches(pattern)))
        })
        .collect();
    serving
        .iter()
        .find(|tuner| !tuner.leased)
        .or_else(|| serving.first())
        .map(|tuner| tuner.id.clone())
}

/// TSMF の分離はプラグインが行うので、TS ごとにプラグインチェーンを作り `zwsdk::abi::PART` の予約値で指定する。
#[cfg(feature = "plugins")]
struct Slots<'a> {
    open: std::collections::BTreeMap<u8, (zwsdk::host::Chain, Reading, bool)>,
    plugins: Option<&'a crate::plugins::Loaded>,
}

#[cfg(feature = "plugins")]
impl<'a> Slots<'a> {
    fn new(plugins: Option<&'a crate::plugins::Loaded>) -> Self {
        Self {
            open: std::collections::BTreeMap::new(),
            plugins,
        }
    }

    fn push(&mut self, bytes: &[u8], outer: &Reading) {
        let Some(plugins) = self.plugins else {
            return;
        };
        for relative in outer.relatives() {
            let number = relative.number;
            let std::collections::btree_map::Entry::Vacant(slot) = self.open.entry(number) else {
                continue;
            };
            let mut chain = plugins.stream_chain_with(None);
            if chain.is_empty() {
                return;
            }
            let container = if relative.tlv {
                zwsdk::host::Container::Tlv
            } else {
                zwsdk::host::Container::Ts
            };
            let failed = chain.start(container, Some(zwsdk::abi::PART | u16::from(number)));
            if !failed.is_empty() {
                continue;
            }
            slot.insert((chain, Reading::new(relative.tlv), relative.tlv));
        }
        for (chain, reading, _) in self.open.values_mut() {
            let mut failed = Vec::new();
            let taken = chain.run(bytes, &mut failed).to_vec();
            reading.push(&taken);
        }
    }

    fn found(&self) -> Vec<Found> {
        self.open
            .iter()
            .flat_map(|(number, (_, reading, tlv))| {
                let tlv = *tlv;
                reading.found().into_iter().map(move |mut one| {
                    one.slot = Some((tlv, *number));
                    one
                })
            })
            .collect()
    }
}
