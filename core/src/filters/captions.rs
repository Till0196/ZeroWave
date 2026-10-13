// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 字幕のフィルター：見ている番組の字幕のコンポーネントだけをそのまま [`Out::Caption`] へ。
//! TS は PES（ARIB STD-B24 第三編）、MMT は TTML の MFU。

use std::collections::VecDeque;

use isdb::descriptors::DATA_COMPONENT_CAPTION_ONE_SEG;

use super::{Kind, Program};
use crate::engine::{CaptionStream, Out};

pub fn forward(
    program: Option<&Program>,
    pid: u16,
    mpu: u32,
    at: Option<i64>,
    data: &[u8],
    out: &mut VecDeque<Out>,
) {
    if program.is_some_and(|program| program.carries(pid, Kind::Caption)) {
        out.push_back(Out::Caption {
            pid,
            mpu,
            at,
            data: data.to_vec(),
        });
    }
}

pub fn streams(program: &Program) -> Vec<CaptionStream> {
    program
        .streams
        .iter()
        .filter(|stream| stream.kind == Kind::Caption)
        .map(|stream| CaptionStream {
            pid: stream.id,
            tag: stream.tag.unwrap_or(0),
            info: stream
                .data
                .as_ref()
                .filter(|d| d.id == isdb::mhcaption::DATA_COMPONENT_CAPTION)
                .and_then(|d| isdb::mhcaption::additional_subtitle_info(&d.additional)),
            one_seg: stream
                .data
                .as_ref()
                .is_some_and(|d| d.id == DATA_COMPONENT_CAPTION_ONE_SEG),
        })
        .collect()
}
