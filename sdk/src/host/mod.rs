// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインを載せる側。`name.wasm` 一つか、`manifest.json` を持つ `name/` で置ける。
//! バイト列は本体の側に置き guest には番号だけを渡す。一つのインスタンスは一つのスレッドで使う。

use std::path::{Path, PathBuf};

use wasmtime::{Caller, Instance, Linker, Memory, Module, Store, TypedFunc};

pub use wasmtime::Engine;

use crate::abi;
use crate::slots::Slots;

mod files;
pub mod fonts;
mod io;
mod layer;
mod proc;
mod pump;
mod sock;
pub mod tuner;
mod ui;
mod video;

pub use crate::draw::FontContext;
pub use io::{
    Control as IoControl, Controls, Io, Live as IoLive, Notify as IoNotify, Store as IoStore,
    truthy,
};
pub use layer::{LayerInstance, Painted, Picture, Placed, SpriteImage};
pub use ui::UiInstance;
pub use video::VideoInstance;

pub type Logger = Box<dyn Fn(Level, &str) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
}

pub const ABI: u32 = abi::VERSION;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Manifest {
    pub abi: u32,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub layer: crate::layer::Manifest,
    #[serde(default)]
    pub video: crate::video::Manifest,
    #[serde(default)]
    pub ui: crate::ui::Manifest,
    #[serde(default)]
    pub config: Vec<Setting>,
    #[serde(default)]
    pub order: i32,
}

impl Manifest {
    fn check_abi(&self) -> Result<(), String> {
        if self.abi == ABI {
            Ok(())
        } else {
            Err(format!(
                "ABI のバージョンが違う: {}（こちらは {ABI}）",
                self.abi
            ))
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub label: String,
    /// `bool`/`text`/`choice`/`program`/`socket`/`folder`/`font`/`list`/`map`/`state`
    /// （不明な値は `text`）。`program`・`font` はユーザーだけが入れられる。`state` はファイルに残さない。
    #[serde(default)]
    pub kind: String,
    /// `list` は `text`/`program`/`folder`、`map` は `text`/`env`。空は `text`。
    /// `env` は `program` と同じくユーザーだけが入れられる。
    #[serde(default)]
    pub item: String,
    /// `list` は文字の配列、`map` は文字のマップでも書ける。
    #[serde(default, deserialize_with = "default_text")]
    pub default: String,
    #[serde(default)]
    pub choices: Vec<(String, String)>,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub section: String,
}

impl Setting {
    pub fn is_env(&self) -> bool {
        self.kind == "map" && self.item == "env"
    }
}

fn default_text<'de, D: serde::Deserializer<'de>>(read: D) -> Result<String, D::Error> {
    let value = <serde_json::Value as serde::Deserialize>::deserialize(read)?;
    Ok(crate::config::default_text(&value))
}

pub struct Plugin {
    engine: Engine,
    module: Option<Module>,
    manifest: Manifest,
    path: PathBuf,
    cache: Option<PathBuf>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Plugin")
            .field("manifest", &self.manifest)
            .field("path", &self.path)
            .finish()
    }
}

const TICK: std::time::Duration = std::time::Duration::from_millis(100);

const MIB: usize = 1 << 20;

/// 種類ごとの上限。超えたら trap にして理由をログに出す。guest のメモリーは縮まないので
/// 一番多い時で決まる。普段の数倍にして、1 GB のマシンでも一つで使い切らない大きさに。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// guest のメモリー。
    memory: usize,
    /// 本体の側に預かるバイト列（[`Bufs`]）の合計。
    bufs: usize,
    /// video の blob の合計。
    blobs: usize,
    /// 一回の呼び出しに待つ時間（`while(1){}` で取られたままにしない）。
    patience: std::time::Duration,
}

