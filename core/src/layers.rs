// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのレイヤー（`layer` hook）を動かすスレッド。一つのレイヤーに一つのスレッド。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zwsdk::host::{Io, LayerInstance, Plugin};
use zwsdk::layer::{Config, Event, Frame, Message, Place, Slot, Want};

use crate::image::Rgba;
use crate::plugins::{Loaded, logger};
use crate::sound::sounding::Say;
use crate::wake::Waker;

const REOPEN_FIRST: Duration = Duration::from_secs(1);
const REOPEN_MOST: Duration = Duration::from_secs(30);

/// 提示の合図がこれだけ来なければ、レイヤーは自分の時計で回る。
const BEAT_LOST: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct Layer {
    pub name: String,
    /// レイヤーのスレッドごとに一つ。
    pub id: u64,
    pub place: Place,
    pub frame: Frame,
    pub picture: Picture,
}

#[derive(Debug)]
pub enum Picture {
    Image(Arc<Rgba>),
    /// 貼った絵の並び（`frame` の幅×高さの画素）。提示する側が GPU で重ねる。重ねられない所は [`Layer::image`]。
    Sprites {
        sprites: Vec<zwsdk::host::Placed>,
        image: OnceLock<Arc<Rgba>>,
    },
}

impl Layer {
    /// 画像。絵の並びなら、初めて求められたときに CPU で重ねる。
    pub fn image(&self) -> &Arc<Rgba> {
        match &self.picture {
            Picture::Image(image) => image,
            Picture::Sprites { sprites, image } => image
                .get_or_init(|| Arc::new(compose(sprites, self.frame.width, self.frame.height))),
        }
    }

    pub fn sprites(&self) -> Option<&[zwsdk::host::Placed]> {
        match &self.picture {
            Picture::Image(_) => None,
            Picture::Sprites { sprites, .. } => Some(sprites),
        }
    }
}

/// 絵の並びを CPU で重ねて、アルファで割った RGBA にする（置く位置は画素に丸める）。
fn compose(sprites: &[zwsdk::host::Placed], width: u32, height: u32) -> Rgba {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0f32; w * h * 4];
    for sprite in sprites {
        let image = &sprite.image;
        let [x, y, sw, sh] = sprite.at;
        let [cx, cy, cw, ch] = sprite.clip;
        let left = (x.max(cx).max(0.0).round() as usize).min(w);
        let top = (y.max(cy).max(0.0).round() as usize).min(h);
        let right = ((x + sw).min(cx + cw).round().max(0.0) as usize).min(w);
        let bottom = ((y + sh).min(cy + ch).round().max(0.0) as usize).min(h);
        for row in top..bottom {
            let v = (((row as f32 + 0.5 - y) / sh) * image.height as f32) as usize;
            for column in left..right {
                let u = (((column as f32 + 0.5 - x) / sw) * image.width as f32) as usize;
                if u >= image.width as usize || v >= image.height as usize {
                    continue;
                }
                let from = (v * image.width as usize + u) * 4;
                let into = (row * w + column) * 4;
                let src = &image.rgba[from..from + 4];
                let a = f32::from(src[3]) / 255.0 * sprite.alpha;
                for channel in 0..3 {
                    out[into + channel] = f32::from(src[channel]) / 255.0 * sprite.alpha
                        + out[into + channel] * (1.0 - a);
                }
                out[into + 3] = a + out[into + 3] * (1.0 - a);
            }
        }
    }
    let rgba = out
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| {
            let a = px[3];
            let un = |c: f32| {
                if a > 0.0 {
                    ((c / a).min(1.0) * 255.0).round() as u8
                } else {
                    0
                }
            };
            [un(px[0]), un(px[1]), un(px[2]), (a * 255.0).round() as u8]
        })
        .collect();
    Rgba::new(width, height, rgba)
}

/// 画面を提示するスレッドとレイヤーのスレッドが共有する物。
#[derive(Debug, Clone, Default)]
pub struct Present {
    beats: Arc<Mutex<Vec<Sender<ToLayer>>>>,
    sprites: Arc<AtomicBool>,
    published: Arc<AtomicU64>,
}

impl Present {
    /// 画面を提示した。`at` はその垂直同期の時刻（サーフェスを取得できた時）。レイヤーはこれを合図に次の一枚を
    /// 作り、プラグインにはこの時刻を今の時刻として見せる（時刻で動かす絵が提示の間隔に揃う）。
    pub fn beat(&self, at: Instant) {
        self.beats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|to| to.send(ToLayer::Beat(at)).is_ok());
    }

    /// 提示する側が絵の並び（[`Picture::Sprites`]）を重ねられるか。
    pub fn set_sprites(&self, can: bool) {
        self.sprites.store(can, Ordering::Relaxed);
    }

    pub fn sprites(&self) -> bool {
        self.sprites.load(Ordering::Relaxed)
    }

    /// 表示中のレイヤーを出し直した回数（変わったかを見る。`Latest` のアドレスは使い回されることがある）。
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Acquire)
    }

    fn join(&self, to: Sender<ToLayer>) {
        self.beats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(to);
    }
}

