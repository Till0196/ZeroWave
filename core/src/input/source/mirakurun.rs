// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Mirakurun から放送と番組表を受ける。`/api/programs` の時刻は UNIX エポックからのミリ秒なので、
//! [`crate::guide::Guide::wall_clock`] で実時刻に合わせる。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use coax::daemon::{Connection, Endpoint, Interrupter};
use coax::http;
use coax::json::{self, Value};

use super::{Bytes, Channel, Error, Source, Streamed, Tuner};
use crate::filters::{Component, Container};
use crate::guide::{Genre, Guide, Key, Picture, Programme, Service, Told};

const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// 選局の応答が無いまま切れた。Mirakurun は TS が届くまで応答ヘッダーを送らず、電波が無いと
/// 何も送らずに切る。
pub const NOTHING_CAME: &str =
    "TS を受信できないまま接続が切れました（電波が出ていないか、放送を休止しています）。";

fn unanswered(how: &str, why: std::io::Error) -> Error {
    match why.kind() {
        std::io::ErrorKind::UnexpectedEof
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::WouldBlock => {
            tracing::warn!("{how}: 応答がない（{why}）");
            Error::Refused(NOTHING_CAME.to_owned())
        }
        _ => Error::from(why),
    }
}

/// ストリームの要求に付ける物。設定のソースごとに選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asking {
    /// `decode=1`（Mirakurun の側の外のプログラムに通す）か `decode=0`。省くと通す側になる。
    pub decode: bool,
    /// `X-Mirakurun-Priority`。使用中のチューナーは、それより高い優先度の要求にだけ譲られる。
    pub priority: i32,
}

impl Default for Asking {
    fn default() -> Self {
        Self {
            decode: true,
            priority: 0,
        }
    }
}

/// ホストとパスごとの [`Asking`]。無い Mirakurun は既定。
static ASKING: RwLock<BTreeMap<(String, String), Asking>> = RwLock::new(BTreeMap::new());

