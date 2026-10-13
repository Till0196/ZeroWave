// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表を sqlite に保存・読み込みする。差分は最後に読み書きしたハッシュ（[`Known`]）と比べる
//! （同時に動く別の deck の番組を古い内容で上書きしないため）。読み込んだ物は放送から受信した物として入れる。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, params};

pub use super::listing::fold;
use super::{Component, Genre, Guide, Key, Moment, Picture, Programme, Service, Told};

pub const BUSY_WAIT: Duration = Duration::from_secs(10);

/// 番組の位置（サービスと開始時刻）。
pub type At = (u16, u16, Moment);

/// 保存済みの番組：位置ごとの内容のダイジェストと終了時刻。
pub type Known = HashMap<At, (i64, Option<Moment>)>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS services (
    network INTEGER NOT NULL,
    service INTEGER NOT NULL,
    transport INTEGER NOT NULL,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    kind INTEGER NOT NULL,
    scrambled INTEGER NOT NULL,
    remote_key INTEGER,
    transport_confirmed INTEGER NOT NULL,
    PRIMARY KEY (network, service)
);
CREATE TABLE IF NOT EXISTS programmes (
    id INTEGER PRIMARY KEY,
    network INTEGER NOT NULL,
    service INTEGER NOT NULL,
    event INTEGER NOT NULL,
    start INTEGER NOT NULL,
    seconds INTEGER,
    ends INTEGER,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    free INTEGER NOT NULL,
    caption INTEGER NOT NULL,
    rating INTEGER,
    rest BLOB NOT NULL,
    digest INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS programmes_at ON programmes (network, service, start);
CREATE INDEX IF NOT EXISTS programmes_by_start ON programmes (start);
CREATE INDEX IF NOT EXISTS programmes_by_end ON programmes (ends);
CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5 (
    title, body, tokenize = 'trigram', content = '', contentless_delete = 1
);
CREATE TABLE IF NOT EXISTS extended (
    programme INTEGER NOT NULL,
    position INTEGER NOT NULL,
    head TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (programme, position)
);
CREATE TABLE IF NOT EXISTS genres (
    programme INTEGER NOT NULL,
    large INTEGER NOT NULL,
    middle INTEGER NOT NULL,
    user INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS genres_by_programme ON genres (programme);
";

/// 番組表を開く（無ければ作る）。開けないファイルは退避して作り直すが、別の deck が開いているなら退避しない。
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    match open_as_is(path) {
        Err(error) if !busy(&error) && path.exists() => {
            let aside = path.with_extension("sqlite.unreadable");
            tracing::warn!(
                "番組表 {} を開けない（{error}）。{} に退避して作り直す",
                path.display(),
                aside.display()
            );
            let _ = std::fs::remove_file(&aside);
            if let Err(why) = std::fs::rename(path, &aside) {
                tracing::warn!("番組表を退避できない: {why}");
                return Err(error);
            }
            for suffix in ["-wal", "-shm"] {
                let mut side = path.as_os_str().to_owned();
                side.push(suffix);
                let _ = std::fs::remove_file(std::path::PathBuf::from(side));
            }
            open_as_is(path)
        }
        other => other,
    }
}

fn busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

fn open_as_is(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(BUSY_WAIT)?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    // 二文字以下の語は trigram インデックスで検索できないので、番組の文字列を正規化して比べる。
    connection.create_scalar_function(
        "zw_fold",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |context| Ok(context.get::<Option<String>>(0)?.map(|text| fold(&text))),
    )?;
    connection.execute_batch(SCHEMA)?;
    Ok(connection)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Saved {
    pub written: usize,
    pub dropped: usize,
}

/// 局と、このプロセスが変えた番組だけを書く。範囲から無くなった番組は、内容が最後に読み書きした時のままなら消す。
/// `keep_since` より前に終わった番組は、どのプロセスが書いた物でも消す。
pub fn save(
    connection: &mut Connection,
    guide: &Guide,
    known: &mut Known,
    kept_from: Moment,
    keep_since: Moment,
) -> rusqlite::Result<Saved> {
    let mut saved = Saved::default();
    // コミットできたときだけ記録済みの状態を進める（ロールバックしたのに進めると二度と書かれない）。
    let mut pending = known.clone();
    let tx = connection.transaction()?;
    {
        let mut service_row = tx.prepare(
            "INSERT INTO services (network, service, transport, name, provider, kind, scrambled,
                remote_key, transport_confirmed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (network, service) DO UPDATE SET transport = excluded.transport,
                name = excluded.name, provider = excluded.provider, kind = excluded.kind,
                scrambled = excluded.scrambled, remote_key = excluded.remote_key,
                transport_confirmed = excluded.transport_confirmed",
        )?;
        for service in guide.services.values() {
            service_row.execute(params![
                service.network,
                service.service,
                service.transport,
                service.name,
                service.provider,
                service.kind,
                service.scrambled,
                service.remote_key,
                service.transport_confirmed,
            ])?;
        }
        let mut write = tx.prepare(
            "INSERT INTO programmes (network, service, event, start, seconds, ends, title, detail,
                free, caption, rating, rest, digest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT (network, service, start) DO UPDATE SET event = excluded.event,
                seconds = excluded.seconds, ends = excluded.ends, title = excluded.title,
                detail = excluded.detail, free = excluded.free, caption = excluded.caption,
                rating = excluded.rating, rest = excluded.rest, digest = excluded.digest
             RETURNING id",
        )?;
        let mut children = Children::prepare(&tx)?;
        let mut present: std::collections::HashSet<(u16, u16, Moment)> =
            std::collections::HashSet::new();
        for programme in guide
            .programmes
            .values()
            .flat_map(|by_time| by_time.values())
        {
            let Some(start) = programme.start else {
                continue;
            };
            let at = (programme.network, programme.service, start);
            present.insert(at);
            let rest = rest_of(programme);
            let rating = programme.details.rating().map(|rating| rating.rating);
            let digest = digest_of(programme, rating, &rest);
            if pending.get(&at).is_some_and(|(had, _)| *had == digest) {
                continue;
            }
            let ends = programme.ends();
            let id: i64 = write.query_row(
                params![
                    programme.network,
                    programme.service,
                    programme.event,
                    start,
                    programme.seconds,
                    ends,
                    programme.title,
                    programme.detail,
                    programme.free,
                    programme.caption,
                    rating,
                    rest,
                    digest,
                ],
                |row| row.get(0),
            )?;
            children.write(id, programme)?;
            pending.insert(at, (digest, ends));
            saved.written += 1;
        }
        let gone: Vec<(At, (i64, Option<Moment>))> = pending
            .iter()
            .filter(|(at, _)| !present.contains(*at))
            .map(|(at, had)| (*at, *had))
            .collect();
        let mut find = tx.prepare(
            "SELECT id FROM programmes
             WHERE network = ?1 AND service = ?2 AND start = ?3 AND digest = ?4",
        )?;
        for ((network, service, start), (digest, ends)) in gone {
            pending.remove(&(network, service, start));
            if ends.is_some_and(|ends| ends <= kept_from) {
                continue;
            }
            let ids: Vec<i64> = find
                .query_map(params![network, service, start, digest], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in ids {
                children.clear(id)?;
                tx.execute("DELETE FROM programmes WHERE id = ?1", [id])?;
                saved.dropped += 1;
            }
        }
    }
    // 長さの分からない番組は一日で終わったと見なす（[`Guide::forget_before`] と同じ）。
    let old = "SELECT id FROM programmes
        WHERE ends <= ?1 OR (ends IS NULL AND start + 86400 <= ?1)";
    tx.execute(
        &format!("DELETE FROM extended WHERE programme IN ({old})"),
        [keep_since],
    )?;
    tx.execute(
        &format!("DELETE FROM genres WHERE programme IN ({old})"),
        [keep_since],
    )?;
    tx.execute(
        &format!("DELETE FROM search WHERE rowid IN ({old})"),
        [keep_since],
    )?;
    saved.dropped += tx.execute(
        "DELETE FROM programmes WHERE ends <= ?1 OR (ends IS NULL AND start + 86400 <= ?1)",
        [keep_since],
    )?;
    tx.commit()?;
    *known = pending;
    Ok(saved)
}

struct Children<'c> {
    extended: rusqlite::Statement<'c>,
    genres: rusqlite::Statement<'c>,
    search: rusqlite::Statement<'c>,
    clear_extended: rusqlite::Statement<'c>,
    clear_genres: rusqlite::Statement<'c>,
    clear_search: rusqlite::Statement<'c>,
}

impl<'c> Children<'c> {
    fn prepare(tx: &'c rusqlite::Transaction<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            extended: tx.prepare(
                "INSERT INTO extended (programme, position, head, body) VALUES (?1, ?2, ?3, ?4)",
            )?,
            genres: tx.prepare(
                "INSERT INTO genres (programme, large, middle, user) VALUES (?1, ?2, ?3, ?4)",
            )?,
            search: tx.prepare("INSERT INTO search (rowid, title, body) VALUES (?1, ?2, ?3)")?,
            clear_extended: tx.prepare("DELETE FROM extended WHERE programme = ?1")?,
            clear_genres: tx.prepare("DELETE FROM genres WHERE programme = ?1")?,
            clear_search: tx.prepare("DELETE FROM search WHERE rowid = ?1")?,
        })
    }

    fn clear(&mut self, id: i64) -> rusqlite::Result<()> {
        self.clear_extended.execute([id])?;
        self.clear_genres.execute([id])?;
        self.clear_search.execute([id])?;
        Ok(())
    }

    fn write(&mut self, id: i64, programme: &Programme) -> rusqlite::Result<()> {
        self.clear(id)?;
        for (position, (head, body)) in programme.extended.iter().enumerate() {
            self.extended
                .execute(params![id, position as i64, head, body])?;
        }
        for genre in &programme.genres {
            self.genres
                .execute(params![id, genre.large, genre.middle, genre.user])?;
        }
        let mut body = programme.detail.clone();
        for (head, text) in &programme.extended {
            body.push('\n');
            body.push_str(head);
            body.push('\n');
            body.push_str(text);
        }
        self.search
            .execute(params![id, fold(&programme.title), fold(&body)])?;
        Ok(())
    }
}

