// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 情報タブの分配システムのノードと、変調・多重フレーム形式・サービスの種別の名前。

use super::*;

/// 有線の変調（JCTEA の技術的条件 表 6.4-4）。不明な値は番号のまま。
fn qam(modulation: u8) -> String {
    match modulation {
        0x01 => "16QAM".to_owned(),
        0x02 => "32QAM".to_owned(),
        0x03 => "64QAM".to_owned(),
        0x04 => "128QAM".to_owned(),
        0x05 => "256QAM".to_owned(),
        0x07 => "1024QAM".to_owned(),
        other => format!("変調 0x{other:02x}"),
    }
}

/// 多重フレーム形式番号（表 6.3-4、6.4-2）。0xF は記述子では「単一TS伝送方式」、
/// フレームヘッダでは「使用しない」。
pub(super) fn frame_type_name(frame_type: u8, in_header: bool) -> &'static str {
    match frame_type {
        0x01 => "TSMF [53,15]（複数TS伝送方式）",
        0x02 => "拡張TSMF [53,15]（複数搬送波伝送方式のみ）",
        0x0f if in_header => "使用しない",
        0x0f => "単一TS伝送方式（多重フレーム化なし）",
        _ => "未定義",
    }
}

pub(super) fn delivery(info: &StreamInfo) -> Vec<Node> {
    let multiplexes = || {
        info.networks
            .iter()
            .flat_map(|network| network.multiplexes.iter())
    };
    let mut nodes = Vec::new();
    let terrestrial: Vec<_> = multiplexes()
        .filter_map(|m| m.terrestrial.as_ref().map(|t| (m, t)))
        .collect();
    if !terrestrial.is_empty() {
        let children = terrestrial
            .iter()
            .enumerate()
            .map(|(n, (multiplex, told))| {
                Node::branch(
                    format!(
                        "TS{} : TSID 0x{:04x} ({}) / エリア {}",
                        n + 1,
                        multiplex.transport,
                        multiplex.transport,
                        told.area_code
                    ),
                    false,
                    told.carriers_hz
                        .iter()
                        .enumerate()
                        .map(|(m, hertz)| {
                            Node::leaf(format!(
                                "周波数{} : {:.6} MHz",
                                m + 1,
                                f64::from(*hertz) / 1_000_000.0
                            ))
                        })
                        .collect(),
                )
            })
            .collect();
        nodes.push(Node::branch("地上分配システム", false, children));
    }
    let satellite: Vec<_> = multiplexes()
        .filter_map(|m| m.satellite.as_ref().map(|t| (m, t)))
        .collect();
    if !satellite.is_empty() {
        let children = satellite
            .iter()
            .enumerate()
            .map(|(n, (multiplex, told))| {
                Node::leaf(format!(
                    "TS{} : TSID 0x{:04x} ({}) / 周波数 {:.5} GHz / {}度{} / {} / {} Msym/s{}",
                    n + 1,
                    multiplex.transport,
                    multiplex.transport,
                    f64::from(told.downlink_khz) / 1_000_000.0,
                    f64::from(told.orbital_position) / 10.0,
                    if told.east { "東" } else { "西" },
                    zwcore::si::polarisation_name(told.polarisation),
                    f64::from(told.symbol_rate) / 1_000_000.0,
                    told.dvb_modulation()
                        .map(|m| format!(" / {m}"))
                        .unwrap_or_default(),
                ))
            })
            .collect();
        nodes.push(Node::branch("衛星分配システム", false, children));
    }
    let cable: Vec<_> = multiplexes()
        .filter_map(|m| m.cable.as_ref().map(|t| (m, t)))
        .collect();
    if !cable.is_empty() {
        let children = cable
            .iter()
            .enumerate()
            .map(|(n, (multiplex, told))| {
                Node::leaf(format!(
                    "TS{} : TSID 0x{:04x} ({}) / 周波数 {:.4} MHz / {} / {} / {:.3} Msym/s / FEC {}",
                    n + 1,
                    multiplex.transport,
                    multiplex.transport,
                    f64::from(told.frequency_hz) / 1_000_000.0,
                    frame_type_name(told.frame_type, false),
                    qam(told.modulation),
                    f64::from(told.symbol_rate) / 1_000_000.0,
                    told.inner_fec().unwrap_or("?"),
                ))
            })
            .collect();
        nodes.push(Node::branch("ケーブル分配システム", false, children));
    }
    // ケーブル複数搬送波伝送分配システム（高度 BS をケーブルに載せたもの）。搬送波ごとに一行。
    let bonded: Vec<_> = multiplexes().filter(|m| !m.bonding.is_empty()).collect();
    if !bonded.is_empty() {
        let children = bonded
            .iter()
            .enumerate()
            .map(|(n, multiplex)| {
                Node::branch(
                    format!(
                        "TS{} : TSID 0x{:04x} ({}) / 搬送波 {} 本",
                        n + 1,
                        multiplex.transport,
                        multiplex.transport,
                        multiplex.bonding.len()
                    ),
                    false,
                    multiplex
                        .bonding
                        .iter()
                        .enumerate()
                        .map(|(m, carrier)| {
                            Node::leaf(format!(
                                "搬送波{} : 周波数 {:.4} MHz / {} / {} / {:.3} Msym/s / FEC {} / グループ {}",
                                m + 1,
                                f64::from(carrier.frequency_hz) / 1_000_000.0,
                                frame_type_name(carrier.frame_type, false),
                                qam(carrier.modulation),
                                f64::from(carrier.symbol_rate) / 1_000_000.0,
                                carrier.inner_fec().unwrap_or("?"),
                                carrier.group_id,
                            ))
                        })
                        .collect(),
                )
            })
            .collect();
        nodes.push(Node::branch(
            "ケーブル複数搬送波伝送分配システム",
            false,
            children,
        ));
    }
    // ケーブル TS 分割システム（J.183 の分割伝送）。
    let divided: Vec<_> = multiplexes()
        .filter(|m| !m.cable_division.is_empty())
        .collect();
    if !divided.is_empty() {
        let children = divided
            .iter()
            .enumerate()
            .map(|(n, multiplex)| {
                Node::branch(
                    format!(
                        "TS{} : TSID 0x{:04x} ({}) / 搬送波 {} 本",
                        n + 1,
                        multiplex.transport,
                        multiplex.transport,
                        multiplex.cable_division.len()
                    ),
                    false,
                    multiplex
                        .cable_division
                        .iter()
                        .enumerate()
                        .map(|(m, carrier)| {
                            Node::leaf(format!(
                                "搬送波{} : 周波数 {:.4} MHz / {} / {} / {:.3} Msym/s / FEC {} / サービス [{}]",
                                m + 1,
                                f64::from(carrier.frequency_hz) / 1_000_000.0,
                                frame_type_name(carrier.frame_type, false),
                                qam(carrier.modulation),
                                f64::from(carrier.symbol_rate) / 1_000_000.0,
                                carrier.inner_fec().unwrap_or("?"),
                                carrier
                                    .services
                                    .iter()
                                    .map(|service| {
                                        format!("SID {}", zwcore::si::hex_id(*service, 4))
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            ))
                        })
                        .collect(),
                )
            })
            .collect();
        nodes.push(Node::branch("ケーブルTS分割システム", false, children));
    }
    let advanced: Vec<_> = multiplexes()
        .filter_map(|m| m.advanced_cable.as_ref().map(|t| (m, t)))
        .collect();
    if !advanced.is_empty() {
        let children = advanced
            .iter()
            .enumerate()
            .map(|(n, (multiplex, told))| {
                Node::branch(
                    format!(
                        "TS{} : TSID 0x{:04x} ({}) / PLP {} / シンボル長 {} / GI {} / 束ね {} / スライス {} 個",
                        n + 1,
                        multiplex.transport,
                        multiplex.transport,
                        told.plp_id,
                        match told.ofdm_symbol_duration {
                            0 => "448us",
                            1 => "597.33us",
                            _ => "予備",
                        },
                        match told.guard_interval {
                            0 => "1/128",
                            1 => "1/64",
                            _ => "予備",
                        },
                        told.bundled_channel,
                        told.slices.len()
                    ),
                    false,
                    told.slices
                        .iter()
                        .map(|slice| {
                            Node::leaf(format!(
                                "スライス {} : 周波数 {:.4} MHz（{}） / 外符号 {} / {} / 内符号 {}",
                                slice.data_slice_id,
                                f64::from(slice.frequency_hz) / 1_000_000.0,
                                match slice.frequency_type {
                                    0 => "スライス中心",
                                    1 => "システム中心",
                                    2 => "初期中心",
                                    _ => "予備",
                                },
                                match slice.outer_code {
                                    1 => "なし".to_owned(),
                                    2 => "RS(204,188)".to_owned(),
                                    3 => "高度ケーブル（L1）".to_owned(),
                                    other => format!("0x{other:x}"),
                                },
                                isdb::nit::AdvancedCable::modulation_name(slice.modulation)
                                    .map_or_else(
                                        || format!("変調 0x{:02x}", slice.modulation),
                                        str::to_owned
                                    ),
                                isdb::nit::AdvancedCable::inner_code_name(slice.inner_code)
                                    .unwrap_or("?"),
                            ))
                        })
                        .collect(),
                )
            })
            .collect();
        nodes.push(Node::branch("高度ケーブル分配システム", false, children));
    }
    if nodes.is_empty() {
        nodes.push(Node::branch(
            "分配システム",
            false,
            vec![Node::leaf("NIT に分配システム記述子が無い")],
        ));
    }
    nodes
}

/// ARIB STD-B10 のサービスの種別の名前。不明な番号は推測せずに `None`。
pub(super) fn kind_name(kind: u8) -> Option<&'static str> {
    Some(match kind {
        0x01 => "デジタルTV",
        0x02 => "デジタル音声",
        0xa1 => "臨時映像",
        0xa2 => "臨時音声",
        0xa3 => "臨時データ",
        0xa4 => "エンジニアリング",
        0xa5 => "プロモーション映像",
        0xa6 => "プロモーション音声",
        0xa7 => "プロモーションデータ",
        0xa8 => "事前蓄積用データ",
        0xa9 => "蓄積専用データ",
        0xaa => "ブックマーク一覧データ",
        0xab => "サーバー型サイマル",
        0xac => "独立ファイル",
        0xad => "超高精細度4K",
        0xc0 => "データ",
        _ => return None,
    })
}
