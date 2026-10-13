// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグイン（wasm）の読み込み。TSMF の分離もプラグインが行うので、入れていなければケーブルの TSMF の
//! 搬送波は映らない。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zwsdk::host::{Chain, Level, Logger, Plugin};

pub type IoOf = Arc<dyn Fn(&str) -> zwsdk::host::Io + Send + Sync>;

/// 現在読み込まれているプラグイン。実行中に差し替わるので、チェーンを作る側は [`newest`] を見る。
static CURRENT: std::sync::RwLock<Option<Arc<Loaded>>> = std::sync::RwLock::new(None);

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn publish(loaded: Arc<Loaded>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn current() -> (Option<Arc<Loaded>>, u64) {
    let loaded = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    (
        loaded,
        GENERATION.load(std::sync::atomic::Ordering::Relaxed),
    )
}

pub fn newest(held: &Option<Arc<Loaded>>) -> Option<Arc<Loaded>> {
    let current = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    match (current, held) {
        (Some(current), Some(_)) => Some(current),
        (_, held) => held.clone(),
    }
}

#[derive(Default)]
pub struct Loaded {
    plugins: Vec<Arc<Plugin>>,
    /// ビルドのたびに作るとコンパイラーのインスタンスが増える。
    engine: Option<zwsdk::host::Engine>,
    io_of: Option<IoOf>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Loaded")
            .field("plugins", &self.plugins)
            .field("io_of", &self.io_of.is_some())
            .finish()
    }
}

impl Loaded {
    pub fn load(folder: &Path, say: &mut dyn FnMut(&str)) -> Self {
        Self::load_except(folder, &[], say)
    }

    #[must_use]
    pub fn with_io(mut self, io_of: IoOf) -> Self {
        self.io_of = Some(io_of);
        self
    }

    pub(crate) fn io_for(&self, plugin: &Plugin) -> zwsdk::host::Io {
        let stem = text_of(plugin.path().file_stem());
        let mut io = self
            .io_of
            .as_ref()
            .map_or_else(zwsdk::host::Io::default, |make| make(&stem));
        // 起動するプログラムと子プロセスの環境変数には既定値を持たせない（決めるのはユーザー）。
        let config = &plugin.manifest().config;
        io.programs = config
            .iter()
            .filter(|setting| setting.kind == "program")
            .map(|setting| setting.key.clone())
            .collect();
        io.envs = config
            .iter()
            .filter(|setting| setting.is_env())
            .map(|setting| setting.key.clone())
            .collect();
        // 宣言の既定値は使わない。空なら coaxd の既定のソケット。
        io.sockets = config
            .iter()
            .filter(|setting| setting.kind == "socket")
            .map(|setting| setting.key.clone())
            .collect();
        // 既定値はプラグインのフォルダー下の相対パスだけが有効。
        io.folders = config
            .iter()
            .filter(|setting| setting.kind == "folder")
            .map(|setting| setting.key.clone())
            .collect();
        io.font_keys = config
            .iter()
            .filter(|setting| setting.kind == "font")
            .map(|setting| setting.key.clone())
            .collect();
        io.states = config
            .iter()
            .filter(|setting| setting.kind == "state")
            .map(|setting| setting.key.clone())
            .collect();
        io.home = Some(home_of(plugin.path()));
        io.defaults = config
            .iter()
            .filter(|setting| setting.kind != "program" && !setting.is_env())
            .map(|setting| (setting.key.clone(), setting.default.clone()))
            .collect();
        if plugin.has("control") {
            io.control = control();
        }
        io
    }

