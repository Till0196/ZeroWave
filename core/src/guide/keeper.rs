// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表のスレッド。マージと一覧の組み立て（数百 ms〜秒かかる）はここでだけ行う。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::listing::{Asked, Channel, Hit, Listing, Search};
use super::{Band, Guide, Key, LogoOf, Moment, Prefs, Programme};

/// EIT は数十秒の周期で送られるので、これより細かくしても意味が無い。
const SAVE_EVERY: Duration = Duration::from_secs(5 * 60);
const OPEN_POLL: Duration = Duration::from_millis(200);
const FORGET_EVERY: Duration = Duration::from_secs(60 * 60);
/// メモリに保持する過去の範囲。番組表の先頭（一時間前の正時）を含む三時間。
const MEMORY_BEHIND: i64 = 3 * 3600;
#[cfg(feature = "epg-db")]
const KEEP_IN_FILE: i64 = 7 * 24 * 3600;
const MOST_FOUND: usize = 500;

struct Store {
    path: Option<PathBuf>,
    #[cfg(feature = "epg-db")]
    connection: Option<rusqlite::Connection>,
    #[cfg(feature = "epg-db")]
    known: super::db::Known,
}

impl Store {
    fn logo_folder(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.with_file_name("logos"))
    }

    fn load_logos(&self, guide: &mut Guide) {
        if let Some(folder) = self.logo_folder() {
            for (network, logo) in super::logo::load_folder(&folder) {
                guide.tell_logo(network, logo);
            }
        }
    }

    fn save_logos(&mut self, guide: &Guide) {
        if let Some(folder) = self.logo_folder() {
            let logos: Vec<(u16, &super::Logo)> = guide
                .logos()
                .map(|(network, logo)| (network, &**logo))
                .collect();
            let written = super::logo::save_folder(&folder, &logos);
            if written > 0 {
                tracing::info!("局ロゴを {written} 枚保存した: {}", folder.display());
            }
        }
    }
}

#[cfg(feature = "epg-db")]
impl Store {
    /// `keep` が偽なら番組はメモリーにだけ置き、ファイルは読み書きしない（局ロゴは `path` の隣に残す）。
    fn open(path: Option<PathBuf>, keep: bool) -> Self {
        let connection = match (&path, keep) {
            (Some(path), true) => match super::db::open(path) {
                Ok(connection) => Some(connection),
                Err(error) => {
                    tracing::warn!("番組表 {} を開けない: {error}", path.display());
                    None
                }
            },
            (Some(_), false) => match super::db::open_in_memory() {
                Ok(connection) => Some(connection),
                Err(error) => {
                    tracing::warn!("番組表をメモリーに作れない: {error}");
                    None
                }
            },
            (None, _) => None,
        };
        Self {
            path,
            connection,
            known: super::db::Known::new(),
        }
    }

    fn load(&mut self, after: Moment) -> Guide {
        let (Some(path), Some(connection)) = (&self.path, &self.connection) else {
            return Guide::new();
        };
        let mut guide = match super::db::load_after(connection, after) {
            Ok((guide, known)) => {
                self.known = known;
                guide
            }
            Err(error) => {
                tracing::warn!("番組表 {} を読み込めない: {error}", path.display());
                Guide::new()
            }
        };
        self.load_logos(&mut guide);
        tracing::info!(
            "番組表 {}: {} 局、{} 番組、局ロゴ {} 枚を読み込んだ",
            path.display(),
            guide.channels().count(),
            guide.programme_count(),
            guide.logos().count()
        );
        guide
    }

