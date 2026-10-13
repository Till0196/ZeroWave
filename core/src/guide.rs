// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表。放送で届いた番組を時刻順に保持する。
//! TS と MMT/TLV は記述子が違うので、読み込む処理を識別子ではなく、セクションがどちらの伝送で届いたかで分ける。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use isdb::nit;
use isdb::tstables::BroadcastTime;

pub use isdb::descriptors::Component;

#[cfg(feature = "epg-db")]
pub mod db;
pub mod keeper;
pub(crate) mod lines;
pub mod listing;
pub mod logo;
mod names;

pub use lines::{details_lines, region_line};
pub use logo::Logo;
pub use names::{
    Described, airtime, audio_name, civil, date_label, epoch, hhmm, language_name, video_name,
    weekday,
};

/// 映像の伝送形式。TS と MMT の項目を分けて持つ（TS に無い項目を SDR と取り違えないため）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub component_type: Option<u8>,
    /// STD-B60 表 7-48〜7-51 のコード値。0 は指定なし（下も同じ）。
    pub resolution: u8,
    pub aspect: u8,
    pub progressive: bool,
    pub frame_rate: u8,
    pub transfer: u8,
}

impl Picture {
    pub fn of_component_type(component_type: u8) -> Self {
        Self {
            component_type: Some(component_type),
            ..Self::default()
        }
    }

    /// 指定が無いときは `None`（SDR ではなく不明）。
    pub fn hdr(&self) -> Option<bool> {
        match self.transfer {
            0 => None,
            4 | 5 => Some(true),
            _ => Some(false),
        }
    }
}

pub type Moment = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Told {
    Broadcast,
    Mirakurun,
    /// EIT の現在・次。時刻と長さは常に優先し、重なる別の番組は消える。
    Now,
}

/// サービスを一意に指す。SID が一意なのは一つの TS の中だけ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub network: u16,
    pub service: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Band(pub u8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    pub ranges: Vec<(u16, u16)>,
}

impl Network {
    pub fn defaults() -> Vec<Self> {
        let one = |name: &str, ranges: &[(u16, u16)]| Self {
            name: name.to_owned(),
            ranges: ranges.to_vec(),
        };
        vec![
            one("GR", &[(0x7880, 0x7fef)]),
            one("BS", &[(0x0004, 0x0004)]),
            one("CS", &[(0x0006, 0x0007)]),
            one("BS4K", &[(0x000b, 0x000b)]),
            one("CS4K", &[(0x000c, 0x000c)]),
            one("SKP", &[(0x000a, 0x000a)]),
            one("CATV", &[(0xfff0, 0xffff)]),
        ]
    }

    fn holds(&self, network: u16) -> bool {
        in_ranges(&self.ranges, network)
    }
}

fn in_ranges(ranges: &[(u16, u16)], network: u16) -> bool {
    ranges
        .iter()
        .any(|(from, to)| (*from..=*to).contains(&network))
}

static NETWORKS: std::sync::RwLock<Option<Vec<Network>>> = std::sync::RwLock::new(None);

fn with_networks<T>(look: impl FnOnce(&[Network]) -> T) -> T {
    let guard = NETWORKS.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_deref() {
        Some(networks) => look(networks),
        None => look(&Network::defaults()),
    }
}

impl Band {
    pub const OTHER: Self = Self(u8::MAX);

    pub fn set_networks(networks: Vec<Network>) {
        let mut guard = NETWORKS.write().unwrap_or_else(|e| e.into_inner());
        *guard = (!networks.is_empty()).then_some(networks);
    }

    pub fn of(network: u16) -> Self {
        with_networks(|networks| {
            networks
                .iter()
                .position(|candidate| candidate.holds(network))
                .map_or(Self::OTHER, |at| Self(at as u8))
        })
    }

    pub fn is_terrestrial(network: u16) -> bool {
        (0x7880..=0x7fef).contains(&network)
    }

    pub fn name(self) -> String {
        with_networks(|networks| {
            networks
                .get(usize::from(self.0))
                .map_or_else(|| String::from("その他"), |network| network.name.clone())
        })
    }

    pub fn guide_title(self) -> String {
        if self == Self::OTHER {
            return String::from("番組表");
        }
        format!("{}番組表", self.name())
    }

    pub fn all() -> Vec<Self> {
        let mut out: Vec<Self> =
            with_networks(|networks| (0..networks.len()).map(|at| Self(at as u8)).collect());
        out.push(Self::OTHER);
        out
    }

    pub fn ranges(self) -> Vec<(u16, u16)> {
        with_networks(|networks| {
            networks
                .get(usize::from(self.0))
                .map_or_else(Vec::new, |network| network.ranges.clone())
        })
    }
}

