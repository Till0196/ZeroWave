// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Mirakurun から放送と番組表を受ける。`/api/programs` の時刻は UNIX エポックからのミリ秒なので、
//! [`crate::guide::Guide::wall_clock`] で実時刻に合わせる。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use coax::daemon::{Connection, Endpoint, Interrupter};
use coax::http;
use coax::json::{self, Value};

use super::{Bytes, Channel, Error, Source, Streamed, Tuner};
use crate::filters::{Component, Container};
use crate::guide::{Genre, Guide, Key, Picture, Programme, Service, Told};

const PRIORITY: i32 = 1;

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
    priority: i32,
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
            priority: PRIORITY,
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

    /// 配列の応答を一要素ずつ読む。一週間分の番組を一度にツリーにすると数百 MB になる。
    fn read_each(&self, path: &str, mut each: impl FnMut(&Value)) -> Result<(), Error> {
        let body = self.ask_bytes(path)?;
        let mut broken = None;
        let walked = each_item(&body, |item| {
            let text = String::from_utf8_lossy(item);
            match json::parse(&text) {
                Ok(value) => each(&value),
                Err(why) => {
                    broken.get_or_insert(why);
                }
            }
        });
        if let Some(why) = broken.or(walked.err()) {
            return Err(Error::Malformed(format!("{path}: {why}")));
        }
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

fn each_item(body: &[u8], mut each: impl FnMut(&[u8])) -> Result<(), String> {
    let mut at = 0;
    let skip = |at: &mut usize| {
        while body.get(*at).is_some_and(u8::is_ascii_whitespace) {
            *at += 1;
        }
    };
    skip(&mut at);
    if body.get(at) != Some(&b'[') {
        return Err(String::from("配列ではない"));
    }
    at += 1;
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(&byte) = body.get(at) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            at += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                start.get_or_insert(at);
            }
            b'[' | b'{' => {
                depth += 1;
                start.get_or_insert(at);
            }
            b']' | b'}' if depth > 0 => depth -= 1,
            b',' | b']' if depth == 0 => {
                if let Some(from) = start.take() {
                    each(body[from..at].trim_ascii_end());
                }
                if byte == b']' {
                    return Ok(());
                }
            }
            byte if byte.is_ascii_whitespace() => {}
            _ => {
                start.get_or_insert(at);
            }
        }
        at += 1;
    }
    Err(String::from("配列が閉じていない"))
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
        // 優先度を送らないと、番組表を取得している常駐のクライアントからチューナーを譲ってもらえないことがある。
        let priority = self.priority.to_string();
        let answer = http::get(
            connection,
            &format!("{}{}", self.root, channel.how),
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
        let mut guide = self.services_into_guide(true)?;
        self.read_each("/api/programs", |said| {
            if let Some(programme) = programme(said, &guide) {
                guide.tell(programme);
            }
        })?;
        crate::memory::give_back();
        Ok(Some(guide))
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
    let format = if four_k(service) { "&format=tlv" } else { "" };
    let by_service = || Some(format!("/api/services/{id}/stream?decode=1{format}"));
    if by == By::Service {
        return by_service();
    }
    if let Some((kind, carrier)) = carriers.carrying.get(&id) {
        let relative = carriers
            .relative
            .get(&id)
            .map_or(String::new(), |relative| format!("&tsmfRelTs={relative}"));
        return Some(format!(
            "/api/channels/{kind}/{carrier}/stream?decode=1{format}{relative}"
        ));
    }
    if by == By::Channel
        && let Some(carrier) = service.get("channel")
        && let (Some(kind), Some(where_)) = (carrier.text("type"), carrier.text("channel"))
    {
        return Some(format!(
            "/api/channels/{kind}/{where_}/stream?decode=1{format}"
        ));
    }
    by_service()
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
            how: String::from("/api/channels/GR/26/stream?decode=1"),
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
            "/api/channels/BS/23/stream?decode=1&tsmfRelTs=2"
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
            "/api/channels/BS4K/45234/stream?decode=1&format=tlv"
        );
        assert_eq!(
            how(&four, &Carriers::default(), By::Auto).unwrap(),
            "/api/services/1100221/stream?decode=1&format=tlv"
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
            "/api/channels/GR/49/stream?decode=1"
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
            "/api/channels/SKY/561250/stream?decode=1"
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
            "/api/services/3275101024/stream?decode=1"
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
            "/api/services/1033293/stream?decode=1"
        );
        assert_eq!(
            how(&sky, &Carriers::default(), By::Channel).unwrap(),
            "/api/channels/SKY/16425/stream?decode=1"
        );
        assert_eq!(
            how(&sky, &carriers(), By::Channel).unwrap(),
            "/api/channels/SKY/561250/stream?decode=1"
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