impl Limits {
    /// TS を 64 KiB ずつ通す。抱えるのは pipe の決まるまでの 8 MiB が一番多い。
    /// 再生のスレッドで呼ぶので待ちは短く、ただし本体の書き込みの待ち（3 秒）よりは長く。
    pub(crate) const STREAM: Self = Self {
        memory: 128 * MIB,
        bufs: 64 * MIB,
        blobs: 0,
        patience: std::time::Duration::from_secs(5),
    };
    /// データ放送の BML で 140 MiB ほど。通信のアプリはキャッシュ（96 MiB まで）と
    /// 4K の画像のデコードが足される。預かるのはフォント（十数 MiB）とフレーム。
    pub(crate) const LAYER: Self = Self {
        memory: 384 * MIB,
        bufs: 96 * MIB,
        blobs: 0,
        patience: std::time::Duration::from_secs(10),
    };
    /// 読めるファイルは 64 MiB まで。RIFE のモデルは読んだバイト列・パースしたレイヤー・
    /// 重みの blob の三つを持つ。
    pub(crate) const VIDEO: Self = Self {
        memory: 256 * MIB,
        bufs: 128 * MIB,
        blobs: 128 * MIB,
        patience: std::time::Duration::from_secs(10),
    };
    /// メニューと設定ページ。フォルダーのファイルを読むこともある（64 MiB まで）。
    pub(crate) const UI: Self = Self {
        memory: 128 * MIB,
        bufs: 96 * MIB,
        blobs: 0,
        patience: std::time::Duration::from_secs(10),
    };
    /// マニフェストを取得するだけ。どの種類のモジュールでも作れるように。
    const PROBE: Self = Self::LAYER;
}

/// テーブル（関数の番号）の要素数。guest のメモリーの外に取られるので別に抑える。
const TABLE_MOST: usize = 1 << 20;

pub fn engine() -> Result<Engine, String> {
    let mut config = wasmtime::Config::new();
    config.cranelift_opt_level(wasmtime::OptLevel::Speed);
    // アドレスのテーブルが `.cwasm` の三割を占める。クラッシュしたときは関数の名前だけで足りる。
    config.generate_address_map(false);
    config.epoch_interruption(true);
    let engine =
        Engine::new(&config).map_err(|error| format!("wasm のエンジンが作れない: {error}"))?;
    let weak = engine.weak();
    std::thread::Builder::new()
        .name(String::from("wasm-clock"))
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                let Some(engine) = weak.upgrade() else { break };
                engine.increment_epoch();
            }
        })
        .map_err(|error| format!("wasm の時計が作れない: {error}"))?;
    Ok(engine)
}

