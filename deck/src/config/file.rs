// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `zerowave.toml`：どう受けるか、どう映すか。人が書き、UI も書く。
//! 選択肢は [`String`] のまま持ち、解釈は [`crate::choice::Choices::parse`]。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct File {
    pub general: General,
    pub video: Video,
    pub audio: Audio,
    pub playback: Playback,
    pub osd: Osd,
    pub operation: Operation,
    pub keys: BTreeMap<String, String>,
    pub guide: Guide,
    pub data: Data,
    pub ui: Ui,
    pub caption: Caption,
    pub log: Log,
    pub record: Record,
    pub capture: Capture,
    #[serde(rename = "line")]
    pub lines: Vec<Line>,
    #[serde(rename = "source")]
    pub sources: Vec<Source>,
    #[serde(rename = "profile")]
    pub profiles: Vec<Profile>,
    #[serde(rename = "network")]
    pub networks: Vec<Network>,
    /// 局の好み。ルートに置くと `[[network]]` の後の `hide` がその network のキーになる。
    pub channel: Prefs,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Prefs {
    pub hide: Vec<String>,
    pub key: BTreeMap<String, u32>,
    pub name: BTreeMap<String, String>,
    pub list: BTreeMap<String, List>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct General {
    pub on_start: String,
    pub on_start_volume: String,
    pub volume: u32,
    pub always_on_top: bool,
    pub single_instance: bool,
    pub no_sleep: bool,
    pub hide_cursor: u32,
    pub title: String,
    /// 郵便番号。同じリモコン番号の局は同じ地域の局を優先（`zwcore::region`）。
    pub zipcode: String,
    pub launch_coaxd: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            on_start: String::from("last"),
            on_start_volume: String::from("last"),
            volume: 80,
            always_on_top: false,
            single_instance: false,
            no_sleep: true,
            hide_cursor: 3,
            title: String::from("{channel} {event}"),
            zipcode: String::new(),
            launch_coaxd: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Video {
    pub decoder: Decoder,
    pub deinterlace: Deinterlace,
    pub scale: Scale,
    pub render: Render,
    pub picture: Picture,
    pub frame: Frame,
    #[serde(rename = "pan")]
    pub pans: Vec<Pan>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Decoder {
    pub default: String,
    pub api: String,
    /// コーデックごとのデコーダーの優先順位（`方式:デバイス` か `方式`、先頭に `!` で使わない）。
    pub mpeg2: Vec<String>,
    pub h264: Vec<String>,
    pub hevc: Vec<String>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            default: String::from("auto"),
            api: String::from("auto"),
            mpeg2: Vec::new(),
            h264: Vec::new(),
            hevc: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Deinterlace {
    pub method: String,
    pub by: String,
    pub adapt_progressive: bool,
    pub telecine: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Scale {
    pub upscale: String,
    pub downscale: String,
    pub antiring: bool,
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            upscale: String::from("lanczos3"),
            downscale: String::from("mitchell"),
            antiring: true,
        }
    }
}

impl Default for Deinterlace {
    fn default() -> Self {
        Self {
            method: String::from("auto"),
            by: String::from("auto"),
            adapt_progressive: true,
            telecine: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Render {
    pub render: String,
    pub backend: String,
    pub gpu: String,
    pub present: String,
    pub hdr: String,
    pub queue: u32,
}

impl Default for Render {
    fn default() -> Self {
        Self {
            render: String::from("auto"),
            backend: String::from("auto"),
            gpu: String::from("auto"),
            present: String::from("auto"),
            hdr: String::from("auto"),
            queue: 3,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Picture {
    pub brightness: i32,
    pub contrast: i32,
    pub hue: i32,
    pub saturation: i32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Frame {
    pub track: String,
    pub side_cut: bool,
    /// `default` / `16:9` / `4:3` / `letterbox` / `double` / `side_cut`。
    pub aspect: String,
    /// 上右下左。
    pub crop: [u32; 4],
    pub fullscreen: String,
    pub keep_aspect: bool,
}

impl Frame {
    pub fn pan_scan(&self, size: (u32, u32)) -> zwcore::output::video::pan_scan::PanScan {
        use zwcore::output::video::pan_scan::PanScan;
        let (width, height) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let [top, right, bottom, left] = self.crop;
        let mut pan = PanScan {
            crop: [
                left as f32 / width,
                top as f32 / height,
                right as f32 / width,
                bottom as f32 / height,
            ],
            mask: !self.side_cut,
            ..PanScan::default()
        };
        match self.aspect.as_str() {
            "16:9" => pan.aspect = Some(16.0 / 9.0),
            "4:3" => pan.aspect = Some(4.0 / 3.0),
            "letterbox" => pan.cut_to = Some(16.0 / 9.0),
            "side_cut" => pan.cut_to = Some(4.0 / 3.0),
            "double" => pan.crop = pan.crop.map(|f| f + 0.125),
            _ => {}
        }
        pan
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            track: String::from("main"),
            side_cut: false,
            aspect: String::from("default"),
            crop: [0; 4],
            fullscreen: String::from("fit"),
            keep_aspect: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Pan {
    pub name: String,
    pub crop: [u32; 4],
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Audio {
    pub device: String,
    pub passthrough: String,
    pub latency: i32,
    pub dual_mono: String,
    pub mono_to_stereo: bool,
    pub downmix: String,
    pub downmix_22: String,
    pub volume_step: u32,
    pub reset_delay_on_change: bool,
    pub decoder: SoundDecoder,
    /// `downmix`（2×6）と `surround`（6×6）は人が決める行列、`seven_one`（8×24）は 22.2ch → 7.1ch。
    pub matrix: BTreeMap<String, Vec<Vec<f32>>>,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            device: String::new(),
            passthrough: String::from("off"),
            latency: 0,
            dual_mono: String::from("main"),
            mono_to_stereo: true,
            downmix: String::from("auto"),
            downmix_22: String::from("auto"),
            volume_step: 5,
            reset_delay_on_change: true,
            decoder: SoundDecoder::default(),
            matrix: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SoundDecoder {
    pub aac: Vec<String>,
    pub mpeg: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Playback {
    pub buffer: u32,
    pub clock: String,
    pub oneseg: String,
    pub restore_mute: bool,
    pub reset_errors_on_tune: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            buffer: 2,
            clock: String::from("auto"),
            oneseg: String::from("fallback"),
            restore_mute: false,
            reset_errors_on_tune: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Osd {
    pub enabled: bool,
    pub font: String,
    pub opacity: u32,
    pub fade: u32,
    pub channel: String,
    pub event: u32,
    pub event_on: Vec<String>,
}

impl Default for Osd {
    fn default() -> Self {
        Self {
            enabled: true,
            font: String::new(),
            opacity: 100,
            fade: 3,
            channel: String::from("logo+name"),
            event: 5,
            event_on: vec![String::from("channel"), String::from("event")],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Operation {
    pub wheel: String,
    pub wheel_shift: String,
    pub wheel_ctrl: String,
    pub double_click: String,
    pub middle_click: String,
    pub right_click: String,
    pub channel_step_skips_sub: bool,
    pub channel_order: String,
}

impl Default for Operation {
    fn default() -> Self {
        Self {
            wheel: String::from("volume"),
            wheel_shift: String::from("channel"),
            wheel_ctrl: String::from("nothing"),
            double_click: String::from("fullscreen"),
            middle_click: String::from("mute"),
            right_click: String::from("menu"),
            channel_step_skips_sub: true,
            channel_order: String::from("remote"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Guide {
    pub hours: u32,
    pub offset: i32,
    pub cache: bool,
    /// 番組詳細を読むネットワーク識別子（`[[network]]` の `nids` と同じ書き方）。
    pub extended: Vec<toml::Value>,
    pub logo: bool,
    pub font: String,
    pub font_size: u32,
}

impl Default for Guide {
    fn default() -> Self {
        Self {
            hours: 4,
            offset: 9,
            cache: true,
            extended: vec![toml::Value::String(String::from("*"))],
            logo: true,
            font: String::new(),
            font_size: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Data {
    pub model_id: String,
    pub maker_id: String,
    pub maker_oui: String,
    pub plugins: String,
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Ui {
    pub theme: String,
    pub menu_bar: bool,
    pub status_bar: bool,
    pub font: String,
    pub font_size: u32,
    pub status_font: String,
    pub status_font_size: u32,
    pub status_height: u32,
    /// 項目の並び・表示・幅。読むときは [`Ui::status_list`] で、書いていない項目を足す。
    pub status_items: Vec<StatusItem>,
    pub status_multi_row: bool,
    pub status_max_rows: u32,
}

pub const STATUS_ITEMS: [&str; 10] = [
    "profile", "channel", "picture", "volume", "sound", "record", "capture", "signal", "error",
    "clock",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct StatusItem {
    pub id: String,
    pub shown: bool,
    pub width: u32,
}

impl Default for StatusItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            shown: true,
            width: 0,
        }
    }
}

impl StatusItem {
    pub fn defaults() -> Vec<Self> {
        STATUS_ITEMS
            .iter()
            .map(|id| Self {
                id: (*id).to_owned(),
                ..Self::default()
            })
            .collect()
    }
}

impl Ui {
    /// 項目の並び。不明な名前と重複は捨て、書いていない項目は既定の位置に足す。
    pub fn status_list(&self) -> Vec<StatusItem> {
        let mut list: Vec<StatusItem> = Vec::with_capacity(STATUS_ITEMS.len());
        for item in &self.status_items {
            if STATUS_ITEMS.contains(&item.id.as_str()) && !list.iter().any(|it| it.id == item.id) {
                list.push(item.clone());
            }
        }
        for (nth, id) in STATUS_ITEMS.iter().enumerate() {
            if list.iter().any(|it| it.id == *id) {
                continue;
            }
            let at = STATUS_ITEMS[..nth]
                .iter()
                .rev()
                .find_map(|before| list.iter().position(|it| it.id == *before))
                .map_or(0, |at| at + 1);
            list.insert(
                at,
                StatusItem {
                    id: (*id).to_owned(),
                    ..StatusItem::default()
                },
            );
        }
        list
    }

    pub fn set_status_list(&mut self, list: Vec<StatusItem>) {
        self.status_items = if list == StatusItem::defaults() {
            Vec::new()
        } else {
            list
        };
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: String::from("auto"),
            menu_bar: true,
            status_bar: true,
            font: String::new(),
            font_size: 0,
            status_font: String::new(),
            status_font_size: 0,
            status_height: 0,
            status_items: Vec::new(),
            status_multi_row: false,
            status_max_rows: 2,
        }
    }
}

/// 字幕。位置と大きさは放送の指定どおりで、ここは出すかどうか、フォント、背景と縁取りの上書き。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Caption {
    /// 字幕（`off`・`first`・`second`）。メニューと同じ値。
    pub shown: String,
    /// `off` のときも強制表示の字幕を出す。
    pub forced: bool,
    /// 文字スーパー（`off`・`first`・`second`）。字幕とは別に選ぶ。
    pub superimpose: String,
    /// `off` のときも強制表示の文字スーパーを出す。
    pub superimpose_forced: bool,
    pub font: String,
    pub superimpose_font: String,
    pub fallback: bool,
    pub background: String,
    pub opacity: u32,
    pub outline: String,
    pub outline_width: u32,
}

/// 字幕の背景の不透明度の既定（%）。ARIB の半透明の色（CLUT の 65〜127）と同じ。
pub const CAPTION_OPACITY: u32 = 50;

impl Default for Caption {
    fn default() -> Self {
        Self {
            shown: String::from("first"),
            forced: true,
            superimpose: String::from("first"),
            superimpose_forced: true,
            font: String::new(),
            superimpose_font: String::new(),
            fallback: true,
            background: String::from("broadcast"),
            opacity: CAPTION_OPACITY,
            outline: String::from("broadcast"),
            outline_width: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Log {
    pub file: bool,
    pub keep: bool,
    pub level: String,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            file: false,
            keep: false,
            level: String::from("info"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Record {
    pub folder: String,
    /// ファイル名。`{date}` `{time}` `{channel}` `{event}` を置き換える。拡張子は書かない。
    pub name: String,
    /// 視聴中のサービスだけを残す（PAT と PMT を書き直す）。既定は受け取ったとおりに書く。
    pub service_only: bool,
    pub caption: bool,
    pub carousel: bool,
    pub eit: bool,
    pub write_unit: u32,
    pub max_pending: u32,
    pub confirm_channel_change: bool,
    pub confirm_exit: bool,
    pub confirm_stop: bool,
    pub confirm_stop_status_bar_only: bool,
}

pub const RECORD_WRITE_UNIT: u32 = 1024;
pub const RECORD_MAX_PENDING: u32 = 256;

impl Record {
    pub fn buffers(&self) -> zwcore::record::Buffers {
        zwcore::record::Buffers {
            write_unit: self.write_unit.clamp(1, 32_768) as usize * 1024,
            max_pending: self.max_pending.clamp(32, 1024) as usize * 1024 * 1024,
        }
    }

    pub fn confirms_stop(&self, status_bar: bool) -> bool {
        self.confirm_stop && (status_bar || !self.confirm_stop_status_bar_only)
    }
}

impl Default for Record {
    fn default() -> Self {
        Self {
            folder: String::new(),
            name: String::from("{date}_{channel}_{event}"),
            service_only: false,
            caption: true,
            carousel: true,
            eit: true,
            write_unit: RECORD_WRITE_UNIT,
            max_pending: RECORD_MAX_PENDING,
            confirm_channel_change: true,
            confirm_exit: true,
            confirm_stop: false,
            confirm_stop_status_bar_only: false,
        }
    }
}

pub const CAPTURE_JPEG_QUALITY: u8 = 90;
pub const CAPTURE_PNG_LEVEL: u8 = 6;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Capture {
    pub folder: String,
    pub name: String,
    pub format: String,
    /// `source` / `display` / `3/4` などの割合 / `1920x1080` などの決まった大きさ。
    pub size: String,
    pub jpeg_quality: u8,
    pub png_level: u8,
    pub icon_saves: bool,
    pub hdr: bool,
    /// キャプチャに含めるもの。書いていない物は既定（新しく読み込んだプラグインは含めない）。
    pub include: BTreeMap<String, bool>,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            folder: String::new(),
            name: String::from("{date}_{time}_{channel}"),
            format: String::from("png"),
            size: String::from("source"),
            jpeg_quality: CAPTURE_JPEG_QUALITY,
            png_level: CAPTURE_PNG_LEVEL,
            icon_saves: true,
            hdr: false,
            include: BTreeMap::new(),
        }
    }
}

impl Capture {
    pub fn includes(&self, layer: &str) -> bool {
        self.include.get(layer).copied().unwrap_or(matches!(
            layer,
            "superimpose" | "caption" | "databroadcast" | crate::capture::VIDEO_FILTER
        ))
    }
}

/// 受信経路。`plan` がキー（`channels.toml` の `[line.<profile>.<plan>]`）。
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Line {
    pub plan: String,
    pub bands: Vec<String>,
    pub only: Vec<u32>,
    pub satellites: Vec<String>,
    pub modulations: Vec<String>,
}

impl Line {
    pub fn key(&self) -> &str {
        &self.plan
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Source {
    pub label: String,
    pub kind: String,
    pub endpoint: String,
    pub priority: i32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pcsc: bool,
    /// coaxd：そのソケットの場所。空なら `PCSCLITE_CSOCK_NAME` か coaxd のソケットの隣。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pcsc_socket: String,
    #[serde(rename = "tuner_setting", skip_serializing_if = "BTreeMap::is_empty")]
    pub tuner_settings: BTreeMap<String, TunerSetting>,
    pub url: String,
    pub by: String,
    /// Mirakurun：要求に `decode=1` を付ける。
    pub decode: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            label: String::new(),
            kind: String::new(),
            endpoint: String::new(),
            priority: 0,
            pcsc: false,
            pcsc_socket: String::new(),
            tuner_settings: BTreeMap::new(),
            url: String::new(),
            by: String::new(),
            decode: true,
        }
    }
}

impl Source {
    pub fn shown(&self) -> String {
        if !self.label.is_empty() {
            return self.label.clone();
        }
        match self.kind.as_str() {
            "mirakurun" => format!("mirakurun {}", self.url),
            "coaxd" if self.endpoint.is_empty() => String::from("coaxd"),
            "coaxd" => format!("coaxd {}", self.endpoint),
            other => other.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub name: String,
    pub source: &'a Source,
    pub tuner: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TunerSetting {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub systems: Vec<String>,
    /// 衛星の LNB 給電：空（給電しない）、`off`、`low`、`high`。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lnb: String,
    /// 前段の利得：空（起動時のまま）、`auto`、`min`、`max`、`+13dB`。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gain: String,
}

impl TunerSetting {
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty() && self.lnb.is_empty() && self.gain.trim().is_empty()
    }

    pub fn to_core(&self) -> zwcore::input::source::coaxd::TunerSetting {
        zwcore::input::source::coaxd::TunerSetting {
            systems: self.systems.clone(),
            lnb: match self.lnb.trim() {
                "on" => String::from("high"),
                other => other.to_owned(),
            },
            gain: self.gain.trim().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// ソースの並び。coaxd はチューナーを指定できる（`coaxd/model:PT3*`・`coaxd/s270:*`）。
    pub sources: Vec<String>,
    #[serde(flatten)]
    pub overrides: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Network {
    pub name: String,
    pub nids: Vec<toml::Value>,
    pub profile: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct List {
    pub channels: Vec<String>,
}

impl File {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn profile(&self, name: &str) -> Profile {
        if !name.is_empty()
            && let Some(found) = self.profiles.iter().find(|profile| profile.name == name)
        {
            return found.clone();
        }
        Profile {
            name: String::new(),
            sources: self
                .sources
                .iter()
                .map(|source| source.label.clone())
                .collect(),
            overrides: BTreeMap::new(),
        }
    }

    /// プロファイルの並びをソースとチューナーに開く。無いソースを指す物は二つ目に。
    pub fn entries_of<'a>(&'a self, profile: &Profile) -> (Vec<Entry<'a>>, Vec<String>) {
        let mut entries = Vec::new();
        let mut unknown = Vec::new();
        for name in &profile.sources {
            let (label, tuner) = match name.split_once('/') {
                Some((label, tuner)) => (label, Some(tuner.to_owned())),
                None => (name.as_str(), None),
            };
            let Some(source) = self.sources.iter().find(|source| source.label == label) else {
                unknown.push(name.clone());
                continue;
            };
            entries.push(Entry {
                name: name.clone(),
                source,
                tuner: if source.kind == "coaxd" { tuner } else { None },
            });
        }
        (entries, unknown)
    }

    /// プロファイルが使う coax のチューナー。チューナーを指定しない coaxd が一つでもあれば空。
    pub fn coaxd_tuners(&self, profile: &Profile) -> Vec<String> {
        let (entries, _) = self.entries_of(profile);
        let mut tuners = Vec::new();
        for entry in entries.iter().filter(|entry| entry.source.kind == "coaxd") {
            match &entry.tuner {
                Some(tuner) => tuners.push(tuner.clone()),
                None => return Vec::new(),
            }
        }
        tuners
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_keeps_everything_by_default() {
        let record = Record::default();
        assert!(!record.service_only);
        assert!(record.caption && record.carousel && record.eit);
    }

    fn hides(file: &File, key: zwcore::guide::Key) -> bool {
        file.channel
            .hide
            .iter()
            .filter_map(|text| super::super::key::KeyPattern::parse(text))
            .any(|pattern| pattern.matches(key))
    }

    #[test]
    fn the_pcsc_socket_is_off_unless_asked_and_not_written_when_off() {
        let file = File::parse(
            r#"
[[source]]
label = "tvrec"
kind = "coaxd"
pcsc = true
pcsc_socket = "/tmp/zw/pcscd.comm"

[[source]]
label = "plain"
kind = "coaxd"
"#,
        )
        .unwrap();
        assert!(file.sources[0].pcsc);
        assert_eq!(file.sources[0].pcsc_socket, "/tmp/zw/pcscd.comm");
        assert!(!file.sources[1].pcsc, "既定は出さない");
        let written = toml::to_string(&file.sources[1]).unwrap();
        assert!(!written.contains("pcsc"), "{written}");
    }

    #[test]
    fn a_profile_orders_tuners_among_the_sources() {
        let file = File::parse(
            r#"
[[source]]
label = "coaxd"
kind = "coaxd"

[[source]]
label = "home"
kind = "mirakurun"
url = "http://tvrec:40772"

[[profile]]
name = "混ぜる"
sources = ["coaxd/dvb:adapter0*", "home", "coaxd/s270:*", "nobody"]
"#,
        )
        .unwrap();
        let (entries, unknown) = file.entries_of(&file.profile("混ぜる"));
        let names: Vec<(&str, Option<&str>)> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.tuner.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("coaxd/dvb:adapter0*", Some("dvb:adapter0*")),
                ("home", None),
                ("coaxd/s270:*", Some("s270:*")),
            ]
        );
        assert_eq!(unknown, ["nobody"]);
        let (entries, _) = file.entries_of(&file.profile(""));
        assert_eq!(entries[0].tuner, None, "all の coaxd はどのチューナーでも");
        assert_eq!(
            file.coaxd_tuners(&file.profile("混ぜる")),
            ["dvb:adapter0*", "s270:*"],
            "ソースの並びに書いたチューナー"
        );
        assert!(
            file.coaxd_tuners(&file.profile("")).is_empty(),
            "all は全部"
        );
    }

    #[test]
    fn a_profile_names_its_tuners() {
        let file = File::parse(
            r#"
[[source]]
label = "coaxd"
kind = "coaxd"

[[profile]]
name = "ZW-T1"
sources = ["coaxd/model:ZW-T1*"]

[[profile]]
name = "全部"
sources = ["coaxd"]
"#,
        )
        .unwrap();
        assert_eq!(file.coaxd_tuners(&file.profile("ZW-T1")), ["model:ZW-T1*"]);
        assert!(file.coaxd_tuners(&file.profile("全部")).is_empty());
        assert!(
            file.coaxd_tuners(&file.profile("")).is_empty(),
            "all は全部"
        );
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(File::parse("").unwrap(), File::default());
        assert_eq!(File::default().video.decoder.default, "auto");
        assert!(File::default().audio.mono_to_stereo);
        assert_eq!(File::default().audio.downmix_22, "auto");
    }

    #[test]
    fn the_example_in_the_notes_reads() {
        let text = r#"
[general]
on_start = "last"

[video.decoder]
default = "auto"
mpeg2   = ["d3d11:AMD Radeon(TM) Graphics", "!system"]

[video.deinterlace]
method = "bwdif"
by     = "software"

[audio]
latency = 120
dual_mono = "sub"

[[line]]
plan  = "isdbt"
bands = ["uhf"]

[[source]]
label = "coaxd"
kind  = "coaxd"

[[source]]
label = "home"
kind  = "mirakurun"
url   = "http://192.0.2.16:40772"

[[profile]]
name = "自宅"
sources = ["home", "coaxd"]
[profile.audio]
latency = 200

[[network]]
name = "地上"
nids = ["0x7880-0x7fef"]

[[network]]
name = "衛星"
nids = [4, 6, 7]

[channel]
hide = ["4:191", "0x7fe6:*"]

[channel.key]
"0x7fe6:0x400" = 7

[channel.name]
"4:101" = "ゼロウェーブBS"

[channel.list."よく見る"]
channels = ["0x7fe6:0x400", "4:101"]
"#;
        let file = File::parse(text).unwrap();
        assert_eq!(
            file.video.decoder.mpeg2,
            ["d3d11:AMD Radeon(TM) Graphics", "!system"]
        );
        assert!(
            file.video.decoder.h264.is_empty(),
            "指定していないコーデックは空"
        );
        assert_eq!(file.audio.latency, 120);
        assert_eq!(file.lines[0].bands, ["uhf"]);
        assert_eq!(file.sources[1].shown(), "home");
        let profile = file.profile("自宅");
        assert_eq!(profile.sources, ["home", "coaxd"]);
        let (entries, unknown) = file.entries_of(&profile);
        assert_eq!(
            entries[0].source.label, "home",
            "プロファイルの並びが優先順"
        );
        assert!(unknown.is_empty());
        assert_eq!(
            profile.overrides["audio"]["latency"],
            toml::Value::Integer(200)
        );
        assert_eq!(file.networks[1].nids.len(), 3);
        assert_eq!(file.channel.key["0x7fe6:0x400"], 7);
        assert_eq!(file.channel.list["よく見る"].channels.len(), 2);
        assert!(hides(
            &file,
            zwcore::guide::Key {
                network: 4,
                service: 191
            }
        ));
        assert!(hides(
            &file,
            zwcore::guide::Key {
                network: 0x7fe6,
                service: 5
            }
        ));
        assert!(!hides(
            &file,
            zwcore::guide::Key {
                network: 4,
                service: 101
            }
        ));
    }

    #[test]
    fn without_profiles_every_source_is_in_the_implicit_one() {
        let file = File::parse(
            r#"
[[source]]
label = "a"
kind = "mirakurun"
[[source]]
label = "b"
kind = "mirakurun"
[[profile]]
name = "x"
sources = ["b", "nothing"]
"#,
        )
        .unwrap();
        assert_eq!(file.profile("").sources, ["a", "b"]);
        assert_eq!(file.profile("no-such").sources, ["a", "b"]);
        let (entries, unknown) = file.entries_of(&file.profile("x"));
        assert_eq!(entries.len(), 1);
        assert_eq!(unknown, ["nothing"]);
    }

    #[test]
    fn unknown_keys_are_skipped() {
        let file = File::parse("[general]\nfuture = 1\n[future]\nx = 2\n").unwrap();
        assert_eq!(file, File::default());
    }

    #[test]
    fn status_items_keep_their_order_and_fill_in_the_missing() {
        let file = File::parse(
            r#"
[ui]
status_items = [
    { id = "clock", shown = true, width = 0 },
    { id = "error", shown = true, width = 150 },
    { id = "nothing" },
    { id = "profile", shown = false },
    { id = "error", width = 9 },
]
status_multi_row = true
status_max_rows = 3
"#,
        )
        .unwrap();
        let list = file.ui.status_list();
        let ids: Vec<&str> = list.iter().map(|it| it.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "clock", "error", "profile", "channel", "picture", "volume", "sound", "record",
                "capture", "signal"
            ]
        );
        assert_eq!(list[1].width, 150);
        assert!(!list[2].shown);
        assert!(list[3].shown && list[3].width == 0);
        assert!(file.ui.status_multi_row);
        assert_eq!(file.ui.status_max_rows, 3);
    }

    #[test]
    fn the_default_status_items_are_the_old_bar() {
        let mut ui = Ui::default();
        assert!(ui.status_items.is_empty());
        assert!(!ui.status_multi_row);
        assert_eq!(ui.status_list(), StatusItem::defaults());
        assert_eq!(ui.status_list().last().unwrap().id, "clock");
        let mut list = ui.status_list();
        list.swap(0, 1);
        ui.set_status_list(list.clone());
        assert_eq!(ui.status_items, list);
        ui.set_status_list(StatusItem::defaults());
        assert!(ui.status_items.is_empty());
    }
}