static DETAILED: std::sync::RwLock<Option<Vec<(u16, u16)>>> = std::sync::RwLock::new(None);

/// 無効にしたネットワークでも EIT は要求し続ける（BS・CS は一つの TS で全局分を送る、TR-B15）。
#[derive(Debug, Clone, Copy)]
pub struct Detailed;

impl Detailed {
    pub fn set(ranges: Option<Vec<(u16, u16)>>) {
        let mut guard = DETAILED.write().unwrap_or_else(|e| e.into_inner());
        *guard = ranges;
    }

    pub fn reads(network: u16) -> bool {
        let guard = DETAILED.read().unwrap_or_else(|e| e.into_inner());
        guard
            .as_deref()
            .is_none_or(|ranges| in_ranges(ranges, network))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    pub names: BTreeMap<Key, String>,
    pub keys: BTreeMap<Key, u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoOf {
    pub id: Option<u16>,
    pub simple: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Service {
    pub network: u16,
    pub transport: u16,
    pub service: u16,
    /// その TS 自身の SDT actual に載った `transport` か。確定した値は SDT other で上書きしない。
    pub transport_confirmed: bool,
    pub name: String,
    pub provider: String,
    pub kind: u8,
    pub scrambled: bool,
    pub remote_key: Option<u8>,
    pub logo_id: Option<u16>,
    pub simple_logo: Option<String>,
}

impl Service {
    pub fn key(&self) -> Key {
        Key {
            network: self.network,
            service: self.service,
        }
    }

    /// データ放送・ワンセグ・放送局が自局の受信機に向けて出すものは並べない。
    pub fn watchable(&self) -> bool {
        matches!(self.kind, 0x01 | 0x02 | 0xad)
    }

    pub fn band(&self) -> Band {
        Band::of(self.network)
    }

    pub fn badge(&self) -> String {
        match &self.simple_logo {
            Some(text) if !text.is_empty() => text.clone(),
            _ => listing::badge_of(&self.name),
        }
    }

    pub fn order(&self) -> (u8, bool, u16, u16) {
        (
            self.remote_key.unwrap_or(u8::MAX),
            !crate::region::is_home(self.network),
            self.network,
            self.service,
        )
    }
}

/// 番組のジャンル。コンテント記述子の大分類と中分類（ARIB STD-B10 付属 H）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Genre {
    pub large: u8,
    pub middle: u8,
    /// `user_nibble` 二つ。0xE0 は TR-B14 付録 B、0xE1 は TR-B15。
    pub user: u8,
}

impl Genre {
    pub fn new(large: u8, middle: u8) -> Self {
        Self {
            large,
            middle,
            user: 0xff,
        }
    }

    pub fn extended(&self) -> bool {
        self.large == 0xe
    }

    pub fn name(self) -> &'static str {
        match self.large {
            0x0 => "ニュース／報道",
            0x1 => "スポーツ",
            0x2 => "情報／ワイドショー",
            0x3 => "ドラマ",
            0x4 => "音楽",
            0x5 => "バラエティ",
            0x6 => "映画",
            0x7 => "アニメ／特撮",
            0x8 => "ドキュメンタリー／教養",
            0x9 => "劇場／公演",
            0xa => "趣味／教育",
            0xb => "福祉",
            0xe => match (self.middle, self.user >> 4) {
                (0x0, _) => "番組付属情報",
                (0x1, 0x0) => "スポーツ（CS）",
                (0x1, 0x1) => "洋画（CS）",
                (0x1, 0x2) => "邦画（CS）",
                _ => "拡張",
            },
            0xf => "その他",
            _ => "予備",
        }
    }

    pub fn middle_name(self) -> Option<&'static str> {
        if self.large == 0xe {
            return self.extended_name();
        }
        let table: &[&str] = match self.large {
            0x0 => &[
                "定時・総合",
                "天気",
                "特集・ドキュメント",
                "政治・国会",
                "経済・市況",
                "海外・国際",
                "解説",
                "討論・会談",
                "報道特番",
                "ローカル・地域",
                "交通",
            ],
            0x1 => &[
                "スポーツニュース",
                "野球",
                "サッカー",
                "ゴルフ",
                "その他の球技",
                "相撲・格闘技",
                "オリンピック・国際大会",
                "マラソン・陸上・水泳",
                "モータースポーツ",
                "マリン・ウィンタースポーツ",
                "競馬・公営競技",
            ],
            0x2 => &[
                "芸能・ワイドショー",
                "ファッション",
                "暮らし・住まい",
                "健康・医療",
                "ショッピング・通販",
                "グルメ・料理",
                "イベント",
                "番組紹介・お知らせ",
            ],
            0x3 => &["国内ドラマ", "海外ドラマ", "時代劇"],
            0x4 => &[
                "国内ロック・ポップス",
                "海外ロック・ポップス",
                "クラシック・オペラ",
                "ジャズ・フュージョン",
                "歌謡曲・演歌",
                "ライブ・コンサート",
                "ランキング・リクエスト",
                "カラオケ・のど自慢",
                "民謡・邦楽",
                "童謡・キッズ",
                "民族音楽・ワールドミュージック",
            ],
            0x5 => &[
                "クイズ",
                "ゲーム",
                "トークバラエティ",
                "お笑い・コメディ",
                "音楽バラエティ",
                "旅バラエティ",
                "料理バラエティ",
            ],
            0x6 => &["洋画", "邦画", "アニメ"],
            0x7 => &["国内アニメ", "海外アニメ", "特撮"],
            0x8 => &[
                "社会・時事",
                "歴史・紀行",
                "自然・動物・環境",
                "宇宙・科学・医学",
                "カルチャー・伝統文化",
                "文学・文芸",
                "スポーツ",
                "ドキュメンタリー全般",
                "インタビュー・討論",
            ],
            0x9 => &[
                "現代劇・新劇",
                "ミュージカル",
                "ダンス・バレエ",
                "落語・演芸",
                "歌舞伎・古典",
            ],
            0xa => &[
                "旅・釣り・アウトドア",
                "園芸・ペット・手芸",
                "音楽・美術・工芸",
                "囲碁・将棋",
                "麻雀・パチンコ",
                "車・オートバイ",
                "コンピュータ・TVゲーム",
                "会話・語学",
                "幼児・小学生",
                "中学生・高校生",
                "大学生・受験",
                "生涯教育・資格",
                "教育問題",
            ],
            0xb => &[
                "高齢者",
                "障害者",
                "社会福祉",
                "ボランティア",
                "手話",
                "文字（字幕）",
                "音声解説",
            ],
            _ => &[],
        };
        match self.middle {
            0xf if self.large <= 0xb => Some("その他"),
            middle => table.get(usize::from(middle)).copied(),
        }
    }