pub fn manifest_at(path: &Path) -> Option<Manifest> {
    if path.is_dir() {
        let text = std::fs::read_to_string(path.join("manifest.json")).ok()?;
        return serde_json::from_str(&text).ok();
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(custom_section(&bytes, abi::MANIFEST)?).ok()
}

impl Plugin {
    pub fn load(engine: &Engine, path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            let text = std::fs::read_to_string(path.join("manifest.json"))
                .map_err(|error| format!("manifest.json が読めない: {error}"))?;
            let manifest: Manifest = serde_json::from_str(&text)
                .map_err(|error| format!("manifest.json が読めない: {error}"))?;
            manifest.check_abi()?;
            let (module, cache) = match &manifest.module {
                Some(name) => {
                    let bytes = std::fs::read(path.join(name))
                        .map_err(|error| format!("{name} が読めない: {error}"))?;
                    let (module, cache) = compile(engine, &bytes, &path.join(name))
                        .map_err(|error| format!("{name} が wasm としてビルドできない: {error}"))?;
                    (Some(module), cache)
                }
                None => (None, None),
            };
            return Ok(Self {
                engine: engine.clone(),
                module,
                manifest,
                path: path.to_owned(),
                cache,
            });
        }
        let bytes = std::fs::read(path).map_err(|error| format!("読めない: {error}"))?;
        Self::from_bytes(engine, &bytes, path)
    }

    pub fn from_bytes(engine: &Engine, bytes: &[u8], path: &Path) -> Result<Self, String> {
        let (module, cache) = compile(engine, bytes, path)
            .map_err(|error| format!("wasm としてビルドできない: {error}"))?;
        let manifest = match custom_section(bytes, abi::MANIFEST) {
            Some(json) => serde_json::from_slice(json)
                .map_err(|error| format!("zw_manifest のセクションが読めない: {error}"))?,
            None => {
                let logger: Logger = Box::new(|_, _| {});
                let mut made =
                    Made::new(engine, &module, Limits::PROBE, logger, None, Io::default())?;
                made.manifest()?
            }
        };
        manifest.check_abi()?;
        Ok(Self {
            engine: engine.clone(),
            module: Some(module),
            manifest,
            path: path.to_owned(),
            cache,
        })
    }

    pub fn cache(&self) -> Option<&Path> {
        self.cache.as_deref()
    }

    pub fn layer_manifest(&self) -> &crate::layer::Manifest {
        &self.manifest.layer
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn has(&self, hook: &str) -> bool {
        self.manifest.hooks.iter().any(|it| it == hook)
    }

    fn module_for(&self, hook: &str) -> Result<&Module, String> {
        if !self.has(hook) {
            return Err(format!("{} は {hook} hook を持たない", self.name()));
        }
        self.module
            .as_ref()
            .ok_or_else(|| format!("{} は wasm を持たない", self.name()))
    }

    pub fn stream(&self, logger: Logger, io: Io) -> Result<StreamInstance, String> {
        let module = self.module_for("stream")?;
        let mut made = Made::new(&self.engine, module, Limits::STREAM, logger, None, io)?;
        let new: TypedFunc<(), u32> = made.func(abi::export::STREAM_NEW)?;
        let start: TypedFunc<(u32, u32, u32), i32> = made.func(abi::export::STREAM_START)?;
        let input: TypedFunc<(u32, u32), i32> = made.func(abi::export::STREAM_INPUT)?;
        let stop: TypedFunc<u32, ()> = made.func(abi::export::STREAM_STOP)?;
        let drop: TypedFunc<u32, ()> = made.func(abi::export::STREAM_DROP)?;
        let handle = new
            .call(made.armed(), ())
            .map_err(|error| format!("zw_stream_new: {error}"))?;
        if handle == 0 {
            return Err("zw_stream_new が 0 を返した".to_owned());
        }
        Ok(StreamInstance {
            name: self.name().to_owned(),
            made,
            handle,
            start,
            input,
            stop,
            drop,
            started: false,
        })
    }

    pub fn layer(
        &self,
        logger: Logger,
        io: Io,
        fonts: FontContext,
        config: &crate::layer::Config,
    ) -> Result<LayerInstance, String> {
        let module = self.module_for("layer")?;
        let (width, height) = config.size;
        let draw = layer::Draw::new(fonts, f64::from(width.max(1)), f64::from(height.max(1)));
        let made = Made::new(&self.engine, module, Limits::LAYER, logger, Some(draw), io)?;
        LayerInstance::new(self.name().to_owned(), made, config)
    }
}

impl Plugin {
    pub fn video(&self, logger: Logger, io: Io) -> Result<VideoInstance, String> {
        let module = self.module_for("video")?;
        let made = Made::new(&self.engine, module, Limits::VIDEO, logger, None, io)?;
        VideoInstance::new(self.name().to_owned(), made)
    }

    pub fn video_manifest(&self) -> &crate::video::Manifest {
        &self.manifest.video
    }

    pub fn ui(&self, logger: Logger, io: Io) -> Result<UiInstance, String> {
        let module = self.module_for("ui")?;
        let made = Made::new(&self.engine, module, Limits::UI, logger, None, io)?;
        UiInstance::new(self.name().to_owned(), made)
    }
}

#[derive(Debug)]
pub struct HostState {
    bufs: Bufs,
    output: Vec<u8>,
    log: LoggerBox,
    draw: Option<layer::Draw>,
    io: Io,
    procs: proc::Procs,
    borrowed: tuner::Borrowed,
    socks: sock::Socks,
    began: std::time::Instant,
    /// 書きかけ（`eprintln!` は一行を何度かに分けて書く）。
    stdio: [Vec<u8>; 2],
    blobs: std::collections::BTreeMap<String, crate::video::Blob>,
    /// 放送で届いたものはこのインスタンスの通信だけに追加する。
    net: crate::net::Session,
    limits: Limits,
}

/// guest が `drop` しないまま作り続けても伸びないよう、数と合計に上限を置く。超えたら番号 0。
#[derive(Debug, Default)]
pub(crate) struct Bufs {
    slots: Slots<Vec<u8>>,
    bytes: usize,
    most: usize,
    /// 断ったことをログに出した（guest が繰り返しても一度だけ）。
    told: bool,
}

impl Bufs {
    const MOST: usize = 4096;

    fn new(most: usize) -> Self {
        Self {
            most,
            ..Self::default()
        }
    }

    pub(crate) fn put(&mut self, bytes: Vec<u8>) -> u32 {
        if self.slots.count() >= Self::MOST || self.bytes + bytes.len() > self.most {
            return 0;
        }
        self.bytes += bytes.len();
        self.slots.put(bytes)
    }

    pub(crate) fn get(&self, handle: u32) -> Option<&Vec<u8>> {
        self.slots.get(handle)
    }

    pub(crate) fn take(&mut self, handle: u32) -> Option<Vec<u8>> {
        let taken = self.slots.take(handle)?;
        self.bytes -= taken.len();
        Some(taken)
    }

    fn refusal(&self, bytes: usize) -> String {
        format!(
            "預かるバイト列が上限を超える（{} 個・{} MiB、足そうとしたのは {bytes} バイト、上限 {} 個・{} MiB）",
            self.slots.count(),
            self.bytes / MIB,
            Self::MOST,
            self.most / MIB
        )
    }
}

struct LoggerBox(Logger);

impl std::fmt::Debug for LoggerBox {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Logger")
    }
}

