// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 情報タブのツリー。[`Node`] で作り、画面とコピーの両方がそれを使う。

use super::delivery::{delivery, frame_type_name, kind_name};
use super::*;

pub(super) fn information(ui: &mut Ui, info: &StreamInfo, skin: &skin::Skin) {
    ui.add_space(8.0);
    for (label, value) in head(info) {
        field(ui, label, &value, skin);
    }
    ui.add_space(6.0);
    let width = ui.available_width();
    let (line, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter()
        .hline(line.x_range(), line.center().y, Stroke::new(1.0, skin.edge));
    ui.add_space(4.0);

    let nodes = tree(info);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(ui, &nodes, 0, skin));
}

pub(super) fn head(info: &StreamInfo) -> [(&'static str, String); 2] {
    let (stream, network) = match info.identity {
        Some(identity) => (
            format!("TSID 0x{:04x} ({})", identity.transport, identity.transport),
            match network_name(info) {
                Some(named) => format!(
                    "NID 0x{:04x} ({}) {named}",
                    identity.network, identity.network
                ),
                None => format!("NID 0x{:04x} ({})", identity.network, identity.network),
            },
        ),
        None => (String::from("—"), String::from("—")),
    };
    [("ストリーム:", stream), ("ネットワーク:", network)]
}

/// 画面に出る順。サービスと受信は開いて、ほかは折りたたんで始まる。
pub(super) fn tree(info: &StreamInfo) -> Vec<Node> {
    let mut nodes = vec![services(info), reception(info)];
    nodes.extend(conditional_access(info));
    nodes.extend(partial(info));
    nodes.push(network_ts(info));
    nodes.push(network_services(info));
    nodes.push(broadcasters(info));
    nodes.extend(logos(info));
    nodes.extend(downloads(info));
    nodes.extend(packages(info));
    nodes.extend(frame_header(info));
    nodes.extend(delivery(info));
    nodes
}

/// ステータスバーの「D / E / S」と同じ数を、何を数えたものか言葉で書く。
fn reception(info: &StreamInfo) -> Node {
    let counts = info.counts;
    let children = vec![
        Node::leaf(format!("パケット: {}", counts.packets)),
        Node::leaf(format!("ドロップ（連続性の飛び）: {}", counts.dropped)),
        Node::leaf(format!(
            "エラー（transport_error_indicator）: {}",
            counts.errors
        )),
        Node::leaf(format!("再生できないパケット: {}", counts.scrambled)),
    ];
    Node::branch("受信", true, children)
}

fn conditional_access(info: &StreamInfo) -> Option<Node> {
    if info.emm.is_empty() {
        return None;
    }
    Some(Node::branch(
        "限定受信 (CAT)",
        false,
        zwcore::si::emm_lines(&info.emm)
            .into_iter()
            .map(Node::leaf)
            .collect(),
    ))
}

fn partial(info: &StreamInfo) -> Option<Node> {
    let partial = info.partial.as_ref()?;
    Some(Node::branch(
        "部分TS (SIT/DIT)",
        true,
        zwcore::si::partial_lines(partial)
            .into_iter()
            .map(Node::leaf)
            .collect(),
    ))
}

fn logos(info: &StreamInfo) -> Option<Node> {
    if info.logos.is_empty() {
        return None;
    }
    Some(Node::branch(
        format!("局ロゴ (CDT) : {} 枚", info.logos.len()),
        false,
        info.logos
            .iter()
            .map(|logo| Node::leaf(zwcore::si::logo_line(logo)))
            .collect(),
    ))
}

fn downloads(info: &StreamInfo) -> Option<Node> {
    if info.downloads.is_empty() {
        return None;
    }
    // メーカーと機種ごとに折りたたむ。告知の文はグループの下に。
    let tables = info
        .downloads
        .iter()
        .map(|table| {
            let groups = zwcore::si::download_contents(table)
                .into_iter()
                .map(|(group, notice)| {
                    if notice.is_empty() {
                        Node::leaf(group)
                    } else {
                        Node::branch(group, false, notice.into_iter().map(Node::leaf).collect())
                    }
                })
                .collect();
            Node::branch(zwcore::si::download_title(table), false, groups)
        })
        .collect();
    Some(Node::branch("ソフトウェアの配信 (SDTT)", false, tables))
}

fn packages(info: &StreamInfo) -> Option<Node> {
    if info.packages.is_empty() && info.layouts.is_empty() {
        return None;
    }
    let mut children: Vec<Node> = info
        .packages
        .iter()
        .map(|package| Node::leaf(zwcore::si::package_line(package)))
        .collect();
    children.extend(
        info.layouts
            .iter()
            .map(|layout| Node::leaf(zwcore::si::layout_line(layout))),
    );
    Some(Node::branch(
        "パッケージ (PLT) / レイアウト (LCT)",
        false,
        children,
    ))
}

fn broadcasters(info: &StreamInfo) -> Node {
    if info.broadcasters.is_empty() {
        return Node::branch("事業者 (BIT)", false, vec![Node::leaf("データ取得中")]);
    }
    let children = info
        .broadcasters
        .iter()
        .map(|broadcaster| {
            let title = format!(
                "{} : broadcaster ID 0x{:02x} ({}) / NID {}",
                if broadcaster.name.is_empty() {
                    "—"
                } else {
                    &broadcaster.name
                },
                broadcaster.id,
                broadcaster.id,
                zwcore::si::hex_id(broadcaster.network, 4)
            );
            Node::branch(
                title,
                false,
                zwcore::si::broadcaster_lines(broadcaster)
                    .into_iter()
                    .map(Node::leaf)
                    .collect(),
            )
        })
        .collect();
    Node::branch("事業者 (BIT)", false, children)
}

fn services(info: &StreamInfo) -> Node {
    if info.programs.is_empty() {
        return Node::branch("サービス", true, vec![Node::leaf("—")]);
    }
    let children = info
        .programs
        .iter()
        .enumerate()
        .map(|(n, program)| {
            let named = info
                .services
                .iter()
                .find(|service| service.id == program.number);
            let mut title = format!("サービス{}", n + 1);
            if let Some(name) = named.map(|s| s.name.as_str()).filter(|s| !s.is_empty()) {
                title.push_str(&format!(" ({name})"));
            }
            title.push_str(&format!(
                " : SID 0x{:04x} ({})",
                program.number, program.number
            ));
            if let Some(kind) = named.map(|s| s.kind).filter(|k| *k != 0) {
                title.push_str(&format!(" / Type 0x{kind:02x}"));
            }
            Node::branch(title, true, components(program))
        })
        .collect();
    Node::branch("サービス", true, children)
}

fn components(program: &zwcore::filters::Program) -> Vec<Node> {
    let mut lines = vec![Node::leaf(format!(
        "PMT : PID 0x{:04x} ({})",
        program.map, program.map
    ))];
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for stream in &program.streams {
        let label = match stream.kind {
            zwcore::filters::Kind::Video(_) => "映像",
            zwcore::filters::Kind::Audio(_) => "音声",
            zwcore::filters::Kind::Caption => "字幕",
            zwcore::filters::Kind::Data => "データ",
            zwcore::filters::Kind::Other => "その他",
        };
        let count = counts.entry(label).or_insert(0);
        *count += 1;
        let tag = stream
            .tag
            .map(|tag| format!(" / component tag 0x{tag:02x}"))
            .unwrap_or_default();
        let decode = stream
            .decode
            .map(|d| {
                format!(
                    " / {}{}{}",
                    d.format_name().unwrap_or("?"),
                    if d.still_picture { " 静止画" } else { "" },
                    if d.sequence_end_code {
                        " 終端符号あり"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        lines.push(Node::leaf(format!(
            "{label}{count} : PID 0x{:04x} ({}) / stream type {} ({}){tag}{decode}",
            stream.id,
            stream.id,
            stream.declared,
            what_declared(&stream.declared),
        )));
    }
    if let Some(region) = &program.region {
        lines.push(Node::leaf(zwcore::guide::region_line(region)));
    }
    if let Some(ntp) = program.emergency_news {
        lines.push(Node::leaf(format!(
            "緊急ニュース : 送出 {}（NTP 0x{ntp:016x}）",
            ntp_time(ntp, zwcore::guide::shown_offset())
        )));
    }
    if program.pcr != 0x1fff {
        lines.push(Node::leaf(format!(
            "PCR : PID 0x{:04x} ({})",
            program.pcr, program.pcr
        )));
    }
    for (m, (pid, system)) in program.ecm.iter().enumerate() {
        lines.push(Node::leaf(format!(
            "ECM{} : PID 0x{pid:04x} ({pid}) / CA system ID {} {}",
            m + 1,
            zwcore::si::hex_id(*system, 4),
            zwcore::si::ca_system_name(*system)
        )));
    }
    lines
}

/// NTP long format（UTC）の時刻を、UTC から `offset` 秒ずれた日時に。
/// 秒の最上位ビットが 0 なら 2036 年が基準（STD-B60 3.1）。
pub(super) fn ntp_time(ntp: u64, offset: i32) -> String {
    /// 1900 年から 1970 年までの秒。
    const NTP_TO_UNIX: i64 = 2_208_988_800;
    let seconds = ntp >> 32;
    let seconds = if seconds & 0x8000_0000 == 0 {
        seconds + (1 << 32)
    } else {
        seconds
    };
    let moment = seconds as i64 - NTP_TO_UNIX + i64::from(offset);
    let (year, month, day, hour, minute, second) = zwcore::guide::civil(moment);
    let zone = match offset {
        zwcore::guide::JST_OFFSET => String::from("JST"),
        0 => String::from("UTC"),
        _ => {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.unsigned_abs() / 60;
            match minutes % 60 {
                0 => format!("UTC{sign}{}", minutes / 60),
                rest => format!("UTC{sign}{}:{rest:02}", minutes / 60),
            }
        }
    };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {zone}")
}

fn frame_header(info: &StreamInfo) -> Option<Node> {
    let header = info.frame_header.as_ref()?;
    let mut lines = vec![
        Node::leaf(format!("多重フレーム同期信号 : 0x{:04x}", header.sync)),
        Node::leaf(format!("バージョン : {}", header.version)),
        Node::leaf(format!(
            "多重フレーム形式 : 0x{:x} {}",
            header.frame_type,
            frame_type_name(header.frame_type, true)
        )),
        Node::leaf(format!(
            "緊急警報指示 : {} ({})",
            u8::from(header.emergency),
            if header.emergency {
                "起動制御が行われている"
            } else {
                "起動制御が行われていない"
            }
        )),
    ];
    if let Some(group) = header.group {
        lines.push(Node::branch(
            "拡張TSMFフレームヘッダ",
            true,
            vec![
                Node::leaf(format!(
                    "搬送波グループ識別 : 0x{:02x} ({})",
                    group.id, group.id
                )),
                Node::leaf(format!("搬送波 : {}/{}", group.sequence, group.carriers)),
                Node::leaf(format!(
                    "スーパーフレーム総数 : {} / 位置 : {}",
                    header.frames, header.position
                )),
            ],
        ));
    }
    for stream in &header.streams {
        let (kind, id) = if stream.tlv {
            ("TLV", "TLVID")
        } else {
            ("TS", "TSID")
        };
        let status = match stream.receive_status {
            0 => "通常",
            1 => "やや悪い",
            2 => "受信不可",
            _ => "不明",
        };
        lines.push(Node::leaf(format!(
            "{kind}{} : {id} 0x{:04x} ({}) / ONID 0x{:04x} ({}) / 受信状態 {status} ({}) / スロット {}",
            stream.number,
            stream.stream_id,
            stream.stream_id,
            stream.original_network_id,
            stream.original_network_id,
            stream.receive_status,
            stream.slots,
        )));
    }
    lines.push(Node::leaf(format!("CRC32 : 0x{:08x}", header.crc)));
    Some(Node::branch("多重フレームヘッダ (TSMF)", false, lines))
}

fn network_ts(info: &StreamInfo) -> Node {
    if info.networks.is_empty() {
        // NIT は数秒に一度回ってくるので、選局した直後は空。
        return Node::branch(
            "ネットワークTS (NIT)",
            false,
            vec![Node::leaf("データ取得中")],
        );
    }
    let children = info
        .networks
        .iter()
        .map(|network| {
            let title = format!(
                "{}{}",
                if network.name.is_empty() {
                    format!("NID 0x{:04x} ({})", network.network, network.network)
                } else {
                    format!(
                        "{} : NID 0x{:04x} ({})",
                        network.name, network.network, network.network
                    )
                },
                if network.own {
                    ""
                } else {
                    "（他ネットワーク）"
                },
            );
            // 緊急警報放送は自ネットワークのノードに、緊急情報記述子が載っている間だけ。
            let mut lines: Vec<Node> = if network.own {
                info.emergency
                    .iter()
                    .map(|warning| Node::leaf(zwcore::si::emergency_line(warning)))
                    .collect()
            } else {
                Vec::new()
            };
            lines.extend(
                network
                    .multiplexes
                    .iter()
                    .enumerate()
                    .map(|(n, multiplex)| multiplex_node(n, multiplex)),
            );
            Node::branch(title, network.own, lines)
        })
        .collect();
    Node::branch("ネットワークTS (NIT)", false, children)
}

fn multiplex_node(n: usize, multiplex: &zwcore::si::Multiplex) -> Node {
    let mut said = format!(
        "TS{} : TSID 0x{:04x} ({})",
        n + 1,
        multiplex.transport,
        multiplex.transport
    );
    if !multiplex.name.is_empty() {
        said.push_str(&format!(" {}", multiplex.name));
    }
    if multiplex.remote_key != 0 {
        said.push_str(&format!(" / リモコン {}", multiplex.remote_key));
    }
    let mut lines: Vec<Node> = zwcore::si::multiplex_lines(multiplex)
        .into_iter()
        .map(Node::leaf)
        .collect();
    for (m, (service, kind)) in multiplex.services.iter().enumerate() {
        let one_seg = if multiplex.partial.contains(service) {
            "（部分受信）"
        } else {
            ""
        };
        lines.push(Node::leaf(format!(
            "サービス{} : SID 0x{service:04x} ({service}) / Type 0x{kind:02x}{}{one_seg}",
            m + 1,
            match kind_name(*kind) {
                Some(named) => format!("（{named}）"),
                None => String::new(),
            },
        )));
    }
    Node::branch(said, false, lines)
}

fn network_services(info: &StreamInfo) -> Node {
    let mut by_transport: std::collections::BTreeMap<(u16, u16), Vec<&Service>> =
        std::collections::BTreeMap::new();
    for service in &info.neighbours {
        by_transport
            .entry((service.network, service.transport))
            .or_default()
            .push(service);
    }
    if by_transport.is_empty() {
        return Node::branch(
            "ネットワークサービス (SDT)",
            false,
            vec![Node::leaf("データ取得中")],
        );
    }
    let own = info.identity.map(|i| (i.network, i.transport));
    let children = by_transport
        .iter()
        .enumerate()
        .map(|(n, ((network, transport), services))| {
            let is_own = own == Some((*network, *transport));
            let title = format!(
                "TS{} : TSID 0x{transport:04x} ({transport}) / NID 0x{network:04x} ({network}){}",
                n + 1,
                if is_own {
                    "（現在のストリーム）"
                } else {
                    ""
                }
            );
            let lines = services
                .iter()
                .enumerate()
                .map(|(m, service)| {
                    let said = format!(
                        "サービス{} ({}) : SID 0x{:04x} ({}) / Type 0x{:02x}{} / CA {}",
                        m + 1,
                        if service.name.is_empty() {
                            "—"
                        } else {
                            &service.name
                        },
                        service.id,
                        service.id,
                        service.kind,
                        match kind_name(service.kind) {
                            Some(named) => format!("（{named}）"),
                            None => String::new(),
                        },
                        u8::from(service.scrambled),
                    );
                    if service.lines.is_empty() {
                        Node::leaf(said)
                    } else {
                        Node::branch(
                            said,
                            false,
                            service.lines.iter().cloned().map(Node::leaf).collect(),
                        )
                    }
                })
                .collect();
            Node::branch(title, is_own, lines)
        })
        .collect();
    Node::branch("ネットワークサービス (SDT)", false, children)
}
