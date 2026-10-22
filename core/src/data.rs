// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! データ放送のスレッド。セクションを `databroadcast` hook のプラグインへ渡し、レンダリングした画像を
//! [`Shown`] にコピーする。字幕・文字スーパーのレイアウトもここで行う。レンダリングのスレッドは文書を待たない。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zwsdk::layer::{Message, Want};

pub use crate::captions::{Captions, LatestCaptions};
use crate::engine::{CaptionStream, DataStream, Out};
use crate::layers::{self, FromLayer, Run};
use crate::plugins::Loaded;
use crate::sound::sounding::Say;
use crate::wake::Waker;

#[derive(Debug)]
pub enum ToBroadcast {
    /// `tune` は送信側を作ったときの [`RETUNES`]。前の局の読み取りスレッドはしばらく残るので、世代が違えば捨てる。
    Out {
        tune: u64,
        out: Out,
    },
    Key(zwsdk::layer::Key),
    /// `tune` は [`Self::Out`] と同じ。選局の通知より先に届いた新しい局の情報を消さないため。
    Broadcast {
        broadcast: zwsdk::layer::Broadcast,
        tune: u64,
    },
    /// キューが詰まっていても届くように、送る側は [`RETUNES`] も進める。
    Retuned,
    Sought,
    /// UNIX 秒と、録画の再生か。録画なら録った時刻を今とする。
    Clock(i64, bool),
    Area(u32, u32),
    Captions(u8),
    /// 文字スーパー（0 表示しない、1 第 1 言語、2 第 2 言語）。
    Superimpose(u8),
    /// 「表示しない」のときも強制表示のものを出すか。
    Forced {
        caption: bool,
        superimpose: bool,
    },
    Popup(Option<(u32, u32)>),
    Plugins(Arc<Loaded>),
}

/// キュー（256）が埋まると `Retuned` は入らないので、スレッドはループのたびにこれを見る。
pub static RETUNES: AtomicU64 = AtomicU64::new(0);

/// データ放送が要求した選局（`epgTune`）：ネットワーク・ストリーム・サービス。
static TUNE_ASKED: Mutex<Option<(u16, u16, u16)>> = Mutex::new(None);

pub fn take_tune() -> Option<(u16, u16, u16)> {
    TUNE_ASKED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn retuned() {
    zwsdk::net::hold();
    RETUNES.fetch_add(1, Ordering::SeqCst);
}

#[derive(Debug)]
pub struct Feed {
    pub to: SyncSender<ToBroadcast>,
    /// 同じ読み取りスレッドで番組を切り替えたら進める。進めないと前の局のとして全部捨てられる。
    pub tune: AtomicU64,
}

impl Feed {
    pub fn new(to: SyncSender<ToBroadcast>) -> Self {
        Self {
            to,
            tune: AtomicU64::new(RETUNES.load(Ordering::SeqCst)),
        }
    }

    /// 落ちても構わない（カルーセルの次の周回で揃う）。
    pub fn send(&self, out: Out) {
        let _ = self.to.try_send(ToBroadcast::Out {
            tune: self.tune.load(Ordering::SeqCst),
            out,
        });
    }
}

pub type Picture = crate::image::Rgba;

#[derive(Debug, Default)]
pub struct Shown {
    pub size: (f32, f32),
    pub aspect: Option<f32>,
    pub invisible: bool,
    pub video: Option<zwsdk::layer::Rect>,
    pub page: Option<Arc<Picture>>,
    pub c_profile: bool,
    pub waiting: bool,
    pub keys: Vec<String>,
}

pub type Latest = Arc<Mutex<Option<Arc<Shown>>>>;

pub struct Config {
    pub wake: Waker,
    pub sound: Option<std::sync::mpsc::Sender<Say>>,
    pub plugins: Option<Arc<Loaded>>,
    /// `None` なら起動している間だけ。残さないと起動のたびに一分待つアプリがある。
    pub nvram: Option<PathBuf>,
    pub cfg: BTreeMap<String, String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Config")
            .field("sound", &self.sound.is_some())
            .field(
                "plugins",
                &self.plugins.as_ref().map_or(0, |p| p.all().len()),
            )
            .field("nvram", &self.nvram)
            .field("cfg", &self.cfg)
            .finish()
    }
}

