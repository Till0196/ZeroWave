// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 設定ファイル。`zerowave.toml` は人・UI、`channels.toml` はチャンネルスキャン・人、`state.toml` はアプリ。
//! 効く順：既定 < セクション < `[profile.<今の>.セクション]` < `ZW_*` < `--set` < コマンドラインのオプション。

pub mod apply;
pub mod channels;
pub mod favourites;
pub mod file;
pub mod key;
pub mod plugins;
pub mod state;

use std::path::PathBuf;

pub use file::File;

pub const FILE: &str = "zerowave.toml";
pub const CHANNELS: &str = "channels.toml";
pub const STATE: &str = "state.toml";

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub found: bool,
    table: toml::Table,
    sets: Vec<(String, toml::Value)>,
    pub channels: channels::Channels,
    pub state: state::State,
}

impl Config {
    /// 保存先から読む。読めない物があっても止まらず、既定のまま理由を最初のトーストに追加する。
    /// `zerowave.toml` が読めなくても「あった」ことにする（雛形で上書きしないため）。
    pub fn load_forgiving(sets: &[String]) -> Self {
        let mut config = Self::default();
        match read_io(FILE) {
            Ok(Some(text)) => {
                config.found = true;
                match toml::from_str(&text) {
                    Ok(table) => config.table = table,
                    Err(error) => {
                        notice(format!(
                            "設定ファイル ({FILE}) を読み込めませんでした。\n既定の設定で起動しました。\n{error}"
                        ));
                    }
                }
            }
            Ok(None) => {}
            Err((path, error)) => {
                config.found = true;
                tracing::warn!("{} が読めない: {error}", path.display());
                notice(format!(
                    "設定を読み込めませんでした。既定の設定で起動しました。\n{error}"
                ));
            }
        }
        if let Ok(Some(text)) = read(STATE) {
            match state::State::parse(&text) {
                Ok(state) => config.state = state,
                Err(error) => set_aside(STATE, &error.to_string()),
            }
        }
        match read_io(CHANNELS) {
            Ok(Some(text)) => {
                match channels::Channels::parse(&text, channels_profile(&config.state.profile)) {
                    Ok(channels) => config.channels = channels,
                    Err(error) => set_aside(CHANNELS, &error.to_string()),
                }
            }
            Ok(None) => {}
            Err((path, error)) => {
                tracing::warn!("{} が読めない: {error}", path.display());
                notice(format!(
                    "チャンネル一覧（{CHANNELS}）を開けません。\n{error}"
                ));
            }
        }
        for set in sets {
            match parse_set(set) {
                Ok(parsed) => config.sets.push(parsed),
                Err(why) => notice(format!("コマンドライン引数 --set を無視しました。\n{why}")),
            }
        }
        config
    }

    #[cfg(test)]
    pub fn of_text(text: &str, sets: &[&str]) -> Result<Self, String> {
        let mut config = Self {
            table: toml::from_str(text).map_err(|error| error.to_string())?,
            found: true,
            ..Self::default()
        };
        for set in sets {
            config.sets.push(parse_set(set)?);
        }
        Ok(config)
    }

    pub fn plain(&self) -> Result<File, String> {
        self.table
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| format!("{FILE}: {error}"))
    }

    pub fn effective(&self, profile: &str) -> Result<File, String> {
        let plain = self.plain()?;
        let chosen = plain.profile(profile);
        let mut table = self.table.clone();
        overlay(&mut table, &chosen.overrides);
        for (path, value) in &self.sets {
            set_path(&mut table, path, value.clone())?;
        }
        table
            .try_into()
            .map_err(|error: toml::de::Error| format!("{FILE}: {error}"))
    }

    pub fn profile_name(&self, asked: Option<&str>) -> String {
        let name = asked
            .map(str::to_owned)
            .unwrap_or_else(|| self.state.profile.clone());
        if name.is_empty() {
            return name;
        }
        match self.plain() {
            Ok(file) if file.profiles.iter().any(|profile| profile.name == name) => name,
            _ => {
                tracing::info!("プロファイル「{name}」が {FILE} に無い。全 source を記述順に使う");
                String::new()
            }
        }
    }
}