/// 画面と共有する、表示中のレイヤー。[`Place::order`] の順。データ放送のレイヤーは入らない。
pub type Latest = Arc<Mutex<Arc<Vec<Arc<Layer>>>>>;

#[derive(Debug)]
pub enum ToLayer {
    Message(Message, Vec<u8>),
    Beat(Instant),
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
    pub present: Present,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Setup")
            .field("sound", &self.sound.is_some())
            .field("nvram", &self.nvram)
            .field("cfg", &self.cfg)
            .field("size", &self.size)
            .field("present", &self.present)
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
        static IDS: AtomicU64 = AtomicU64::new(1);
        let (to, rx) = channel();
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let name = plugin.name().to_owned();
        let thread = Worker {
            name: name.clone(),
            id: IDS.fetch_add(1, Ordering::Relaxed),
            shown: std::cell::Cell::new(None),
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
        self.setup.present.join(to.clone());
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

/// 画面の作り方に関わる絵の形（幅・高さ・座標系の大きさ・縦横比）。
type Shape = (u32, u32, (u32, u32), Option<u32>);

fn shape(frame: &Frame) -> Shape {
    (
        frame.width,
        frame.height,
        (frame.size.0.to_bits(), frame.size.1.to_bits()),
        frame.aspect.map(f32::to_bits),
    )
}

fn is_key(message: &ToLayer) -> bool {
    matches!(message, ToLayer::Message(Message::Key(_), _))
}

struct Worker {
    name: String,
    id: u64,
    /// 最後に出した絵の並びの形（同じ形の間は画面を作り直さなくてよい）。
    shown: std::cell::Cell<Option<Shape>>,
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
        // 提示の合図が来ている間は合図で一枚作り、来なくなったら自分の時計で作る。
        let mut beaten: Option<Instant> = None;
        // 合図で最後に作った時の、合図の時刻。
        let mut beat_made: Option<Instant> = None;
        loop {
            let wait = if beaten.is_some_and(|at| at.elapsed() < BEAT_LOST) {
                BEAT_LOST
            } else {
                self.interval.saturating_sub(ticked.elapsed())
            };
            let mut hurry = false;
            let mut beat = false;
            let mut next = match from.recv_timeout(wait) {
                Err(RecvTimeoutError::Disconnected) => return,
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
            };
            while let Some(message) = next.take().or_else(|| from.try_recv().ok()) {
                if let ToLayer::Beat(at) = message {
                    beaten = Some(at);
                    beat = true;
                    continue;
                }
                hurry |= is_key(&message);
                if !self.take(&mut layer, &mut area, message) {
                    return;
                }
            }
            let clock = beaten.filter(|at| at.elapsed() < BEAT_LOST);
            let due = match clock {
                // 提示の間隔の揺れで一回飛ばさないよう、少し早くても作る。
                Some(at) => {
                    beat && beat_made.is_none_or(|made| {
                        at.saturating_duration_since(made) + self.interval / 4 >= self.interval
                    })
                }
                None => ticked.elapsed() >= self.interval,
            };
            if !hurry && !due {
                continue;
            }
            ticked = Instant::now();
            if due && clock.is_some() {
                beat_made = clock;
            }
            layer.set_clock(clock);
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
        let sprites = self.place != Place::At(Slot::Databroadcast) && self.setup.present.sprites();
        match layer.frame(area, sprites) {
            Ok(None) => {}
            Ok(Some(None)) => self.publish(None),
            Ok(Some(Some((frame, picture)))) => {
                let picture = match picture {
                    zwsdk::host::Picture::Rgba(rgba) => {
                        Picture::Image(Arc::new(Rgba::new(frame.width, frame.height, rgba)))
                    }
                    zwsdk::host::Picture::Sprites(sprites) => Picture::Sprites {
                        sprites,
                        image: OnceLock::new(),
                    },
                };
                self.publish(Some((frame, picture)));
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

    fn publish(&self, painted: Option<(Frame, Picture)>) {
        if self.place == Place::At(Slot::Databroadcast) {
            let shown = painted.map(|(frame, picture)| {
                let layer = Layer {
                    name: String::new(),
                    id: self.id,
                    place: self.place,
                    frame,
                    picture,
                };
                let page = Arc::clone(layer.image());
                let frame = layer.frame;
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
            // 絵の並びは提示する側が提示のたびに読んで重ねるので、形が変わらなければ画面は作り直さなくてよい。
            let now = painted
                .as_ref()
                .filter(|(_, picture)| matches!(picture, Picture::Sprites { .. }))
                .map(|(frame, _)| shape(frame));
            let quiet = now.is_some() && self.shown.get() == now;
            self.shown.set(now);
            if let Some((frame, picture)) = painted {
                layers.push(Arc::new(Layer {
                    name: self.name.clone(),
                    id: self.id,
                    place: self.place,
                    frame,
                    picture,
                }));
                layers.sort_by_key(|layer| layer.place.order());
            }
            *latest = Arc::new(layers);
            self.setup.present.published.fetch_add(1, Ordering::Release);
            if quiet {
                return;
            }
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
