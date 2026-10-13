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