impl HostState {
    fn log(&self, level: Level, text: &str) {
        (self.log.0)(level, text);
    }

    /// guest に渡すバイト列を登録する。拒否したら 0（guest には失敗として返る）、ログには一度だけ。
    pub(crate) fn put_buf(&mut self, bytes: Vec<u8>) -> u32 {
        let len = bytes.len();
        let handle = self.bufs.put(bytes);
        if handle == 0 && !self.bufs.told {
            self.bufs.told = true;
            let why = self.bufs.refusal(len);
            self.log(Level::Warning, &why);
        }
        handle
    }
}

impl wasmtime::ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired <= self.limits.memory {
            return Ok(true);
        }
        // 失敗を返しても guest は確保の失敗でクラッシュするだけなので、理由が分かるようここで止める。
        let why = format!(
            "メモリーが上限 {} MiB を超えるので止める（{} MiB から {} MiB へ伸ばそうとした）",
            self.limits.memory / MIB,
            current / MIB,
            desired.div_ceil(MIB)
        );
        self.log(Level::Warning, &why);
        Err(wasmtime::Error::msg(why))
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= TABLE_MOST)
    }
}

pub(super) struct Made {
    store: Store<HostState>,
    instance: Instance,
    memory: Memory,
}

impl Made {
    fn new(
        engine: &Engine,
        module: &Module,
        limits: Limits,
        logger: Logger,
        draw: Option<layer::Draw>,
        io: Io,
    ) -> Result<Self, String> {
        let mut linker: Linker<HostState> = Linker::new(engine);
        link_all(&mut linker).map_err(|error| format!("import がビルドできない: {error}"))?;
        let mut store = Store::new(
            engine,
            HostState {
                blobs: std::collections::BTreeMap::new(),
                bufs: Bufs::new(limits.bufs),
                output: Vec::new(),
                log: LoggerBox(logger),
                draw,
                io,
                procs: Slots::with_most(16),
                socks: Slots::with_most(64),
                borrowed: Slots::with_most(16),
                began: std::time::Instant::now(),
                stdio: [Vec::new(), Vec::new()],
                net: crate::net::Session::new(),
                limits,
            },
        );
        store.limiter(|state| state);
        store.epoch_deadline_callback(|store| {
            let patience = store.data().limits.patience;
            let why = format!(
                "一回の呼び出しが {} 秒を超えたので止める",
                patience.as_secs()
            );
            store.data().log(Level::Warning, &why);
            Err(wasmtime::Error::msg(why))
        });
        store.set_epoch_deadline(ticks(limits.patience));
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|error| format!("作れない: {error}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("memory が無い")?;
        // WASI の reactor は、ほかを呼ぶ前に `_initialize` を一度呼ぶ決まり。`proc_exit(0)` は成功。
        if let Ok(initialize) = instance.get_typed_func::<(), ()>(&mut store, "_initialize")
            && let Err(error) = initialize.call(&mut store, ())
            && !matches!(error.downcast_ref::<layer::Exit>(), Some(layer::Exit(0)))
        {
            return Err(format!("_initialize: {error}"));
        }
        Ok(Self {
            store,
            instance,
            memory,
        })
    }

    fn armed(&mut self) -> &mut Store<HostState> {
        let patience = self.store.data().limits.patience;
        self.store.set_epoch_deadline(ticks(patience));
        &mut self.store
    }

    /// 本体から guest へ渡すバイト列を登録する。登録できなければ呼ばずに止める。
    fn give(&mut self, bytes: Vec<u8>) -> Result<u32, String> {
        let len = bytes.len();
        let state = self.store.data_mut();
        match state.bufs.put(bytes) {
            0 => {
                let why = state.bufs.refusal(len);
                state.log(Level::Warning, &why);
                Err(why)
            }
            handle => Ok(handle),
        }
    }

    fn func<P, R>(&mut self, name: &str) -> Result<TypedFunc<P, R>, String>
    where
        P: wasmtime::WasmParams,
        R: wasmtime::WasmResults,
    {
        self.instance
            .get_typed_func::<P, R>(&mut self.store, name)
            .map_err(|error| format!("{name}: {error}"))
    }

    fn manifest(&mut self) -> Result<Manifest, String> {
        let manifest: TypedFunc<(), u64> = self.func(abi::MANIFEST)?;
        let packed = manifest
            .call(self.armed(), ())
            .map_err(|error| format!("zw_manifest: {error}"))?;
        let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        let bytes = self
            .memory
            .data(&self.store)
            .get(ptr..ptr + len)
            .ok_or("zw_manifest がメモリの外を指した")?;
        serde_json::from_slice(bytes).map_err(|error| format!("マニフェストが読めない: {error}"))
    }
}

/// 大きなモジュールはビルドに十秒かかるので `.cwasm` で残す。名前は内容のハッシュ。
fn compile(
    engine: &Engine,
    bytes: &[u8],
    path: &Path,
) -> wasmtime::Result<(Module, Option<PathBuf>)> {
    let stamp = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    let stem = path.file_stem().map_or_else(
        || String::from("plugin"),
        |s| s.to_string_lossy().into_owned(),
    );
    let cached_name = format!("{stem}-{stamp:016x}.cwasm");
    // `.cwasm` はネイティブコードとしてそのまま実行されるので、ほかの人が書ける所（/tmp など）には置かない。
    let mut places = Vec::new();
    if let Some(dir) = cache_dir_of(path) {
        places.push(dir.join(&cached_name));
    }
    if let Some(dir) = user_cache_dir() {
        places.push(dir.join(&cached_name));
    }
    for place in &places {
        if place.is_file() && only_mine(place) {
            // SAFETY: 自分が `serialize` で書いたもの。違う物は wasmtime が断る。
            if let Ok(module) = unsafe { Module::deserialize_file(engine, place) } {
                return Ok((module, Some(place.clone())));
            }
        }
    }
    let module = Module::new(engine, bytes)?;
    let mut cache = None;
    if let Ok(serialized) = module.serialize() {
        for place in &places {
            let Some(dir) = place.parent() else {
                continue;
            };
            if std::fs::create_dir_all(dir).is_err() || !keep_private(dir) {
                continue;
            }
            // もう一つの deck が mmap で読んでいる物を切り詰めないよう、隣に書いてからリネームする。
            let mut name = place.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".tmp-{}", std::process::id()));
            let temporary = place.with_file_name(name);
            let written = std::fs::write(&temporary, &serialized)
                .and_then(|()| only_owner_writes(&temporary))
                .and_then(|()| std::fs::rename(&temporary, place));
            if written.is_ok() {
                cache = Some(place.clone());
                break;
            }
            let _ = std::fs::remove_file(&temporary);
        }
    }
    Ok((module, cache))
}