/// 設定のソースの URL と選択。変わったら `true`。
pub fn set_asking<'a>(sources: impl IntoIterator<Item = (&'a str, Asking)>) -> bool {
    let wanted: BTreeMap<(String, String), Asking> = sources
        .into_iter()
        .filter_map(|(url, asking)| Some((split(url).ok()?, asking)))
        .collect();
    let mut kept = ASKING.write().unwrap_or_else(|e| e.into_inner());
    if *kept == wanted {
        return false;
    }
    *kept = wanted;
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum By {
    #[default]
    Auto,
    Channel,
    Service,
}

impl By {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "auto" => Self::Auto,
            "channel" | "channels" | "carrier" => Self::Channel,
            "service" | "services" => Self::Service,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub struct Mirakurun {
    host: String,
    by: By,
    root: String,
    name: String,
    /// 選局の前に切断する（チューナーが一つの相手は `503` で断る）。
    open: Option<Interrupter>,
    cut: Arc<AtomicBool>,
}

impl Mirakurun {
    pub fn at(url: &str) -> Result<Self, Error> {
        let (host, root) = split(url)?;
        Ok(Self {
            name: format!("Mirakurun {host}"),
            host,
            root,
            by: By::Auto,
            open: None,
            cut: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn set_by(&mut self, by: By) {
        self.by = by;
    }

    fn ask(&self, path: &str) -> Result<String, Error> {
        let body = self.ask_bytes(path)?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    fn connect(&self) -> Result<Connection, Error> {
        let connection = Connection::to(&Endpoint::Network(self.host.clone()))?;
        connection.set_read_timeout(PATIENCE)?;
        Ok(connection)
    }

    fn ask_bytes(&self, path: &str) -> Result<Vec<u8>, Error> {
        let mut answer = http::get(
            self.connect()?,
            &format!("{}{path}", self.root),
            self.host.as_str(),
            &[],
        )?;
        let body = answer.read_to_end()?;
        if answer.status != 200 {
            return Err(Error::Refused(format!(
                "{path}: {} {}",
                answer.status,
                String::from_utf8_lossy(&body).trim()
            )));
        }
        Ok(body)
    }

    /// 配列の応答を一要素ずつ、届いた分から読む。一週間分の番組を一度にツリーにすると数百 MB になり、
    /// 応答をまるごと持つだけでも数十 MB になる。
    fn read_each(&self, path: &str, mut each: impl FnMut(&Value)) -> Result<(), Error> {
        let mut answer = http::get(
            self.connect()?,
            &format!("{}{path}", self.root),
            self.host.as_str(),
            &[],
        )?;
        if answer.status != 200 {
            let body = answer.read_to_end()?;
            return Err(Error::Refused(format!(
                "{path}: {} {}",
                answer.status,
                String::from_utf8_lossy(&body).trim()
            )));
        }
        let mut broken = None;
        let mut items = Items::default();
        let mut chunk = vec![0u8; 64 << 10];
        let mut parse = |item: &[u8]| {
            let text = String::from_utf8_lossy(item);
            match json::parse(&text) {
                Ok(value) => each(&value),
                Err(why) => {
                    broken.get_or_insert(why);
                }
            }
        };
        let walked = loop {
            let read = answer.read_body(&mut chunk)?;
            if read == 0 {
                break items.finish();
            }
            if let Err(why) = items.feed(&chunk[..read], &mut parse) {
                break Err(why);
            }
        };
        if let Some(why) = broken.or(walked.err()) {
            return Err(Error::Malformed(format!("{path}: {why}")));
        }
        Ok(())
    }

    /// 全局の番組表を [`PART`] 番組ずつ渡す。一度に全部を作ると、受け取った側がまとめ終えるまで二組並ぶ。
    /// 最初の一つに局と局ロゴが入り、後の物にも局は入る（番組の時刻を合わせるため）。
    pub fn guide_in_parts(&mut self, mut part: impl FnMut(Guide)) -> Result<(), Error> {
        let stations = self.services_into_guide(true)?;
        let mut filling = stations.stations();
        let mut first = Some(stations);
        self.read_each("/api/programs", |said| {
            let Some(programme) = programme(said, &filling) else {
                return;
            };
            filling.tell(programme);
            if filling.programme_count() >= PART {
                let fresh = filling.stations();
                let done = std::mem::replace(&mut filling, fresh);
                part(match first.take() {
                    Some(mut first) => {
                        first.merge(done);
                        first
                    }
                    None => done,
                });
            }
        })?;
        part(match first.take() {
            Some(mut first) => {
                first.merge(filling);
                first
            }
            None => filling,
        });
        crate::memory::give_back();
        Ok(())
    }

    fn read(&self, path: &str) -> Result<Value, Error> {
        let body = self.ask(path)?;
        json::parse(&body).map_err(|why| Error::Malformed(format!("{path}: {why}")))
    }

    fn services_into_guide(&self, with_logos: bool) -> Result<Guide, Error> {
        let mut guide = Guide::new();
        let mut fetched: std::collections::BTreeSet<(u16, u16)> = Default::default();
        for service in self.read("/api/services")?.items() {
            let Some(key) = key(service) else {
                continue;
            };
            let logo_id = number(service, "logoId")
                .filter(|id| (0..=0x1ff).contains(id))
                .map(|id| id as u16);
            if with_logos
                && let Some(logo_id) = logo_id
                && flag(service, "hasLogoData").unwrap_or(false)
                && fetched.insert((key.network, logo_id))
                && let Some(id) = number(service, "id")
                && let Ok(png) = self.ask_bytes(&format!("/api/services/{id}/logo"))
                && png.starts_with(b"\x89PNG")
            {
                guide.tell_logo(
                    key.network,
                    crate::guide::Logo {
                        logo_type: 5,
                        logo_id,
                        version: 0,
                        png,
                    },
                );
            }
            // `transportStreamId` が無ければ物理チャンネルの名前から仮の番号を決める（SDT で置き換わる）。
            let told = number(service, "transportStreamId")
                .map(|id| id as u16)
                .filter(|id| *id != 0);
            let transport = told.unwrap_or_else(|| {
                let name = service
                    .get("channel")
                    .and_then(|channel| channel.text("channel"))
                    .unwrap_or_default();
                carrier_stand_in(name)
            });
            guide.tell_service(Service {
                network: key.network,
                transport,
                transport_confirmed: told.is_some(),
                service: key.service,
                name: service.text("name").unwrap_or_default().to_owned(),
                provider: String::new(),
                kind: number(service, "type").unwrap_or_default() as u8,
                scrambled: false,
                remote_key: number(service, "remoteControlKeyId").map(|key| key as u8),
                logo_id,
                simple_logo: None,
            });
        }
        Ok(guide)
    }

    fn carriers(&self) -> Carriers {
        let said = match self.read("/api/channels") {
            Ok(said) => said,
            Err(why) => {
                tracing::warn!(
                    "チャンネル一覧を取得できない（サービスの選択は Mirakurun に任せる）: {why}"
                );
                return Carriers::default();
            }
        };
        let mut carriers = Carriers::default();
        for carrier in said.items() {
            let (Some(kind), Some(where_)) = (carrier.text("type"), carrier.text("channel")) else {
                continue;
            };
            let at = (kind.to_owned(), where_.to_owned());
            for service in items(carrier, "services") {
                if let Some(id) = number(service, "id") {
                    carriers.carrying.insert(id, at.clone());
                    match number(service, "tsmfRelTs") {
                        Some(relative) => {
                            carriers.relative.insert(id, relative);
                        }
                        None => {
                            carriers.relative.remove(&id);
                        }
                    }
                }
            }
        }
        carriers
    }

    fn asking(&self) -> Asking {
        ASKING
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(self.host.clone(), self.root.clone()))
            .copied()
            .unwrap_or_default()
    }

    pub fn version(&self) -> Result<String, Error> {
        let said = self.read("/api/version")?;
        Ok(said.text("current").unwrap_or_default().to_owned())
    }
}

fn split(url: &str) -> Result<(String, String), Error> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], rest[at..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(Error::Refused(format!("URL にホストが無い: {url}")));
    }
    let host = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:40772")
    };
    Ok((host, path.to_owned()))
}

/// 番組の `video`。Mirakurun と DMirakurun でフィールドが違うので全部コピーし、名前を付けるときに使い分ける。
fn picture(said: &Value) -> Option<Picture> {
    let component_type = number(said, "componentType").map(|kind| kind as u8);
    let resolution = said.get("resolution").and_then(Value::as_str);
    let frame_rate = said.get("frameRate").and_then(Value::as_str);
    let transfer = said.get("transferCharacteristics").and_then(Value::as_str);
    if component_type.is_none() && resolution.is_none() {
        return None;
    }
    let (resolution, progressive) = match resolution {
        Some(resolution) => {
            let lines = match resolution.trim_end_matches(['p', 'i']) {
                "180" => 1,
                "240" => 2,
                "480" => 3,
                "720" => 4,
                "1080" => 5,
                "2160" => 6,
                "4320" => 7,
                _ => 0,
            };
            (lines, resolution.ends_with('p'))
        }
        None => (0, false),
    };
    Some(Picture {
        component_type,
        resolution,
        aspect: 0,
        progressive,
        frame_rate: match frame_rate {
            Some("15") => 1,
            Some("23.976") => 2,
            Some("24") => 3,
            Some("25") => 4,
            Some("29.97") => 5,
            Some("30") => 6,
            Some("50") => 7,
            Some("59.94") => 8,
            Some("60") => 9,
            Some("100") => 10,
            Some("119.88") => 11,
            Some("120") => 12,
            _ => 0,
        },
        transfer: match transfer {
            Some("bt709") => 1,
            Some("iec61966") => 2,
            Some("bt2020") => 3,
            Some("bt2100-pq") => 4,
            Some("bt2100-hlg") => 5,
            _ => 0,
        },
    })
}

fn sound(said: &Value) -> Option<Component> {
    let langs: Vec<String> = items(said, "langs")
        .iter()
        .filter_map(|lang| lang.as_str().map(str::to_owned))
        .collect();
    Some(Component {
        kind: number(said, "componentType")? as u8,
        tag: number(said, "componentTag").map(|tag| tag as u16),
        language: langs.first().cloned().unwrap_or_default(),
        second: langs.get(1).cloned(),
        main: flag(said, "isMain").unwrap_or(false),
        name: None,
        quality: 0,
        sampling: match number(said, "samplingRate") {
            Some(16_000) => 1,
            Some(22_050) => 2,
            Some(24_000) => 3,
            Some(32_000) => 5,
            Some(44_100) => 6,
            Some(48_000) => 7,
            _ => 0,
        },
        simulcast: None,
    })
}

fn number(said: &Value, name: &str) -> Option<i64> {
    match said.get(name)? {
        Value::Number(digits) => digits.parse::<f64>().ok().map(|value| value as i64),
        _ => None,
    }
}

fn flag(said: &Value, name: &str) -> Option<bool> {
    said.get(name).and_then(Value::as_bool)
}

fn items<'a>(said: &'a Value, name: &str) -> &'a [Value] {
    said.get(name).map(Value::items).unwrap_or_default()
}

