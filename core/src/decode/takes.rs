// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レンダラーが DMA-BUF のまま受け取れる形式を、デコーダーが出力形式を選ぶときに問い合わせる所。
//! 誰も登録していなければ [`accepts`] は `None` で、デコーダーは Vulkan に問い合わせる。

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// フレーム全体の DRM フォーマット（`NV12`・`P010`・`P030`・`NV15`）とモディファイアの組。
static TAKEN: RwLock<Option<Vec<(u32, u64)>>> = RwLock::new(None);

/// レンダラーが一枚が一フィールドの DMA-BUF を、組まずに一フィールドずつ画面へ描けるか。
static LONE_FIELDS: AtomicBool = AtomicBool::new(false);

/// Broadcom のモディファイアは列の高さの引数を持つが、EGL が列挙するのは引数無しなので比べる前に外す。
const BROADCOM: u64 = 0x07;

pub fn without_parameter(modifier: u64) -> u64 {
    if modifier >> 56 == BROADCOM {
        modifier & !(((1u64 << 48) - 1) << 8)
    } else {
        modifier
    }
}

pub fn offer(taken: Vec<(u32, u64)>) {
    let taken = taken
        .into_iter()
        .map(|(fourcc, modifier)| (fourcc, without_parameter(modifier)))
        .collect();
    if let Ok(mut held) = TAKEN.write() {
        *held = Some(taken);
    }
}

/// GLES 2.0 のレンダラーが、一枚が一フィールドの DMA-BUF を外部テクスチャで一フィールドずつ描けると知らせる。
pub fn offer_lone_fields(yes: bool) {
    LONE_FIELDS.store(yes, Ordering::Relaxed);
}

/// 一フィールドのまま渡してよいか。誰も知らせていなければ偽。
pub fn takes_lone_fields() -> bool {
    LONE_FIELDS.load(Ordering::Relaxed)
}

/// レンダラーがこの形式を受け取るか。誰も登録していなければ `None`。
pub fn accepts(fourcc: u32, modifier: u64) -> Option<bool> {
    let held = TAKEN.read().ok()?;
    let taken = held.as_ref()?;
    let modifier = without_parameter(modifier);
    Some(taken.contains(&(fourcc, modifier)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_height_of_a_sand_modifier_does_not_take_part() {
        let sand = (BROADCOM << 56) | 4;
        let with_height = (BROADCOM << 56) | (2176 << 8) | 4;
        assert_eq!(without_parameter(with_height), sand);
        assert_eq!(without_parameter(0), 0);
        let arm = 0x0810_0000_0000_0001;
        assert_eq!(without_parameter(arm), arm);
    }
}