fn user_cache_dir() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = if cfg!(windows) {
        absolute("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        absolute("HOME")?.join("Library").join("Caches")
    } else {
        absolute("XDG_CACHE_HOME").or_else(|| Some(absolute("HOME")?.join(".cache")))?
    };
    Some(base.join("zerowave").join("plugins_cache"))
}

/// Unix では権限を変更できるかで自分の物かを確かめる（ほかのユーザーが作ったフォルダーは使わない）。
fn keep_private(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        true
    }
}

fn only_owner_writes(file: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn only_mine(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(dir) = file.parent() else {
            return false;
        };
        if !keep_private(dir) {
            return false;
        }
        let (Ok(dir), Ok(meta)) = (std::fs::metadata(dir), std::fs::symlink_metadata(file)) else {
            return false;
        };
        meta.is_file() && meta.uid() == dir.uid() && meta.mode() & 0o022 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        true
    }
}

fn cache_dir_of(module: &Path) -> Option<PathBuf> {
    let mut dir = module.parent()?;
    if dir.join("manifest.json").is_file() {
        dir = dir.parent()?;
    }
    Some(cache_dir(dir))
}

pub fn cache_dir(folder: &Path) -> PathBuf {
    let name = folder.file_name().map_or_else(
        || String::from("plugins"),
        |n| n.to_string_lossy().into_owned(),
    );
    folder.with_file_name(format!("{name}_cache"))
}

pub fn sweep_caches(folder: &Path, keep: &[&Path]) -> usize {
    let mut swept = 0;
    for dir in std::iter::once(cache_dir(folder)).chain(user_cache_dir()) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "cwasm")
                && !keep.iter().any(|k| *k == path)
                && std::fs::remove_file(&path).is_ok()
            {
                swept += 1;
            }
        }
    }
    swept
}

