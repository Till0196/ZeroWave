// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! スレッドの間で渡すもの。サーフェスはそれを使うスレッドが持つ（イベントループが持つと
//! モーダルループで映像が止まり、`configure` がコマンドの送信とぶつかる）。

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use zwcore::guide::{Band, Key};

use crate::draw::overlay::Drawn;
use crate::draw::scene::Scene;

/// 再生スレッドが最後に出したフレームの大きさ。ウィンドウの大きさが変わったとき、メインスレッドは新しい大きさの
/// 1 フレーム出るまで `WM_SIZE` を返さずに待つ（OS の引き伸ばしで文字が伸びないように）。
#[derive(Debug, Default)]
pub struct Presented {
    shown: Mutex<Shown>,
    changed: Condvar,
    /// 送ったが再生スレッドがまだ受け取っていない画面の更新の数（止まっている間にため続けないように）。
    drawn_waiting: std::sync::atomic::AtomicUsize,
}

impl Presented {
    pub fn drawn_sent(&self) {
        self.drawn_waiting
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    pub fn drawn_taken(&self) {
        let _ = self.drawn_waiting.try_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |waiting| waiting.checked_sub(1),
        );
    }

    pub fn drawn_waiting(&self) -> bool {
        self.drawn_waiting
            .load(std::sync::atomic::Ordering::Acquire)
            > 0
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Shown {
    size: (u32, u32),
    whole: bool,
    /// 一度でも出したか。出さないバックエンド（GLES 2.0）では待たない。
    ever: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caught {
    Up,
    Nobody,
    Late,
}

impl Presented {
    /// 1 フレーム出した。垂直同期ごとに呼ばれるので、変わったときだけ起こす。
    pub fn say(&self, size: (u32, u32), whole: bool) {
        let now = Shown {
            size,
            whole,
            ever: true,
        };
        let Ok(mut shown) = self.shown.lock() else {
            return;
        };
        if *shown != now {
            *shown = now;
            self.changed.notify_all();
        }
    }

    pub fn wait_for(&self, size: (u32, u32), most: Duration) -> Caught {
        let until = Instant::now() + most;
        let Ok(mut shown) = self.shown.lock() else {
            return Caught::Nobody;
        };
        loop {
            if !shown.ever {
                return Caught::Nobody;
            }
            if shown.size == size && shown.whole {
                return Caught::Up;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Caught::Late;
            }
            shown = match self.changed.wait_timeout(shown, left) {
                Ok((shown, _)) => shown,
                Err(_) => return Caught::Nobody,
            };
        }
    }
}

pub enum ToStage {
    Drawn(Box<Drawn>),
    Resized((u32, u32)),
    /// 映像を出す矩形（ピクセル）。内接させた映像そのもの（でないと丸めの差で OSD のバーが狭くなる）。
    Area {
        left: u32,
        top: u32,
        width: u32,
        height: u32,
    },
    /// 映像をサーフェスの中のどこに縮めるか（データ放送の文書が置いた場所）。`None` は全部。
    Inset(Option<[f32; 5]>),
    Capture(Box<crate::capture::Job>),
    Hidden(bool),
    /// ウィンドウが全画面表示になった・戻った。
    FullScreen(bool),
    /// ユーザーがウィンドウを掴んで動かしている。その間サーフェスはコンポジターが保持して返さないので取りに行かない。
    Grabbed(bool),
    Display(u32),
    Monitor(crate::draw::monitor::Monitor),
    Wants {
        guide: bool,
        info: bool,
        mini: bool,
    },
    Act(Act),
    Quit,
    Settings(Box<crate::config::File>),
    Replugged(bool),
    /// 視聴中のチューナーを空ける（添えるのはトーストに出す理由）・戻す（`None`）。
    YieldTuner(Option<&'static str>),
    Scanned(Vec<zwcore::guide::Service>),
    /// デバイスを作り直した（[`Request::Reinstance`] の応答）。
    Reinstanced {
        instance: wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
    },
}

impl std::fmt::Debug for ToStage {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Drawn(_) => out.write_str("Drawn"),
            Self::Resized(size) => write!(out, "Resized({}x{})", size.0, size.1),
            Self::Area {
                left,
                top,
                width,
                height,
            } => write!(out, "Area({left},{top} {width}x{height})"),
            Self::Inset(inset) => write!(out, "Inset({inset:?})"),
            Self::Capture(_) => out.write_str("Capture"),
            Self::Hidden(hidden) => write!(out, "Hidden({hidden})"),
            Self::FullScreen(full) => write!(out, "FullScreen({full})"),
            Self::Grabbed(grabbed) => write!(out, "Grabbed({grabbed})"),
            Self::Display(hz) => write!(out, "Display({hz})"),
            Self::Monitor(monitor) => write!(out, "Monitor({monitor:?})"),
            Self::Wants { .. } => out.write_str("Wants"),
            Self::Act(act) => write!(out, "Act({act:?})"),
            Self::Settings(_) => out.write_str("Settings"),
            Self::Replugged(streams) => write!(out, "Replugged({streams})"),
            Self::YieldTuner(yielding) => write!(out, "YieldTuner({yielding:?})"),
            Self::Scanned(services) => write!(out, "Scanned({})", services.len()),
            Self::Reinstanced { .. } => out.write_str("Reinstanced"),
            Self::Quit => out.write_str("Quit"),
        }
    }
}

#[derive(Debug)]
pub enum Act {
    /// 録画を始める・止める。`status_bar` はステータスバーで押したか（停止の確認に使う）。
    Record {
        status_bar: bool,
    },
    Reset,
    ResetViewer,
    RebuildPlayer,
    ResetErrorCount,
    /// 再生オフ。映像と音を止め、受信と録画は続ける。
    PlaybackOff,
    Aspect(String),
    AddFavourite,
    ClearRecent,
    ChannelBack,
    ChannelForward,
    ChannelPrevious,
    NextSound,
    Sound(usize),
    Picture(usize),
    NextPicture,
    StepChannel(bool),
    TuneRemote {
        band: Band,
        number: u8,
    },
    ShowOsd,
    VolumeBy(i32),
    ToggleMute,
    /// 音ずれ補正をミリ秒だけ動かす（正は音声を遅らせる）。`None` は 0 に戻す。
    AudioDelay(Option<i32>),
    SetVolume(u32),
    Profile(String),
    PluginPick {
        stem: String,
        id: String,
    },
    Toast(String),
    Tune(Key),
    Open(std::path::PathBuf),
    /// 「再生モード」。入っていなければ入り、入っていれば最後に選局を要求した局へ戻る。
    PlayerMode,
    Seek(f32),
    SeekBy(i64),
    Pause,
    /// 録画の再生の速さ（等速は 1）。同じ速さをもう一度なら等速へ。
    Pace(f64),
    Band(Band),
    StripShift(i32),
    StripRows(u32),
}

pub enum FromStage {
    Scene(Box<Scene>),
    Done,
}

impl std::fmt::Debug for FromStage {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Scene(_) => "Scene",
            Self::Done => "Done",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// メインウィンドウの真ん中。その画面の作業領域からははみ出さない。
    Centred,
    /// 閉じたときの位置と大きさを `state.toml` の `[panes]` に保存する（キーはその名前）。
    Kept(&'static str),
}

/// 別ウィンドウのオーナー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    None,
    Main,
    Pane(winit::window::WindowId),
}

/// メインスレッドへの要求（別ウィンドウのスレッドから）。winit は Windows でウィンドウのハンドルを
/// イベントループのスレッドにしか渡さないので、ウィンドウはここで開く。
#[derive(Debug)]
pub enum Request {
    Captions(String),
    Window {
        title: String,
        size: (f32, f32),
        buttons: winit::window::WindowButtons,
        resizable: bool,
        min_size: Option<(f32, f32)>,
        place: Place,
        /// オーナーにするウィンドウ（その上に出て、タスクバーには出ない）。
        owner: Owner,
    },
    /// 開いているウィンドウのオーナーを替える（Windows だけ）。
    Owner {
        window: winit::window::WindowId,
        owner: Owner,
    },
    Title(String),
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    SystemTheme(winit::window::Theme),
    /// 別ウィンドウに OS のテキストコントロールを出してほしい（コントロールはウィンドウのスレッドでしか触れない）。
    Text {
        window: winit::window::WindowId,
        shown: Vec<crate::window::text::Shown>,
    },
    /// 別ウィンドウを閉じた。イベントループが持つウィンドウの参照を手放さないとウィンドウが消えない。
    Closed(winit::window::WindowId),
    Remote(crate::ui::panes::remote::Pressed),
    Panel(crate::ui::guide::panel::Said),
    Settings(Box<crate::config::File>),
    Replugged(bool),
    YieldTuner(Option<&'static str>),
    Scanned(Vec<zwcore::guide::Service>),
    /// wgpu のインスタンスとメインウィンドウのサーフェスを作り直してほしい（イベントループでしか作れない）。
    Reinstance(Option<wgpu::Backends>),
}

#[derive(Debug, Clone, Copy)]
pub struct Wake;
