// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! wasm の境界の名前。guest の `#[link]` はリテラルしか取れないので、`tests/abi.rs` が一致を確かめる。

/// 構造を変えたら上げる。
pub const VERSION: u32 = 1;

pub const MANIFEST: &str = "zw_manifest";
/// JavaScript のプラグインのスクリプト（`sdk/ts` の `zw-stamp` が書き込む）。
pub const SCRIPT: &str = "zw_script";
pub const ALLOC: &str = "zw_alloc";
pub const FREE: &str = "zw_free";

pub mod import {
    pub const BUF: &str = "zw_buf_v1";
    pub const OUTPUT: &str = "zw_output_v1";
    pub const LOG: &str = "zw_log_v1";
    pub const CLOCK: &str = "zw_clock_v1";
    pub const CFG: &str = "zw_cfg_v1";
    /// フォントを取得する。フォント名なら本体がそのフォントだけのファイルにして渡す。
    pub const FONT: &str = "zw_font_v1";
    pub const NET: &str = "zw_net_v1";
    /// 起動できるのは `kind = "program"` のキーにユーザーが入れたパスだけ。
    pub const PROC: &str = "zw_proc_v1";
    pub const TUNER: &str = "zw_tuner_v1";
    pub const SOCK: &str = "zw_sock_v1";
    pub const FILES: &str = "zw_files_v1";
    pub const DRAW: &str = "zw_draw_v1";
    pub const LAYER: &str = "zw_layer_v1";
    pub const UI: &str = "zw_ui_v1";
    pub const CONTROL: &str = "zw_control_v1";
    pub const VIDEO: &str = "zw_video_v1";
}

/// 名前に `.` を含むのでソースには渡らない。
pub mod word {
    pub const PART: &str = "multiplex.part";
    /// スロットが複数の搬送波に分かれているときの周波数（`,` 区切り）。
    pub const CARRIERS: &str = "multiplex.carriers";
}

pub fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

pub mod export {
    pub const STREAM_NEW: &str = "zw_stream_new";
    pub const STREAM_START_RELATIVE: &str = "zw_stream_start_relative";
    pub const STREAM_START: &str = "zw_stream_start";
    pub const STREAM_INPUT: &str = "zw_stream_input";
    pub const STREAM_STOP: &str = "zw_stream_stop";
    pub const STREAM_DROP: &str = "zw_stream_drop";
    pub const LAYER_NEW: &str = "zw_layer_new";
    pub const LAYER_MESSAGE: &str = "zw_layer_message";
    pub const LAYER_TICK: &str = "zw_layer_tick";
    pub const LAYER_FRAME: &str = "zw_layer_frame";
    pub const LAYER_EVENTS: &str = "zw_layer_events";
    pub const LAYER_DROP: &str = "zw_layer_drop";
    pub const VIDEO_NEW: &str = "zw_video_new";
    pub const VIDEO_BUILD: &str = "zw_video_build";
    pub const VIDEO_KEY: &str = "zw_video_key";
    pub const VIDEO_EVENT: &str = "zw_video_event";
    pub const VIDEO_DROP: &str = "zw_video_drop";
    pub const UI_MENU: &str = "zw_ui_menu";
    pub const UI_PICK: &str = "zw_ui_pick";
    pub const UI_PAGE: &str = "zw_ui_page";
    pub const UI_EVENT: &str = "zw_ui_event";
}
