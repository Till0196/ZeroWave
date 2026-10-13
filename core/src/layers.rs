// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのレイヤー（`layer` hook）を動かすスレッド。一つのレイヤーに一つのスレッド。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zwsdk::host::{Io, LayerInstance, Plugin};
use zwsdk::layer::{Config, Event, Frame, Message, Place, Slot, Want};

use crate::image::Rgba;
use crate::plugins::{Loaded, logger};
use crate::sound::sounding::Say;
use crate::wake::Waker;

const REOPEN_FIRST: Duration = Duration::from_secs(1);
const REOPEN_MOST: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct Layer {
    pub name: String,
    pub place: Place,
    pub frame: Frame,
    pub image: Arc<Rgba>,
}

/// 画面と共有する、表示中のレイヤー。[`Place::order`] の順。データ放送のレイヤーは入らない。
pub type Latest = Arc<Mutex<Arc<Vec<Arc<Layer>>>>>;

#[derive(Debug)]
pub enum ToLayer {
    Message(Message, Vec<u8>),
    Quit,
}

#[derive(Debug)]
pub enum FromLayer {
    CaptionDisplay {
        language: u32,
        on: bool,
    },
    Tune {
        network: u16,
        transport: u16,
        service: u16,
    },
}

#[derive(Debug, Clone)]
pub struct Run {
    pub name: String,
    pub place: Place,
    manifest: zwsdk::layer::Manifest,
    to: Sender<ToLayer>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    plugin: Arc<Plugin>,
}

/// レイヤーのキューにバッファしてよいデータの上限。レイヤーがスクリプトでハングしたときにだけ効く。
const LAYER_QUEUE_MOST: usize = 256 << 20;

impl Run {
    pub fn is(&self, plugin: &Arc<Plugin>) -> bool {
        Arc::ptr_eq(&self.plugin, plugin)
    }

    pub fn quit(&self) {
        let _ = self.to.send(ToLayer::Quit);
    }

    pub fn wants(&self, want: Want) -> bool {
        self.manifest.wants(want)
    }

    /// 断片は捨てずにバッファする（捨てると大きなモジュールが次の周期までそろわない）。
    /// [`LAYER_QUEUE_MOST`] を超えたらデータを持つものだけ捨てる。
    pub fn send(&self, message: Message, bytes: &[u8]) {
        use std::sync::atomic::Ordering;
        if !bytes.is_empty() {
            let queued = self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
            if queued + bytes.len() > LAYER_QUEUE_MOST {
                self.queued.fetch_sub(bytes.len(), Ordering::Relaxed);
                return;
            }
        }
        if self
            .to
            .send(ToLayer::Message(message, bytes.to_vec()))
            .is_err()
        {
            self.queued.fetch_sub(bytes.len(), Ordering::Relaxed);
        }
    }

    pub fn tell(&self, message: Message) {
        self.send(message, &[]);
    }
}

#[derive(Clone)]
pub struct Setup {
    pub wake: Waker,
    pub sound: Option<Sender<Say>>,
    pub nvram: Option<PathBuf>,
    pub cfg: BTreeMap<String, String>,
    /// レイヤーの座標系のサイズ。レイアウトは 1920×1080 で書く。
    pub size: (u32, u32),
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Setup")
            .field("sound", &self.sound.is_some())
            .field("nvram", &self.nvram)
            .field("cfg", &self.cfg)
            .field("size", &self.size)
            .finish()
    }
}

/// 設定ウィンドウでプラグインを追加したとき、再起動せずにスレッドを起動するために持つ。
#[derive(Clone)]
pub struct Spawner {
    setup: Setup,
    latest: Latest,
    broadcast: crate::data::Latest,
    from: Sender<FromLayer>,
}

impl std::fmt::Debug for Spawner {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Spawner")
            .field("setup", &self.setup)
            .finish_non_exhaustive()
    }
}