#[derive(Debug)]
pub struct Handles {
    pub to: SyncSender<ToBroadcast>,
    pub broadcast: Latest,
    pub captions: LatestCaptions,
    pub layers: layers::Latest,
    pub present: layers::Present,
}

const LAYER_SIZE: (u32, u32) = (1920, 1080);

pub fn spawn(config: Config) -> Handles {
    let (to, from) = sync_channel(256);
    let captions: LatestCaptions = Arc::new(Mutex::new(None));
    let (from_layers, replies) = channel();
    let present = layers::Present::default();
    let setup = layers::Setup {
        wake: Arc::clone(&config.wake),
        sound: config.sound.clone(),
        nvram: config.nvram.clone(),
        cfg: config.cfg.clone(),
        size: LAYER_SIZE,
        present: present.clone(),
    };
    // 後から追加されたレイヤーを起動するため、プラグインが無くても起動の仕組みは作る。
    let (runs, latest_layers, broadcast, spawner) = layers::spawn_all(
        config.plugins.as_deref().unwrap_or(&Loaded::default()),
        &setup,
        from_layers,
    );
    let shared_captions = Arc::clone(&captions);
    let shared_broadcast = Arc::clone(&broadcast);
    if let Err(error) = std::thread::Builder::new()
        .name(String::from("broadcast"))
        .spawn(move || {
            run(
                from,
                replies,
                runs,
                spawner,
                shared_captions,
                shared_broadcast,
                config,
            )
        })
    {
        tracing::warn!("データ放送のスレッドを起動できない: {error}");
    }
    Handles {
        to,
        broadcast,
        captions,
        layers: latest_layers,
        present,
    }
}

struct Thread {
    captions: crate::captions::Tracker,
    layers: Vec<Run>,
    spawner: layers::Spawner,
    kept: Arc<Mutex<Vec<Run>>>,
    streams: Vec<DataStream>,
    application: Option<Message>,
    caption_streams: Vec<CaptionStream>,
    area: (u32, u32),
    popup: Option<(u32, u32)>,
    retunes_seen: u64,
    sound: Option<std::sync::mpsc::Sender<Say>>,
    replaces_captions: bool,
}

/// パニックしたら状態を捨てて作り直す（レイヤーのスレッドは自分で作り直す）。
fn run(
    from: Receiver<ToBroadcast>,
    replies: Receiver<FromLayer>,
    runs: Vec<Run>,
    spawner: layers::Spawner,
    captions: LatestCaptions,
    broadcast: Latest,
    config: Config,
) {
    let kept = Arc::new(Mutex::new(runs));
    loop {
        // レイヤーのハンドルはコピーして渡す（パニックしてもレイヤーのスレッドを巻き添えにしない）。
        let taken = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runs = kept.lock().unwrap_or_else(|e| e.into_inner()).clone();
            once(&from, &replies, runs, &spawner, &kept, &captions, &config)
        }));
        if taken.is_ok() {
            return;
        }
        tracing::warn!("データ放送のスレッドが異常終了した。再起動する");
        *captions.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *broadcast.lock().unwrap_or_else(|e| e.into_inner()) = None;
        (config.wake)();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn once(
    from: &Receiver<ToBroadcast>,
    replies: &Receiver<FromLayer>,
    layers: Vec<Run>,
    spawner: &layers::Spawner,
    kept: &Arc<Mutex<Vec<Run>>>,
    latest_captions: &LatestCaptions,
    config: &Config,
) {
    let replaces_captions = replaces_captions(&layers);
    let mut thread = Thread {
        captions: crate::captions::Tracker::default(),
        layers,
        spawner: spawner.clone(),
        kept: Arc::clone(kept),
        streams: Vec::new(),
        application: None,
        caption_streams: Vec::new(),
        area: (1920, 1080),
        popup: None,
        retunes_seen: 0,
        sound: config.sound.clone(),
        replaces_captions,
    };
    loop {
        match from.recv_timeout(Duration::from_millis(50)) {
            Err(RecvTimeoutError::Disconnected) => return,
            Ok(message) => thread.take(message),
            Err(RecvTimeoutError::Timeout) => {}
        }
        while let Ok(message) = from.try_recv() {
            thread.take(message);
        }
        while let Ok(reply) = replies.try_recv() {
            if thread.reply(reply) {
                (config.wake)();
            }
        }
        if RETUNES.load(Ordering::SeqCst) != thread.retunes_seen {
            thread.take(ToBroadcast::Retuned);
        }
        if thread.publish_captions(latest_captions) {
            (config.wake)();
        }
    }
}

