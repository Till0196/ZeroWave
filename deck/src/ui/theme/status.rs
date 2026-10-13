// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::Color32;

pub const HEIGHT: f32 = 22.0;
/// 既定の文字の大きさ（論理ピクセル）。設定のポイントは [`REFERENCE_PT`] との比でこれに掛ける。
pub const TEXT_SIZE: f32 = 11.5;
/// 大きさの基準のポイント（Meiryo UI 9pt）。
pub const REFERENCE_PT: f32 = 9.0;
pub const V_PAD: f32 = 5.0;
/// 欄の幅を測る見本の文字列。そのフォントで測った幅が欄の幅になる。
pub const PROFILE_SAMPLE: &str = "<プロファイル>";
pub const CHANNEL_SAMPLE: &str = "ゼロウェーブテレビ１・東京";
pub const PICTURE_SAMPLE: &str = "7680 x 4320 (100 %)";
pub const SOUND_SAMPLE: &str = "主音声＋副音声";
/// 録画の欄の文字列の見本。前にマーク（停止の四角か録画の丸）が付く。
pub const RECORD_SAMPLES: &[&str] = &["<録画>", "00:00:00"];
pub const RECORD_ICON_EMS: f32 = 1.1;
pub const RECORD_GAP_EMS: f32 = 0.35;
pub const QUALITY_SAMPLE: &str = "00.00 dB / 000.00 Mbps";
pub const COUNTS_SAMPLE: &str = "D 0000 / E 0000 / S 0000";
pub const CLOCK_SAMPLE: &str = "00:00";
pub const PAD: f32 = 8.0;
pub const CLOCK_PAD: f32 = 12.0;
/// 文字ではない欄（音量のバー、カメラ）の幅、文字の高さの何倍か。
pub const VOLUME_EMS: f32 = 6.0;
pub const VOLUME_HEIGHT: f32 = 8.0;
pub const CAPTURE_EMS: f32 = 1.6;
pub const CAPTURE_ICON_EMS: f32 = 1.4;

#[derive(Debug, Clone, Copy)]
pub struct Skin {
    pub background: Color32,
    pub edge: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub dim: Color32,
    /// 値の入った欄。山括弧の仮の文字列より少し明るくする。
    pub value: Color32,
    pub clock: Color32,
    pub dot_recording: Color32,
    /// ドロップ・エラーの数が 0 でないときの文字。赤は録画中の点に使うので橙。
    pub trouble: Color32,
    /// 受信できない・信号レベルが低下しているときの信号の欄（赤字）。
    pub alarm: Color32,
    /// 音量の棒。塗った所は文字と同じ色、消音中は `dim`。
    pub volume_track: Color32,
    pub volume_edge: Color32,
    pub volume_filled: Color32,
    pub seek_back: Color32,
    pub seek_done: Color32,
    pub pushed: Color32,
}

pub const DARK: Skin = Skin {
    background: Color32::from_rgb(0x11, 0x11, 0x13),
    edge: Color32::from_rgb(0x25, 0x25, 0x29),
    divider: Color32::from_rgb(0x25, 0x25, 0x29),
    text: Color32::from_rgb(0xa8, 0xa8, 0xad),
    dim: Color32::from_rgb(0x6c, 0x6c, 0x72),
    value: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    clock: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    dot_recording: Color32::from_rgb(0xe0, 0x46, 0x46),
    trouble: Color32::from_rgb(0xe3, 0xb4, 0x5a),
    alarm: Color32::from_rgb(0xe2, 0x78, 0x6c),
    volume_track: Color32::from_rgb(0x34, 0x34, 0x3a),
    volume_edge: Color32::from_rgba_premultiplied(20, 20, 22, 22),
    volume_filled: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    seek_back: Color32::from_rgb(0x34, 0x34, 0x3a),
    seek_done: Color32::from_rgb(0xf2, 0xf2, 0xf3),
    pushed: Color32::from_rgb(0x4a, 0x7a, 0xe0),
};

/// 映像の下でバーに見えるよう、番組表の背景より少し沈める。
pub const LIGHT: Skin = Skin {
    background: Color32::from_rgb(0xec, 0xed, 0xf1),
    edge: Color32::from_rgb(0xd4, 0xd6, 0xde),
    divider: Color32::from_rgb(0xd0, 0xd2, 0xdc),
    text: Color32::from_rgb(0x3c, 0x40, 0x4c),
    dim: Color32::from_rgb(0x5f, 0x64, 0x72),
    value: Color32::from_rgb(0x24, 0x28, 0x34),
    clock: Color32::from_rgb(0x48, 0x4c, 0x5a),
    dot_recording: Color32::from_rgb(0xd8, 0x30, 0x30),
    trouble: Color32::from_rgb(0x9a, 0x50, 0x00),
    alarm: Color32::from_rgb(0xc8, 0x20, 0x20),
    volume_track: Color32::from_rgb(0xd0, 0xd2, 0xdc),
    volume_edge: Color32::from_rgba_premultiplied(0, 0, 0, 16),
    volume_filled: Color32::from_rgb(0x50, 0x54, 0x60),
    seek_back: Color32::from_rgb(0xd0, 0xd2, 0xdc),
    seek_done: Color32::from_rgb(0x50, 0x54, 0x60),
    pushed: Color32::from_rgb(0xc8, 0xd6, 0xf4),
};

pub const fn skin(mode: super::Mode) -> Skin {
    match mode {
        super::Mode::Light => LIGHT,
        super::Mode::Dark => DARK,
    }
}
