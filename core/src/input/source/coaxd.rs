// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! coaxd から放送を受ける。[`Source::channels`] が返すのは物理チャンネルで、局名は選局してから
//! SDT で分かる。チューナーが対応を通知した方式のチャンネルだけを返す。

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use coax::daemon::{Daemon, Endpoint, Interrupter};
pub use coax::launch::Launcher;
use coax::{delivery, plan};

use super::{Bytes, Channel, Error, Source, Streamed, Tuner};
use crate::filters::Container;

const ISDB_T: &str = "ISDBT";
const ISDB_S: &str = "ISDBS";
const ISDB_S3: &str = "ISDBS3";

/// 選局の応答を待つ時間。搬送波のロックに数秒かかるチューナーがある。
const PATIENCE: Duration = Duration::from_secs(20);

/// 開いたストリームでデータが途切れてもよい時間。無いと読み取りスレッドが永久に待つ。
const SILENCE: Duration = Duration::from_secs(30);

static LAUNCHER: RwLock<Option<Launcher>> = RwLock::new(None);

pub fn launch_with(launcher: Option<Launcher>) {
    *LAUNCHER.write().unwrap_or_else(|e| e.into_inner()) = launcher;
}

pub use coax::tuning::{TunerSetting, systems_used};

static SETTINGS: RwLock<std::collections::BTreeMap<String, TunerSetting>> =
    RwLock::new(std::collections::BTreeMap::new());

pub fn set_tuner_settings(settings: std::collections::BTreeMap<String, TunerSetting>) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

/// ISDB-S3 をユーザーが指定したチューナー。通知された方式に ISDB-S3 が無いので、要求するときに ISDB-S と
/// シンボルレートに直す（[`with_setting`]）。
static SAID_S3: RwLock<std::collections::BTreeSet<String>> =
    RwLock::new(std::collections::BTreeSet::new());

/// 方式を通知しないチューナーにユーザーが指定した方式を足す。
pub fn declare(tuners: &mut [DaemonTuner]) {
    let settings = SETTINGS.read().unwrap_or_else(|e| e.into_inner());
    let mut said_s3 = SAID_S3.write().unwrap_or_else(|e| e.into_inner());
    for tuner in tuners {
        let Some(setting) = settings.get(&tuner.id) else {
            said_s3.remove(&tuner.id);
            continue;
        };
        let used = systems_used(&tuner.deliveries, setting);
        if coax::tuning::s3_as_s(&tuner.deliveries, &used) {
            said_s3.insert(tuner.id.clone());
        } else {
            said_s3.remove(&tuner.id);
        }
        if tuner.deliveries.is_empty() {
            tuner.deliveries = used;
        } else {
            let extra: Vec<String> = used
                .into_iter()
                .filter(|one| !tuner.deliveries.iter().any(|had| delivery::same(had, one)))
                .collect();
            tuner.deliveries.extend(extra);
        }
    }
}

pub fn declare_boards(boards: &mut [Board]) {
    for board in boards {
        let mut tuners: Vec<DaemonTuner> =
            board.tuners.iter().map(|held| held.tuner.clone()).collect();
        declare(&mut tuners);
        for (held, tuner) in board.tuners.iter_mut().zip(tuners) {
            held.tuner = tuner;
        }
    }
}

pub fn tuner_setting(tuner: &str) -> TunerSetting {
    SETTINGS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(tuner)
        .cloned()
        .unwrap_or_default()
}