impl Thread {
    fn tell(&self, want: Want, message: Message, bytes: &[u8]) {
        for run in self.layers.iter().filter(|run| run.wants(want)) {
            run.send(message.clone(), bytes);
        }
    }

    fn tell_all(&self, message: Message) {
        for run in &self.layers {
            run.tell(message.clone());
        }
    }

    fn take(&mut self, message: ToBroadcast) {
        if let ToBroadcast::Broadcast { broadcast, tune } = message {
            let current = RETUNES.load(Ordering::SeqCst);
            if tune != current {
                return;
            }
            if self.retunes_seen != current {
                // 新しい局の情報が `Retuned` より先に届いた。先に閉じる（あとで閉じると新しい情報ごと消える）。
                self.take(ToBroadcast::Retuned);
            }
            self.tell(Want::Broadcast, Message::Broadcast(broadcast), &[]);
            return;
        }
        let out = match message {
            ToBroadcast::Out { tune, out } => {
                let current = RETUNES.load(Ordering::SeqCst);
                if tune != current {
                    return;
                }
                if self.retunes_seen != current {
                    self.take(ToBroadcast::Retuned);
                }
                out
            }
            other => {
                self.take_other(other);
                return;
            }
        };
        match out {
            Out::DataStreams(streams) => {
                if streams != self.streams {
                    let message = Message::Streams(streams.iter().map(stream_of).collect());
                    self.streams = streams;
                    self.tell(Want::Streams, message, &[]);
                }
            }
            Out::ApplicationService(application) => {
                let message =
                    Message::Application(application.map(|a| zwsdk::layer::ApplicationService {
                        resolution: a.resolution,
                        ait: a.ait,
                        data: a.data,
                        emts: a.emts,
                    }));
                self.application = Some(message.clone());
                self.tell(Want::Streams, message, &[]);
            }
            Out::Data { pid, section } => {
                self.tell(Want::Streams, Message::Section { pid }, &section);
            }
            Out::Item {
                pid,
                mpu,
                item,
                fragmentation,
                data,
            } => {
                self.tell(
                    Want::Streams,
                    Message::Item {
                        pid,
                        mpu,
                        item,
                        fragmentation,
                    },
                    &data,
                );
            }
            Out::CaptionStreams(streams) => {
                if self.caption_streams != streams {
                    self.captions.ttml.assets_left(&streams);
                    self.caption_streams = streams;
                    let message = Message::CaptionStreams(
                        self.caption_streams.iter().map(caption_of).collect(),
                    );
                    self.tell(Want::Captions, message, &[]);
                }
            }
            Out::Caption { pid, data, .. } => {
                self.captions
                    .take(pid, &data, &self.caption_streams, self.sound.as_ref());
                if self.caption_streams.iter().any(|c| c.pid == pid) || self.replaces_captions {
                    self.tell(Want::Captions, Message::Caption { pid }, &data);
                }
            }
            _ => {}
        }
    }