    pub fn ui(&self) -> Vec<(Arc<Plugin>, zwsdk::host::Io)> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.has("ui"))
            .map(|plugin| (Arc::clone(plugin), self.io_for(plugin)))
            .collect()
    }

    pub fn video(&self) -> Vec<(Arc<Plugin>, zwsdk::host::Io)> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.has("video"))
            .map(|plugin| (Arc::clone(plugin), self.io_for(plugin)))
            .collect()
    }

    pub fn load_except(folder: &Path, disabled: &[String], say: &mut dyn FnMut(&str)) -> Self {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Self::default();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
                    || (path.is_dir() && path.join("manifest.json").is_file())
            })
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Self::default();
        }
        let engine = match zwsdk::host::engine() {
            Ok(engine) => engine,
            Err(why) => {
                say(&format!("プラグイン  {why}"));
                return Self::default();
            }
        };
        let mut plugins = Vec::new();
        for path in paths {
            let file = text_of(path.file_name());
            let stem = text_of(path.file_stem());
            if disabled.contains(&stem) {
                say(&format!("プラグイン  {file} は設定で無効にしてある"));
                continue;
            }
            match Plugin::load(&engine, &path) {
                Ok(plugin) => {
                    say(&loaded_line(&plugin, &file));
                    plugins.push(Arc::new(plugin));
                }
                Err(why) => say(&format!("プラグイン  {file} を読み込めない: {why}")),
            }
        }
        let keep: Vec<&Path> = plugins.iter().filter_map(|p| p.cache()).collect();
        let swept = zwsdk::host::sweep_caches(folder, &keep);
        if swept > 0 {
            say(&format!("プラグイン  古いビルド結果を {swept} 個消した"));
        }
        Self {
            plugins,
            engine: Some(engine),
            io_of: None,
        }
    }

    /// 同じ名前があれば差し替える。`.cwasm` は削除しない（削除するとほかのプラグインの物まで消える）。
    pub fn plus(&self, path: &Path, say: &mut dyn FnMut(&str)) -> Result<Self, String> {
        let engine = match self.engine.clone() {
            Some(engine) => engine,
            None => zwsdk::host::engine()?,
        };
        let file = text_of(path.file_name());
        let plugin = Plugin::load(&engine, path).map_err(|why| format!("{file}: {why}"))?;
        say(&loaded_line(&plugin, &file));
        let stem = path.file_stem();
        let mut plugins: Vec<Arc<Plugin>> = self
            .plugins
            .iter()
            .filter(|had| had.path().file_stem() != stem)
            .map(Arc::clone)
            .collect();
        plugins.push(Arc::new(plugin));
        plugins.sort_by_key(|plugin| plugin.path().to_path_buf());
        Ok(Self {
            plugins,
            engine: Some(engine),
            io_of: self.io_of.clone(),
        })
    }

    #[must_use]
    pub fn clone_of(loaded: &Self) -> Self {
        Self {
            plugins: loaded.plugins.clone(),
            engine: loaded.engine.clone(),
            io_of: loaded.io_of.clone(),
        }
    }

    #[must_use]
    pub fn without(&self, stem: &str) -> Self {
        Self {
            plugins: self
                .plugins
                .iter()
                .filter(|plugin| {
                    plugin
                        .path()
                        .file_stem()
                        .is_none_or(|had| had.to_string_lossy() != stem)
                })
                .map(Arc::clone)
                .collect(),
            engine: self.engine.clone(),
            io_of: self.io_of.clone(),
        }
    }

    pub fn all(&self) -> &[Arc<Plugin>] {
        &self.plugins
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// 選局のたびに作る。`"tuners"` を宣言したプラグインには [`lend`] のチューナー貸し出しハンドルを渡す。
    pub fn stream_chain(&self) -> Chain {
        self.stream_chain_with(lender())
    }

    pub fn stream_chain_with(&self, lender: Option<zwsdk::host::tuner::LenderOf>) -> Chain {
        let mut ordered: Vec<&Arc<Plugin>> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.has("stream"))
            .collect();
        ordered.sort_by_key(|plugin| plugin.manifest().order);
        let instances = ordered
            .into_iter()
            .filter_map(|plugin| {
                let name = plugin.name().to_owned();
                let mut io = self.io_for(plugin);
                if plugin.has("tuners") {
                    io.lender = lender.clone();
                }
                plugin
                    .stream(logger(name), io)
                    .map_err(|why| tracing::warn!("プラグインを起動できない: {why}"))
                    .ok()
            })
            .collect();
        Chain::new(instances)
    }
}

pub fn logger(name: String) -> Logger {
    Box::new(move |level, text| match level {
        Level::Info => tracing::info!("{name}  {text}"),
        Level::Warning => tracing::warn!("{name}  {text}"),
    })
}

fn loaded_line(plugin: &Plugin, file: &str) -> String {
    format!(
        "プラグイン  {} {}（{file}: {}）",
        plugin.name(),
        plugin.manifest().version,
        plugin.manifest().hooks.join(" ")
    )
}

fn home_of(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_owned()
    } else {
        path.with_extension("")
    }
}

fn text_of(name: Option<&OsStr>) -> String {
    name.map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// 現在の局のチューナー貸し出しハンドル。貸し出せないソースを開いたら空にする（前の局の情報で借りない）。
static LENDER: std::sync::RwLock<Option<zwsdk::host::tuner::LenderOf>> =
    std::sync::RwLock::new(None);

pub fn lend(lender: Option<zwsdk::host::tuner::LenderOf>) {
    *LENDER.write().unwrap_or_else(|e| e.into_inner()) = lender;
}

pub fn lender() -> Option<zwsdk::host::tuner::LenderOf> {
    LENDER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// リモコンのインターフェース（`zw_control_v1`）。`"control"` を宣言したプラグインに渡す。
static CONTROL: std::sync::RwLock<Option<zwsdk::host::IoControl>> = std::sync::RwLock::new(None);

pub fn give_control(control: zwsdk::host::IoControl) {
    *CONTROL.write().unwrap_or_else(|e| e.into_inner()) = Some(control);
}

fn control() -> Option<zwsdk::host::IoControl> {
    CONTROL.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_set_wins_only_where_there_was_one() {
        let held = Some(Arc::new(Loaded::default()));
        let newer = Arc::new(Loaded::default());
        publish(Arc::clone(&newer));
        let chosen = newest(&held).expect("ある");
        assert!(Arc::ptr_eq(&chosen, &newer), "差し替えた方");
        assert!(newest(&None).is_none(), "持っていなければ無いまま");
    }

    #[test]
    fn taking_one_out_keeps_the_rest() {
        let loaded = Loaded::default();
        assert!(loaded.without("nothing").all().is_empty());
        assert!(Loaded::clone_of(&loaded).all().is_empty());
    }
}
