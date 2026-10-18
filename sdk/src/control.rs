// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! リモコン API（`zw_control_v1`）のデータ型。hooks で `"control"` を宣言したプラグインだけ。

use serde::{Deserialize, Serialize};

/// 本体のキー割り当てと同じ名前。一覧は [`Channels::actions`] が返す。
pub mod action {
    pub const CHANNEL_UP: &str = "channel.up";
    pub const CHANNEL_DOWN: &str = "channel.down";
    pub const VOLUME_UP: &str = "volume.up";
    pub const VOLUME_DOWN: &str = "volume.down";
    pub const MUTE: &str = "mute";
    pub const SETTINGS: &str = "settings";
    pub const GUIDE: &str = "guide";

    /// 番号ボタン（1〜12）。いま見ている種別の中で。
    pub fn number(number: u8) -> String {
        format!("channel.key {number}")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// 本体のメニューと同じ文字列。
    pub band: String,
    pub remote: Option<u8>,
    pub name: String,
    pub nid: u16,
    pub sid: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    pub channels: Vec<Channel>,
    /// （NID、SID）
    pub watching: Option<(u16, u16)>,
    pub actions: Vec<String>,
}

/// いま見ている局と番組（`now`）。TVTest の `GetServiceInfo`・`GetCurrentChannelInfo`・`GetCurrentProgramInfo` に当たる。
/// 放送ソースが無く録画だけを見ているときも、ストリームの SDT・EIT から分かる分は入る。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Now {
    /// 選局の途中や、まだ分からないときは `None`。
    #[serde(default)]
    pub station: Option<Station>,
    /// EIT の現在の番組。
    #[serde(default)]
    pub event: Option<Event>,
    /// EIT の次の番組。
    #[serde(default)]
    pub next: Option<Event>,
    /// 録画を再生している（番組の時刻は放送した時刻）。
    #[serde(default)]
    pub recording: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub network: u16,
    #[serde(default)]
    pub transport: u16,
    pub service: u16,
    /// サービス名（SDT）。
    #[serde(default)]
    pub name: String,
    /// 事業者名（SDT）。
    #[serde(default)]
    pub provider: String,
    /// 本体のメニューと同じ帯域の名前。分からなければ空。
    #[serde(default)]
    pub band: String,
    #[serde(default)]
    pub remote: Option<u8>,
    /// ロゴ ID（SDT のロゴ伝送記述子）。
    #[serde(default)]
    pub logo_id: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: u16,
    #[serde(default)]
    pub name: String,
    /// 短形式イベント記述子のテキスト（番組の説明）。
    #[serde(default)]
    pub text: String,
    /// 拡張形式イベント記述子（項目名、本文）。
    #[serde(default)]
    pub extended: Vec<(String, String)>,
    /// ARIB のジャンル（大分類、中分類）。
    #[serde(default)]
    pub genres: Vec<(u8, u8)>,
    /// UNIX 秒。
    #[serde(default)]
    pub start: Option<i64>,
    /// 秒。
    #[serde(default)]
    pub duration: Option<u32>,
}

/// 本体（`host`）。TVTest の `GetHostInfo` に当たる。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// `ZeroWave` など。
    pub name: String,
    /// パッケージのバージョン（`0.1.2` など）。
    pub version: String,
    /// ビルドの識別（コミットなど）。分からなければ空。
    #[serde(default)]
    pub build: String,
}

/// `logo` の種類。好みの順（大きい物から）で一番目にあるもの。
pub const LOGO_ANY: u8 = 0xff;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_now_reads_with_fields_missing_or_unknown() {
        let now: Now = serde_json::from_str(
            r#"{"station":{"network":32736,"service":1024,"name":"NHK総合1・東京","later":1},
                "event":{"id":7,"name":"ニュース","start":1700000000}}"#,
        )
        .expect("読める");
        let station = now.station.expect("局");
        assert_eq!((station.network, station.service), (0x7fe0, 1024));
        assert_eq!(station.logo_id, None);
        let event = now.event.expect("番組");
        assert_eq!(event.text, "");
        assert_eq!(event.duration, None);
        assert!(!now.recording);
        let empty: Now = serde_json::from_str("{}").expect("空でも読める");
        assert_eq!(empty, Now::default());
    }
}