/// 番組の内容のハッシュ（FNV-1a）。ファイルに保存されるので `std` のハッシュは使わない。
fn digest_of(programme: &Programme, rating: Option<u8>, rest: &[u8]) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in (bytes.len() as u32).to_le_bytes().iter().chain(bytes) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(&programme.event.to_le_bytes());
    eat(&programme.seconds.unwrap_or(u32::MAX).to_le_bytes());
    eat(programme.title.as_bytes());
    eat(programme.detail.as_bytes());
    eat(&[
        u8::from(programme.free),
        u8::from(programme.caption),
        rating.unwrap_or(0xff),
    ]);
    eat(rest);
    for (head, body) in &programme.extended {
        eat(head.as_bytes());
        eat(body.as_bytes());
    }
    for genre in &programme.genres {
        eat(&[genre.large, genre.middle, genre.user]);
    }
    hash as i64
}

pub fn load_after(connection: &Connection, after: Moment) -> rusqlite::Result<(Guide, Known)> {
    let mut guide = Guide::new();
    let mut services = connection.prepare(
        "SELECT network, service, transport, name, provider, kind, scrambled, remote_key,
            transport_confirmed FROM services",
    )?;
    let rows = services.query_map([], |row| {
        Ok(Service {
            network: row.get(0)?,
            service: row.get(1)?,
            transport: row.get(2)?,
            name: row.get(3)?,
            provider: row.get(4)?,
            kind: row.get(5)?,
            scrambled: row.get(6)?,
            remote_key: row.get(7)?,
            transport_confirmed: row.get(8)?,
            logo_id: None,
            simple_logo: None,
        })
    })?;
    for row in rows {
        guide.tell_service(row?);
    }
    let mut known = Known::new();
    for (programme, digest) in read_programmes(
        connection,
        "WHERE ends IS NULL OR ends > ?1 ORDER BY network, service, start",
        params![after],
    )? {
        if let Some(start) = programme.start {
            known.insert(
                (programme.network, programme.service, start),
                (digest, programme.ends()),
            );
        }
        guide.tell(programme);
    }
    guide.forget_before(after);
    Ok((guide, known))
}