pub fn with_setting<'a>(
    asked: &[(&'a str, String)],
    tuner: Option<&str>,
) -> Vec<(&'a str, String)> {
    let mut asking = asked.to_vec();
    let Some(tuner) = tuner else {
        return asking;
    };
    let system = asked
        .iter()
        .find(|(name, _)| *name == "DELIVERY_SYSTEM")
        .map(|(_, system)| system.clone())
        .unwrap_or_default();
    // デーモンが断るので ISDB-S の名前とシンボルレートで要求する。
    let s3_as_s = delivery::same(&system, ISDB_S3)
        && SAID_S3
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(tuner);
    if s3_as_s && let Some(rate) = coax::tuning::ask_s3_as_s(&mut asking) {
        asking.push(rate);
    }
    for (name, value) in tuner_setting(tuner).words(&system) {
        if !asking.iter().any(|(known, _)| *known == name) {
            asking.push((name, value));
        }
    }
    asking
}

pub fn found() -> Option<std::path::PathBuf> {
    LAUNCHER
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .find()
}

pub fn boards(endpoint: &str, priority: i32) -> Result<Vec<coax::daemon::Board>, String> {
    let daemon = Daemon::new(endpoint_of(endpoint)).with_priority(priority);
    reach(&daemon).map_err(|why| why.to_string())?;
    let mut boards = daemon.listing().map_err(|why| explain(&why.to_string()))?;
    declare_boards(&mut boards);
    Ok(boards)
}

pub fn boards_as_told(endpoint: &str, priority: i32) -> Result<Vec<coax::daemon::Board>, String> {
    let daemon = Daemon::new(endpoint_of(endpoint)).with_priority(priority);
    reach(&daemon).map_err(|why| why.to_string())?;
    daemon.listing().map_err(|why| explain(&why.to_string()))
}

pub fn same_system(one: &str, other: &str) -> bool {
    delivery::same(one, other)
}

pub(crate) fn endpoint_of(written: &str) -> Endpoint {
    if written.trim().is_empty() {
        Endpoint::default_endpoint()
    } else {
        Endpoint::parse(written)
    }
}

pub use coax::daemon::{Board, Controls, GainControl, Held, LnbControl, Tuner as DaemonTuner};

/// 応答が無く、この PC のソケットなら起動して待つ。バージョンを返す。
pub fn reach(daemon: &Daemon) -> Result<String, Error> {
    let launcher = LAUNCHER.read().unwrap_or_else(|e| e.into_inner()).clone();
    match launcher {
        Some(launcher) => daemon.ensure_running(&launcher),
        None => daemon.version(),
    }
    .map(|said| version_text(&said))
    .map_err(unreachable)
}

fn unreachable(why: impl fmt::Display) -> Error {
    Error::Unreachable(explain(&why.to_string()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Running {
    pub version: String,
    pub started: String,
    pub arch: String,
    /// ソケットの相手のプロセス。ネットワーク接続や、OS が返さないときは無い。
    pub pid: Option<u32>,
    pub executable: Option<std::path::PathBuf>,
}

pub fn running(endpoint: &str) -> Option<Running> {
    let daemon = Daemon::new(endpoint_of(endpoint));
    let said = daemon.version().ok()?;
    let parsed = coax::json::parse(&said).ok();
    let text = |name: &str| {
        parsed
            .as_ref()
            .and_then(|parsed| parsed.text(name))
            .unwrap_or_default()
            .to_owned()
    };
    let serving = daemon.serving().ok();
    Some(Running {
        version: version_text(&said),
        started: text("started"),
        arch: text("arch"),
        pid: serving.as_ref().map(|serving| serving.pid),
        executable: serving.and_then(|serving| serving.executable),
    })
}

/// coaxd に接続して問い合わせると使用中と数えられ、終了を遅らせるので、接続せずに確かめる。
pub fn still_running(pid: u32) -> bool {
    coax::process::executable_of(pid).is_ok()
}

pub fn version_text(said: &str) -> String {
    let Ok(parsed) = coax::json::parse(said) else {
        return said.trim().to_owned();
    };
    let version = parsed.text("version").unwrap_or_default();
    let commit = parsed.text("commit").unwrap_or_default();
    match (version.is_empty(), commit.is_empty()) {
        (true, true) => said.trim().to_owned(),
        (false, true) => version.to_owned(),
        (true, false) => commit.to_owned(),
        (false, false) => format!("{version}（{commit}）"),
    }
}

/// エラーの JSON はコードを日本語の短い文にし、元のメッセージを括弧で添える。
pub fn explain(text: &str) -> String {
    let (head, said) = explain_apart(text);
    if said.is_empty() {
        head
    } else {
        format!("{head}（{said}）")
    }
}

pub fn refusal_code(text: &str) -> Option<String> {
    let (open, close) = (text.find('{')?, text.rfind('}')?);
    if open >= close {
        return None;
    }
    let parsed = coax::json::parse(&text[open..=close]).ok()?;
    let code = parsed.get("error")?.text("code")?;
    Some(code.to_owned())
}

pub fn explain_apart(text: &str) -> (String, String) {
    if let (Some(open), Some(close)) = (text.find('{'), text.rfind('}'))
        && open < close
        && let Ok(parsed) = coax::json::parse(&text[open..=close])
        && let Some(problem) = parsed.get("error")
    {
        let code = problem.text("code").unwrap_or_default();
        let said: Vec<&str> = [problem.text("message"), problem.text("reason")]
            .into_iter()
            .flatten()
            .filter(|one| !one.is_empty())
            .collect();
        let reason = problem.text("reason").unwrap_or_default();
        let head = match (code, reason) {
            ("unsupported", "lnb_supply") => {
                "このチューナーは LNB 給電をそのとおりにできません（チューナー設定を確かめてください）"
            }
            ("unsupported", "gain") => {
                "このチューナーはそのゲインを設定できません（チューナー設定を確かめてください）"
            }
            // 同期したのに ISDB-S のフレームが無い：ISDB-S3 か、未使用のトランスポンダ。
            ("tune_failed", _) if said.iter().any(|one| one.contains("carries no stream")) => {
                "ISDB-S の放送はありません（ISDB-S3 か、未使用のトランスポンダです）"
            }
            (code, _) => match code {
                "tuner_busy" => "チューナーが使用中です",
                "unknown_tuner" => "そのチューナーが見つかりません",
                "unsupported" => "この放送を受けられるチューナーがありません",
                "tune_failed" => "選局に失敗しました",
                "no_lock" => "信号を受けられません（ロックしない）",
                "no_sync" => "TS が届きません（アンテナ線を確かめてください）",
                "device_gone" => "チューナーが抜かれました",
                "no_card_reader" => "coaxd に PC/SC のカードリーダーがありません",
                "card_error" => "PC/SC のカードリーダーが応答しません",
                "no_devices" => "USB デバイスを読めません",
                "no_firmware" => "ファームウェアがありません",
                "no_access" => "チューナーを開く権限がありません",
                "invalid_parameter" => "選局の指定が読めません",
                "" => "coaxd が断りました",
                other => return (format!("coaxd が断りました（{other}）"), said.join("、")),
            },
        };
        return (head.to_owned(), said.join("、"));
    }
    if let Some(rest) = text.strip_prefix("reach the daemon at ")
        && let Some((at, why)) = rest.split_once(": ")
    {
        return (format!("coaxd（{at}）に届きません: {why}"), String::new());
    }
    if let Some(status) = text.strip_prefix("the daemon answered ") {
        return (format!("coaxd が {status} を返しました"), String::new());
    }
    if let Some(why) = text.strip_prefix("could not read from the daemon: ") {
        return (
            format!("coaxd から読めません（{}）", socket_error(why)),
            String::new(),
        );
    }
    if let Some(why) = text.strip_prefix("could not write to the daemon: ") {
        return (
            format!("coaxd へ書けません（{}）", socket_error(why)),
            String::new(),
        );
    }
    (text.to_owned(), String::new())
}

fn socket_error(why: &str) -> String {
    let said = match why.strip_prefix("winsock error ").map(str::trim) {
        Some("10060") => "応答がなくタイムアウト",
        Some("10054") => "相手が接続を切った",
        Some("10061") => "接続を断られた",
        Some("10053") => "接続が途中で切れた",
        _ => return why.to_owned(),
    };
    format!("{said}、{why}")
}

#[derive(Debug)]
pub struct Coaxd {
    daemon: Daemon,
    /// `read` は相手が応答しないと待ち続けるので、中断するためのハンドル。分割フレームでは複数。
    interrupters: Vec<Interrupter>,
    cut: Arc<AtomicBool>,
    tuner: Option<String>,
    name: String,
    /// 切断直後の「使用中」は待って要求し直す（デーモンがチューナーを解放する前に次の選局が届く）。
    released: Option<Instant>,
    tuners: Vec<String>,
}

type Opened = (coax::http::Response<coax::daemon::Connection>, Interrupter);

const RELEASE: Duration = Duration::from_secs(3);
const RELEASE_POLL: Duration = Duration::from_millis(150);

impl Coaxd {
    /// ここでは接続しない（coaxd が動いていなくても設定を開けるように）。
    pub fn at(endpoint: Endpoint, priority: i32) -> Self {
        let name = match &endpoint {
            Endpoint::Socket(path) => format!("coaxd {path}"),
            Endpoint::Network(address) => format!("coaxd {address}"),
        };
        Self {
            daemon: Daemon::new(endpoint).with_priority(priority),
            interrupters: Vec::new(),
            cut: Arc::new(AtomicBool::new(false)),
            tuner: None,
            name,
            released: None,
            tuners: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_tuners(mut self, tuners: Vec<String>) -> Self {
        self.tuners = tuners;
        self
    }

    pub fn default_endpoint(priority: i32) -> Self {
        Self::at(Endpoint::default_endpoint(), priority)
    }

    pub fn written(endpoint: &str, priority: i32) -> Self {
        Self::at(endpoint_of(endpoint), priority)
    }

    pub fn version(&self) -> Result<String, Error> {
        reach(&self.daemon)
    }

    /// 対応方式を通知しないチューナーが一つでもあれば絞り込まない。
    fn deliveries(&self) -> Result<Option<BTreeSet<String>>, Error> {
        reach(&self.daemon)?;
        let mut tuners = self.daemon.tuners().map_err(unreachable)?;
        declare(&mut tuners);
        // ほかのチューナーの方式を数えると、ISDB-T しか受信しないプロファイルに衛星のチャンネルが並ぶ。
        if !self.tuners.is_empty() {
            tuners.retain(|tuner| self.tuners.iter().any(|pattern| tuner.matches(pattern)));
        }
        if tuners.iter().any(|tuner| tuner.deliveries.is_empty()) {
            return Ok(None);
        }
        Ok(Some(
            tuners
                .iter()
                .flat_map(|tuner| &tuner.deliveries)
                .map(|system| delivery::wire(system))
                .collect(),
        ))
    }
}

/// 切断した直後（[`RELEASE`] の間）の「使用中」だけ待って要求し直す。
fn stream_patiently(
    daemon: &Daemon,
    asked: &[(&str, String)],
    released: Option<Instant>,
) -> std::io::Result<Opened> {
    let mut waited = 0u32;
    loop {
        match daemon.stream_waiting(asked, Some(PATIENCE)) {
            Err(why)
                if why.to_string().contains("tuner_busy")
                    && released.is_some_and(|at| at.elapsed() < RELEASE) =>
            {
                waited += 1;
                std::thread::sleep(RELEASE_POLL);
            }
            answer => {
                if waited > 0 {
                    tracing::info!(
                        "チューナーの解放を待って再要求した（{waited} 回、{} ms）",
                        released.map_or(0, |at| at.elapsed().as_millis())
                    );
                }
                return answer;
            }
        }
    }
}

/// チューナーが限定されていれば、受信できるものを空いている順に指定し、断られたら次を試す。
fn stream_among(
    daemon: &Daemon,
    tuners: &[String],
    asked: &[(&str, String)],
    released: Option<Instant>,
) -> std::io::Result<Opened> {
    if let Some((_, named)) = asked.iter().find(|(name, _)| *name == "tuner") {
        let named = named.clone();
        return stream_patiently(daemon, &with_setting(asked, Some(&named)), released);
    }
    let set = !SETTINGS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty();
    if tuners.is_empty() && !set {
        return stream_patiently(daemon, asked, released);
    }
    let system = asked
        .iter()
        .find(|(name, _)| *name == "DELIVERY_SYSTEM")
        .map(|(_, system)| system.clone())
        .unwrap_or_default();
    let mut listed = daemon.tuners()?;
    declare(&mut listed);
    let candidates = candidates_for(&listed, tuners, &system);
    let mut last = None;
    for candidate in &candidates {
        let mut asking = with_setting(asked, candidate.as_deref());
        if let Some(id) = candidate {
            asking.push(("tuner", id.clone()));
        }
        match stream_patiently(daemon, &asking, released) {
            Ok(opened) => return Ok(opened),
            Err(why) => last = Some(why),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::other(if tuners.is_empty() {
            String::from("この放送に割り当てたチューナーがありません（チューナー個別設定）")
        } else {
            format!(
                "使用できるチューナー（{}）にこの放送を受信できるものがありません",
                tuners.join("、")
            )
        })
    }))
}

fn candidates_for(listed: &[DaemonTuner], tuners: &[String], system: &str) -> Vec<Option<String>> {
    let allowed = |id: &str| tuner_setting(id).allows(system);
    if !tuners.is_empty() {
        return coax::daemon::candidates(listed, tuners, system)
            .into_iter()
            .filter(|candidate| candidate.as_deref().is_none_or(allowed))
            .collect();
    }
    let receiving: Vec<&DaemonTuner> = listed
        .iter()
        .filter(|tuner| tuner.receives(system))
        .collect();
    if receiving.is_empty() {
        return vec![None];
    }
    let (free, busy): (Vec<&DaemonTuner>, Vec<&DaemonTuner>) = receiving
        .into_iter()
        .filter(|tuner| allowed(&tuner.id))
        .partition(|tuner| !tuner.leased);
    free.into_iter()
        .chain(busy)
        .map(|tuner| Some(tuner.id.clone()))
        .collect()
}

fn open_words(
    daemon: &Daemon,
    tuners: &[String],
    words: &str,
    released: Option<Instant>,
    what: &str,
) -> Result<Opened, String> {
    let asked = properties(words);
    if asked.is_empty() {
        return Err(format!("選局の指定が正しくありません: {words}"));
    }
    let asked = for_daemon(&asked);
    let (answer, interrupter) =
        stream_among(daemon, tuners, &asked, released).map_err(|why| explain(&why.to_string()))?;
    if let Err(why) = answer.get_ref().set_read_timeout(SILENCE) {
        tracing::warn!("{what}にタイムアウトを設定できない: {why}");
    }
    Ok((answer, interrupter))
}

fn how(properties: &[(&str, String)]) -> String {
    properties
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn for_daemon<'a>(asked: &[(&'a str, String)]) -> Vec<(&'a str, String)> {
    asked
        .iter()
        .filter(|(name, _)| !name.contains('.'))
        .cloned()
        .collect()
}

fn properties(how: &str) -> Vec<(&str, String)> {
    how.split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            Some((name, value.to_owned()))
        })
        .collect()
}

impl Source for Coaxd {
    fn name(&self) -> &str {
        &self.name
    }

    fn channels(&mut self) -> Result<Vec<Channel>, Error> {
        let received = self.deliveries()?;
        let takes = |system: &str| {
            received
                .as_ref()
                .is_none_or(|received| received.contains(system))
        };
        let mut channels = Vec::new();
        if takes(ISDB_T) {
            // VHF は含めない（放送はもう無い）。
            for carrier in plan::terrestrial(&plan::DEFAULT_BANDS) {
                channels.push(Channel {
                    name: carrier.name.clone(),
                    key: None,
                    order: None,
                    transport: None,
                    how: how(&[
                        ("DELIVERY_SYSTEM", ISDB_T.to_owned()),
                        ("FREQUENCY", carrier.frequency_hz.to_string()),
                    ]),
                    container: Container::Ts,
                });
            }
        }
        for system in [ISDB_S, ISDB_S3] {
            if !takes(system) {
                continue;
            }
            for transponder in plan::satellite() {
                channels.push(Channel {
                    name: transponder.name.clone(),
                    key: None,
                    order: None,
                    transport: None,
                    container: if system == ISDB_S3 {
                        Container::Tlv
                    } else {
                        Container::Ts
                    },
                    how: how(&[
                        ("DELIVERY_SYSTEM", system.to_owned()),
                        ("FREQUENCY", transponder.frequency_khz.to_string()),
                    ]),
                });
            }
        }
        Ok(channels)
    }

    fn open(&mut self, channel: &Channel) -> Result<(Bytes, Container), Error> {
        reach(&self.daemon)?;
        let (answer, interrupter) = open_words(
            &self.daemon,
            &self.tuners,
            &channel.how,
            self.released,
            "ストリーム",
        )
        .map_err(Error::Refused)?;
        self.tuner = answer.header("x-coax-tuner").map(str::to_owned);
        self.interrupters = vec![interrupter];
        self.cut = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "plugins")]
        crate::plugins::lend(Some(Arc::new(Lending {
            daemon: self.daemon.clone(),
            words: channel.how.clone(),
            tuners: self.tuners.clone(),
        })));
        Ok((
            Box::new(Streamed::new(answer, Arc::clone(&self.cut))),
            channel.container,
        ))
    }

    fn tuners(&mut self) -> Result<Vec<Tuner>, Error> {
        reach(&self.daemon)?;
        let mut boards = self.daemon.listing().map_err(unreachable)?;
        declare_boards(&mut boards);
        Ok(boards
            .iter()
            .flat_map(|board| &board.tuners)
            .map(|held| Tuner {
                id: held.tuner.id.clone(),
                name: held.tuner.label(),
                busy: held.tuner.leased,
                locked: held.locked,
                cnr: held.cnr,
            })
            .collect())
    }

    fn cnr(&mut self) -> Option<f32> {
        self.daemon.cnr(self.tuner.as_deref()?).ok().flatten()
    }

    fn interrupt(&mut self) {
        // 閉じてからフラグを立てると、戻った `read` がフラグを見る前にエラーを返す。
        self.cut.store(true, Ordering::Relaxed);
        if !self.interrupters.is_empty() {
            self.released = Some(Instant::now());
        }
        for interrupter in self.interrupters.drain(..) {
            interrupter.interrupt();
        }
        // 保持していると、解放したチューナーの信号を見続けてしまう。
        self.tuner = None;
    }

    fn holding(&self) -> Option<String> {
        self.tuner.clone()
    }
}