/// 全局の番組表を一度に渡す番組の数（一週間分で 3 万を超える）。
const PART: usize = 4096;

#[cfg(test)]
fn each_item(body: &[u8], mut each: impl FnMut(&[u8])) -> Result<(), String> {
    let mut items = Items::default();
    items.feed(body, &mut each)?;
    items.finish()
}

/// JSON の配列を、届いた分ずつ一要素に切り分ける（応答をまるごと持たない）。要素の途中で切れた分だけ残す。
#[derive(Debug, Default)]
struct Items {
    /// まだ渡していない分。`start` 以降が書きかけの要素。
    pending: Vec<u8>,
    opened: bool,
    closed: bool,
    depth: usize,
    start: Option<usize>,
    in_string: bool,
    escaped: bool,
}

impl Items {
    fn feed(&mut self, chunk: &[u8], each: &mut impl FnMut(&[u8])) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        let from = self.pending.len();
        self.pending.extend_from_slice(chunk);
        let mut at = from;
        while let Some(&byte) = self.pending.get(at) {
            at += 1;
            if !self.opened {
                match byte {
                    b'[' => self.opened = true,
                    byte if byte.is_ascii_whitespace() => {}
                    _ => return Err(String::from("配列ではない")),
                }
                continue;
            }
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => {
                    self.in_string = true;
                    self.start.get_or_insert(at - 1);
                }
                b'[' | b'{' => {
                    self.depth += 1;
                    self.start.get_or_insert(at - 1);
                }
                b']' | b'}' if self.depth > 0 => self.depth -= 1,
                b',' | b']' if self.depth == 0 => {
                    if let Some(begun) = self.start.take() {
                        each(self.pending[begun..at - 1].trim_ascii_end());
                    }
                    if byte == b']' {
                        self.closed = true;
                        self.pending.clear();
                        return Ok(());
                    }
                }
                byte if byte.is_ascii_whitespace() => {}
                _ => {
                    self.start.get_or_insert(at - 1);
                }
            }
        }
        // 渡し終えた分を捨てる。書きかけの要素は先頭へ寄せて残す。
        let keep = self.start.unwrap_or(self.pending.len());
        self.pending.drain(..keep);
        if let Some(start) = self.start.as_mut() {
            *start = 0;
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), String> {
        match (self.opened, self.closed) {
            (_, true) => Ok(()),
            (false, false) => Err(String::from("配列ではない")),
            (true, false) => Err(String::from("配列が閉じていない")),
        }
    }
}

