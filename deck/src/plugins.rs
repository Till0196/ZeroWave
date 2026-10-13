// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグイン（wasm）のフォルダー。場所は `ZW_PLUGINS` > `[data] plugins` > 設定と同じ場所 > 本体と同じフォルダー。

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use zwcore::plugins::Loaded;

static LOADED: RwLock<Option<Arc<Loaded>>> = RwLock::new(None);

pub fn loaded() -> Arc<Loaded> {
    if let Some(loaded) = LOADED.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return loaded;
    }
    // ロックを取ってからビルドする（読み取りロックの後だと二つのスレッドが同時にビルドし、空のセットが残る）。
    let mut guard = LOADED.write().unwrap_or_else(|e| e.into_inner());
    if let Some(loaded) = guard.as_ref() {
        return Arc::clone(loaded);
    }
    let made = Arc::new(match folder() {
        Some(folder) => {
            tracing::info!("プラグインのフォルダー {}", folder.display());
            Loaded::load_except(
                &folder,
                &DISABLED.read().unwrap_or_else(|e| e.into_inner()),
                &mut |line| tracing::info!("{line}"),
            )
            .with_io(crate::config::plugins::io_of())
        }
        None => {
            tracing::info!("プラグインのフォルダーが不明");
            Loaded::default()
        }
    });
    *guard = Some(Arc::clone(&made));
    zwcore::plugins::publish(Arc::clone(&made));
    made
}

fn install(now: Loaded) -> bool {
    let now = Arc::new(now);
    let streams = |loaded: &Loaded| -> Vec<*const zwsdk::host::Plugin> {
        loaded
            .all()
            .iter()
            .filter(|plugin| plugin.has("stream"))
            .map(Arc::as_ptr)
            .collect()
    };
    let mut guard = LOADED.write().unwrap_or_else(|e| e.into_inner());
    let changed = guard
        .as_deref()
        .is_none_or(|had| streams(had) != streams(&now));
    *guard = Some(Arc::clone(&now));
    drop(guard);
    zwcore::plugins::publish(now);
    changed
}

static RELOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn reloading() -> bool {
    RELOADING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn expect_reload() {
    RELOADING.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn reconfigure(folder: &str, disabled: &[String]) -> bool {
    RELOADING.store(true, std::sync::atomic::Ordering::Relaxed);
    let changed = reload(folder, disabled);
    RELOADING.store(false, std::sync::atomic::Ordering::Relaxed);
    changed
}

fn reload(folder: &str, disabled: &[String]) -> bool {
    let moved = {
        let mut preferred = PREFERRED.write().unwrap_or_else(|e| e.into_inner());
        let wanted = (!folder.is_empty()).then(|| PathBuf::from(folder));
        let moved = *preferred != wanted;
        *preferred = wanted;
        moved
    };
    *DISABLED.write().unwrap_or_else(|e| e.into_inner()) = disabled.to_vec();
    let Some(place) = self::folder() else {
        return install(Loaded::default());
    };
    if moved {
        tracing::info!("プラグインのフォルダー {}", place.display());
        let made = Loaded::load_except(&place, disabled, &mut |line| tracing::info!("{line}"))
            .with_io(crate::config::plugins::io_of());
        return install(made);
    }
    let had = loaded();
    let mut now = Loaded::clone_of(&had);
    for plugin in had.all() {
        let stem = stem_of(plugin);
        if disabled.contains(&stem) {
            tracing::info!("プラグイン {stem} を無効にした");
            now = now.without(&stem);
        }
    }
    for stem in available() {
        if disabled.contains(&stem) || now.all().iter().any(|plugin| stem_of(plugin) == stem) {
            continue;
        }
        let path = [place.join(format!("{stem}.wasm")), place.join(&stem)]
            .into_iter()
            .find(|path| path.exists());
        let Some(path) = path else {
            continue;
        };
        match now.plus(&path, &mut |line| tracing::info!("{line}")) {
            Ok(plus) => now = plus,
            Err(why) => tracing::warn!("プラグイン {why}"),
        }
    }
    install(now)
}

pub fn stem_of(plugin: &zwsdk::host::Plugin) -> String {
    plugin
        .path()
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

pub fn named(stem: &str) -> Option<Arc<zwsdk::host::Plugin>> {
    loaded()
        .all()
        .iter()
        .find(|plugin| stem_of(plugin) == stem)
        .cloned()
}

pub fn shown_name(stem: &str) -> String {
    if let Some(plugin) = named(stem) {
        return tidy_name(plugin.name(), stem);
    }
    let Some(folder) = folder() else {
        return stem.to_owned();
    };
    let Some(path) = [folder.join(format!("{stem}.wasm")), folder.join(stem)]
        .into_iter()
        .find(|path| path.exists())
    else {
        return stem.to_owned();
    };
    // 一覧を表示するたびに呼ばれるので、読んだ物はファイルの更新時刻と一緒にキャッシュする（wasm は数十 MB ある）。
    type Seen = std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, String)>;
    static SEEN: std::sync::LazyLock<std::sync::Mutex<Seen>> =
        std::sync::LazyLock::new(Default::default);
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, name)) = seen.get(&path)
        && *when == modified
    {
        return name.clone();
    }
    let name = zwsdk::host::manifest_at(&path).map_or_else(
        || stem.to_owned(),
        |manifest| tidy_name(&manifest.name, stem),
    );
    seen.insert(path, (modified, name.clone()));
    name
}