    fn extended_name(self) -> Option<&'static str> {
        let (upper, lower) = (self.user >> 4, self.user & 0x0f);
        match self.middle {
            0x0 => Some(match (upper, lower) {
                (0x0, 0x0) => "中止の可能性あり",
                (0x0, 0x1) => "延長の可能性あり",
                (0x0, 0x2) => "中断の可能性あり",
                (0x0, 0x3) => "同一シリーズの別話放送の可能性あり",
                (0x0, 0x4) => "未定枠",
                (0x0, 0x5) => "繰り上げの可能性あり",
                (0x1, 0x0) => "中断ニュースあり",
                (0x1, 0x1) => "臨時サービスあり",
                _ => return None,
            }),
            0x1 => {
                let table: &[&str] = match upper {
                    0x0 => &[
                        "テニス",
                        "バスケットボール",
                        "ラグビー",
                        "アメリカンフットボール",
                        "ボクシング",
                        "プロレス",
                    ],
                    0x1 => &[
                        "アクション",
                        "SF／ファンタジー",
                        "コメディー",
                        "サスペンス／ミステリー",
                        "恋愛／ロマンス",
                        "ホラー／スリラー",
                        "ウエスタン",
                        "ドラマ／社会派ドラマ",
                        "アニメーション",
                        "ドキュメンタリー",
                        "アドベンチャー／冒険",
                        "ミュージカル／音楽映画",
                        "ホームドラマ",
                    ],
                    0x2 => &[
                        "アクション",
                        "SF／ファンタジー",
                        "お笑い／コメディー",
                        "サスペンス／ミステリー",
                        "恋愛／ロマンス",
                        "ホラー／スリラー",
                        "青春／学園／アイドル",
                        "任侠／時代劇",
                        "アニメーション",
                        "ドキュメンタリー",
                        "アドベンチャー／冒険",
                        "ミュージカル／音楽映画",
                        "ホームドラマ",
                    ],
                    _ => return None,
                };
                match lower {
                    0xf => Some("その他"),
                    lower => table.get(usize::from(lower)).copied(),
                }
            }
            _ => None,
        }
    }

    pub fn family(self) -> Option<u8> {
        match (self.large, self.middle, self.user >> 4) {
            (0xe, 0x1, 0x0) => Some(0x1),
            (0xe, 0x1, 0x1 | 0x2) => Some(0x6),
            (0xe, _, _) => None,
            (large, _, _) => Some(large),
        }
    }

    pub fn describe(self) -> String {
        match self.middle_name() {
            Some(middle) => format!("{}（{middle}）", self.name()),
            None => self.name().to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programme {
    pub service: u16,
    pub network: u16,
    pub event: u16,
    pub start: Option<Moment>,
    pub seconds: Option<u32>,
    pub title: String,
    pub detail: String,
    pub extended: Vec<(String, String)>,
    pub genres: Vec<Genre>,
    pub video: Option<Picture>,
    pub audios: Vec<Component>,
    pub free: bool,
    /// Mirakurun は返さないので、放送から届くまで偽。
    pub caption: bool,
    pub shared: Vec<(u16, u16)>,
    pub told: Told,
    pub details: Details,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Details {
    pub ratings: Vec<isdb::descriptors::ParentalRating>,
    pub contracts: Vec<isdb::descriptors::CaContract>,
    pub series: Option<isdb::descriptors::Series>,
    pub groups: Option<isdb::descriptors::ComponentGroups>,
    pub region: Option<isdb::descriptors::TargetRegion>,
    pub links: Vec<isdb::descriptors::Hyperlink>,
    pub multimedia: Vec<isdb::descriptors::MultimediaService>,
}

impl Details {
    pub fn minimum_age(&self) -> Option<u8> {
        self.rating()
            .and_then(|rating| isdb::descriptors::rating_age(rating.rating))
    }

    pub(crate) fn rating(&self) -> Option<&isdb::descriptors::ParentalRating> {
        self.ratings
            .iter()
            .find(|rating| rating.country == "JPN")
            .or_else(|| self.ratings.first())
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn fill_from(&mut self, told: Details) {
        if self.ratings.is_empty() {
            self.ratings = told.ratings;
        }
        if self.contracts.is_empty() {
            self.contracts = told.contracts;
        }
        if self.series.is_none() {
            self.series = told.series;
        }
        if self.groups.is_none() {
            self.groups = told.groups;
        }
        if self.region.is_none() {
            self.region = told.region;
        }
        if self.links.is_empty() {
            self.links = told.links;
        }
        if self.multimedia.is_empty() {
            self.multimedia = told.multimedia;
        }
    }
}

impl Programme {
    pub fn scheduled(key: Key, event: u16, start: Option<Moment>, seconds: Option<u32>) -> Self {
        Self {
            service: key.service,
            network: key.network,
            event,
            start,
            seconds,
            title: String::new(),
            detail: String::new(),
            extended: Vec::new(),
            genres: Vec::new(),
            video: None,
            audios: Vec::new(),
            free: true,
            caption: false,
            shared: Vec::new(),
            told: Told::Broadcast,
            details: Details::default(),
        }
    }

    pub fn key(&self) -> Key {
        Key {
            network: self.network,
            service: self.service,
        }
    }

    pub fn genre(&self) -> Option<&Genre> {
        self.genres.iter().find(|genre| genre.family().is_some())
    }

    pub fn audio(&self) -> Option<&Component> {
        self.audios
            .iter()
            .find(|sound| sound.main)
            .or_else(|| self.audios.first())
    }

    pub fn ends(&self) -> Option<Moment> {
        Some(self.start? + i64::from(self.seconds?))
    }

    /// マージする。現在・次はタイトルだけ、スケジュールは説明だけのことがあるので空でないものを優先する。
    fn absorb(&mut self, told: Programme) {
        if self.event != told.event {
            *self = told;
            return;
        }
        if !told.title.is_empty() {
            self.title = told.title;
        }
        if !told.detail.is_empty() {
            self.detail = told.detail;
        }
        if !told.extended.is_empty() {
            self.extended = told.extended;
        }
        if !told.genres.is_empty() {
            self.genres = told.genres;
        }
        self.video = told.video.or(self.video);
        if !told.audios.is_empty() {
            self.audios = told.audios;
        }
        self.seconds = told.seconds.or(self.seconds);
        self.free = told.free;
        self.caption |= told.caption;
        if !told.shared.is_empty() {
            self.shared = told.shared;
        }
        if !told.details.is_empty() {
            let mut details = told.details;
            details.fill_from(std::mem::take(&mut self.details));
            self.details = details;
        }
        // Mirakurun のままだと後から届く情報が時刻とタイトルを戻し、視聴中の番組がちらつく。
        self.told = told.told;
    }

    fn fill_from(&mut self, told: Programme) {
        if self.title.is_empty() {
            self.title = told.title;
        }
        if self.detail.is_empty() {
            self.detail = told.detail;
        }
        if self.extended.is_empty() {
            self.extended = told.extended;
        }
        if self.genres.is_empty() {
            self.genres = told.genres;
        }
        if self.video.is_none() {
            self.video = told.video;
        }
        if self.audios.is_empty() {
            self.audios = told.audios;
        }
        if self.seconds.is_none() {
            self.seconds = told.seconds;
        }
        if self.shared.is_empty() {
            self.shared = told.shared;
        }
        self.details.fill_from(told.details);
    }

    pub fn at(&self, moment: Moment) -> bool {
        let Some(start) = self.start else {
            return false;
        };
        match self.ends() {
            Some(ends) => (start..ends).contains(&moment),
            None => start <= moment,
        }
    }
}

/// 一つのサービスのスケジュールの受信状況（STD-B10 付録 1.4.2.1）。終わりは `last_table_id`、
/// 32 区分で空の区分も来る、過ぎた区分は来ない、バージョンが上がっても記録は捨てない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Arrival {
    tables: BTreeMap<u8, TableArrival>,
    last_table: BTreeMap<u8, u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TableArrival {
    version: u8,
    seen: BTreeSet<u8>,
    segments: BTreeMap<u8, u8>,
}

const SEGMENT_SECONDS: i64 = 3 * 3600;

impl TableArrival {
    fn filled(&self, first: bool, now: Moment) -> (usize, usize) {
        let passed = if first {
            (now.rem_euclid(86_400) / SEGMENT_SECONDS) as usize
        } else {
            0
        };
        let mut wanted = 0;
        let mut arrived = 0;
        for segment in 0..32usize {
            let base = (segment * 8) as u8;
            let expected = match self.segments.get(&(segment as u8)) {
                Some(last) => usize::from(last.saturating_sub(base)) + 1,
                None if segment < passed => 0,
                None => 1,
            };
            let seen = self
                .seen
                .range(base..base.saturating_add(8))
                .count()
                .min(expected);
            wanted += expected;
            arrived += seen;
        }
        (arrived, wanted)
    }
}

impl Arrival {
    fn absorb(&mut self, other: Arrival) {
        for (table_id, theirs) in other.tables {
            match self.tables.get_mut(&table_id) {
                Some(mine) if mine.version == theirs.version => {
                    mine.seen.extend(theirs.seen);
                    mine.segments.extend(theirs.segments);
                }
                _ => {
                    self.tables.insert(table_id, theirs);
                }
            }
        }
        self.last_table.extend(other.last_table);
    }

    fn seen(&mut self, version: u8, table_id: u8, number: u8, segment_last: u8, last_table: u8) {
        let table = self.tables.entry(table_id).or_default();
        table.version = version;
        table.seen.insert(number);
        table.segments.insert(number / 8, segment_last);
        self.last_table.insert(table_id & 0xf8, last_table);
    }

    pub(crate) fn complete(&self, now: Moment) -> bool {
        let (arrived, wanted) = self.filled(now);
        wanted > 0 && arrived >= wanted
    }

    fn filled(&self, now: Moment) -> (usize, usize) {
        let empty = TableArrival::default();
        let mut total = (0, 0);
        for (first_id, last_id) in &self.last_table {
            for table_id in *first_id..=(*last_id).max(*first_id) {
                let table = self.tables.get(&table_id).unwrap_or(&empty);
                let (arrived, wanted) = table.filled(table_id == *first_id, now);
                total.0 += arrived;
                total.1 += wanted;
            }
        }
        total
    }
}

#[derive(Debug, Clone)]
pub struct Guide {
    services: BTreeMap<Key, Service>,
    /// 開始時刻をキーにする（イベント ID だと時刻が変わった番組が二重に並ぶ）。
    programmes: BTreeMap<Key, BTreeMap<Moment, Programme>>,
    arrivals: BTreeMap<Key, Arrival>,
    logos: BTreeMap<(u16, u8, u16), Arc<Logo>>,
    /// 見える画素が一つも無いロゴ（受け取ったときに調べる）。表示では無いものとして扱う。
    blank_logos: std::collections::BTreeSet<(u16, u8, u16)>,
    now: Option<(Moment, std::time::Instant)>,
    /// 放送の時刻と UTC の差、秒。Mirakurun（UTC）の時刻をこれで合わせる。
    offset: i32,
    revision: u64,
    sections: std::collections::HashMap<(Key, u16, u8), u8>,
}

pub const JST_OFFSET: i32 = 9 * 3600;

/// BS のリモコン番号は放送に含まれないので、サービス番号のテーブルで決める。
pub fn default_remote_key(network: u16, service: u16) -> Option<u8> {
    const BS: [(u16, u16, u8); 11] = [
        (101, 2, 1),
        (141, 3, 4),
        (151, 3, 5),
        (161, 3, 6),
        (171, 3, 7),
        (181, 3, 8),
        (191, 3, 9),
        (200, 3, 10),
        (211, 3, 11),
        (222, 1, 12),
        (231, 3, 3),
    ];
    if network != 0x0004 {
        return None;
    }
    BS.iter()
        .find(|(base, count, _)| (*base..base + count).contains(&service))
        .map(|(_, _, key)| *key)
}

impl Default for Guide {
    fn default() -> Self {
        Self {
            services: BTreeMap::new(),
            programmes: BTreeMap::new(),
            arrivals: BTreeMap::new(),
            logos: BTreeMap::new(),
            blank_logos: std::collections::BTreeSet::new(),
            now: None,
            offset: JST_OFFSET,
            revision: 0,
            sections: std::collections::HashMap::new(),
        }
    }
}

impl Guide {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// `table` は table_id（部分受信のテーブルは 0x100 を足して H-EIT と分ける）。
    pub(crate) fn seen_section(&mut self, key: Key, table: u16, number: u8, version: u8) -> bool {
        self.sections.insert((key, table, number), version) == Some(version)
    }

    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.services.values()
    }

    pub fn channels(&self) -> impl Iterator<Item = &Service> {
        self.services.values().filter(|service| service.watchable())
    }

    pub fn apply_prefs(&mut self, prefs: &Prefs) {
        self.revision += 1;
        for (key, name) in &prefs.names {
            if let Some(service) = self.services.get_mut(key) {
                service.name = name.clone();
            }
        }
        for (key, remote) in &prefs.keys {
            if let Some(service) = self.services.get_mut(key) {
                service.remote_key = Some(*remote);
            }
        }
    }

    pub fn service(&self, key: Key) -> Option<&Service> {
        self.services.get(&key)
    }

    pub fn programmes(&self, key: Key) -> impl Iterator<Item = &Programme> {
        self.programmes
            .get(&key)
            .into_iter()
            .flat_map(|by_time| by_time.values())
    }

    pub fn at(&self, key: Key, moment: Moment) -> Option<&Programme> {
        self.programmes
            .get(&key)?
            .range(..=moment)
            .next_back()
            .map(|(_, programme)| programme)
            .filter(|programme| programme.at(moment))
    }

    pub fn after(&self, key: Key, moment: Moment) -> Option<&Programme> {
        let current = self.at(key, moment);
        self.programmes
            .get(&key)?
            .range(moment + 1..)
            .map(|(_, programme)| programme)
            .find(|programme| current.is_none_or(|now| now.event != programme.event))
    }

    pub fn now(&self) -> Option<Moment> {
        self.now
            .map(|(moment, heard)| moment + heard.elapsed().as_secs() as i64)
    }

    pub fn machine_now(&self) -> Moment {
        use std::time::{SystemTime, UNIX_EPOCH};

        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or_default();
        since + i64::from(self.offset)
    }

    pub fn now_or_machine(&self) -> Moment {
        self.now().unwrap_or_else(|| self.machine_now())
    }

    pub fn offset(&self) -> i32 {
        self.offset
    }

    pub fn set_offset(&mut self, seconds: i32) {
        self.offset = seconds;
    }

    pub fn wall_clock(&self, utc: i64) -> Moment {
        utc + i64::from(self.offset)
    }

    pub fn complete(&self, key: Key) -> bool {
        let now = self.now_or_machine();
        self.arrivals
            .get(&key)
            .is_some_and(|arrival| arrival.complete(now))
    }

    pub fn filled(&self, key: Key) -> (usize, usize) {
        let now = self.now_or_machine();
        self.arrivals
            .get(&key)
            .map(|arrival| arrival.filled(now))
            .unwrap_or_default()
    }

    pub fn tell_service(&mut self, told: Service) {
        let entry = self.services.entry(told.key()).or_default();
        let remote_key = entry
            .remote_key
            .or(told.remote_key)
            .or_else(|| default_remote_key(told.network, told.service));
        let logo_id = told.logo_id.or(entry.logo_id);
        let simple_logo = told
            .simple_logo
            .clone()
            .or_else(|| entry.simple_logo.clone());
        let (transport, transport_confirmed) = if told.transport == 0 {
            (entry.transport, entry.transport_confirmed)
        } else if told.transport_confirmed {
            (told.transport, true)
        } else if entry.transport == 0 {
            (told.transport, false)
        } else {
            (entry.transport, entry.transport_confirmed)
        };
        let fresh = Service {
            remote_key,
            transport,
            transport_confirmed,
            logo_id,
            simple_logo,
            ..told
        };
        if *entry != fresh {
            *entry = fresh;
            self.revision += 1;
        }
    }

    fn service_entry(&mut self, key: Key) -> &mut Service {
        self.services.entry(key).or_insert_with(|| Service {
            network: key.network,
            service: key.service,
            ..Service::default()
        })
    }

    pub fn tell_simple_logo(&mut self, key: Key, text: String) {
        let entry = self.service_entry(key);
        if entry.simple_logo.as_ref() != Some(&text) {
            entry.simple_logo = Some(text);
            self.revision += 1;
        }
    }

    pub fn tell_logo_id(&mut self, key: Key, logo_id: u16) {
        let entry = self.service_entry(key);
        if entry.logo_id != Some(logo_id) {
            entry.logo_id = Some(logo_id);
            self.revision += 1;
        }
    }

    /// 新しいバージョンか、同じでも内容が違えば取る（BS/CS は一局だけ差し替えることがある）。
    pub fn tell_logo(&mut self, network: u16, logo: Logo) {
        self.take_logo(network, Arc::new(logo));
    }

    pub fn learn_logos(&mut self, from: &Self) {
        for ((network, _, _), logo) in &from.logos {
            self.take_logo(*network, Arc::clone(logo));
        }
    }

    fn take_logo(&mut self, network: u16, logo: Arc<Logo>) {
        let key = (network, logo.logo_type, logo.logo_id);
        let take = match self.logos.get(&key) {
            None => true,
            Some(had) => match logo::compare_versions(logo.version, had.version) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => had.png != logo.png,
                std::cmp::Ordering::Less => false,
            },
        };
        if take {
            if logo.visible() {
                self.blank_logos.remove(&key);
            } else {
                self.blank_logos.insert(key);
            }
            self.logos.insert(key, logo);
            self.revision += 1;
        }
    }

    pub fn logo_map(&self) -> BTreeMap<Key, LogoOf> {
        self.services
            .iter()
            .filter(|(_, service)| service.logo_id.is_some() || service.simple_logo.is_some())
            .map(|(key, service)| {
                (
                    *key,
                    LogoOf {
                        id: service.logo_id,
                        simple: service.simple_logo.clone(),
                    },
                )
            })
            .collect()
    }

    pub fn fill_logos(&mut self, map: &BTreeMap<Key, LogoOf>) {
        self.revision += 1;
        for (key, service) in &mut self.services {
            let Some(known) = map.get(key) else {
                continue;
            };
            if service.logo_id.is_none() {
                service.logo_id = known.id;
            }
            if service.simple_logo.is_none() {
                service.simple_logo.clone_from(&known.simple);
            }
        }
    }

    pub fn lend_logos(&mut self, from: &Self) {
        self.revision += 1;
        for (place, logo) in &from.logos {
            if self.logos.contains_key(place) {
                continue;
            }
            self.logos.insert(*place, Arc::clone(logo));
            if from.blank_logos.contains(place) {
                self.blank_logos.insert(*place);
            }
        }
    }

    /// 見える画素の無いロゴは `None`（局名のバッジを出す）。サブチャンネルなどに透明のロゴを割り当てる放送がある。
    pub fn logo(&self, key: Key, logo_type: u8) -> Option<&Arc<Logo>> {
        let logo_id = self.services.get(&key)?.logo_id?;
        let place = (key.network, logo_type, logo_id);
        if self.blank_logos.contains(&place) {
            return None;
        }
        self.logos.get(&place)
    }

    pub fn logo_of(&self, key: Key) -> Option<&Arc<Logo>> {
        logo::PREFERRED
            .iter()
            .find_map(|logo_type| self.logo(key, *logo_type))
    }

    pub fn logos(&self) -> impl Iterator<Item = (u16, &Arc<Logo>)> {
        self.logos
            .iter()
            .map(|((network, _, _), logo)| (*network, logo))
    }

    pub fn tell_listed(&mut self, key: Key, kind: u8, transport: u16) {
        let entry = self.service_entry(key);
        let was = (entry.kind, entry.transport, entry.remote_key);
        if entry.kind == 0 {
            entry.kind = kind;
        }
        if entry.transport == 0 {
            entry.transport = transport;
        }
        if entry.remote_key.is_none() {
            entry.remote_key = default_remote_key(key.network, key.service);
        }
        if was != (entry.kind, entry.transport, entry.remote_key) {
            self.revision += 1;
        }
    }

    pub fn tell_remote_key(&mut self, key: Key, remote_key: u8) {
        let entry = self.service_entry(key);
        if entry.remote_key != Some(remote_key) {
            entry.remote_key = Some(remote_key);
            self.revision += 1;
        }
    }

    pub fn tell_time(&mut self, moment: Moment) {
        self.now = Some((moment, std::time::Instant::now()));
    }

    pub fn forget_time(&mut self) {
        self.now = None;
    }

    pub fn merge(&mut self, other: Guide) {
        self.revision += 1;
        for (key, service) in other.services {
            let known = self.services.get(&key);
            if known.is_some_and(|known| !known.name.is_empty()) && service.name.is_empty() {
                continue;
            }
            self.tell_service(service);
        }
        for (_, by_time) in other.programmes {
            for (_, programme) in by_time {
                self.tell(programme);
            }
        }
        for (key, arrival) in other.arrivals {
            self.arrivals.entry(key).or_default().absorb(arrival);
        }
        for ((network, _, _), logo) in other.logos {
            self.tell_logo(network, Arc::unwrap_or_clone(logo));
        }
    }

    pub fn programme_count(&self) -> usize {
        self.programmes.values().map(BTreeMap::len).sum()
    }

    pub fn tell(&mut self, programme: Programme) {
        self.revision += 1;
        let Some(start) = programme.start else {
            return;
        };
        let by_time = self.programmes.entry(programme.key()).or_default();
        // 現在・次の番組は固定する（古い編成で入れ替わってちらつくため）。
        if programme.told != Told::Now
            && by_time.values().any(|known| {
                known.told == Told::Now
                    && known.start != Some(start)
                    && (known.event == programme.event || overlaps(known, &programme))
            })
        {
            return;
        }
        // 重なる番組を消すのは現在・次だけ（ほかは届く順で正しい番組を消しかねない）。
        if programme.told == Told::Now
            && let Some(ends) = programme.ends()
        {
            let overlapped: Vec<Moment> = by_time
                .iter()
                .filter(|(at, known)| {
                    known.event != programme.event
                        && **at < ends
                        && known.ends().is_none_or(|known_ends| known_ends > start)
                })
                .map(|(at, _)| *at)
                .collect();
            for at in overlapped {
                by_time.remove(&at);
            }
        }
        match by_time.get_mut(&start) {
            Some(known) => {
                // 放送からの情報を優先する。Mirakurun は空いた項目だけ足し、スケジュールは Mirakurun を上書きしない。
                match (known.told, programme.told) {
                    (Told::Mirakurun, Told::Broadcast) => return,
                    (Told::Now, Told::Broadcast | Told::Mirakurun)
                        if known.event != programme.event =>
                    {
                        return;
                    }
                    (Told::Broadcast | Told::Now, Told::Mirakurun)
                    | (Told::Now, Told::Broadcast) => {
                        known.fill_from(programme);
                        return;
                    }
                    _ => {}
                }
                known.absorb(programme);
            }
            None => {
                let moved: Vec<Moment> = by_time
                    .iter()
                    .filter(|(_, known)| known.event == programme.event)
                    .map(|(at, _)| *at)
                    .collect();
                for at in moved {
                    by_time.remove(&at);
                }
                by_time.insert(start, programme);
            }
        }
    }

    /// 過去の番組を規則を通さず置く（イベント ID の再利用で今日の番組を消さないため）。
    pub fn tell_past(&mut self, programme: Programme) {
        self.revision += 1;
        let Some(start) = programme.start else {
            return;
        };
        self.programmes
            .entry(programme.key())
            .or_default()
            .entry(start)
            .or_insert(programme);
    }

    pub fn forget_before(&mut self, moment: Moment) {
        let before = self.programme_count();
        for by_time in self.programmes.values_mut() {
            by_time.retain(|start, programme| programme.ends().unwrap_or(*start + 86_400) > moment);
        }
        self.programmes.retain(|_, by_time| !by_time.is_empty());
        if self.programme_count() != before {
            self.revision += 1;
        }
    }

    pub(crate) fn tell_arrival(
        &mut self,
        key: Key,
        version: u8,
        table_id: u8,
        number: u8,
        segment_last: u8,
        last_table: u8,
    ) {
        self.arrivals.entry(key).or_default().seen(
            version,
            table_id,
            number,
            segment_last,
            last_table,
        );
    }
}

fn overlaps(a: &Programme, b: &Programme) -> bool {
    let (Some(a_start), Some(a_ends), Some(b_start), Some(b_ends)) =
        (a.start, a.ends(), b.start, b.ends())
    else {
        return false;
    };
    a_start < b_ends && b_start < a_ends
}

impl Guide {
    pub fn on_network_information(&mut self, read: &nit::Described) {
        let network = read.network.id;
        let buttons: Vec<(Key, u8)> = read
            .transports
            .iter()
            .filter(|transport| transport.remote_control_key_id != 0)
            .flat_map(|transport| {
                transport.services.iter().map(move |service| {
                    (
                        Key {
                            network: transport.original_network_id,
                            service: service.id,
                        },
                        transport.remote_control_key_id,
                    )
                })
            })
            .chain(read.remote_keys.iter().map(|button| {
                (
                    Key {
                        network,
                        service: button.service_id,
                    },
                    button.remote_control_key_id,
                )
            }))
            .collect();
        for (key, button) in buttons {
            self.tell_remote_key(key, button);
        }
        let listed: Vec<(Key, u8, u16)> = read
            .transports
            .iter()
            .flat_map(|transport| {
                transport.services.iter().map(move |service| {
                    (
                        Key {
                            network: transport.original_network_id,
                            service: service.id,
                        },
                        service.kind,
                        transport.stream_id,
                    )
                })
            })
            .collect();
        for (key, kind, transport) in listed {
            self.tell_listed(key, kind, transport);
        }
    }
}

#[cfg(test)]
mod tests;