impl Source for Mirakurun {
    fn name(&self) -> &str {
        &self.name
    }

    fn channels(&mut self) -> Result<Vec<Channel>, Error> {
        let said = self.read("/api/services")?;
        let carriers = self.carriers();
        Ok(said
            .items()
            .iter()
            .filter_map(|service| {
                Some(Channel {
                    name: service.text("name").unwrap_or_default().to_owned(),
                    key: key(service),
                    transport: number(service, "transportStreamId")
                        .map(|id| id as u16)
                        .filter(|id| *id != 0),
                    order: number(service, "remoteControlKeyId").map(|key| key as u32),
                    how: how(service, &carriers, self.by)?,
                    container: if four_k(service) {
                        Container::Tlv
                    } else {
                        Container::Ts
                    },
                })
            })
            .collect())
    }

    fn open(&mut self, channel: &Channel) -> Result<(Bytes, Container), Error> {
        #[cfg(feature = "plugins")]
        crate::plugins::lend(None);
        // 前のストリームを開いたまま要求すると `503 Tuner Resource Unavailable`。
        self.interrupt();
        let connection = self.connect()?;
        let interrupter = connection.interrupter().ok();
        let asking = self.asking();
        let priority = asking.priority.to_string();
        let answer = http::get(
            connection,
            &format!("{}{}", self.root, with_decode(&channel.how, asking.decode)),
            self.host.as_str(),
            &[("X-Mirakurun-Priority", priority.as_str())],
        )
        .map_err(|why| unanswered(&channel.how, why))?;
        if answer.status != 200 {
            return Err(Error::Refused(format!(
                "{}: {} {}",
                channel.how, answer.status, answer.reason
            )));
        }
        self.open = interrupter;
        self.cut = Arc::new(AtomicBool::new(false));
        Ok((
            Box::new(Streamed::new(answer, Arc::clone(&self.cut))),
            channel.container,
        ))
    }