    fn take_other(&mut self, message: ToBroadcast) {
        match message {
            ToBroadcast::Out { .. } => unreachable!("Out は take が受ける"),
            ToBroadcast::Key(key) => self.tell(Want::Keys, Message::Key(key), &[]),
            ToBroadcast::Clock(unix, recorded) => {
                self.tell(Want::Clock, Message::Clock { unix, recorded }, &[]);
            }
            ToBroadcast::Area(width, height) => {
                if self.area != (width, height) && width > 0 && height > 0 {
                    self.area = (width, height);
                    self.tell_all(Message::Area { width, height });
                }
            }
            ToBroadcast::Broadcast { .. } => unreachable!("Broadcast は take が受ける"),
            ToBroadcast::Captions(wanted) => self.want_captions(wanted),
            ToBroadcast::Superimpose(wanted) => self.captions.want_superimpose(wanted),
            ToBroadcast::Forced {
                caption,
                superimpose,
            } => self.captions.force(caption, superimpose),
            ToBroadcast::Popup(area) => {
                self.popup = area;
                self.tell(Want::Popup, Message::Popup(area), &[]);
            }
            ToBroadcast::Plugins(loaded) => self.replug(&loaded),
            ToBroadcast::Sought => {
                self.captions.clear();
                self.tell_all(Message::Sought);
            }
            ToBroadcast::Retuned => {
                // 一度の選局で一度だけ（二度やると先に届いた新しい局のコンポーネントまで捨てる）。
                let current = RETUNES.load(Ordering::SeqCst);
                if self.retunes_seen == current {
                    return;
                }
                self.retunes_seen = current;
                self.captions.reset();
                self.streams.clear();
                self.application = None;
                self.caption_streams.clear();
                self.tell_all(Message::Retuned);
                zwsdk::net::release();
            }
        }
    }

    fn replug(&mut self, loaded: &Loaded) {
        let fresh = self.spawner.sync(&mut self.layers, loaded);
        *self.kept.lock().unwrap_or_else(|e| e.into_inner()) = self.layers.clone();
        self.replaces_captions = replaces_captions(&self.layers);
        for run in &fresh {
            tracing::info!("レイヤー {} を追加した", run.name);
            run.tell(Message::Area {
                width: self.area.0,
                height: self.area.1,
            });
            if run.wants(Want::Popup) && self.popup.is_some() {
                run.tell(Message::Popup(self.popup));
            }
            if run.wants(Want::Streams) && !self.streams.is_empty() {
                run.tell(Message::Streams(
                    self.streams.iter().map(stream_of).collect(),
                ));
            }
            if run.wants(Want::Streams)
                && let Some(application) = &self.application
            {
                run.tell(application.clone());
            }
            if run.wants(Want::Captions) && !self.caption_streams.is_empty() {
                run.tell(Message::CaptionStreams(
                    self.caption_streams.iter().map(caption_of).collect(),
                ));
            }
        }
    }

    fn want_captions(&mut self, wanted: u8) {
        let shown = self.captions.want(wanted);
        self.tell(Want::Captions, Message::CaptionShown(shown), &[]);
    }

    fn reply(&mut self, reply: FromLayer) -> bool {
        match reply {
            FromLayer::CaptionDisplay { language, on } => {
                tracing::info!(
                    "レイヤーから字幕 {language} の{}要求",
                    if on { "表示" } else { "非表示" }
                );
                self.want_captions(if on { (language as u8).clamp(1, 2) } else { 0 });
                false
            }
            FromLayer::Tune {
                network,
                transport,
                service,
            } => {
                tracing::info!("レイヤーから選局要求: {network:04x}.{transport:04x}.{service:04x}");
                *TUNE_ASKED.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((network, transport, service));
                true
            }
        }
    }

    fn publish_captions(&mut self, latest: &LatestCaptions) -> bool {
        let Some(shown) = self.captions.snapshot() else {
            return false;
        };
        *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(shown));
        true
    }
}

fn replaces_captions(layers: &[Run]) -> bool {
    layers
        .iter()
        .any(|run| run.place == zwsdk::layer::Place::At(zwsdk::layer::Slot::Captions))
}

fn stream_of(s: &DataStream) -> zwsdk::layer::DataStream {
    zwsdk::layer::DataStream {
        pid: s.pid,
        tag: s.tag,
        coding: s.coding,
        entry: s.entry,
        auto_start: s.auto_start,
        mmt: s.mmt,
    }
}

fn caption_of(stream: &CaptionStream) -> zwsdk::layer::CaptionStream {
    let info = stream.info.as_ref();
    zwsdk::layer::CaptionStream {
        pid: stream.pid,
        tag: stream.tag,
        language: info.map_or_else(|| String::from("jpn"), |i| i.language.clone()),
        kind: info.map_or(0, |i| i.kind),
        tmd: info.map_or(0b1111, |i| i.tmd),
        resolution: info.map_or(1, |i| i.resolution),
        compression: info.map_or(0, |i| i.compression),
        reference_start: info.and_then(|i| i.reference_start),
    }
}