#[cfg(feature = "plugins")]
struct Lending {
    daemon: Daemon,
    words: String,
    tuners: Vec<String>,
}

#[cfg(feature = "plugins")]
impl zwsdk::host::tuner::Lender for Lending {
    fn words(&self) -> String {
        self.words.clone()
    }

    fn open(&self, words: &str) -> Result<zwsdk::host::tuner::Lent, String> {
        let (answer, interrupter) = open_words(
            &self.daemon,
            &self.tuners,
            words,
            None,
            "プラグインが開いたストリーム",
        )?;
        let cut = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&cut);
        Ok(zwsdk::host::tuner::Lent {
            bytes: Box::new(Streamed::new(answer, cut)),
            stop: Box::new(move || {
                stopping.store(true, Ordering::Relaxed);
                interrupter.interrupt();
            }),
        })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_said_isdb_s3_is_asked_for_as_isdb_s() {
        let tuner = |id: &str, told: &[&str]| DaemonTuner {
            id: id.to_owned(),
            deliveries: told.iter().map(|one| (*one).to_owned()).collect(),
            leased: false,
            model: String::new(),
            name: String::new(),
        };
        let mut settings = std::collections::BTreeMap::new();
        let both = TunerSetting {
            systems: vec![String::from("ISDB-S"), String::from("ISDB-S3")],
            ..TunerSetting::default()
        };
        settings.insert(String::from("test-dvb:0"), both.clone());
        settings.insert(String::from("test-bda:0"), both.clone());
        settings.insert(String::from("test-sony:0"), both);
        set_tuner_settings(settings);
        let mut tuners = [
            tuner("test-dvb:0", &["ISDB-T", "ISDB-S"]),
            tuner("test-bda:0", &[]),
            tuner("test-sony:0", &["ISDB-S", "ISDB-S3"]),
        ];
        declare(&mut tuners);
        assert!(tuners[0].receives("ISDBS3"), "追加した ISDB-S3");
        assert!(!tuner_setting("test-dvb:0").allows("ISDBT"));
        assert!(tuners[1].receives("ISDBS3"));
        let asked = [
            ("DELIVERY_SYSTEM", String::from("ISDBS3")),
            ("FREQUENCY", String::from("2241660")),
        ];
        for id in ["test-dvb:0", "test-bda:0"] {
            let asking = with_setting(&asked, Some(id));
            assert!(
                asking.contains(&("DELIVERY_SYSTEM", String::from("ISDBS"))),
                "{id}"
            );
            assert!(
                asking.contains(&("SYMBOL_RATE", String::from(coax::tuning::S3_SYMBOL_RATE))),
                "{id}"
            );
        }
        let native = with_setting(&asked, Some("test-sony:0"));
        assert!(native.contains(&("DELIVERY_SYSTEM", String::from("ISDBS3"))));
        assert!(!native.iter().any(|(name, _)| *name == "SYMBOL_RATE"));
    }