pub fn load_range(
    connection: &Connection,
    from: Moment,
    until: Moment,
    before: Moment,
) -> rusqlite::Result<Vec<Programme>> {
    Ok(read_programmes(
        connection,
        "WHERE start < ?2 AND ends > ?1 AND ends <= ?3 ORDER BY network, service, start",
        params![from, until, before],
    )?
    .into_iter()
    .map(|(programme, _)| programme)
    .collect())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub query: String,
    pub titles_only: bool,
    pub genre: Option<(u8, Option<u8>)>,
    pub ends_after: Option<Moment>,
    pub within: Option<Vec<(u16, u16)>>,
    pub without: Vec<(u16, u16)>,
}

/// 語が無ければ `None`。三文字以上は trigram インデックス、二文字以下は番組の文字列を正規化して順に比べる。
fn conditions(filter: &Filter) -> Option<(String, Vec<rusqlite::types::Value>)> {
    let words: Vec<String> = fold(&filter.query)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return None;
    }
    let (long, short): (Vec<&String>, Vec<&String>) =
        words.iter().partition(|word| word.chars().count() >= 3);
    let mut terms: Vec<String> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if !long.is_empty() {
        let column = if filter.titles_only { "title : " } else { "" };
        let matched = long
            .iter()
            .map(|word| format!("{column}\"{}\"", word.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        values.push(matched.into());
        terms.push(format!(
            "id IN (SELECT rowid FROM search WHERE search MATCH ?{})",
            values.len()
        ));
    }
    for word in short {
        values.push(word.clone().into());
        let n = values.len();
        terms.push(if filter.titles_only {
            format!("instr(zw_fold(title), ?{n}) > 0")
        } else {
            format!(
                "(instr(zw_fold(title), ?{n}) > 0 OR instr(zw_fold(detail), ?{n}) > 0
                  OR EXISTS (SELECT 1 FROM extended WHERE extended.programme = programmes.id
                      AND (instr(zw_fold(head), ?{n}) > 0 OR instr(zw_fold(body), ?{n}) > 0)))"
            )
        });
    }
    if let Some((large, middle)) = filter.genre {
        let middle = middle.map_or_else(String::new, |middle| {
            format!(" AND genres.middle = {middle}")
        });
        terms.push(format!(
            "EXISTS (SELECT 1 FROM genres WHERE genres.programme = programmes.id
                AND genres.large = {large}{middle})"
        ));
    }
    if let Some(after) = filter.ends_after {
        values.push(after.into());
        terms.push(format!("coalesce(ends, start) > ?{}", values.len()));
    }
    let ranges = |ranges: &[(u16, u16)]| {
        ranges
            .iter()
            .map(|(from, to)| format!("network BETWEEN {from} AND {to}"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    if let Some(within) = &filter.within {
        if within.is_empty() {
            terms.push(String::from("0"));
        } else {
            terms.push(format!("({})", ranges(within)));
        }
    }
    if !filter.without.is_empty() {
        terms.push(format!("NOT ({})", ranges(&filter.without)));
    }
    Some((terms.join(" AND "), values))
}

pub fn search(
    connection: &Connection,
    filter: &Filter,
    most: usize,
) -> rusqlite::Result<Vec<Programme>> {
    let Some((terms, values)) = conditions(filter) else {
        return Ok(Vec::new());
    };
    let filter = format!("WHERE {terms} ORDER BY start LIMIT {most}");
    Ok(read_programmes(
        connection,
        &filter,
        rusqlite::params_from_iter(values.iter()),
    )?
    .into_iter()
    .map(|(programme, _)| programme)
    .collect())
}

pub fn count_by_network(
    connection: &Connection,
    filter: &Filter,
) -> rusqlite::Result<Vec<(u16, usize)>> {
    let Some((terms, values)) = conditions(filter) else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(&format!(
        "SELECT network, count(*) FROM programmes WHERE {terms} GROUP BY network ORDER BY network"
    ))?;
    let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
        Ok((row.get::<_, u16>(0)?, row.get::<_, i64>(1)? as usize))
    })?;
    rows.collect()
}