    fn interrupt(&mut self) {
        self.cut.store(true, Ordering::Relaxed);
        if let Some(open) = self.open.take() {
            open.interrupt();
        }
    }

    /// 重い（数十 MB）ので、選局のたびには [`Source::guide_of`] を使う。
    fn guide(&mut self) -> Result<Option<Guide>, Error> {
        let mut whole: Option<Guide> = None;
        self.guide_in_parts(|part| match whole.as_mut() {
            Some(whole) => whole.merge(part),
            None => whole = Some(part),
        })?;
        Ok(whole)
    }

    fn guide_of(&mut self, key: Key) -> Result<Option<Guide>, Error> {
        let mut guide = self.services_into_guide(false)?;
        self.read_each(
            &format!(
                "/api/programs?serviceId={}&networkId={}",
                key.service, key.network
            ),
            |said| {
                if let Some(programme) = programme(said, &guide) {
                    guide.tell(programme);
                }
            },
        )?;
        Ok(Some(guide))
    }

    fn tuners(&mut self) -> Result<Vec<Tuner>, Error> {
        let said = self.read("/api/tuners")?;
        Ok(said
            .items()
            .iter()
            .map(|tuner| Tuner {
                id: number(tuner, "index").unwrap_or_default().to_string(),
                name: tuner.text("name").unwrap_or_default().to_owned(),
                busy: flag(tuner, "isUsing").unwrap_or(false),
                locked: None,
                cnr: None,
            })
            .collect())
    }
}

#[derive(Debug, Default)]
struct Carriers {
    carrying: BTreeMap<i64, (String, String)>,
    relative: BTreeMap<i64, i64>,
}

/// 一覧にある局は搬送波ごと受信し、自分で局を選ぶ。局を指定して要求すると、違う搬送波を選局して
/// ヘッダーも返さずに切ることがある。
fn how(service: &Value, carriers: &Carriers, by: By) -> Option<String> {
    let id = number(service, "id")?;
    // 無いと TS に変換する途中で MMT/TLV が崩れる。
    let format = four_k(service).then(|| String::from("format=tlv"));
    let by_service = || {
        Some(stream(
            &format!("/api/services/{id}/stream"),
            [format.clone()],
        ))
    };
    if by == By::Service {
        return by_service();
    }
    if let Some((kind, carrier)) = carriers.carrying.get(&id) {
        let relative = carriers
            .relative
            .get(&id)
            .map(|relative| format!("tsmfRelTs={relative}"));
        return Some(stream(
            &format!("/api/channels/{kind}/{carrier}/stream"),
            [format.clone(), relative],
        ));
    }
    if by == By::Channel
        && let Some(carrier) = service.get("channel")
        && let (Some(kind), Some(where_)) = (carrier.text("type"), carrier.text("channel"))
    {
        return Some(stream(
            &format!("/api/channels/{kind}/{where_}/stream"),
            [format.clone()],
        ));
    }
    by_service()
}

fn stream<const N: usize>(path: &str, query: [Option<String>; N]) -> String {
    joined(path, query.into_iter().flatten().collect())
}

/// 開くときに `decode` を設定に合わせて付ける（一覧と覚えておく `how` は設定に依らない）。
fn with_decode(how: &str, decode: bool) -> String {
    let (path, query) = how.split_once('?').unwrap_or((how, ""));
    let query = std::iter::once(format!("decode={}", u8::from(decode)))
        .chain(
            query
                .split('&')
                .filter(|part| !part.is_empty() && !part.starts_with("decode="))
                .map(str::to_owned),
        )
        .collect();
    joined(path, query)
}

fn joined(path: &str, query: Vec<String>) -> String {
    if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", query.join("&"))
    }
}

fn four_k(service: &Value) -> bool {
    if let Some(kind) = service
        .get("channel")
        .and_then(|channel| channel.text("type"))
    {
        return kind == "BS4K" || kind == "CS4K";
    }
    matches!(number(service, "networkId"), Some(0x000b) | Some(0x000c))
}

fn key(service: &Value) -> Option<Key> {
    Some(Key {
        network: number(service, "networkId")? as u16,
        service: number(service, "serviceId")? as u16,
    })
}

fn carrier_stand_in(name: &str) -> u16 {
    if name.is_empty() {
        return 0;
    }
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let folded = ((hash >> 16) ^ hash) as u16;
    if folded == 0 { 1 } else { folded }
}