    use super::*;

    #[test]
    fn the_words_of_a_tune_survive_a_round_trip() {
        let written = how(&[
            ("DELIVERY_SYSTEM", ISDB_T.to_owned()),
            ("FREQUENCY", "473142857".to_owned()),
        ]);
        assert_eq!(written, "DELIVERY_SYSTEM=ISDBT&FREQUENCY=473142857");
        assert_eq!(
            properties(&written),
            [
                ("DELIVERY_SYSTEM", ISDB_T.to_owned()),
                ("FREQUENCY", "473142857".to_owned()),
            ]
        );
    }

    #[test]
    fn the_daemons_json_is_said_in_words() {
        assert_eq!(
            version_text(
                r#"{"version":"0.1.0","commit":"20260923-sianoring","started":"2026-09-23T11:57:44Z"}"#
            ),
            "0.1.0（20260923-sianoring）"
        );
        assert_eq!(version_text("0.1.0"), "0.1.0", "JSON でなければそのまま");
        assert_eq!(
            explain(
                r#"the daemon refused the channel: 503 Service Unavailable {"error":{"code":"tuner_busy","message":"every tuner is in use"}}"#
            ),
            "チューナーが使用中です（every tuner is in use）"
        );
        assert_eq!(
            explain(
                r#"the daemon refused the channel: 404 {"error":{"code":"unsupported","reason":"ISDB-S3"}}"#
            ),
            "この放送を受けられるチューナーがありません（ISDB-S3）"
        );
        assert_eq!(
            explain(r#"x {"error":{"code":"brand_new","message":"m"}}"#),
            "coaxd が断りました（brand_new）（m）"
        );
        assert_eq!(
            explain("reach the daemon at 192.0.2.16:40772: connection refused"),
            "coaxd（192.0.2.16:40772）に届きません: connection refused"
        );
        assert_eq!(
            explain("the daemon answered 500"),
            "coaxd が 500 を返しました"
        );
        assert_eq!(
            explain("could not read from the daemon: winsock error 10060"),
            "coaxd から読めません（応答がなくタイムアウト、winsock error 10060）"
        );
        assert_eq!(explain("ほかのエラー"), "ほかのエラー");
        assert_eq!(
            refusal_code(r#"the daemon refused the channel: 504 {"error":{"code":"no_lock"}}"#)
                .as_deref(),
            Some("no_lock")
        );
        assert_eq!(refusal_code("the daemon answered 500"), None);
        assert_eq!(
            explain_apart(r#"x {"error":{"code":"tuner_busy","message":"every tuner is in use"}}"#),
            (
                String::from("チューナーが使用中です"),
                String::from("every tuner is in use")
            )
        );
    }

    #[test]
    fn words_for_plugins_are_not_told_to_the_daemon() {
        let asked = properties(
            "DELIVERY_SYSTEM=ISDBC&FREQUENCY=471000000&multiplex.carriers=471000000,477000000&multiplex.part=tlv:1",
        );
        assert_eq!(
            for_daemon(&asked),
            [
                ("DELIVERY_SYSTEM", "ISDBC".to_owned()),
                ("FREQUENCY", "471000000".to_owned()),
            ]
        );
    }

    #[test]
    fn a_word_with_no_value_is_dropped_and_the_rest_read() {
        assert_eq!(
            properties("DELIVERY_SYSTEM=ISDBT&こわれている&FREQUENCY=1"),
            [
                ("DELIVERY_SYSTEM", "ISDBT".to_owned()),
                ("FREQUENCY", "1".to_owned()),
            ]
        );
    }
}