fn overlay(table: &mut toml::Table, overrides: &std::collections::BTreeMap<String, toml::Value>) {
    for (section, value) in overrides {
        if let toml::Value::Table(over) = value {
            let base = table
                .entry(section.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(base) = base {
                merge(base, over);
            }
        }
    }
}

pub fn layered(file: &File, profile: &str) -> File {
    let chosen = file.profile(profile);
    if chosen.overrides.is_empty() {
        return file.clone();
    }
    let Ok(mut table) = toml::Table::try_from(file) else {
        return file.clone();
    };
    overlay(&mut table, &chosen.overrides);
    table.try_into().unwrap_or_else(|why: toml::de::Error| {
        tracing::warn!("[profile.{profile}] の上書きが読めない: {why}");
        file.clone()
    })
}

fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (name, value) in over {
        match (base.get_mut(name), value) {
            (Some(toml::Value::Table(into)), toml::Value::Table(from)) => merge(into, from),
            (_, value) => {
                base.insert(name.clone(), value.clone());
            }
        }
    }
}

/// `--set キー=値` の一つ。値は TOML として読み、読めなければ文字列。
fn parse_set(text: &str) -> Result<(String, toml::Value), String> {
    let (path, value) = text
        .split_once('=')
        .ok_or_else(|| format!("--set は セクション.キー=値 の形: {text}"))?;
    let path = path.trim();
    if path.is_empty() {
        return Err(format!("--set のキーが空: {text}"));
    }
    let value = value.trim();
    let parsed = format!("v = {value}")
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut table| table.remove("v"))
        .unwrap_or_else(|| toml::Value::String(value.to_owned()));
    Ok((path.to_owned(), parsed))
}

fn set_path(table: &mut toml::Table, path: &str, value: toml::Value) -> Result<(), String> {
    let mut parts = path.split('.').peekable();
    let mut here = table;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            here.insert(part.to_owned(), value);
            return Ok(());
        }
        let next = here
            .entry(part.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        here = match next {
            toml::Value::Table(inner) => inner,
            _ => return Err(format!("--set {path}: 「{part}」はテーブルでない")),
        };
    }
    Ok(())
}

fn read(name: &str) -> Result<Option<String>, String> {
    read_io(name).map_err(|(path, error)| format!("{} が読めない: {error}", path.display()))
}

fn read_io(name: &str) -> Result<Option<String>, (std::path::PathBuf, std::io::Error)> {
    let Some(path) = crate::state::file(name) else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err((path, error)),
    }
}

/// 読めないファイルを `<名前>.bad` へ退避する（そのまま書くとお気に入りやスキャンの結果が消える）。
fn set_aside(name: &str, error: &str) {
    tracing::warn!("{name} が読めない: {error}");
    let Some(path) = path_of(name) else {
        return;
    };
    let mut bad = path.file_name().unwrap_or_default().to_os_string();
    bad.push(".bad");
    let bad = path.with_file_name(bad);
    match std::fs::rename(&path, &bad) {
        Ok(()) => notice(format!(
            "{name} を読み込めませんでした。\n{} に退避して、既定の内容で起動しました。\n{error}",
            bad.display()
        )),
        Err(why) => notice(format!(
            "{name} を読み込めませんでした。\n退避もできませんでした（{why}）。\n{error}"
        )),
    }
}

pub fn modified() -> Option<std::time::SystemTime> {
    let path = path_of(FILE)?;
    std::fs::metadata(path).ok()?.modified().ok()
}

pub fn merge_reload(loaded: &File, edited: &File, fresh: &File) -> File {
    let pending = changes(loaded, edited);
    if pending.is_empty() {
        return fresh.clone();
    }
    let Ok(mut table) = toml::Table::try_from(fresh) else {
        return fresh.clone();
    };
    for change in pending {
        let Some((leaf, sections)) = change.path.split_last() else {
            continue;
        };
        fn descend<'t>(
            table: &'t mut toml::Table,
            sections: &[String],
        ) -> Option<&'t mut toml::Table> {
            let mut at = table;
            for section in sections {
                let entry = at
                    .entry(section.clone())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                at = entry.as_table_mut()?;
            }
            Some(at)
        }
        if let Some(at) = descend(&mut table, sections) {
            at.insert(leaf.clone(), change.value);
        }
    }
    table.try_into().unwrap_or_else(|_| fresh.clone())
}