/// ウィンドウのタイトルや一覧に出す名前。制御文字を除き、長すぎれば切る。空ならファイル名。
fn tidy_name(name: &str, stem: &str) -> String {
    let tidy: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
    let tidy = tidy.trim();
    if tidy.is_empty() {
        stem.to_owned()
    } else {
        tidy.to_owned()
    }
}

pub fn add(from: &std::path::Path) -> Result<(String, bool), String> {
    let Some(folder) = folder() else {
        return Err(String::from("プラグインフォルダーが分からない"));
    };
    let name = from
        .file_name()
        .ok_or_else(|| String::from("ファイルの名前が読めない"))?;
    let stem = from
        .file_stem()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    if !from
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
    {
        return Err(String::from("wasm ではない"));
    }
    let _ = std::fs::create_dir_all(&folder);
    let to = folder.join(name);
    if to != from {
        std::fs::copy(from, &to)
            .map_err(|error| format!("{} にコピーできない: {error}", to.display()))?;
    }
    let had = loaded();
    let now = had.plus(&to, &mut |line| tracing::info!("{line}"))?;
    Ok((stem, install(now)))
}

pub fn prefer(folder: &str, disabled: &[String]) {
    if !folder.is_empty() {
        *PREFERRED.write().unwrap_or_else(|e| e.into_inner()) = Some(PathBuf::from(folder));
    }
    *DISABLED.write().unwrap_or_else(|e| e.into_inner()) = disabled.to_vec();
}

static PREFERRED: RwLock<Option<PathBuf>> = RwLock::new(None);
static DISABLED: RwLock<Vec<String>> = RwLock::new(Vec::new());

pub fn available() -> Vec<String> {
    let Some(folder) = folder() else {
        return Vec::new();
    };
    available_in(&folder)
}

fn available_in(folder: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
                || (path.is_dir() && path.join("manifest.json").is_file())
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn folder() -> Option<PathBuf> {
    if let Some(given) = std::env::var_os("ZW_PLUGINS") {
        return Some(PathBuf::from(given));
    }
    if let Some(preferred) = PREFERRED.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(preferred);
    }
    let beside_config = crate::state::file("plugins");
    if let Some(beside_config) = &beside_config
        && beside_config.is_dir()
    {
        return Some(beside_config.clone());
    }
    let beside_binary = std::env::current_exe().ok()?.parent()?.join("plugins");
    if beside_binary.is_dir() || beside_config.is_none() {
        return Some(beside_binary);
    }
    beside_config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_and_the_loaded_set_use_the_same_names() {
        let Ok(wasm) = std::env::var("ZW_PLUGIN_WASM") else {
            tracing::info!("ZW_PLUGIN_WASM が無いので飛ばす");
            return;
        };
        let folder = std::env::temp_dir().join(format!("zw-names-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&folder);
        let into = folder.join("zerowave_tsmf.wasm");
        if std::fs::copy(&wasm, &into).is_err() {
            tracing::info!("{wasm} をコピーできないのでスキップ");
            return;
        }
        let listed = available_in(&folder);
        let held = Loaded::load(&folder, &mut |_| {});
        let stems: Vec<String> = held.all().iter().map(|plugin| stem_of(plugin)).collect();
        assert_eq!(listed, stems, "一覧と読み込んだセットで名前が違う");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn everyone_sees_the_same_set() {
        let first = loaded();
        let second = loaded();
        assert!(Arc::ptr_eq(&first, &second), "同じセットを見ている");
        let hands: Vec<_> = (0..4).map(|_| std::thread::spawn(loaded)).collect();
        for hand in hands {
            let theirs = hand.join().expect("スレッドを join できる");
            assert!(Arc::ptr_eq(&first, &theirs), "別のスレッドからも同じセット");
        }
    }
}