fn custom_section<'a>(wasm: &'a [u8], name: &str) -> Option<&'a [u8]> {
    // wasm の長さは u32 の LEB128（5 バイトまで）。
    fn leb(bytes: &[u8], at: &mut usize) -> Option<usize> {
        let (mut value, mut shift) = (0u64, 0u32);
        loop {
            let byte = *bytes.get(*at)?;
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return usize::try_from(u32::try_from(value).ok()?).ok();
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
    }
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return None;
    }
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = leb(wasm, &mut at)?;
        let end = at.checked_add(size)?;
        let body = wasm.get(at..end)?;
        if id == 0 {
            let mut inner = 0;
            let len = leb(body, &mut inner)?;
            let found = body.get(inner..inner.checked_add(len)?)?;
            if found == name.as_bytes() {
                return body.get(inner + len..);
            }
        }
        at = end;
    }
    None
}

fn ticks(patience: std::time::Duration) -> u64 {
    (patience.as_millis() / TICK.as_millis()).max(1) as u64
}

fn guest_memory(caller: &mut Caller<'_, HostState>) -> Option<Memory> {
    caller.get_export("memory")?.into_memory()
}

fn guest_bytes(caller: &mut Caller<'_, HostState>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let memory = guest_memory(caller)?;
    memory
        .data(&caller)
        .get(ptr as usize..(ptr as usize).checked_add(len as usize)?)
        .map(<[u8]>::to_vec)
}

fn guest_text(caller: &mut Caller<'_, HostState>, ptr: u32, len: u32) -> String {
    guest_bytes(caller, ptr, len)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn write_guest(caller: &mut Caller<'_, HostState>, ptr: u32, bytes: &[u8]) {
    let Some(memory) = guest_memory(caller) else {
        return;
    };
    let at = ptr as usize;
    if let Some(slot) = memory.data_mut(caller).get_mut(at..at + bytes.len()) {
        slot.copy_from_slice(bytes);
    }
}

fn since_epoch() -> std::time::Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

fn link_all(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    link(linker)?;
    layer::link(linker)?;
    layer::link_wasi(linker)?;
    io::link(linker)?;
    files::link(linker)?;
    proc::link(linker)?;
    sock::link(linker)?;
    video::link(linker)?;
    tuner::link(linker)
}

fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::BUF,
        "len",
        |caller: Caller<'_, HostState>, buf: u32| -> u32 {
            caller
                .data()
                .bufs
                .get(buf)
                .map_or(0, |bytes| bytes.len() as u32)
        },
    )?;
    linker.func_wrap(
        abi::import::BUF,
        "read",
        |mut caller: Caller<'_, HostState>, buf: u32, from: u32, ptr: u32, len: u32| -> u32 {
            let Some(memory) = guest_memory(&mut caller) else {
                return 0;
            };
            let (data, state) = memory.data_and_store_mut(&mut caller);
            let Some(bytes) = state.bufs.get(buf) else {
                return 0;
            };
            let n = (len as usize).min(bytes.len().saturating_sub(from as usize));
            let Some(into) = data.get_mut(ptr as usize..ptr as usize + n) else {
                return 0;
            };
            crate::plug::abi::copy_from(bytes, from as usize, into) as u32
        },
    )?;
    linker.func_wrap(
        abi::import::BUF,
        "create",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
            match guest_bytes(&mut caller, ptr, len) {
                Some(bytes) => caller.data_mut().put_buf(bytes),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::BUF,
        "drop",
        |mut caller: Caller<'_, HostState>, buf: u32| {
            caller.data_mut().bufs.take(buf);
        },
    )?;
    linker.func_wrap(
        abi::import::OUTPUT,
        "write",
        |mut caller: Caller<'_, HostState>, buf: u32| {
            let HostState { bufs, output, .. } = caller.data_mut();
            if let Some(bytes) = bufs.get(buf) {
                output.extend_from_slice(bytes);
            }
        },
    )?;
    linker.func_wrap(
        abi::import::OUTPUT,
        "write_bytes",
        |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| {
            if let Some(bytes) = guest_bytes(&mut caller, ptr, len) {
                caller.data_mut().output.extend_from_slice(&bytes);
            }
        },
    )?;
    linker.func_wrap(
        abi::import::LOG,
        "log",
        |mut caller: Caller<'_, HostState>, level: u32, ptr: u32, len: u32| {
            if let Some(bytes) = guest_bytes(&mut caller, ptr, len) {
                let level = if level == 1 {
                    Level::Warning
                } else {
                    Level::Info
                };
                caller.data().log(level, &String::from_utf8_lossy(&bytes));
            }
        },
    )?;
    Ok(())
}

pub struct StreamInstance {
    name: String,
    made: Made,
    handle: u32,
    start: TypedFunc<(u32, u32, u32), i32>,
    input: TypedFunc<(u32, u32), i32>,
    stop: TypedFunc<u32, ()>,
    drop: TypedFunc<u32, ()>,
    started: bool,
}