impl Spawner {
    pub fn spawn(&self, plugins: &Loaded, plugin: &Arc<Plugin>) -> Option<Run> {
        if !plugin.has("layer") {
            return None;
        }
        let manifest = plugin.layer_manifest().clone();
        let place = manifest.place();
        let (to, rx) = channel();
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let name = plugin.name().to_owned();
        let thread = Worker {
            name: name.clone(),
            queued: Arc::clone(&queued),
            place,
            own: plugins.io_for(plugin),
            plugin: Arc::clone(plugin),
            setup: self.setup.clone(),
            interval: manifest
                .fps
                .filter(|fps| *fps > 0)
                .map_or(Duration::from_millis(50), |fps| {
                    Duration::from_micros(1_000_000 / u64::from(fps))
                }),
            latest: Arc::clone(&self.latest),
            broadcast: Arc::clone(&self.broadcast),
            from: self.from.clone(),
        };
        if let Err(error) = std::thread::Builder::new()
            .name(format!("layer:{name}"))
            .spawn(move || thread.run(rx))
        {
            tracing::warn!("レイヤー {name} のスレッドを起動できない: {error}");
            return None;
        }
        Some(Run {
            name,
            place,
            manifest,
            to,
            queued,
            plugin: Arc::clone(plugin),
        })
    }

    /// 無くなったものは停止し、新しいものは起動する（残ったものの文書は開き直さない）。起動したものを返す。
    pub fn sync(&self, runs: &mut Vec<Run>, plugins: &Loaded) -> Vec<Run> {
        runs.retain(|run| {
            let kept = plugins.all().iter().any(|plugin| run.is(plugin));
            if !kept {
                tracing::info!("レイヤー {} を停止する", run.name);
                run.quit();
            }
            kept
        });
        let mut fresh = Vec::new();
        for plugin in plugins.all() {
            if runs.iter().any(|run| run.is(plugin)) {
                continue;
            }
            if let Some(run) = self.spawn(plugins, plugin) {
                fresh.push(run.clone());
                runs.push(run);
            }
        }
        runs.sort_by_key(|run| run.place.order());
        fresh
    }
}

pub fn spawn_all(
    plugins: &Loaded,
    setup: &Setup,
    from: Sender<FromLayer>,
) -> (Vec<Run>, Latest, crate::data::Latest, Spawner) {
    let latest: Latest = Arc::new(Mutex::new(Arc::new(Vec::new())));
    let broadcast: crate::data::Latest = Arc::new(Mutex::new(None));
    let spawner = Spawner {
        setup: setup.clone(),
        latest: Arc::clone(&latest),
        broadcast: Arc::clone(&broadcast),
        from,
    };
    let mut runs: Vec<Run> = plugins
        .all()
        .iter()
        .filter_map(|plugin| spawner.spawn(plugins, plugin))
        .collect();
    runs.sort_by_key(|run| run.place.order());
    (runs, latest, broadcast, spawner)
}

fn is_key(message: &ToLayer) -> bool {
    matches!(message, ToLayer::Message(Message::Key(_), _))
}

struct Worker {
    name: String,
    place: Place,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    plugin: Arc<Plugin>,
    own: Io,
    setup: Setup,
    interval: Duration,
    latest: Latest,
    broadcast: crate::data::Latest,
    from: Sender<FromLayer>,
}

impl Worker {
    fn keeps_nvram(&self) -> bool {
        self.own
            .get("nvram")
            .is_none_or(|value| zwsdk::host::truthy(&value))
    }

    fn open(&self) -> Option<LayerInstance> {
        let mut io = self.own.clone();
        let faces = zwsdk::fonts::Faces::find();
        let fonts = zwsdk::draw::font_context(&faces);
        if let Some(sans) = faces.sans {
            io.fonts.insert(String::from("sans"), sans);
        }
        if let Some(mono) = faces.mono {
            io.fonts.insert(String::from("mono"), mono);
        }
        io.cfg
            .extend(self.setup.cfg.iter().map(|(k, v)| (k.clone(), v.clone())));
        // NVRAM を残さないと、フラグが無いときに一分待ってから書き込むエントリーポイントで、起動のたびに一分待つ。
        let config = Config {
            nvram: if self.keeps_nvram() {
                nvram::load(self.setup.nvram.as_deref())
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            },
            size: self.setup.size,
        };
        match self
            .plugin
            .layer(logger(self.name.clone()), io, fonts, &config)
        {
            Ok(layer) => {
                tracing::info!("レイヤー {} を起動した（{:?}）", self.name, self.place);
                Some(layer)
            }
            Err(error) => {
                tracing::warn!("レイヤー {} を起動できない: {error}", self.name);
                None
            }
        }
    }

