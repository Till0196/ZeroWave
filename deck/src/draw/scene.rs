// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 重ねるもののスナップショット。画面（`crate::deck`）はこれ以外を読まない。
//! [`Scene::due`] が次に変わる時刻を返し、重ねるスレッドの待ちを決める。

use std::sync::Arc;

use zwcore::guide::{Band, Moment};

use crate::ui;

#[derive(Debug, Default, Clone)]
pub struct Scene {
    pub now: i64,
    pub status: ui::Status,
    pub osd: Option<(ui::overlay::osd::Osd, f32)>,
    pub strip: Option<Banded>,
    /// 映像を表示するサイズ、ピクセル（1440×1080 の 16:9 は 1920×1080）。
    pub picture: Option<(u32, u32)>,
    /// 映像のアスペクト比（横÷縦）。`picture` は丸めたピクセル数なのでずれる。OSD と字幕はこちらで内接させる。
    pub ratio: Option<f32>,
    pub crop: [f32; 4],
    pub backdrop: Backdrop,
    pub toast: Option<(Toasted, f32)>,
    /// サーフェスが HDR か（同じ濃さだと HDR では OSD の背景が濃く見える）。
    pub hdr: bool,
    pub video_filters: Vec<String>,
    pub progress: Option<zwcore::player::Progress>,
    pub menu: Arc<crate::window::menu::Filling>,
    pub now_playing: String,
    pub event: String,
    pub idle: Option<Arc<Idle>>,
    pub trouble: Option<Arc<Trouble>>,
    pub volume: Option<(Volume, f32)>,
}

impl Scene {
    pub fn uncropped(&self, shown: egui::Rect) -> egui::Rect {
        let [left, top, right, bottom] = self.crop;
        let wide = shown.width() / (1.0 - left - right).max(1e-6);
        let tall = shown.height() / (1.0 - top - bottom).max(1e-6);
        egui::Rect::from_min_size(
            egui::pos2(shown.left() - wide * left, shown.top() - tall * top),
            egui::vec2(wide, tall),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub volume: u32,
    pub muted: bool,
    pub passthrough: bool,
}

#[derive(Debug, Clone)]
pub struct Idle {
    pub now: crate::ui::guide::detail::Detail,
    pub band: String,
    pub next: Option<zwcore::guide::Programme>,
}

/// 映らないときの画面。ラジオの待ち画面と同じ枠・同じ座標に出す。
#[derive(Debug, Clone)]
pub struct Trouble {
    pub code: &'static str,
    pub title: &'static str,
    pub guide: &'static str,
    pub station: Option<Station>,
    pub rows: Vec<(String, String)>,
    pub below: Option<(String, String)>,
    pub dim: bool,
}

impl Trouble {
    pub fn mark(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (self.code, &self.rows, &self.below, self.dim).hash(&mut hasher);
        self.station
            .as_ref()
            .map(|station| (&station.channel, &station.band, station.logo.is_some()))
            .hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Debug, Clone)]
pub struct Station {
    pub channel: String,
    pub badge: String,
    pub badge_colour: egui::Color32,
    pub logo: Option<Arc<zwcore::guide::Logo>>,
    pub band: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    #[default]
    None,
    Waiting,
    /// 選局している最中。真っ黒（切り替わったのか壊れたのかが分からなくならないように）。
    Black,
}

#[derive(Debug, Clone)]
pub struct Banded {
    pub listing: Arc<ui::guide::epg::Listing>,
    pub now_when: String,
    pub now_title: String,
    pub band: Band,
    pub bands: Vec<Band>,
    pub watching: Option<zwcore::guide::Key>,
    pub from: Moment,
    /// いま何時か。一覧の `now` は正時にしかビルドし直されない。
    pub moment: Moment,
    pub detail: Option<Box<Idle>>,
    pub guide: Arc<zwcore::guide::Guide>,
    pub plugins: Arc<crate::plugin_ui::Shared>,
}

impl Scene {
    pub fn due(&self, _now: i64) -> Option<i64> {
        None
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Toasted {
    pub title: String,
    pub details: Vec<String>,
    pub tone: zwsdk::ui::Tone,
    pub progress: Option<f32>,
}