    fn save(&mut self, guide: &Guide, kept_from: Moment) {
        let (Some(path), Some(connection)) = (&self.path, &mut self.connection) else {
            return;
        };
        let keep_since = guide.now_or_machine().min(guide.machine_now()) - KEEP_IN_FILE;
        let began = Instant::now();
        match super::db::save(connection, guide, &mut self.known, kept_from, keep_since) {
            Ok(saved) if saved != super::db::Saved::default() => tracing::debug!(
                "番組表を保存: 書き込み {}、削除 {} 番組（{} ms）",
                saved.written,
                saved.dropped,
                began.elapsed().as_millis()
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!("番組表を保存できない {}: {error}", path.display()),
        }
    }

    fn past(&self, from: Moment, until: Moment, before: Moment) -> Vec<Programme> {
        let Some(connection) = &self.connection else {
            return Vec::new();
        };
        super::db::load_range(connection, from, until, before).unwrap_or_else(|error| {
            tracing::warn!("過去の番組表を読み込めない: {error}");
            Vec::new()
        })
    }

    fn search(&self, filter: &super::db::Filter, most: usize) -> Vec<Programme> {
        let Some(connection) = &self.connection else {
            return Vec::new();
        };
        super::db::search(connection, filter, most).unwrap_or_else(|error| {
            tracing::warn!("番組表を検索できない: {error}");
            Vec::new()
        })
    }

    fn count(&self, filter: &super::db::Filter) -> Vec<(u16, usize)> {
        let Some(connection) = &self.connection else {
            return Vec::new();
        };
        super::db::count_by_network(connection, filter).unwrap_or_else(|error| {
            tracing::warn!("番組表の件数を取得できない: {error}");
            Vec::new()
        })
    }
}

#[cfg(not(feature = "epg-db"))]
impl Store {
    fn open(path: Option<PathBuf>, _: bool) -> Self {
        Self { path }
    }
    fn load(&mut self, _: Moment) -> Guide {
        let mut guide = Guide::new();
        self.load_logos(&mut guide);
        guide
    }
    fn save(&mut self, _: &Guide, _: Moment) {}
    fn past(&self, _: Moment, _: Moment, _: Moment) -> Vec<Programme> {
        Vec::new()
    }
    fn search(&self, _: &Filter, _: usize) -> Vec<Programme> {
        Vec::new()
    }
    fn count(&self, _: &Filter) -> Vec<(u16, usize)> {
        Vec::new()
    }
}

#[cfg(not(feature = "epg-db"))]
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct Filter {
    query: String,
    titles_only: bool,
    genre: Option<(u8, Option<u8>)>,
    ends_after: Option<Moment>,
    within: Option<Vec<(u16, u16)>>,
    without: Vec<(u16, u16)>,
}

pub enum ToGuide {
    Merge(Box<Guide>),
    Heard(Box<Guide>),
    Receivable(Option<Arc<BTreeSet<Key>>>),
    List(Ask),
    Prefs(Prefs),
    Clock(Option<Moment>),
    Search(Option<Asked>),
    Logos(std::collections::BTreeMap<Key, LogoOf>),
    /// 録画の再生に入る・出る。再生中は録画に含まれる情報だけで組み立て、ファイルは読み書きしない。
    Recording(bool),
    /// 番組表をファイルに保存するか（設定の `guide.cache`）。
    Keep(bool),
}

impl std::fmt::Debug for ToGuide {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Merge(_) => out.write_str("Merge"),
            Self::Heard(_) => out.write_str("Heard"),
            Self::Receivable(keys) => {
                write!(out, "Receivable({:?})", keys.as_ref().map(|k| k.len()))
            }
            Self::List(ask) => write!(out, "List({ask:?})"),
            Self::Clock(moment) => write!(out, "Clock({moment:?})"),
            Self::Search(query) => write!(out, "Search({query:?})"),
            Self::Logos(map) => write!(out, "Logos({})", map.len()),
            Self::Recording(on) => write!(out, "Recording({on})"),
            Self::Keep(on) => write!(out, "Keep({on})"),
            Self::Prefs(prefs) => write!(
                out,
                "Prefs({} names, {} keys)",
                prefs.names.len(),
                prefs.keys.len()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub band: Band,
    pub from: Option<Moment>,
    pub hours: u32,
    pub watching: Option<Key>,
}

/// 番組表のファイルを別のスレッドで開いて読む（読む間も局と番組を受け付ける）。
fn open_aside(
    cache: Option<PathBuf>,
    keep: bool,
    after: Moment,
) -> Option<Receiver<(Store, Guide)>> {
    let (send, opened) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(String::from("guide-file"))
        .spawn(move || {
            let mut store = Store::open(cache, keep);
            let guide = store.load(after);
            let _ = send.send((store, guide));
        });
    match spawned {
        Ok(_) => Some(opened),
        Err(error) => {
            tracing::warn!("番組表のファイルを読むスレッドを起動できない: {error}");
            None
        }
    }
}

pub fn keeping(
    asked: &Receiver<ToGuide>,
    merged: &Sender<Arc<Guide>>,
    listed: &Sender<Box<Listing>>,
    cache: Option<PathBuf>,
    keep: bool,
) {
    let mut keeping_file = keep;
    // 戻さない。録画の再生で時計が過去に飛んでも、ファイルにある過去の番組を消えたと見なさないように。
    let mut kept_from = Guide::new().machine_now() - MEMORY_BEHIND;
    // 読み終えるまでは保存も検索もしない（読む前の差分で書くと、ファイルの番組を上書きする）。
    let mut opening = open_aside(cache.clone(), keep, kept_from);
    let mut store: Option<Store> = opening.is_none().then(|| Store::open(None, keep));
    let mut master = Guide::new();
    let mut was: Option<(Ask, u64)> = None;
    let mut version = 0u64;
    let mut wanted: Option<Ask> = None;
    let mut receivable: Option<Arc<BTreeSet<Key>>> = None;
    let mut prefs = Prefs::default();
    let mut found: Option<Arc<Search>> = None;
    let mut logo_map: std::collections::BTreeMap<Key, LogoOf> = std::collections::BTreeMap::new();
    let mut recording: Option<Guide> = None;
    let blank = Store::open(None, true);
    let mut saved = Instant::now();
    let mut forgot = Instant::now();
    let mut dirty = false;
    loop {
        let wait = if opening.is_some() {
            OPEN_POLL
        } else {
            SAVE_EVERY
        };
        let first = match asked.recv_timeout(wait) {
            Ok(first) => Some(first),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(store) = store.as_mut() {
                    if dirty {
                        store.save(&master, kept_from);
                    }
                    store.save_logos(&master);
                }
                return;
            }
        };
        let mut fresh = false;
        if let Some(back) = &opening {
            match back.try_recv() {
                Ok((mut opened, mut loaded)) => {
                    opening = None;
                    // ファイルの番組を先に置き、読む間に届いた物を後から重ねる（届いた順と同じ規則で決まる）。
                    if let Some(now) = master.now() {
                        loaded.tell_time(now);
                    }
                    loaded.merge(std::mem::take(&mut master));
                    master = loaded;
                    if keeping_file != keep {
                        opened = Store::open(cache.clone(), keeping_file);
                        opened.save(&master, kept_from);
                    }
                    store = Some(opened);
                    fresh = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    opening = None;
                    store = Some(Store::open(None, keeping_file));
                }
            }
        }
        let mut query: Option<Option<Asked>> = None;
        for what in first.into_iter().chain(asked.try_iter()) {
            match what {
                ToGuide::Merge(guide) => {
                    master.merge(*guide);
                    fresh |= recording.is_none();
                    dirty |= recording.is_some();
                }
                ToGuide::Heard(guide) => {
                    match recording.as_mut() {
                        Some(recorded) => {
                            let before = master.revision();
                            master.learn_logos(&guide);
                            dirty |= master.revision() != before;
                            recorded.merge(*guide);
                        }
                        None => master.merge(*guide),
                    }
                    fresh = true;
                }
                ToGuide::Recording(on) => {
                    recording = on.then(|| {
                        let mut recorded = Guide::new();
                        recorded.lend_logos(&master);
                        recorded
                    });
                    fresh = true;
                }
                ToGuide::List(ask) => wanted = Some(ask),
                ToGuide::Receivable(keys) => {
                    if receivable != keys {
                        receivable = keys;
                        version += 1;
                    }
                }
                ToGuide::Prefs(fresh_prefs) => {
                    if prefs != fresh_prefs {
                        prefs = fresh_prefs;
                        fresh = true;
                    }
                }
                ToGuide::Clock(moment) => {
                    let clocked = recording.as_mut().unwrap_or(&mut master);
                    let jumped = match (moment, clocked.now()) {
                        (Some(told), Some(had)) => (told - had).abs() > CLOCK_SLACK,
                        (None, None) => false,
                        _ => true,
                    };
                    match moment {
                        Some(told) => clocked.tell_time(told),
                        None => clocked.forget_time(),
                    }
                    if jumped {
                        fresh = true;
                    }
                }
                ToGuide::Search(asked) => query = Some(asked),
                ToGuide::Logos(map) => {
                    logo_map = map;
                    fresh = true;
                }
                ToGuide::Keep(on) if on != keeping_file => {
                    keeping_file = on;
                    if let Some(store) = store.as_mut() {
                        *store = Store::open(cache.clone(), on);
                        store.save(&master, kept_from);
                        dirty = false;
                        saved = Instant::now();
                    }
                }
                ToGuide::Keep(_) => {}
            }
        }
        if forgot.elapsed() >= FORGET_EVERY {
            forgot = Instant::now();
            // 放送の時刻で判定する（録画の再生中にシステムの時計で捨てると番組が消える）。メモリから消す前に保存する。
            let behind = master.now_or_machine() - MEMORY_BEHIND;
            if behind > kept_from {
                if dirty && let Some(store) = store.as_mut() {
                    saved = Instant::now();
                    dirty = false;
                    store.save(&master, kept_from);
                }
                kept_from = behind;
                master.forget_before(kept_from);
                fresh = true;
            }
        }
        if fresh {
            version += 1;
            if recording.is_none() {
                dirty = true;
            }
            let active = recording.as_mut().unwrap_or(&mut master);
            active.fill_logos(&logo_map);
            let mut shown = active.clone();
            shown.apply_prefs(&prefs);
            if merged.send(Arc::new(shown)).is_err() {
                if let Some(store) = store.as_mut() {
                    store.save(&master, kept_from);
                    store.save_logos(&master);
                }
                return;
            }
        }
        if dirty
            && saved.elapsed() >= SAVE_EVERY
            && let Some(store) = store.as_mut()
        {
            saved = Instant::now();
            dirty = false;
            store.save(&master, kept_from);
            store.save_logos(&master);
        }
        if let Some(query) = query {
            // 検索の前に保存する（届いたばかりの番組も検索できるように）。
            if dirty && let Some(store) = store.as_mut() {
                saved = Instant::now();
                dirty = false;
                store.save(&master, kept_from);
            }
            let (searched, guide) = match &recording {
                Some(recorded) => (&blank, recorded),
                None => (store.as_ref().unwrap_or(&blank), &master),
            };
            found = query
                .filter(|asked| !asked.query.trim().is_empty())
                .map(|asked| Arc::new(search(searched, guide, &prefs, &asked)));
            was = None;
        }
        let Some(ask) = wanted else {
            continue;
        };
        if was == Some((ask, version)) {
            continue;
        }
        was = Some((ask, version));
        let (mut shown, kept) = match &recording {
            Some(recorded) => (recorded.clone(), &blank),
            None => (master.clone(), store.as_ref().unwrap_or(&blank)),
        };
        shown.apply_prefs(&prefs);
        let mut listing = listing(&mut shown, kept, kept_from, ask, receivable.as_deref());
        listing.search = found.clone();
        if listed.send(Box::new(listing)).is_err() {
            return;
        }
    }
}

const CLOCK_SLACK: i64 = 3;

#[cfg(feature = "epg-db")]
use super::db::Filter;

fn filter_of(asked: &Asked, now: Moment) -> Filter {
    let (within, without) = match asked.band {
        Some(band) => scope(band),
        None => (None, Vec::new()),
    };
    Filter {
        query: asked.query.clone(),
        titles_only: asked.titles_only,
        genre: asked.genre,
        ends_after: (!asked.past).then_some(now),
        within,
        without,
    }
}

/// ネットワーク ID の範囲（両端を含む）。
type Range = (u16, u16);

/// 種別のネットワーク ID の範囲：含まれる物と、前の種別が先に取った物（[`Band::of`] は上から順に判定する）。
pub fn scope(band: Band) -> (Option<Vec<Range>>, Vec<Range>) {
    let before: Vec<(u16, u16)> = Band::all()
        .into_iter()
        .filter(|other| *other != Band::OTHER && (band == Band::OTHER || other.0 < band.0))
        .flat_map(Band::ranges)
        .collect();
    if band == Band::OTHER {
        return (None, before);
    }
    (Some(band.ranges()), before)
}

pub fn tally(by_network: &[(u16, usize)]) -> Vec<(Band, usize)> {
    let mut counts: std::collections::BTreeMap<Band, usize> = std::collections::BTreeMap::new();
    for (network, count) in by_network {
        *counts.entry(Band::of(*network)).or_default() += count;
    }
    counts.into_iter().collect()
}

fn search(store: &Store, guide: &Guide, prefs: &Prefs, asked: &Asked) -> Search {
    let began = Instant::now();
    let filter = filter_of(asked, guide.now_or_machine());
    let mut programmes = store.search(&filter, MOST_FOUND + 1);
    let more = programmes.len() > MOST_FOUND;
    programmes.truncate(MOST_FOUND);
    let counts = tally(&store.count(&Filter {
        within: None,
        without: Vec::new(),
        ..filter
    }));
    let hits: Vec<Hit> = programmes
        .into_iter()
        .map(|programme| {
            let key = programme.key();
            let service = guide.service(key);
            let channel = prefs
                .names
                .get(&key)
                .cloned()
                .or_else(|| service.map(|service| service.name.clone()))
                .unwrap_or_default();
            let badge =
                service.map_or_else(|| super::listing::badge_of(&channel), super::Service::badge);
            Hit {
                key,
                channel,
                badge,
                logo: guide.logo_of(key).cloned(),
                programme,
            }
        })
        .collect();
    tracing::debug!(
        "番組検索「{}」: {} 件（{} ms）",
        asked.query,
        hits.len(),
        began.elapsed().as_millis()
    );
    Search {
        asked: asked.clone(),
        hits,
        more,
        counts,
    }
}

fn listing(
    guide: &mut Guide,
    store: &Store,
    kept_from: Moment,
    ask: Ask,
    only: Option<&BTreeSet<Key>>,
) -> Listing {
    let now = guide.now_or_machine();
    let from = ask
        .from
        .unwrap_or_else(|| now - now.rem_euclid(3600) - 3600);
    let until = from + i64::from(ask.hours) * 3600;
    if from < kept_from {
        for programme in store.past(from, until, kept_from) {
            guide.tell_past(programme);
        }
    }
    Listing {
        channels: channels_of(guide, guide, ask.band, from, ask.hours, only, ask.watching),
        from,
        hours: ask.hours,
        now: Some(now),
        band: ask.band,
        bands: bands_of(guide, ask.band, only),
        watching: ask.watching,
        jump: None,
        search: None,
    }
}

/// この種別の局をリモコン番号順に。サブチャンネルは時間帯に自分の番組がある時だけ。
pub fn channels_of(
    stations: &Guide,
    guide: &Guide,
    band: Band,
    from: Moment,
    hours: u32,
    only: Option<&BTreeSet<Key>>,
    watching: Option<Key>,
) -> Vec<Channel> {
    let until = from + i64::from(hours) * 3600;
    let mut services: Vec<&crate::guide::Service> = stations
        .channels()
        .filter(|service| service.band() == band)
        .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
        .collect();
    services.sort_by_key(|service| service.order());
    let channels: Vec<Channel> = services
        .into_iter()
        .map(|service| {
            let key = service.key();
            Channel {
                key,
                transport: service.transport,
                name: service.name.clone(),
                badge: service.badge(),
                remote_key: service.remote_key,
                logo: stations
                    .logo_of(key)
                    .or_else(|| guide.logo_of(key))
                    .cloned(),
                programmes: guide
                    .programmes(key)
                    .filter(|programme| shown(programme, from, until))
                    .cloned()
                    .collect(),
                filled: guide.filled(key),
            }
        })
        .collect();
    let mut kept: Vec<Channel> = Vec::with_capacity(channels.len());
    for channel in channels {
        if Some(channel.key) == watching || needed(&kept, &channel) {
            kept.push(channel);
        }
    }
    kept
}

fn needed(kept: &[Channel], channel: &Channel) -> bool {
    let siblings: Vec<&Channel> = kept
        .iter()
        .filter(|other| {
            other.transport == channel.transport && other.key.network == channel.key.network
        })
        .collect();
    if siblings.is_empty() {
        return true;
    }
    channel
        .programmes
        .iter()
        .any(|programme| own_programme(programme, &siblings))
}

/// 本チャンネルと共有でない番組か（どちらが参照していてもよい）。タイトルの無い番組は数えない。
fn own_programme(programme: &crate::guide::Programme, siblings: &[&Channel]) -> bool {
    if programme.start.is_none() || programme.title.is_empty() {
        return false;
    }
    let shared = siblings.iter().any(|sibling| {
        sibling.programmes.iter().any(|other| {
            other.start == programme.start
                && (programme.shared.contains(&(other.service, other.event))
                    || other.shared.contains(&(programme.service, programme.event)))
        })
    });
    !shared
}

pub fn bands_of(guide: &Guide, fallback: Band, only: Option<&BTreeSet<Key>>) -> Vec<Band> {
    let mut seen: Vec<Band> = guide
        .channels()
        .filter(|service| only.is_none_or(|only| only.contains(&service.key())))
        .map(|service| service.band())
        .collect();
    seen.sort();
    seen.dedup();
    // 列の無いタブは壊れて見えるので一つは出す。
    if seen.is_empty() {
        seen.push(fallback);
    }
    seen
}

pub fn shown(programme: &crate::guide::Programme, from: Moment, until: Moment) -> bool {
    let Some(start) = programme.start else {
        return false;
    };
    let ends = programme.ends().unwrap_or(start);
    start < until && ends > from
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(service: u16, event: u16, start: i64, title: &str) -> crate::guide::Programme {
        let mut programme = crate::guide::Programme {
            title: String::from("題"),
            ..crate::guide::Programme::scheduled(
                crate::guide::Key {
                    service: 1,
                    network: 1,
                },
                1,
                Some(start),
                Some(1800),
            )
        };
        programme.service = service;
        programme.network = 4;
        programme.event = event;
        programme.title = title.to_owned();
        programme
    }

    fn channel(service: u16, programmes: Vec<crate::guide::Programme>) -> Channel {
        Channel {
            key: Key {
                network: 4,
                service,
            },
            transport: 0x40f1,
            programmes,
            ..Channel::default()
        }
    }

    #[cfg(feature = "epg-db")]
    #[test]
    fn a_guide_not_kept_never_touches_the_file() {
        let folder = std::env::temp_dir().join(format!("zw-keep-{}", std::process::id()));
        let path = folder.join("epg.sqlite");
        let _ = std::fs::remove_dir_all(&folder);
        let mut store = Store::open(Some(path.clone()), false);
        store.save(&Guide::new(), 0);
        assert!(store.connection.is_some(), "検索はメモリーで引ける");
        assert!(!path.exists());
        drop(Store::open(Some(path.clone()), true));
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_band_scope_leaves_out_what_an_earlier_band_took() {
        let (within, without) = scope(Band(0));
        assert_eq!(within, Some(Band(0).ranges()));
        assert!(without.is_empty());
        let (within, without) = scope(Band(1));
        assert_eq!(within, Some(Band(1).ranges()));
        assert_eq!(without, Band(0).ranges());
        let (within, without) = scope(Band::OTHER);
        assert_eq!(within, None);
        assert!(without.contains(&Band(0).ranges()[0]));
        assert!(without.contains(&Band(1).ranges()[0]));
    }

    #[test]
    fn counts_are_summed_per_band() {
        let ground = Band::of(0x7fe1);
        let bs = Band::of(0x0004);
        let counts = tally(&[(0x0004, 1), (0x7fe1, 2), (0x7fe8, 1), (0x0123, 4)]);
        assert!(counts.contains(&(ground, 3)));
        assert!(counts.contains(&(bs, 1)));
        assert!(counts.contains(&(Band::OTHER, 4)));
        assert_eq!(counts.len(), 3);
    }

    #[test]
    fn a_search_looks_ahead_unless_the_past_is_asked() {
        let asked = Asked {
            query: String::from("灯台"),
            band: Some(Band::of(0x0004)),
            titles_only: true,
            ..Asked::default()
        };
        let filter = filter_of(&asked, 5000);
        assert_eq!(filter.ends_after, Some(5000));
        assert!(filter.titles_only);
        assert_eq!(filter.within, Some(Band::of(0x0004).ranges()));
        let filter = filter_of(
            &Asked {
                past: true,
                band: None,
                ..asked
            },
            5000,
        );
        assert_eq!(filter.ends_after, None);
        assert_eq!(filter.within, None);
    }

    #[test]
    fn a_sub_channel_is_a_column_only_while_it_has_a_programme_of_its_own() {
        let main = channel(101, vec![programme(101, 1, 0, "うつわの旅")]);
        let mut shared = programme(102, 1, 0, "うつわの旅");
        shared.shared = vec![(101, 1)];
        let idle = channel(102, vec![shared.clone(), programme(102, 2, 1800, "")]);
        let busy = channel(102, vec![shared, programme(102, 3, 1800, "相撲中継")]);
        let kept = vec![main];
        assert!(!needed(&kept, &idle));
        assert!(needed(&kept, &busy));
        assert!(needed(&[], &idle));
    }
}
