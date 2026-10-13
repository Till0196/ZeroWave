// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レイヤーのプラグイン（`layer` hook）と本体の間のデータ型。本体のレイヤーを置き換えるか上下に重ねる。
//! JSON で、重いものはバイト列で別に渡しここには番号だけを書く。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    Video,
    /// 本体には無く、プラグインが出す。[`Frame`] のドキュメントのサイズ・映像の位置・別ウィンドウはこのレイヤーだけ見る。
    Databroadcast,
    Captions,
    Osd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    At(Slot),
    Above(Slot),
    Below(Slot),
}

impl Place {
    pub fn order(self) -> (Slot, u8) {
        match self {
            Self::Below(slot) => (slot, 0),
            Self::At(slot) => (slot, 1),
            Self::Above(slot) => (slot, 2),
        }
    }

    pub fn slot(self) -> Slot {
        match self {
            Self::At(slot) | Self::Above(slot) | Self::Below(slot) => slot,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Want {
    Streams,
    Captions,
    Keys,
    Clock,
    Area,
    Broadcast,
    Popup,
}

/// 配置先は `at`・`above`・`below` のどれか一つ（無ければ字幕の上）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Slot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<Slot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<Slot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wants: Option<Vec<Want>>,
    /// 一秒に何回レンダリングするか。無ければフレームが変わったと通知したときだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
}

impl Manifest {
    pub fn place(&self) -> Place {
        if let Some(slot) = self.at {
            Place::At(slot)
        } else if let Some(slot) = self.above {
            Place::Above(slot)
        } else if let Some(slot) = self.below {
            Place::Below(slot)
        } else {
            Place::Above(Slot::Captions)
        }
    }

    pub fn wants(&self, want: Want) -> bool {
        match &self.wants {
            Some(wants) => wants.contains(&want),
            None => matches!(want, Want::Clock | Want::Area | Want::Keys),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStream {
    pub pid: u16,
    pub tag: u16,
    pub coding: u16,
    pub entry: bool,
    pub auto_start: bool,
    /// MMT のアセット（`aapp`）か（エントリーは MH-AIT）。
    pub mmt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionStream {
    pub pid: u16,
    pub tag: u16,
    pub language: String,
    pub kind: u8,
    pub tmd: u8,
    pub resolution: u8,
    pub compression: u8,
    pub reference_start: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationService {
    pub resolution: u8,
    pub ait: u16,
    pub data: Option<u16>,
    pub emts: Vec<(u8, u16)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broadcast {
    pub network: u16,
    pub transport: u16,
    pub service: u16,
    pub event: u16,
    pub start: Option<i64>,
    pub duration: Option<u32>,
    pub name: String,
    pub captions: bool,
    pub next: Option<(u16, Option<i64>, Option<u32>, String)>,
    pub audio: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Digit(u8),
    Enter,
    Back,
    Blue,
    Red,
    Green,
    Yellow,
    DataButton,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Streams(Vec<DataStream>),
    Application(Option<ApplicationService>),
    /// DSM-CC、MH-AIT、DDMT/DAMT、EMT。内容は添えたバイト列。
    Section {
        pid: u16,
    },
    Item {
        pid: u16,
        mpu: u32,
        item: u32,
        fragmentation: u8,
    },
    CaptionStreams(Vec<CaptionStream>),
    Caption {
        pid: u16,
    },
    Key(Key),
    Broadcast(Broadcast),
    Retuned,
    Sought,
    Clock {
        unix: i64,
        recorded: bool,
    },
    Area {
        width: u32,
        height: u32,
    },
    CaptionShown(u8),
    Popup(Option<(u32, u32)>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Log(String),
    Tune {
        network: u16,
        transport: u16,
        service: u16,
    },
    Sound(u32),
    Audio,
    AudioStop,
    CaptionDisplay {
        language: u32,
        on: bool,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Rgba,
    Canvas,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub size: (f32, f32),
    #[serde(default)]
    pub aspect: Option<f32>,
    #[serde(default)]
    pub invisible: bool,
    /// ドキュメントの座標。無ければ全画面。データ放送のレイヤーだけが指定できる。
    #[serde(default)]
    pub video: Option<Rect>,
    /// C プロファイルのドキュメントか。別ウィンドウに出す。
    #[serde(default)]
    pub c_profile: bool,
    #[serde(default)]
    pub waiting: bool,
    /// ARIB の `used-key-list`。本体はここに無いキーグループのキーを自分で使う。
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `writePersistentArray` の内容：領域の名前 → バイト列。
    #[serde(default)]
    pub nvram: Vec<(String, Vec<u8>)>,
    #[serde(default)]
    pub size: (u32, u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticked {
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub nvram: Option<Vec<(String, Vec<u8>)>>,
}

pub fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

pub fn from_json<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Option<T> {
    serde_json::from_slice(bytes).ok()
}

// 応答は JSON の一行とその後ろの生のバイト列。書くのは `export_layer!` と `zwsdk/javy`。

#[doc(hidden)]
pub fn encode_frame(frame: &Frame, rgba: &[u8]) -> Vec<u8> {
    let mut out = to_json(frame);
    out.push(b'\n');
    out.extend_from_slice(rgba);
    out
}

#[doc(hidden)]
pub fn decode_frame(bytes: &[u8]) -> Option<(Frame, &[u8])> {
    let (json, rgba) = split_line(bytes)?;
    Some((from_json(json).unwrap_or_default(), rgba))
}

#[doc(hidden)]
pub fn encode_events(events: &[Event], audios: &[Vec<u8>]) -> Vec<u8> {
    let mut out = to_json(&events);
    out.push(b'\n');
    for audio in audios {
        out.extend_from_slice(&(audio.len() as u32).to_be_bytes());
        out.extend_from_slice(audio);
    }
    out
}

#[doc(hidden)]
pub fn decode_events(bytes: &[u8]) -> (Vec<Event>, Vec<Vec<u8>>) {
    let Some((json, mut rest)) = split_line(bytes) else {
        return (Vec::new(), Vec::new());
    };
    let mut audios = Vec::new();
    while let Some((n, tail)) = rest.split_first_chunk() {
        let Some(audio) = tail.get(..u32::from_be_bytes(*n) as usize) else {
            break;
        };
        audios.push(audio.to_vec());
        rest = &tail[audio.len()..];
    }
    (from_json(json).unwrap_or_default(), audios)
}

fn split_line(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|&b| b == b'\n')?;
    Some((&bytes[..at], &bytes[at + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_places_the_layer() {
        let m: Manifest =
            serde_json::from_str(r#"{"at":"databroadcast","wants":["streams","keys"]}"#).unwrap();
        assert_eq!(m.place(), Place::At(Slot::Databroadcast));
        assert!(m.wants(Want::Streams));
        assert!(!m.wants(Want::Clock));
        let m: Manifest = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(m.place(), Place::Above(Slot::Captions));
        assert!(m.wants(Want::Clock) && !m.wants(Want::Streams));
        assert!(Place::Below(Slot::Osd).order() > Place::Above(Slot::Captions).order());
    }

    #[test]
    fn frames_and_events_survive_the_wire() {
        let frame = Frame {
            width: 2,
            height: 1,
            ..Frame::default()
        };
        let bytes = encode_frame(&frame, &[1; 8]);
        assert_eq!(decode_frame(&bytes), Some((frame, &[1u8; 8][..])));
        let events = vec![Event::Audio, Event::Sound(3)];
        let audios = vec![vec![1, 2, 3], Vec::new()];
        let bytes = encode_events(&events, &audios);
        assert_eq!(decode_events(&bytes), (events, audios));
        assert_eq!(decode_events(b"[]"), (Vec::new(), Vec::new()), "改行が無い");
    }

    #[test]
    fn messages_survive_json() {
        for message in [
            Message::Key(Key::DataButton),
            Message::Key(Key::Digit(3)),
            Message::Section { pid: 0x1234 },
            Message::Popup(None),
            Message::Clock {
                unix: 1,
                recorded: true,
            },
        ] {
            let json = to_json(&message);
            assert_eq!(
                from_json::<Message>(&json),
                Some(message.clone()),
                "{}",
                String::from_utf8_lossy(&json)
            );
        }
    }
}