    /// 異常終了しても再起動する。スレッドごと終了すると、そのレイヤーは二度と表示されない。
    fn run(self, from: Receiver<ToLayer>) {
        loop {
            let taken = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.once(&from)));
            if taken.is_ok() {
                // 画像を残すと外したレイヤーが画面に残ったままになる。
                self.publish(None);
                return;
            }
            tracing::warn!(
                "レイヤー {} のスレッドが異常終了した。再起動する",
                self.name
            );
            self.publish(None);
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn once(&self, from: &Receiver<ToLayer>) {
        let Some(mut layer) = self.open() else {
            while let Ok(message) = from.recv() {
                if matches!(message, ToLayer::Quit) {
                    return;
                }
            }
            return;
        };
        let mut area = self.setup.size;
        let mut ticked = Instant::now() - self.interval;
        // 再起動は重いので、間隔を延ばしていく。
        let mut reopen_after: Option<Instant> = None;
        let mut reopen_wait = REOPEN_FIRST;
        loop {
            let wait = self.interval.saturating_sub(ticked.elapsed());
            let mut hurry = false;
            match from.recv_timeout(wait) {
                Err(RecvTimeoutError::Disconnected) => return,
                Ok(message) => {
                    hurry |= is_key(&message);
                    if !self.take(&mut layer, &mut area, message) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            while let Ok(message) = from.try_recv() {
                hurry |= is_key(&message);
                if !self.take(&mut layer, &mut area, message) {
                    return;
                }
            }
            if !hurry && ticked.elapsed() < self.interval {
                continue;
            }
            ticked = Instant::now();
            if reopen_after.is_some_and(|at| ticked < at) {
                continue;
            }
            if reopen_after.is_some() || !self.turn(&mut layer, area) {
                self.publish(None);
                let Some(again) = self.open() else {
                    reopen_after = Some(ticked + reopen_wait);
                    reopen_wait = (reopen_wait * 2).min(REOPEN_MOST);
                    continue;
                };
                layer = again;
                reopen_after = None;
                reopen_wait = REOPEN_FIRST;
            }
        }
    }

    fn take(&self, layer: &mut LayerInstance, area: &mut (u32, u32), message: ToLayer) -> bool {
        let ToLayer::Message(message, bytes) = message else {
            return false;
        };
        self.queued
            .fetch_sub(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        if let Message::Area { width, height } = &message {
            *area = (*width, *height);
        }
        if let Err(error) = layer.message(&message, &bytes) {
            self.fell(error);
        }
        // 残すと前の局のデータ放送が一瞬出る。
        if matches!(message, Message::Retuned) {
            self.publish(None);
        }
        true
    }

    fn fell(&self, error: impl std::fmt::Display) -> bool {
        tracing::warn!("レイヤー {} でエラー: {error}", self.name);
        false
    }

    fn say(&self, say: Say) {
        if let Some(sound) = &self.setup.sound {
            let _ = sound.send(say);
        }
    }

    fn turn(&self, layer: &mut LayerInstance, area: (u32, u32)) -> bool {
        let ticked = match layer.tick() {
            Ok(ticked) => ticked,
            Err(error) => return self.fell(error),
        };
        if let Some(nvram) = ticked.nvram
            && self.keeps_nvram()
        {
            nvram::save(self.setup.nvram.as_deref(), &nvram.into_iter().collect());
        }
        match layer.frame(area) {
            Ok(None) => {}
            Ok(Some(None)) => self.publish(None),
            Ok(Some(Some((frame, rgba)))) => {
                let image = Arc::new(Rgba::new(frame.width, frame.height, rgba));
                self.publish(Some((frame, image)));
            }
            Err(error) => return self.fell(error),
        }
        let (events, mut audios) = match layer.events() {
            Ok(events) => events,
            Err(error) => return self.fell(error),
        };
        audios.reverse();
        for event in events {
            match event {
                Event::Log(text) => tracing::info!("{}  {text}", self.name),
                Event::Sound(number) => self.say(Say::Chime(number)),
                Event::Audio => {
                    if let Some(bytes) = audios.pop() {
                        self.say(Say::Aiff(bytes));
                    }
                }
                Event::AudioStop => self.say(Say::Aiff(Vec::new())),
                Event::CaptionDisplay { language, on } => {
                    let _ = self.from.send(FromLayer::CaptionDisplay { language, on });
                }
                Event::Tune {
                    network,
                    transport,
                    service,
                } => {
                    let _ = self.from.send(FromLayer::Tune {
                        network,
                        transport,
                        service,
                    });
                }
            }
        }
        true
    }

    fn publish(&self, painted: Option<(Frame, Arc<Rgba>)>) {
        if self.place == Place::At(Slot::Databroadcast) {
            let shown = painted.map(|(frame, page)| {
                Arc::new(crate::data::Shown {
                    size: frame.size,
                    aspect: frame.aspect,
                    invisible: frame.invisible,
                    video: frame.video,
                    page: Some(page),
                    c_profile: frame.c_profile,
                    waiting: frame.waiting,
                    keys: frame.keys,
                })
            });
            *self.broadcast.lock().unwrap_or_else(|e| e.into_inner()) = shown;
        } else {
            let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
            let mut layers: Vec<Arc<Layer>> = latest
                .iter()
                .filter(|layer| layer.name != self.name)
                .cloned()
                .collect();
            if let Some((frame, image)) = painted {
                layers.push(Arc::new(Layer {
                    name: self.name.clone(),
                    place: self.place,
                    frame,
                    image,
                }));
                layers.sort_by_key(|layer| layer.place.order());
            }
            *latest = Arc::new(layers);
        }
        (self.setup.wake)();
    }
}

/// 不揮発メモリー（NVRAM）の保存先。
mod nvram {
    use std::collections::BTreeMap;
    use std::path::Path;

    pub fn load(path: Option<&Path>) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let Some(path) = path else {
            return out;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return out;
        };
        for line in text.lines() {
            let Some((key, hex)) = line.split_once('\t') else {
                continue;
            };
            if !usable(key) || hex.len() % 2 != 0 {
                continue;
            }
            let bytes: Option<Vec<u8>> = hex
                .as_bytes()
                .chunks(2)
                .map(|pair| {
                    let digit = |b: u8| char::from(b).to_digit(16);
                    Some((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
                })
                .collect();
            if let Some(bytes) = bytes {
                out.insert(key.to_owned(), bytes);
            }
        }
        out
    }

    fn usable(key: &str) -> bool {
        !key.is_empty() && !key.chars().any(char::is_control)
    }

    /// 既存の内容にマージして置き換えで書く（もう一つの deck が書いた領域を消さない）。
    pub fn save(path: Option<&Path>, nvram: &BTreeMap<String, Vec<u8>>) {
        let Some(path) = path else {
            return;
        };
        let mut merged = load(Some(path));
        merged.extend(
            nvram
                .iter()
                .map(|(key, bytes)| (key.clone(), bytes.clone())),
        );
        let mut text = String::new();
        for (key, bytes) in merged.iter().filter(|(key, _)| usable(key)) {
            text.push_str(key);
            text.push('\t');
            for b in bytes {
                text.push_str(&format!("{b:02x}"));
            }
            text.push('\n');
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".tmp-{}", std::process::id()));
        let temporary = path.with_file_name(name);
        let written = std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&temporary);
            });
        if let Err(error) = written {
            tracing::warn!("NVRAM を保存できない {}: {error}", path.display());
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn odd_lines_are_skipped_and_odd_keys_are_not_written() {
            let dir = std::env::temp_dir().join(format!("zw-nvram-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("nvram.txt");
            std::fs::write(&path, "a\tあい\nb\t0g\nc\t012\nd\t00ff\n").unwrap();
            let read = load(Some(&path));
            assert_eq!(read.len(), 1, "{read:?}");
            assert_eq!(read["d"], [0x00, 0xff]);
            let mut written = BTreeMap::new();
            written.insert(String::from("e\nf\t00"), vec![1]);
            written.insert(String::from("g"), vec![2]);
            save(Some(&path), &written);
            let read = load(Some(&path));
            assert_eq!(read.keys().collect::<Vec<_>>(), ["d", "g"]);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