impl std::fmt::Debug for StreamInstance {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("StreamInstance")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Unknown = 0,
    Ts = 1,
    Tlv = 2,
}

impl StreamInstance {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn start(&mut self, container: Container, service: Option<u16>) -> Result<(), String> {
        let code = self
            .start
            .call(
                self.made.armed(),
                (
                    self.handle,
                    container as u32,
                    u32::from(service.unwrap_or(0)),
                ),
            )
            .map_err(|error| format!("{}: start: {error}", self.name))?;
        if code != 0 {
            return Err(format!("{} が始められない（{code}）", self.name));
        }
        self.started = true;
        Ok(())
    }

    pub fn input(&mut self, bytes: &[u8]) -> Result<&[u8], String> {
        self.made.store.data_mut().output.clear();
        let buf = self
            .made
            .give(bytes.to_vec())
            .map_err(|why| format!("{}: input: {why}", self.name))?;
        let result = self
            .input
            .call(self.made.armed(), (self.handle, buf))
            .map_err(|error| format!("{}: input: {error}", self.name));
        let state = self.made.store.data_mut();
        state.bufs.take(buf);
        match result {
            Ok(0) => Ok(&state.output),
            Ok(code) => Err(format!("{} が拒否した（{code}）", self.name)),
            Err(why) => Err(why),
        }
    }

    pub fn stop(&mut self) {
        if self.started {
            self.started = false;
            let _ = self.stop.call(self.made.armed(), self.handle);
        }
    }
}

impl Drop for StreamInstance {
    fn drop(&mut self) {
        self.stop();
        let _ = self.drop.call(self.made.armed(), self.handle);
    }
}

#[derive(Debug, Default)]
pub struct Chain {
    instances: Vec<StreamInstance>,
    buffers: [Vec<u8>; 2],
}