fn read_programmes(
    connection: &Connection,
    filter: &str,
    values: impl rusqlite::Params,
) -> rusqlite::Result<Vec<(Programme, i64)>> {
    let mut extended = connection
        .prepare_cached("SELECT head, body FROM extended WHERE programme = ?1 ORDER BY position")?;
    let mut genres =
        connection.prepare_cached("SELECT large, middle, user FROM genres WHERE programme = ?1")?;
    let mut programmes = connection.prepare(&format!(
        "SELECT id, network, service, event, start, seconds, title, detail, free, caption, rating,
            rest, digest FROM programmes {filter}"
    ))?;
    let rows = programmes.query_map(values, |row| {
        let key = Key {
            network: row.get(1)?,
            service: row.get(2)?,
        };
        Ok((
            row.get::<_, i64>(0)?,
            Programme {
                title: row.get(6)?,
                detail: row.get(7)?,
                free: row.get(8)?,
                caption: row.get(9)?,
                told: Told::Broadcast,
                ..Programme::scheduled(key, row.get(3)?, row.get(4)?, row.get(5)?)
            },
            row.get::<_, Option<u8>>(10)?,
            row.get::<_, Vec<u8>>(11)?,
            row.get::<_, i64>(12)?,
        ))
    })?;
    let mut read = Vec::new();
    for row in rows {
        let (id, mut programme, rating, rest, digest) = row?;
        programme.extended = extended
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        programme.genres = genres
            .query_map([id], |row| {
                Ok(Genre {
                    large: row.get(0)?,
                    middle: row.get(1)?,
                    user: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        if let Some(rating) = rating.filter(|rating| *rating != 0) {
            programme
                .details
                .ratings
                .push(isdb::descriptors::ParentalRating {
                    country: "JPN".to_owned(),
                    rating,
                });
        }
        let _ = read_rest(&rest, &mut programme);
        read.push((programme, digest));
    }
    Ok(read)
}

fn rest_of(programme: &Programme) -> Vec<u8> {
    let mut out = Vec::new();
    match &programme.video {
        Some(picture) => out.extend_from_slice(&[
            picture.component_type.unwrap_or(0xff),
            picture.resolution,
            picture.aspect,
            u8::from(picture.progressive),
            picture.frame_rate,
            picture.transfer,
        ]),
        None => out.extend_from_slice(&[0xff; 6]),
    }
    out.push(programme.audios.len().min(15) as u8);
    for sound in programme.audios.iter().take(15) {
        out.push(sound.kind);
        out.extend_from_slice(&sound.tag.unwrap_or(0xffff).to_le_bytes());
        out.push(u8::from(sound.main) | (sound.quality << 1) | (sound.sampling << 3));
        out.push(sound.simulcast.unwrap_or(0xff));
        put_str(&mut out, &sound.language);
        put_str(&mut out, sound.second.as_deref().unwrap_or(""));
        put_str(&mut out, sound.name.as_deref().unwrap_or(""));
    }
    out.push(programme.shared.len().min(15) as u8);
    for (service, event) in programme.shared.iter().take(15) {
        out.extend_from_slice(&service.to_le_bytes());
        out.extend_from_slice(&event.to_le_bytes());
    }
    match &programme.details.series {
        Some(series) => {
            out.push(1);
            out.extend_from_slice(&series.id.to_le_bytes());
            out.push((series.repeat_label << 4) | (series.pattern & 0x07));
            out.extend_from_slice(&series.expire_mjd.unwrap_or(0xffff).to_le_bytes());
            out.extend_from_slice(&series.episode.to_le_bytes());
            out.extend_from_slice(&series.last_episode.to_le_bytes());
            put_str(&mut out, &series.name);
        }
        None => out.push(0),
    }
    out
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn read_rest(bytes: &[u8], programme: &mut Programme) -> Option<()> {
    let mut r = Reader { bytes, at: 0 };
    let picture = r.take(6)?;
    programme.video = (picture != [0xff; 6]).then(|| Picture {
        component_type: Some(picture[0]).filter(|kind| *kind != 0xff),
        resolution: picture[1],
        aspect: picture[2],
        progressive: picture[3] != 0,
        frame_rate: picture[4],
        transfer: picture[5],
    });
    for _ in 0..r.u8()? {
        let kind = r.u8()?;
        let tag = Some(r.u16()?).filter(|tag| *tag != 0xffff);
        let flags = r.u8()?;
        let simulcast = Some(r.u8()?).filter(|group| *group != 0xff);
        let language = r.str()?;
        let second = Some(r.str()?).filter(|second| !second.is_empty());
        let name = Some(r.str()?).filter(|name| !name.is_empty());
        programme.audios.push(Component {
            kind,
            tag,
            language,
            second,
            main: flags & 1 != 0,
            name,
            quality: (flags >> 1) & 0x03,
            sampling: (flags >> 3) & 0x07,
            simulcast,
        });
    }
    for _ in 0..r.u8()? {
        programme.shared.push((r.u16()?, r.u16()?));
    }
    if r.u8()? == 1 {
        let id = r.u16()?;
        let packed = r.u8()?;
        programme.details.series = Some(isdb::descriptors::Series {
            id,
            repeat_label: packed >> 4,
            pattern: packed & 0x07,
            expire_mjd: Some(r.u16()?).filter(|mjd| *mjd != 0xffff),
            episode: r.u16()?,
            last_episode: r.u16()?,
            name: r.str()?,
        });
    }
    Some(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(key: Key, event: u16, start: Moment, title: &str) -> Programme {
        Programme {
            title: title.to_owned(),
            detail: "中身".to_owned(),
            extended: vec![
                ("出演".to_owned(), "誰か".to_owned()),
                ("音楽".to_owned(), "誰か二".to_owned()),
            ],
            genres: vec![Genre::new(1, 3)],
            video: Some(Picture {
                component_type: None,
                resolution: 6,
                aspect: 3,
                progressive: true,
                frame_rate: 8,
                transfer: 5,
            }),
            audios: vec![Component {
                kind: 0x09,
                tag: Some(0x10),
                language: "jpn".to_owned(),
                second: None,
                main: true,
                name: Some("日本語".to_owned()),
                quality: 1,
                sampling: 7,
                simulcast: None,
            }],
            caption: true,
            shared: vec![(key.service + 1, event)],
            ..Programme::scheduled(key, event, Some(start), Some(1800))
        }
    }

    fn scratch() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "zw-guide-db-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn a_guide(key: Key) -> Guide {
        let mut guide = Guide::new();
        guide.tell_service(Service {
            network: key.network,
            transport: 0x7880,
            transport_confirmed: true,
            service: key.service,
            name: "ゼロウェーブ総合".to_owned(),
            provider: String::new(),
            kind: 1,
            scrambled: false,
            remote_key: Some(1),
            logo_id: Some(0x123),
            simple_logo: Some("総合".to_owned()),
        });
        guide.tell(programme(key, 1, 1000, "はじめ"));
        guide.tell(programme(key, 2, 2800, "つぎ"));
        guide
    }

    const KEY: Key = Key {
        network: 0x7880,
        service: 0x0400,
    };

    fn titles(guide: &Guide) -> Vec<String> {
        guide.programmes(KEY).map(|p| p.title.clone()).collect()
    }

    #[test]
    fn a_guide_survives_a_round_trip() {
        let dir = scratch();
        let guide = a_guide(KEY);
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        save(&mut connection, &guide, &mut Known::new(), 0, 0).unwrap();
        drop(connection);

        let connection = open(&dir.join("epg.sqlite")).unwrap();
        let (back, known) = load_after(&connection, 0).unwrap();
        assert_eq!(known.len(), 2);
        assert_eq!(back.service(KEY).unwrap().name, "ゼロウェーブ総合");
        assert_eq!(back.service(KEY).unwrap().remote_key, Some(1));
        assert_eq!(back.service(KEY).unwrap().logo_id, None);
        let programmes: Vec<&Programme> = back.programmes(KEY).collect();
        assert_eq!(programmes.len(), 2);
        assert_eq!(programmes[0].title, "はじめ");
        assert_eq!(
            programmes[0].extended,
            vec![
                ("出演".to_owned(), "誰か".to_owned()),
                ("音楽".to_owned(), "誰か二".to_owned())
            ]
        );
        assert_eq!(programmes[0].genres[0].large, 1);
        assert!(programmes[0].caption);
        assert_eq!(programmes[0].audios[0].name.as_deref(), Some("日本語"));
        assert_eq!(programmes[0].video.as_ref().unwrap().frame_rate, 8);
        assert_eq!(programmes[0].shared, vec![(KEY.service + 1, 1)]);
        assert_eq!(programmes[0].told, Told::Broadcast);
        assert_eq!(programmes[1].title, "つぎ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_programme_gone_from_memory_is_removed() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let mut known = Known::new();
        save(&mut connection, &a_guide(KEY), &mut known, 0, 0).unwrap();
        let mut fewer = Guide::new();
        fewer.tell(programme(KEY, 3, 5000, "あと"));
        let saved = save(&mut connection, &fewer, &mut known, 0, 0).unwrap();
        assert_eq!(
            saved,
            Saved {
                written: 1,
                dropped: 2
            }
        );
        let (back, _) = load_after(&connection, 0).unwrap();
        assert_eq!(titles(&back), vec!["あと".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_programmes_are_dropped_on_load() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        save(&mut connection, &a_guide(KEY), &mut Known::new(), 0, 0).unwrap();
        let (back, known) = load_after(&connection, 3000).unwrap();
        assert_eq!(titles(&back), vec!["つぎ".to_owned()]);
        assert_eq!(known.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_changed_programmes_are_written() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let mut guide = a_guide(KEY);
        let mut known = Known::new();
        let first = save(&mut connection, &guide, &mut known, 0, 0).unwrap();
        assert_eq!(
            first,
            Saved {
                written: 2,
                dropped: 0
            }
        );
        let again = save(&mut connection, &guide, &mut known, 0, 0).unwrap();
        assert_eq!(again, Saved::default());
        guide.tell(programme(KEY, 2, 2800, "つぎ（改）"));
        let changed = save(&mut connection, &guide, &mut known, 0, 0).unwrap();
        assert_eq!(
            changed,
            Saved {
                written: 1,
                dropped: 0
            }
        );
        let (back, _) = load_after(&connection, 0).unwrap();
        assert_eq!(
            titles(&back),
            vec!["はじめ".to_owned(), "つぎ（改）".to_owned()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_past_stays_in_the_file() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let mut known = Known::new();
        save(&mut connection, &a_guide(KEY), &mut known, 0, 0).unwrap();
        let mut later = Guide::new();
        later.tell(programme(KEY, 3, 5000, "あと"));
        let saved = save(&mut connection, &later, &mut known, 3000, 0).unwrap();
        assert_eq!(
            saved,
            Saved {
                written: 1,
                dropped: 1
            }
        );
        let past = load_range(&connection, 0, 10_000, 3000).unwrap();
        let past_titles: Vec<String> = past.iter().map(|p| p.title.clone()).collect();
        assert_eq!(past_titles, vec!["はじめ".to_owned()]);
        assert_eq!(past[0].extended.len(), 2);
        let (now, _) = load_after(&connection, 3000).unwrap();
        assert_eq!(titles(&now), vec!["あと".to_owned()]);
        let pruned = save(&mut connection, &later, &mut known, 3000, 2800).unwrap();
        assert_eq!(
            pruned,
            Saved {
                written: 0,
                dropped: 1
            }
        );
        assert!(load_range(&connection, 0, 10_000, 3000).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_decks_do_not_undo_each_other() {
        let dir = scratch();
        let path = dir.join("epg.sqlite");
        let mut first = open(&path).unwrap();
        save(&mut first, &a_guide(KEY), &mut Known::new(), 0, 0).unwrap();
        drop(first);
        let mut a = open(&path).unwrap();
        let mut b = open(&path).unwrap();
        let (mut guide_a, mut known_a) = load_after(&a, 0).unwrap();
        let (mut guide_b, mut known_b) = load_after(&b, 0).unwrap();
        guide_b.tell(programme(KEY, 2, 2800, "つぎ（延長）"));
        let other = Key {
            network: 4,
            service: 101,
        };
        guide_b.tell(programme(other, 9, 1000, "BS の番組"));
        save(&mut b, &guide_b, &mut known_b, 0, 0).unwrap();
        guide_a.tell(programme(KEY, 1, 1000, "はじめ（改）"));
        let saved = save(&mut a, &guide_a, &mut known_a, 0, 0).unwrap();
        assert_eq!(
            saved,
            Saved {
                written: 1,
                dropped: 0
            }
        );
        let (back, _) = load_after(&a, 0).unwrap();
        assert_eq!(
            titles(&back),
            vec!["はじめ（改）".to_owned(), "つぎ（延長）".to_owned()]
        );
        let bs: Vec<String> = back.programmes(other).map(|p| p.title.clone()).collect();
        assert_eq!(bs, vec!["BS の番組".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn programmes_are_found_by_their_words() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let mut guide = Guide::new();
        let mut news = programme(KEY, 1, 1000, "ＺＷニュース７");
        news.detail = "今日の出来事を伝えます".to_owned();
        guide.tell(news);
        let mut drama = programme(KEY, 2, 2800, "連続ドラマ");
        drama.extended = vec![("出演".to_owned(), "山田太郎".to_owned())];
        guide.tell(drama);
        let mut known = Known::new();
        save(&mut connection, &guide, &mut known, 0, 0).unwrap();
        let found = |connection: &Connection, query: &str| -> Vec<String> {
            let filter = Filter {
                query: query.to_owned(),
                ..Filter::default()
            };
            search(connection, &filter, 10)
                .unwrap()
                .into_iter()
                .map(|p| p.title)
                .collect()
        };
        assert_eq!(found(&connection, "zw"), vec!["ＺＷニュース７".to_owned()]);
        assert_eq!(
            found(&connection, "ニュース 7"),
            vec!["ＺＷニュース７".to_owned()]
        );
        assert_eq!(
            found(&connection, "出来事"),
            vec!["ＺＷニュース７".to_owned()]
        );
        assert_eq!(found(&connection, "山田"), vec!["連続ドラマ".to_owned()]);
        assert_eq!(
            found(&connection, "山田太郎"),
            vec!["連続ドラマ".to_owned()]
        );
        assert!(found(&connection, "山田 ニュース").is_empty());
        assert!(found(&connection, "  ").is_empty());
        guide.tell(programme(KEY, 2, 2800, "単発ドラマ"));
        save(&mut connection, &guide, &mut known, 0, 0).unwrap();
        assert!(found(&connection, "連続").is_empty());
        assert_eq!(found(&connection, "単発"), vec!["単発ドラマ".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_search_narrows_by_band_genre_time_and_where_it_looks() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let bs = Key {
            network: 4,
            service: 101,
        };
        let mut guide = Guide::new();
        let mut drama = programme(KEY, 1, 10_000, "灯台守の娘");
        drama.genres = vec![Genre::new(0x3, 0x0)];
        guide.tell(drama);
        let mut news = programme(KEY, 2, 1000, "ニュース");
        news.detail = "岬の灯台守を訪ねる".to_owned();
        news.genres = vec![Genre::new(0x0, 0x0)];
        guide.tell(news);
        let mut film = programme(bs, 3, 20_000, "映画「灯台の光」");
        film.genres = vec![Genre::new(0x6, 0x1)];
        guide.tell(film);
        save(&mut connection, &guide, &mut Known::new(), 0, 0).unwrap();
        let found = |filter: Filter| -> Vec<String> {
            search(&connection, &filter, 10)
                .unwrap()
                .into_iter()
                .map(|p| p.title)
                .collect()
        };
        let asked = |query: &str| Filter {
            query: query.to_owned(),
            ..Filter::default()
        };
        assert_eq!(
            found(asked("灯台")),
            vec!["ニュース", "灯台守の娘", "映画「灯台の光」"]
        );
        assert_eq!(
            found(Filter {
                ends_after: Some(5000),
                ..asked("灯台")
            }),
            vec!["灯台守の娘", "映画「灯台の光」"]
        );
        assert_eq!(
            found(Filter {
                titles_only: true,
                ..asked("灯台")
            }),
            vec!["灯台守の娘", "映画「灯台の光」"]
        );
        assert_eq!(
            found(Filter {
                titles_only: true,
                ..asked("灯台守")
            }),
            vec!["灯台守の娘"]
        );
        assert_eq!(found(asked("灯台守")), vec!["ニュース", "灯台守の娘"]);
        assert_eq!(
            found(Filter {
                genre: Some((0x3, None)),
                ..asked("灯台")
            }),
            vec!["灯台守の娘"]
        );
        assert_eq!(
            found(Filter {
                genre: Some((0x6, Some(0x1))),
                ..asked("灯台")
            }),
            vec!["映画「灯台の光」"]
        );
        assert!(
            found(Filter {
                genre: Some((0x6, Some(0x0))),
                ..asked("灯台")
            })
            .is_empty()
        );
        assert_eq!(
            found(Filter {
                within: Some(vec![(4, 4)]),
                ..asked("灯台")
            }),
            vec!["映画「灯台の光」"]
        );
        assert_eq!(
            found(Filter {
                without: vec![(4, 4)],
                ..asked("灯台")
            }),
            vec!["ニュース", "灯台守の娘"]
        );
        assert!(
            found(Filter {
                within: Some(Vec::new()),
                ..asked("灯台")
            })
            .is_empty()
        );
        assert_eq!(
            count_by_network(&connection, &asked("灯台")).unwrap(),
            vec![(4, 1), (0x7880, 2)]
        );
        assert_eq!(
            count_by_network(
                &connection,
                &Filter {
                    ends_after: Some(5000),
                    ..asked("灯台")
                }
            )
            .unwrap(),
            vec![(4, 1), (0x7880, 1)]
        );
        assert!(
            count_by_network(&connection, &asked(" "))
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn past_programmes_in_memory_are_written_once() {
        let dir = scratch();
        let mut connection = open(&dir.join("epg.sqlite")).unwrap();
        let guide = a_guide(KEY);
        let mut known = Known::new();
        let first = save(&mut connection, &guide, &mut known, 10_000, 0).unwrap();
        assert_eq!(
            first,
            Saved {
                written: 2,
                dropped: 0
            }
        );
        let again = save(&mut connection, &guide, &mut known, 10_000, 0).unwrap();
        assert_eq!(again, Saved::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_is_set_aside() {
        let dir = scratch();
        let path = dir.join("epg.sqlite");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE programmes (id INTEGER PRIMARY KEY, network INTEGER, start INTEGER);",
            )
            .unwrap();
        }
        let mut connection = open(&path).unwrap();
        assert!(dir.join("epg.sqlite.unreadable").exists());
        save(&mut connection, &a_guide(KEY), &mut Known::new(), 0, 0).unwrap();
        let (back, _) = load_after(&connection, 0).unwrap();
        assert_eq!(titles(&back).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