static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_restart() {
    RESTART.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn restart_requested() -> bool {
    RESTART.load(std::sync::atomic::Ordering::Relaxed)
}

static FIRST_RUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_first_run() {
    FIRST_RUN.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn first_run() -> bool {
    FIRST_RUN.load(std::sync::atomic::Ordering::Relaxed)
}

static NOTICES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn notice(text: impl Into<String>) {
    let text = text.into();
    tracing::info!("{}", text.replace('\n', " / "));
    if let Ok(mut notices) = NOTICES.lock() {
        notices.push(text);
    }
}

pub fn take_notices() -> Vec<String> {
    NOTICES
        .lock()
        .map(|mut n| std::mem::take(&mut *n))
        .unwrap_or_default()
}

static OVERRIDDEN: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub fn set_overridden(keys: Vec<String>) {
    let _ = OVERRIDDEN.set(keys);
}

pub fn overridden() -> &'static [String] {
    OVERRIDDEN.get().map_or(&[], Vec::as_slice)
}

pub fn load_plain() -> File {
    match read(FILE) {
        Ok(Some(text)) => match File::parse(&text) {
            Ok(file) => file,
            Err(why) => {
                tracing::warn!("{FILE}: {why}");
                File::default()
            }
        },
        Ok(None) => File::default(),
        Err(why) => {
            tracing::warn!("{why}");
            File::default()
        }
    }
}

pub fn path_of(name: &str) -> Option<PathBuf> {
    crate::state::file(name)
}

pub fn write_whole(path: &std::path::Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path.with_file_name(name);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// この deck だけが動いていることを示すロックを取る。ほかの deck が持っていれば偽、ロックが使えなければ真。
pub fn claim_single() -> bool {
    let Some(path) = path_of("zerowave.instance") else {
        return true;
    };
    match lock(&path, false) {
        Ok(Some(held)) => {
            *SINGLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(held);
            true
        }
        Ok(None) => false,
        Err(_) => true,
    }
}

/// 二重起動防止のロック。終わるまで持つ。
static SINGLE: std::sync::Mutex<Option<coax::lock::Held>> = std::sync::Mutex::new(None);

/// 二重起動防止のロックを放す（再起動するとき・終えるとき）。
pub fn release_single() {
    SINGLE.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub fn lock_file(name: &str) -> Option<coax::lock::Held> {
    lock(&path_of(name)?, true).ok().flatten()
}

/// このファイルのロック（`wait` なら保持しているプロセスが放すのを待つ）。設定のフォルダーには何も置かない。
pub fn lock(path: &std::path::Path, wait: bool) -> std::io::Result<Option<coax::lock::Held>> {
    let name = path.to_string_lossy();
    // Windows では大文字と小文字の違う二つの書き方が同じファイル。
    let name = if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.into_owned()
    };
    coax::lock::take(&format!("zerowave-{name}"), wait)
}

pub fn save_state(state: &state::State) {
    let Some(path) = path_of(STATE) else {
        return;
    };
    if let Err(error) = write_whole(&path, state.write()) {
        tracing::warn!("{} に書けない: {error}", path.display());
    }
}

pub fn load_state() -> state::State {
    match read(STATE) {
        Ok(Some(text)) => state::State::parse(&text).unwrap_or_else(|error| {
            set_aside(STATE, &error.to_string());
            state::State::default()
        }),
        _ => state::State::default(),
    }
}

pub fn channels_profile(profile: &str) -> &str {
    if profile.is_empty() {
        crate::ui::panes::settings::ALL
    } else {
        profile
    }
}

pub fn load_channels() -> channels::Channels {
    load_channels_of(&load_state().profile)
}

pub fn load_channels_of(profile: &str) -> channels::Channels {
    let profile = channels_profile(profile);
    match read(CHANNELS) {
        Ok(Some(text)) => channels::Channels::parse(&text, profile).unwrap_or_else(|error| {
            set_aside(CHANNELS, &error.to_string());
            channels::Channels::of(profile)
        }),
        _ => channels::Channels::of(profile),
    }
}

/// チャンネルマップを書く。変更した方式のテーブルだけ差し替える（コメントと並びを残す）。
pub fn save_channels(channels: &channels::Channels) {
    let Some(path) = path_of(CHANNELS) else {
        return;
    };
    let had = read(CHANNELS).ok().flatten().unwrap_or_default();
    if let Err(error) = write_whole(&path, channels.written_over(&had)) {
        tracing::warn!("{} に書けない: {error}", path.display());
    }
}

/// そのプロファイルで使うチャンネルマップ：自分の結果の無い方式のみ all の結果を足した物。
/// 読むためだけの物（書くと all の結果がそのプロファイルにコピーされる）。
pub fn load_channels_used(profile: &str) -> channels::Channels {
    let mut own = load_channels_of(profile);
    if own.profile != crate::ui::panes::settings::ALL {
        for (plan, line) in load_channels_of("").lines {
            own.lines.entry(plan).or_insert(line);
        }
    }
    own
}

pub fn delete_channels_line(profile: &str, plan: &str) {
    let profile = channels_profile(profile);
    let _lock = lock_file(CHANNELS);
    let Some(path) = path_of(CHANNELS) else {
        return;
    };
    let Some(had) = read(CHANNELS).ok().flatten() else {
        return;
    };
    let Some(left) = channels::Channels::removed(&had, profile, plan) else {
        return;
    };
    match write_whole(&path, left) {
        Ok(()) => tracing::info!("プロファイル「{profile}」の {plan} のスキャン結果を削除した"),
        Err(error) => tracing::warn!("{} に書けない: {error}", path.display()),
    }
}

pub fn copy_channels_lines(from: &str, to: &str, plans: &[String]) {
    let source = load_channels_of(from);
    let plans: Vec<(String, channels::LineTable)> = plans
        .iter()
        .filter_map(|plan| Some((plan.clone(), source.lines.get(plan)?.clone())))
        .collect();
    if plans.is_empty() {
        return;
    }
    update_channels_of(to, move |channels| {
        for (plan, line) in plans {
            channels.lines.insert(plan, line);
        }
    });
}

pub fn rename_channels_profile(from: &str, to: &str) {
    let _lock = lock_file(CHANNELS);
    let Some(path) = path_of(CHANNELS) else {
        return;
    };
    let Some(had) = read(CHANNELS).ok().flatten() else {
        return;
    };
    let Some(moved) = channels::Channels::renamed(&had, from, to) else {
        return;
    };
    match write_whole(&path, moved) {
        Ok(()) => tracing::info!("スキャン結果をプロファイル「{from}」から「{to}」へ移した"),
        Err(error) => tracing::warn!("{} に書けない: {error}", path.display()),
    }
}

pub fn update_channels_of(profile: &str, change: impl FnOnce(&mut channels::Channels)) {
    let _lock = lock_file(CHANNELS);
    let mut channels = load_channels_of(profile);
    change(&mut channels);
    save_channels(&channels);
}

/// 状態を読み直して一部を変え、書く。ロックで一度に一つ（同時に書くと先に書いた方が消える）。
pub fn update_state(change: impl FnOnce(&mut state::State)) {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let _lock = lock_file(STATE);
    // 読めないファイルの上に既定を書かない（中身が全部消える）。
    let mut state = match read(STATE) {
        Ok(Some(text)) => match state::State::parse(&text) {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!("{STATE} が読めないため保存しない: {error}");
                return;
            }
        },
        Ok(None) => state::State::default(),
        Err(error) => {
            tracing::warn!("{STATE} が読めないため保存しない: {error}");
            return;
        }
    };
    change(&mut state);
    save_state(&state);
}

pub fn remember_last(key: zwcore::guide::Key) {
    update_state(|state| {
        state.last = Some(key);
        let profile = state.profile.clone();
        state.watched(&profile, key);
    });
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: Vec<String>,
    pub value: toml::Value,
    pub is_default: bool,
}

impl Change {
    fn shown(&self) -> String {
        self.path.join(".")
    }
}

/// 二つの `File` で値が違うキー。テーブルの配列は丸ごと一つの値として比べる。
pub fn changes(loaded: &File, edited: &File) -> Vec<Change> {
    let (Ok(was), Ok(now), Ok(default)) = (
        toml::Table::try_from(loaded),
        toml::Table::try_from(edited),
        toml::Table::try_from(File::default()),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(&mut Vec::new(), &was, &now, &default, &mut out);
    for change in &mut out {
        if let ([key], toml::Value::Array(items)) = (change.path.as_slice(), &mut change.value)
            && let Some(base) = element_default(key)
        {
            for item in items {
                if let toml::Value::Table(fields) = item {
                    strip_defaults(fields, &base);
                }
            }
        }
    }
    out
}

fn element_default(key: &str) -> Option<toml::Table> {
    match key {
        "line" => toml::Table::try_from(file::Line::default()).ok(),
        "source" => toml::Table::try_from(file::Source::default()).ok(),
        "profile" => toml::Table::try_from(file::Profile::default()).ok(),
        _ => None,
    }
}

fn strip_defaults(fields: &mut toml::Table, base: &toml::Table) {
    let empty = toml::Table::new();
    fields.retain(|key, value| {
        if let toml::Value::Table(inner) = value {
            let inner_base = match base.get(key) {
                Some(toml::Value::Table(table)) => table,
                _ => &empty,
            };
            strip_defaults(inner, inner_base);
            return !inner.is_empty();
        }
        base.get(key) != Some(value)
    });
}

fn walk(
    path: &mut Vec<String>,
    was: &toml::Table,
    now: &toml::Table,
    default: &toml::Table,
    out: &mut Vec<Change>,
) {
    for (key, value) in now {
        let before = was.get(key);
        let base = default.get(key);
        path.push(key.clone());
        match (value, before) {
            (toml::Value::Table(now_table), Some(toml::Value::Table(was_table))) => {
                let empty = toml::Table::new();
                let base_table = match base {
                    Some(toml::Value::Table(table)) => table,
                    _ => &empty,
                };
                walk(path, was_table, now_table, base_table, out);
            }
            _ => {
                if before != Some(value) {
                    out.push(Change {
                        path: path.clone(),
                        value: value.clone(),
                        is_default: base == Some(value),
                    });
                }
            }
        }
        path.pop();
    }
    for key in was.keys() {
        if now.contains_key(key) {
            continue;
        }
        path.push(key.clone());
        out.push(Change {
            path: path.clone(),
            value: toml::Value::Boolean(false),
            is_default: true,
        });
        path.pop();
    }
}

/// 変えたキーだけを `zerowave.toml` に書く。コメントと並びは `toml_edit` が保つ。
pub fn save_changes(changes: &[Change]) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    let Some(path) = path_of(FILE) else {
        return Err(String::from("設定を置ける場所が無い"));
    };
    let text = read(FILE)?.unwrap_or_default();
    let written = apply_changes(&text, changes)?;
    write_whole(&path, written).map_err(|error| format!("{} に書けない: {error}", path.display()))
}

pub fn apply_changes(text: &str, changes: &[Change]) -> Result<String, String> {
    use toml_edit::{DocumentMut, Item, Table};
    let mut document: DocumentMut = text.parse().map_err(|error| format!("{FILE}: {error}"))?;
    let mut emptied: Vec<Vec<String>> = Vec::new();
    for change in changes {
        let Some((leaf, sections)) = change.path.split_last() else {
            continue;
        };
        let mut table: &mut Table = document.as_table_mut();
        let mut missing = false;
        for section in sections {
            if change.is_default && table.get(section).is_none() {
                missing = true;
                break;
            }
            let entry = table.entry(section).or_insert_with(|| {
                let mut fresh = Table::new();
                fresh.set_implicit(true);
                Item::Table(fresh)
            });
            table = match entry.as_table_mut() {
                Some(table) => table,
                None => {
                    return Err(format!(
                        "{FILE}: {} はテーブルではないので {} を置けない",
                        section,
                        change.shown()
                    ));
                }
            };
        }
        if missing {
            continue;
        }
        if change.is_default {
            table.remove(leaf);
            emptied.push(sections.to_vec());
        } else if INLINE_LISTS.contains(&change.path.join(".").as_str())
            && let toml::Value::Array(items) = &change.value
        {
            table[leaf] = toml_edit::value(
                inline_list(items)
                    .ok_or_else(|| format!("{FILE}: {} は書ける形ではない", change.shown()))?,
            );
        } else if let Some(tables) = array_of_tables(&change.value) {
            table[leaf] = Item::ArrayOfTables(tables?);
        } else {
            let value = edit_value(&change.value)
                .ok_or_else(|| format!("{FILE}: {} は書ける形ではない", change.shown()))?;
            table[leaf] = toml_edit::value(value);
        }
    }
    for path in emptied {
        prune(document.as_table_mut(), &path);
    }
    Ok(document.to_string())
}

const INLINE_LISTS: &[&str] = &["ui.status_items"];

fn inline_list(items: &[toml::Value]) -> Option<toml_edit::Value> {
    let mut array = toml_edit::Array::new();
    for item in items {
        let mut value = edit_value(item)?;
        value.decor_mut().set_prefix("\n    ");
        array.push_formatted(value);
    }
    array.set_trailing_comma(true);
    array.set_trailing("\n");
    Some(toml_edit::Value::from(array))
}

fn array_of_tables(value: &toml::Value) -> Option<Result<toml_edit::ArrayOfTables, String>> {
    let toml::Value::Array(items) = value else {
        return None;
    };
    if items.is_empty() || !items.iter().all(|item| item.is_table()) {
        return None;
    }
    let mut out = toml_edit::ArrayOfTables::new();
    for item in items {
        let toml::Value::Table(fields) = item else {
            return None;
        };
        match edit_table(fields) {
            Some(table) => out.push(table),
            None => {
                return Some(Err(format!(
                    "{FILE}: テーブルの配列の中に書ける形ではない値がある"
                )));
            }
        }
    }
    Some(Ok(out))
}

/// テーブルの配列の一要素。中のテーブルは見出しの付いたテーブルに（行内だと人が読めない）。
fn edit_table(fields: &toml::Table) -> Option<toml_edit::Table> {
    let mut table = toml_edit::Table::new();
    for (key, field) in fields {
        table[key] = match field {
            toml::Value::Table(inner) => toml_edit::Item::Table(edit_table(inner)?),
            other => toml_edit::value(edit_value(other)?),
        };
    }
    Some(table)
}

fn edit_value(value: &toml::Value) -> Option<toml_edit::Value> {
    Some(match value {
        toml::Value::String(text) => toml_edit::Value::from(text.as_str()),
        toml::Value::Integer(number) => toml_edit::Value::from(*number),
        toml::Value::Float(number) => toml_edit::Value::from(*number),
        toml::Value::Boolean(flag) => toml_edit::Value::from(*flag),
        toml::Value::Datetime(when) => toml_edit::Value::from(*when),
        toml::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(edit_value(item)?);
            }
            toml_edit::Value::from(array)
        }
        toml::Value::Table(table) => {
            let mut inline = toml_edit::InlineTable::new();
            for (key, item) in table {
                inline.insert(key, edit_value(item)?);
            }
            toml_edit::Value::from(inline)
        }
    })
}