impl Chain {
    pub fn new(instances: Vec<StreamInstance>) -> Self {
        Self {
            instances,
            buffers: [Vec::new(), Vec::new()],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.instances.iter().map(StreamInstance::name).collect()
    }

    pub fn start(&mut self, container: Container, service: Option<u16>) -> Vec<String> {
        let mut refused = Vec::new();
        self.instances
            .retain_mut(|instance| match instance.start(container, service) {
                Ok(()) => true,
                Err(why) => {
                    refused.push(why);
                    false
                }
            });
        refused
    }

    pub fn run<'a>(&'a mut self, bytes: &'a [u8], failed: &mut Vec<String>) -> &'a [u8] {
        if self.instances.is_empty() {
            return bytes;
        }
        let [mut current, mut next] = std::mem::take(&mut self.buffers);
        current.clear();
        current.extend_from_slice(bytes);
        let mut gone = Vec::new();
        for (n, instance) in self.instances.iter_mut().enumerate() {
            match instance.input(&current) {
                Ok(out) => {
                    next.clear();
                    next.extend_from_slice(out);
                    std::mem::swap(&mut current, &mut next);
                }
                Err(why) => {
                    failed.push(why);
                    gone.push(n);
                }
            }
        }
        for n in gone.into_iter().rev() {
            self.instances.remove(n);
        }
        self.buffers = [current, next];
        &self.buffers[0]
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    #[test]
    fn sweeps_only_unused_cwasm() {
        let root = std::env::temp_dir().join(format!("zw-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("plugins");
        let cache = cache_dir(&dir);
        assert_eq!(cache, root.join("plugins_cache"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let used = cache.join("a-0001.cwasm");
        let stale = cache.join("a-0000.cwasm");
        let gone = cache.join("b-0002.cwasm");
        let wasm = dir.join("a.wasm");
        for f in [&used, &stale, &gone, &wasm] {
            std::fs::write(f, b"x").unwrap();
        }
        let swept = sweep_caches(&dir, &[used.as_path()]);
        assert!(swept >= 2);
        assert!(used.is_file());
        assert!(wasm.is_file());
        assert!(!stale.exists());
        assert!(!gone.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leb(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn section(id: u8, body: &[u8], out: &mut Vec<u8>) {
        out.push(id);
        leb(body.len(), out);
        out.extend_from_slice(body);
    }

    fn name(text: &str, out: &mut Vec<u8>) {
        leb(text.len(), out);
        out.extend_from_slice(text.as_bytes());
    }

    /// `zw_stream_input` の中身だけを差し替えた stream のモジュール。
    fn stream_module(input: &[u8]) -> Vec<u8> {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        let types = [
            &[0x04, 0x60, 0x00, 0x01, 0x7f][..],
            &[0x60, 0x03, 0x7f, 0x7f, 0x7f, 0x01, 0x7f],
            &[0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f],
            &[0x60, 0x01, 0x7f, 0x00],
        ]
        .concat();
        section(1, &types, &mut wasm);
        section(3, &[0x05, 0x00, 0x01, 0x02, 0x03, 0x03], &mut wasm);
        section(5, &[0x01, 0x00, 0x01], &mut wasm);
        let mut exports = vec![0x06];
        name("memory", &mut exports);
        exports.extend_from_slice(&[0x02, 0x00]);
        for (n, export) in [
            abi::export::STREAM_NEW,
            abi::export::STREAM_START,
            abi::export::STREAM_INPUT,
            abi::export::STREAM_STOP,
            abi::export::STREAM_DROP,
        ]
        .into_iter()
        .enumerate()
        {
            name(export, &mut exports);
            exports.extend_from_slice(&[0x00, n as u8]);
        }
        section(7, &exports, &mut wasm);
        let mut code = vec![0x05];
        for body in [
            &[0x00, 0x41, 0x01, 0x0b][..],
            &[0x00, 0x41, 0x00, 0x0b],
            input,
            &[0x00, 0x0b],
            &[0x00, 0x0b],
        ] {
            leb(body.len(), &mut code);
            code.extend_from_slice(body);
        }
        section(10, &code, &mut wasm);
        let mut custom = Vec::new();
        name(abi::MANIFEST, &mut custom);
        custom.extend_from_slice(
            format!(r#"{{"abi":{ABI},"name":"t","hooks":["stream"]}}"#).as_bytes(),
        );
        section(0, &custom, &mut wasm);
        wasm
    }

    fn run_stream(input: &[u8]) -> (Result<(), String>, Vec<String>) {
        let engine = engine().expect("エンジン");
        let place = std::env::temp_dir()
            .join(format!("zw-limits-{}", std::process::id()))
            .join("t.wasm");
        let plugin = Plugin::from_bytes(&engine, &stream_module(input), &place).expect("載る");
        if let Some(cache) = plugin.cache() {
            let _ = std::fs::remove_file(cache);
        }
        let logged = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = logged.clone();
        let logger: Logger = Box::new(move |_, text| into.lock().unwrap().push(text.to_owned()));
        let mut stream = plugin.stream(logger, Io::default()).expect("作れる");
        let result = stream.input(&[0; 188]).map(|_| ());
        drop(stream);
        let logged = logged.lock().unwrap().clone();
        (result, logged)
    }

    #[test]
    fn growing_past_the_limit_stops_the_plugin_with_the_reason() {
        // memory.grow 4096 ページ（256 MiB）。
        let (result, logged) =
            run_stream(&[0x00, 0x41, 0x80, 0x20, 0x40, 0x00, 0x1a, 0x41, 0x00, 0x0b]);
        assert!(result.is_err());
        assert!(
            logged.iter().any(|line| line.contains("メモリーが上限")),
            "{logged:?}"
        );
    }

    #[test]
    fn a_call_that_does_not_return_is_stopped_with_the_reason() {
        // loop br 0 end
        let (result, logged) = run_stream(&[0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, 0x41, 0x00, 0x0b]);
        assert!(result.is_err());
        assert!(
            logged.iter().any(|line| line.contains("秒を超えた")),
            "{logged:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_cache_others_can_write_is_not_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zw-cwasm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.cwasm");
        std::fs::write(&file, b"x").unwrap();
        only_owner_writes(&file).unwrap();
        assert!(only_mine(&file));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(!only_mine(&file), "ほかの人が書ける");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "フォルダーは自分だけがアクセスできる");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_and_a_map_are_declared_with_their_defaults() {
        let config: Vec<Setting> = serde_json::from_str(
            r#"[
                {"key":"args","kind":"list","item":"folder","default":["-v","x y"]},
                {"key":"env","kind":"map","item":"env","default":{"B":"2","A":"$HOME"}},
                {"key":"name","default":"abc"},
                {"key":"bare","kind":"list"}
            ]"#,
        )
        .expect("読める");
        assert_eq!(config[0].item, "folder");
        assert_eq!(config[0].default, r#"["-v","x y"]"#);
        assert!(config[1].is_env());
        assert_eq!(config[1].default, r#"{"A":"$HOME","B":"2"}"#);
        assert_eq!(config[2].default, "abc");
        assert!(!config[2].is_env());
        assert_eq!(config[3].default, "");
    }
}