fn programme(said: &Value, guide: &Guide) -> Option<Programme> {
    let network = number(said, "networkId")? as u16;
    let service = number(said, "serviceId")? as u16;
    let start = number(said, "startAt").map(|ms| guide.wall_clock(ms.div_euclid(1000)));
    let seconds = number(said, "duration").map(|ms| (ms / 1000) as u32);
    let genres = items(said, "genres")
        .iter()
        .filter_map(|genre| {
            Some(Genre {
                large: number(genre, "lv1")? as u8,
                middle: number(genre, "lv2").unwrap_or_default() as u8,
                user: match (number(genre, "un1"), number(genre, "un2")) {
                    (Some(upper), Some(lower)) => ((upper as u8) << 4) | (lower as u8 & 0x0f),
                    _ => 0xff,
                },
            })
        })
        .collect();
    let extended = match said.get("extended") {
        Some(Value::Object(pairs)) => pairs
            .iter()
            .filter_map(|(heading, value)| Some((heading.clone(), value.as_str()?.to_owned())))
            .collect(),
        _ => Vec::new(),
    };
    Some(Programme {
        service,
        network,
        event: number(said, "eventId").unwrap_or_default() as u16,
        start,
        seconds,
        title: said.text("name").unwrap_or_default().to_owned(),
        detail: said.text("description").unwrap_or_default().to_owned(),
        extended,
        genres,
        video: said.get("video").and_then(picture),
        audios: items(said, "audios").iter().filter_map(sound).collect(),
        free: flag(said, "isFree").unwrap_or(true),
        caption: false,
        details: Default::default(),
        shared: items(said, "relatedItems")
            .iter()
            .filter(|item| item.text("type") == Some("shared"))
            .filter_map(|item| {
                Some((
                    number(item, "serviceId")? as u16,
                    number(item, "eventId")? as u16,
                ))
            })
            .collect(),
        told: Told::Mirakurun,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::Described;

    #[test]
    fn an_array_cut_anywhere_gives_the_same_items() {
        let body = br#" [ {"a":[1,{"b":"x]y"}]}, "q\"]," , 12 ,[ ], null ] "#;
        let mut whole = Vec::new();
        each_item(body, |item| whole.push(item.to_vec())).expect("配列");
        for size in 1..body.len() {
            let mut got = Vec::new();
            let mut items = Items::default();
            for chunk in body.chunks(size) {
                items
                    .feed(chunk, &mut |item: &[u8]| got.push(item.to_vec()))
                    .expect("配列");
            }
            items.finish().expect("閉じている");
            assert_eq!(got, whole, "{size} バイトずつ");
        }
        let mut items = Items::default();
        items.feed(b"[1, 2", &mut |_: &[u8]| {}).unwrap();
        assert!(items.finish().is_err(), "閉じていない");
    }

    #[test]
    fn the_items_of_an_array_come_out_one_by_one() {
        let body = br#" [ {"a":[1,{"b":"x]y"}]}, "q\"]," , 12 ,[ ], null ] "#;
        let mut got = Vec::new();
        each_item(body, |item| {
            got.push(String::from_utf8_lossy(item).into_owned())
        })
        .expect("配列");
        assert_eq!(
            got,
            [
                r#"{"a":[1,{"b":"x]y"}]}"#,
                r#""q\"],""#,
                "12",
                "[ ]",
                "null"
            ]
        );
        assert!(each_item(b"{}", |_| {}).is_err());
        assert!(each_item(b"[1, 2", |_| {}).is_err());
        let mut none = 0;
        each_item(b"[]", |_| none += 1).expect("空の配列");
        assert_eq!(none, 0);
    }

    #[test]
    fn every_way_of_writing_the_url_reaches_the_same_place() {
        for written in [
            "http://mirakurun:40772",
            "http://mirakurun:40772/",
            "mirakurun:40772",
            "http://mirakurun:40772/",
        ] {
            assert_eq!(
                split(written).unwrap(),
                ("mirakurun:40772".to_owned(), String::new()),
                "{written}",
            );
        }
    }

    #[test]
    fn a_stream_cut_before_any_answer_is_nothing_came() {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("the port").port();
        let quiet = std::thread::spawn(move || {
            let (mut asked, _) = listener.accept().expect("a request");
            // 読み残すと RST で切断される。
            let mut request = Vec::new();
            let mut piece = [0u8; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                match asked.read(&mut piece) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&piece[..read]),
                }
            }
        });
        let mut source = Mirakurun::at(&format!("http://127.0.0.1:{port}")).unwrap();
        let channel = Channel {
            name: String::from("ゼロウェーブ教育"),
            key: None,
            order: None,
            transport: None,
            how: String::from("/api/channels/GR/26/stream"),
            container: Container::Ts,
        };
        let why = source.open(&channel).err().expect("refused");
        quiet.join().unwrap();
        assert!(
            matches!(&why, Error::Refused(said) if said == NOTHING_CAME),
            "{why}"
        );
    }

    #[test]
    fn decode_and_priority_follow_the_setting_of_the_source() {
        use std::io::Read as _;
        let asked_for = |decode: bool| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
            let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let heard = std::thread::spawn(move || {
                let (mut asked, _) = listener.accept().expect("a request");
                let mut request = Vec::new();
                let mut piece = [0u8; 1024];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    match asked.read(&mut piece) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&piece[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let first = request.lines().next().unwrap_or_default().to_owned();
                assert!(request.contains("X-Mirakurun-Priority: 3\r\n"), "{request}");
                first
            });
            set_asking([(
                url.as_str(),
                Asking {
                    decode,
                    priority: 3,
                },
            )]);
            let mut source = Mirakurun::at(&url).unwrap();
            let channel = Channel {
                name: String::from("ゼロウェーブ教育"),
                key: None,
                order: None,
                transport: None,
                how: String::from("/api/channels/GR/26/stream"),
                container: Container::Ts,
            };
            let _ = source.open(&channel);
            heard.join().unwrap()
        };
        assert_eq!(
            asked_for(true),
            "GET /api/channels/GR/26/stream?decode=1 HTTP/1.1"
        );
        assert_eq!(
            asked_for(false),
            "GET /api/channels/GR/26/stream?decode=0 HTTP/1.1"
        );
        set_asking([]);
    }

    #[test]
    fn a_url_with_no_port_takes_the_usual_one() {
        assert_eq!(
            split("http://mirakurun").unwrap(),
            ("mirakurun:40772".to_owned(), String::new())
        );
    }

    fn service(said: &str) -> Value {
        json::parse(said).unwrap()
    }

    fn carriers() -> Carriers {
        let mut carriers = Carriers::default();
        for (id, at) in [
            (1033293, ("SKY", "561250")),
            (3233626624, ("GR", "49")),
            (1100221, ("BS4K", "45234")),
        ] {
            carriers
                .carrying
                .insert(id, (at.0.to_owned(), at.1.to_owned()));
        }
        carriers
            .carrying
            .insert(400201, ("BS".to_owned(), "23".to_owned()));
        carriers.relative.insert(400201, 2);
        carriers
    }

    #[test]
    fn a_stream_of_a_multiplexing_frame_is_asked_for_by_its_relative_number() {
        let bs10 = service(r#"{"id": 400201, "channel": {"type": "BS", "channel": "16626"}}"#);
        assert_eq!(
            how(&bs10, &carriers(), By::Auto).unwrap(),
            "/api/channels/BS/23/stream?tsmfRelTs=2"
        );
    }

    #[test]
    fn a_programmes_picture_and_sounds_are_read() {
        let four = service(
            r#"{"type": "mpeg2", "resolution": "2160p", "streamContent": 1, "componentType": 147}"#,
        );
        assert_eq!(
            picture(&four).unwrap().name().as_deref(),
            Some("2160p、アスペクト比16:9 パンベクトルなし")
        );
        let hlg = service(
            r#"{"type": "mpeg2", "resolution": "2160p", "streamContent": 1, "componentType": 0,
                "frameRate": "59.94", "transferCharacteristics": "bt2100-hlg"}"#,
        );
        let hlg = picture(&hlg).unwrap();
        assert_eq!(hlg.hdr(), Some(true));
        assert_eq!(hlg.name().as_deref(), Some("2160p 59.94fps、HDR（HLG）"));
        let two = service(
            r#"{"type": "mpeg2", "resolution": "1080i", "streamContent": 1, "componentType": 179}"#,
        );
        assert_eq!(
            picture(&two).unwrap().name().as_deref(),
            Some("1080i（1125i）、アスペクト比16:9 パンベクトルなし")
        );
        let audio = service(
            r#"{"componentType": 3, "componentTag": 16, "isMain": true, "samplingRate": 48000, "langs": ["jpn"]}"#,
        );
        assert_eq!(
            sound(&audio).unwrap().describe(),
            "日本語　2/0モード（ステレオ）、48kHz、主音声"
        );
    }

    #[test]
    fn a_four_k_service_is_asked_for_as_tlv() {
        let four = service(
            r#"{"id": 1100221, "serviceId": 221, "networkId": 11,
                "channel": {"type": "BS4K", "channel": "45234"}}"#,
        );
        assert_eq!(
            how(&four, &carriers(), By::Auto).unwrap(),
            "/api/channels/BS4K/45234/stream?format=tlv"
        );
        assert_eq!(
            how(&four, &Carriers::default(), By::Auto).unwrap(),
            "/api/services/1100221/stream?format=tlv"
        );
        assert!(four_k(&four));
    }

    #[test]
    fn a_service_the_list_carries_is_taken_by_its_carrier() {
        let terrestrial = service(
            r#"{"id": 3233626624, "serviceId": 26624, "networkId": 32336,
                "channel": {"type": "GR", "channel": "32336"}}"#,
        );
        assert_eq!(
            how(&terrestrial, &carriers(), By::Auto).unwrap(),
            "/api/channels/GR/49/stream"
        );
        assert!(!four_k(&terrestrial));
    }

    #[test]
    fn a_service_naming_a_carrier_nobody_has_is_taken_from_where_it_really_is() {
        let sky = service(
            r#"{"id": 1033293, "serviceId": 33293, "networkId": 10,
                "channel": {"type": "SKY", "channel": "16425"}}"#,
        );
        assert_eq!(
            how(&sky, &carriers(), By::Auto).unwrap(),
            "/api/channels/SKY/561250/stream"
        );
    }

    #[test]
    fn a_service_no_carrier_lists_is_asked_for_on_its_own() {
        let terrestrial = service(
            r#"{"id": 3275101024, "serviceId": 1024, "networkId": 32751,
                "channel": {"type": "GR", "channel": "27"}}"#,
        );
        assert_eq!(
            how(&terrestrial, &carriers(), By::Auto).unwrap(),
            "/api/services/3275101024/stream"
        );
    }

    #[test]
    fn the_way_asked_for_is_followed() {
        let sky = service(
            r#"{"id": 1033293, "serviceId": 33293, "networkId": 10,
                "channel": {"type": "SKY", "channel": "16425"}}"#,
        );
        assert_eq!(
            how(&sky, &carriers(), By::Service).unwrap(),
            "/api/services/1033293/stream"
        );
        assert_eq!(
            how(&sky, &Carriers::default(), By::Channel).unwrap(),
            "/api/channels/SKY/16425/stream"
        );
        assert_eq!(
            how(&sky, &carriers(), By::Channel).unwrap(),
            "/api/channels/SKY/561250/stream"
        );
        assert_eq!(By::parse("channel"), Some(By::Channel));
        assert_eq!(By::parse("service"), Some(By::Service));
        assert_eq!(By::parse("x"), None);
    }

    #[test]
    fn with_no_list_of_carriers_the_service_is_taken_at_its_word() {
        let sky = service(
            r#"{"id": 1033293, "serviceId": 33293, "networkId": 10,
                "channel": {"type": "SKY", "channel": "16425"}}"#,
        );
        assert_eq!(
            how(&sky, &Carriers::default(), By::Auto).unwrap(),
            "/api/services/1033293/stream"
        );
    }

    #[test]
    fn decode_is_put_when_the_stream_is_opened() {
        assert_eq!(
            with_decode("/api/channels/BS/23/stream?tsmfRelTs=2", true),
            "/api/channels/BS/23/stream?decode=1&tsmfRelTs=2"
        );
        assert_eq!(
            with_decode("/api/channels/GR/26/stream", false),
            "/api/channels/GR/26/stream?decode=0"
        );
        assert_eq!(
            with_decode("/api/services/1100221/stream?decode=1&format=tlv", false),
            "/api/services/1100221/stream?decode=0&format=tlv"
        );
        assert_eq!(
            with_decode("/api/services/1033293/stream?decode=1", true),
            "/api/services/1033293/stream?decode=1"
        );
    }

    #[test]
    fn a_path_after_the_host_is_kept() {
        assert_eq!(
            split("http://box:80/mirakurun/").unwrap(),
            ("box:80".to_owned(), "/mirakurun".to_owned())
        );
    }
}
