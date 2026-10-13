// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! データ放送のフィルター：見ている番組のものだけを、解析せずに外へ渡す。

use std::collections::VecDeque;

use isdb::mhdata::{TABLE_DAMT, TABLE_DCCT, TABLE_DDMT};
use isdb::mmttables::{TABLE_EMT, TABLE_MH_AIT};

use super::packet::{Origin, Section};
use super::{Kind, Program};
use crate::engine::{DataStream, Out};

fn watched(program: Option<&Program>, id: u16) -> bool {
    program.is_some_and(|program| program.carries(id, Kind::Data))
}

/// MH-AIT と EMT は、アプリケーションサービス記述子が指定する識別子のものだけ（他のサービスの
/// MH-AIT も乗っている）。
pub fn forward(program: Option<&Program>, section: &Section<'_>, out: &mut VecDeque<Out>) {
    let application = program.and_then(|program| program.application.as_ref());
    let wanted = match section.origin {
        Origin::Ts => watched(program, section.pid),
        Origin::MmtSi => match section.table_id() {
            TABLE_MH_AIT => application.is_some_and(|a| a.ait == section.pid),
            TABLE_EMT => application.is_some_and(|a| a.emt_tag(section.pid).is_some()),
            // 指定が無い（記述子の無い局）ならどれでも。
            TABLE_DDMT | TABLE_DAMT | TABLE_DCCT => application
                .and_then(|a| a.data)
                .is_none_or(|data| data == section.pid),
            _ => false,
        },
        Origin::TlvSi => false,
    };
    if wanted {
        out.push_back(Out::Data {
            pid: section.pid,
            section: section.data.to_vec(),
        });
    }
}

pub fn forward_item(
    program: Option<&Program>,
    pid: u16,
    mpu: u32,
    item: u32,
    fragmentation: u8,
    data: &[u8],
    out: &mut VecDeque<Out>,
) {
    if watched(program, pid) {
        out.push_back(Out::Item {
            pid,
            mpu,
            item,
            fragmentation,
            data: data.to_vec(),
        });
    }
}

pub fn streams(program: &Program) -> Vec<DataStream> {
    program
        .streams
        .iter()
        .filter(|stream| stream.kind == Kind::Data)
        .filter_map(|stream| {
            let Some(component) = stream.data.as_ref() else {
                return Some(DataStream {
                    pid: stream.id,
                    tag: stream.tag.unwrap_or(0),
                    coding: 0,
                    entry: false,
                    auto_start: false,
                    mmt: true,
                });
            };
            if !isdb::bxml::is_bml(component.id) {
                return None;
            }
            let info = isdb::bxml::additional_info(&component.additional);
            Some(DataStream {
                pid: stream.id,
                tag: stream.tag?,
                coding: component.id,
                entry: info.as_ref().is_some_and(|info| info.entry_point),
                auto_start: info.as_ref().is_some_and(|info| info.auto_start),
                mmt: false,
            })
        })
        .collect()
}

/// 同じストリームで番組を選び直したときも呼ぶ（テーブルは変わらない）。
pub fn announce(program: &Program, out: &mut VecDeque<Out>) {
    out.push_back(Out::DataStreams(streams(program)));
    out.push_back(Out::ApplicationService(program.application.clone()));
    out.push_back(Out::CaptionStreams(super::captions::streams(program)));
}