fn prune(root: &mut toml_edit::Table, path: &[String]) {
    let Some((leaf, parents)) = path.split_last() else {
        return;
    };
    let empty = {
        let mut table = &mut *root;
        for section in parents {
            match table.get_mut(section).and_then(|item| item.as_table_mut()) {
                Some(inner) => table = inner,
                None => return,
            }
        }
        let empty = table
            .get(leaf)
            .and_then(|item| item.as_table())
            .is_some_and(|inner| inner.is_empty());
        if empty {
            table.remove(leaf);
        }
        empty
    };
    if empty {
        prune(root, parents);
    }
}

pub const TEMPLATE: &str = "# ZeroWave の設定。無いキーは既定の値。\n";

pub fn write_template() -> Option<PathBuf> {
    let path = path_of(FILE)?;
    if path.exists() {
        return Some(path);
    }
    write_whole(&path, TEMPLATE).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_keys_that_differ_are_changes() {
        let loaded = File::default();
        let mut edited = loaded.clone();
        assert!(changes(&loaded, &edited).is_empty());
        edited.audio.downmix = String::from("off");
        edited.general.volume = 40;
        let found = changes(&loaded, &edited);
        assert_eq!(found.len(), 2, "{found:?}");
        let downmix = found
            .iter()
            .find(|c| c.path == ["audio", "downmix"])
            .unwrap();
        assert_eq!(downmix.value, toml::Value::String(String::from("off")));
        assert!(!downmix.is_default);
        let mut loaded = File::default();
        loaded.audio.downmix = String::from("off");
        let edited = File::default();
        let found = changes(&loaded, &edited);
        assert_eq!(found.len(), 1);
        assert!(found[0].is_default);
    }

    #[test]
    fn writing_keeps_comments_and_removes_what_went_back_to_default() {
        let text = "# 私の設定\n[audio]\nlatency = 10   # サウンドバー\ndownmix = \"off\"\n\n[video.decoder]\ndefault = \"software\"\n";
        let changes = vec![
            Change {
                path: vec![String::from("audio"), String::from("downmix")],
                value: toml::Value::String(String::from("auto")),
                is_default: true,
            },
            Change {
                path: vec![String::from("audio"), String::from("mono_to_stereo")],
                value: toml::Value::Boolean(false),
                is_default: false,
            },
            Change {
                path: vec![String::from("general"), String::from("volume")],
                value: toml::Value::Integer(40),
                is_default: false,
            },
        ];
        let written = apply_changes(text, &changes).unwrap();
        assert!(written.starts_with("# 私の設定\n"), "{written}");
        assert!(
            written.contains("latency = 10   # サウンドバー"),
            "{written}"
        );
        assert!(!written.contains("downmix"), "{written}");
        assert!(written.contains("mono_to_stereo = false"), "{written}");
        assert!(written.contains("[general]\nvolume = 40"), "{written}");
        assert!(
            written.contains("[video.decoder]\ndefault = \"software\""),
            "{written}"
        );
        let file = File::parse(&written).unwrap();
        assert_eq!(file.general.volume, 40);
        assert!(!file.audio.mono_to_stereo);
        assert_eq!(file.audio.downmix, "auto");
        let only = "[audio]\ndownmix = \"off\"\n";
        let written = apply_changes(only, &changes[..1]).unwrap();
        assert!(!written.contains("[audio]"), "{written:?}");
        let written = apply_changes("", &changes[1..]).unwrap();
        assert!(
            written.contains("[audio]\nmono_to_stereo = false"),
            "{written}"
        );
    }

    #[test]
    fn a_removed_map_entry_is_removed_from_the_file() {
        let mut loaded = File::default();
        loaded
            .keys
            .insert(String::from("m"), String::from("nothing"));
        loaded
            .keys
            .insert(String::from("ctrl+up"), String::from("channel.up"));
        let mut edited = loaded.clone();
        edited.keys.remove("m");
        let changes = changes(&loaded, &edited);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].is_default);
        let text = "[keys]\nm = \"nothing\"\n\"ctrl+up\" = \"channel.up\"\n";
        let written = apply_changes(text, &changes).unwrap();
        assert!(!written.contains("m = "), "{written}");
        assert!(
            written.contains("\"ctrl+up\" = \"channel.up\""),
            "{written}"
        );
    }

    #[test]
    fn a_profiles_channel_prefs_are_written_under_its_own_heading() {
        let loaded = File::default();
        let mut edited = File::default();
        let mut name = toml::Table::new();
        name.insert(
            "0x7fef:1024".into(),
            toml::Value::String("ゼロウェーブ".into()),
        );
        let mut channel = toml::Table::new();
        channel.insert("name".into(), toml::Value::Table(name));
        channel.insert(
            "hide".into(),
            toml::Value::Array(vec![toml::Value::String("0x7fe8:2048".into())]),
        );
        let mut profile = file::Profile {
            name: String::from("遠く"),
            ..Default::default()
        };
        profile
            .overrides
            .insert("channel".into(), toml::Value::Table(channel));
        edited.profiles.push(profile);
        let written = apply_changes("", &changes(&loaded, &edited)).unwrap();
        assert!(
            written.contains("[profile.channel.name]"),
            "見出しで書く: {written}"
        );
        let read = File::parse(&written).unwrap();
        assert_eq!(read.profiles, edited.profiles);
        assert_eq!(
            layered(&read, "遠く").channel.name["0x7fef:1024"],
            "ゼロウェーブ"
        );
    }

    #[test]
    fn sources_are_written_as_an_array_of_tables() {
        let loaded = File::default();
        let mut edited = File::default();
        edited.sources.push(file::Source {
            label: String::from("home"),
            kind: String::from("mirakurun"),
            url: String::from("http://192.0.2.16:40772"),
            ..Default::default()
        });
        edited.profiles.push(file::Profile {
            name: String::from("自宅"),
            sources: vec![String::from("home")],
            ..Default::default()
        });
        let changes = changes(&loaded, &edited);
        assert_eq!(changes.len(), 2, "{changes:?}");
        let written = apply_changes("# 私の設定\n[audio]\nlatency = 10\n", &changes).unwrap();
        assert!(written.contains("[[source]]\n"), "{written}");
        assert!(written.contains("label = \"home\""), "{written}");
        assert!(written.contains("[[profile]]\n"), "{written}");
        assert!(
            !written.contains("source = ["),
            "行内の配列にしない: {written}"
        );
        for unset in ["endpoint", "priority", "tuner", "by ="] {
            assert!(!written.contains(unset), "{unset} を書かない: {written}");
        }
        let read = File::parse(&written).unwrap();
        assert_eq!(read.sources, edited.sources);
        assert_eq!(read.profiles, edited.profiles);
        assert_eq!(read.audio.latency, 10);
        let gone = changes_between(&read, &File::default());
        let written = apply_changes(&written, &gone).unwrap();
        assert!(!written.contains("[[source]]"), "{written}");
        assert!(File::parse(&written).unwrap().sources.is_empty());
    }

    #[test]
    fn status_items_are_written_inline_under_ui() {
        let loaded = File::default();
        let mut edited = File::default();
        let mut list = file::StatusItem::defaults();
        list.rotate_left(1);
        list[0].width = 150;
        list[1].shown = false;
        edited.ui.set_status_list(list.clone());
        edited.ui.status_multi_row = true;
        let changes = changes(&loaded, &edited);
        let written = apply_changes("[ui]\ntheme = \"dark\"\n", &changes).unwrap();
        assert!(!written.contains("[[ui.status_items]]"), "{written}");
        assert!(
            written.contains("\n    { id = \"channel\", shown = true, width = 150 },\n"),
            "{written}"
        );
        assert!(written.contains("status_multi_row = true"), "{written}");
        let read = File::parse(&written).unwrap();
        assert_eq!(read.ui.status_list(), list);
        assert_eq!(read.ui.theme, "dark");
        let gone = changes_between(&read, &File::default());
        let written = apply_changes(&written, &gone).unwrap();
        assert!(!written.contains("status_items"), "{written}");
    }

    fn changes_between(loaded: &File, edited: &File) -> Vec<Change> {
        changes(loaded, edited)
    }

    #[test]
    fn a_reload_keeps_what_the_window_has_not_applied_yet() {
        let loaded = File::default();
        let mut edited = loaded.clone();
        edited.audio.downmix = String::from("off");
        let mut fresh = File::default();
        fresh.audio.latency = 120;
        fresh.audio.downmix = String::from("stereo");
        let merged = merge_reload(&loaded, &edited, &fresh);
        assert_eq!(merged.audio.latency, 120);
        assert_eq!(merged.audio.downmix, "off");
        assert_eq!(merge_reload(&loaded, &loaded, &fresh), fresh);
    }

    #[test]
    fn a_profile_overrides_only_what_it_says() {
        let config = Config::of_text(
            r#"
[audio]
latency = 10
dual_mono = "sub"
[[source]]
label = "far"
kind = "mirakurun"
[[profile]]
name = "遠く"
sources = ["far"]
[profile.audio]
latency = 200
"#,
            &[],
        )
        .unwrap();
        let far = config.effective("遠く").unwrap();
        assert_eq!(far.audio.latency, 200);
        assert_eq!(far.audio.dual_mono, "sub", "指定していないキーは元のまま");
        assert_eq!(config.effective("").unwrap().audio.latency, 10);
        assert_eq!(config.plain().unwrap().profiles.len(), 1);
    }

    #[test]
    fn a_profile_layers_its_channel_prefs_over_the_common_ones() {
        let config = Config::of_text(
            r#"
[channel]
hide = ["0x7fef:1032"]
[channel.name]
"0x7fef:1024" = "総合"
"0x7fef:1025" = "教育"
[[source]]
label = "far"
kind = "mirakurun"
[[profile]]
name = "遠く"
sources = ["far"]
[profile.channel]
hide = ["0x7fe8:2048"]
[profile.channel.name]
"0x7fef:1024" = "ゼロウェーブ"
[profile.channel.key]
"0x7fef:1024" = 3
"#,
            &[],
        )
        .unwrap();
        let plain = config.plain().unwrap();
        let far = layered(&plain, "遠く");
        assert_eq!(far, config.effective("遠く").unwrap(), "起動時と同じ");
        assert_eq!(far.channel.name["0x7fef:1024"], "ゼロウェーブ");
        assert_eq!(
            far.channel.name["0x7fef:1025"], "教育",
            "指定していない局は共通のまま"
        );
        assert_eq!(far.channel.key["0x7fef:1024"], 3);
        assert_eq!(far.channel.hide, ["0x7fe8:2048"], "非表示は置き換わる");
        assert_eq!(layered(&plain, ""), plain);
    }

    #[test]
    fn set_comes_after_the_profile_and_takes_bare_words() {
        let config = Config::of_text(
            "[audio]\nlatency = 10\n",
            &[
                "audio.latency=300",
                "audio.device=hw:1",
                "video.decoder.default=software",
                "audio.downmix=off",
            ],
        )
        .unwrap();
        let file = config.effective("").unwrap();
        assert_eq!(file.audio.latency, 300);
        assert_eq!(file.audio.device, "hw:1");
        assert_eq!(file.video.decoder.default, "software");
        assert_eq!(file.audio.downmix, "off");
        assert!(Config::of_text("", &["nonsense"]).is_err());
        assert!(Config::of_text("", &["=1"]).is_err());
        let config = Config::of_text("audio = 1\n", &["audio.latency=1"]).unwrap();
        assert!(config.effective("").is_err());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_an_error() {
        let config = Config::of_text("[audio]\nlatency = \"slow\"\n", &[]).unwrap();
        let error = config.effective("").unwrap_err();
        assert!(error.contains(FILE), "{error}");
    }

    #[test]
    fn the_template_is_all_comments() {
        let config = Config::of_text(TEMPLATE, &[]).unwrap();
        assert_eq!(config.effective("").unwrap(), File::default());
    }

    #[test]
    fn a_profile_that_is_not_there_falls_back_to_the_implicit_one() {
        let mut config = Config::of_text("[[profile]]\nname = \"a\"\n", &[]).unwrap();
        assert_eq!(config.profile_name(Some("a")), "a");
        assert_eq!(config.profile_name(Some("b")), "");
        config.state.profile = String::from("a");
        assert_eq!(config.profile_name(None), "a");
    }
}
