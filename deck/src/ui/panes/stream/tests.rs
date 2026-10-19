// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::delivery::kind_name;
use super::*;

#[test]
fn a_programme_identifier_says_which_service_it_belongs_to() {
    let carried = [(0x0100, 1024, "0x02")];
    assert_eq!(
        what_carries(Container::Ts, 0x0100, &carried),
        "[1024] MPEG-2 Video"
    );
    let carried = [(0x0100, 1024, "hev1")];
    assert_eq!(
        what_carries(Container::Tlv, 0x0100, &carried),
        "[1024] HEVC"
    );
}

#[test]
fn a_fixed_identifier_is_named_without_the_programmes() {
    assert_eq!(what_carries(Container::Ts, 0x0012, &[]), "H-EIT");
    assert_eq!(what_carries(Container::Ts, 0x0010, &[]), "NIT");
    assert_eq!(what_carries(Container::Tlv, 0x8000, &[]), "MH-EIT");
    assert_eq!(what_carries(Container::Tlv, 0x0000, &[]), "PA (PLT/MPT)");
}

/// どちらでもない識別子は何も返さないこと（推測で書かない）。
#[test]
fn an_identifier_nothing_named_says_nothing() {
    assert_eq!(what_carries(Container::Ts, 0x0abc, &[]), "");
}

#[test]
fn a_service_kind_is_named_where_the_standard_names_it() {
    assert_eq!(kind_name(0x01), Some("デジタルTV"));
    assert_eq!(kind_name(0xad), Some("超高精細度4K"));
    assert_eq!(kind_name(0x5a), None);
}

fn an_info() -> StreamInfo {
    StreamInfo {
        identity: Some(Identity {
            transport: 0x7fef,
            network: 0x7fef,
        }),
        networks: vec![NetworkInfo {
            network: 0x7fef,
            name: String::from("ゼロウェーブ総合・東京"),
            own: true,
            multiplexes: vec![zwcore::si::Multiplex {
                transport: 0x7fef,
                network: 0x7fef,
                name: String::from("ゼロウェーブ総合"),
                remote_key: 1,
                services: vec![(1024, 0x01)],
                carried: String::from("地上 エリア 1445"),
                partial: Vec::new(),
                ..zwcore::si::Multiplex::default()
            }],
        }],
        counts: zwcore::filters::Counts {
            packets: 1_843_217,
            dropped: 0,
            errors: 2,
            scrambled: 0,
        },
        emm: vec![(0x0005, 0x0082)],
        pids: vec![Row {
            pid: 0x0111,
            counted: Counted {
                arrived: 1_203_344,
                passed: 1_203_344,
                ..Counted::default()
            },
            what: String::from("[1024] MPEG-2 Video"),
        }],
        ..StreamInfo::default()
    }
}

/// コピーは画面の文そのまま。違うのは深さの字下げだけで、折りたたんだノードも中身ごと。
#[test]
fn what_is_written_out_is_what_the_tab_shows() {
    let info = an_info();
    let out = written(&info);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "ストリーム: TSID 0x7fef (32751)");
    assert_eq!(
        lines[1],
        "ネットワーク: NID 0x7fef (32751) ゼロウェーブ総合・東京"
    );
    assert_eq!(lines[2], "サービス");
    assert_eq!(lines[3], "  —");
    assert_eq!(lines[4], "受信");
    assert_eq!(lines[5], "  パケット: 1843217");
    assert_eq!(lines[6], "  ドロップ（連続性の飛び）: 0");
    assert_eq!(lines[7], "  エラー（transport_error_indicator）: 2");
    assert_eq!(lines[8], "  読めないパケット: 0");
    assert_eq!(lines[9], "限定受信 (CAT)");
    assert_eq!(
        lines[10],
        "  EMM : CA system ID 0x0005 (5) ARIB STD-B25 / PID 0x0082 (130)"
    );
    assert_eq!(lines[11], "ネットワークTS (NIT)");
    assert_eq!(lines[12], "  ゼロウェーブ総合・東京 : NID 0x7fef (32751)");
    assert_eq!(
        lines[13],
        "    TS1 : TSID 0x7fef (32751) ゼロウェーブ総合 / リモコン 1"
    );
    assert!(
        lines.contains(&"      サービス1 : SID 0x0400 (1024) / Type 0x01（デジタルTV）"),
        "{out}"
    );
    assert!(!out.contains("CAT: "));
    assert!(!out.contains("NIT: "));
    assert!(!out.contains("受信: パケット"));
    assert!(lines.contains(&"ネットワークサービス (SDT)"));
    assert!(lines.contains(&"事業者 (BIT)"));
    assert!(lines.contains(&"地上分配システム") || lines.contains(&"分配システム"));
    let table = out
        .find("\n\nPID\t入力\t出力\tドロップ\tエラー\t内容\n")
        .expect("テーブル");
    assert!(out[table..].contains("0x0111\t1203344\t1203344\t0\t0\t[1024] MPEG-2 Video\n"));
}

#[test]
fn the_copy_and_the_tab_come_from_the_same_tree() {
    fn flatten(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Node::Branch {
                    label, children, ..
                } => {
                    out.push(label.clone());
                    flatten(children, out);
                }
                Node::Leaf(text) => out.push(text.clone()),
            }
        }
    }
    let info = an_info();
    let mut shown = Vec::new();
    flatten(&information::tree(&info), &mut shown);
    let out = written(&info);
    let copied: Vec<&str> = out
        .lines()
        .skip(2)
        .take_while(|line| !line.is_empty())
        .map(str::trim_start)
        .collect();
    assert_eq!(copied, shown);
}

#[test]
fn an_emergency_warning_is_listed_under_the_own_network() {
    let mut info = an_info();
    assert!(!written(&info).contains("緊急警報放送"));
    info.emergency = vec![isdb::nitdescriptors::EmergencyService {
        service_id: 0x0400,
        started: true,
        signal_level: 0,
        area_codes: vec![0x5a5],
    }];
    let out = written(&info);
    assert!(
        out.contains(
            "\n  ゼロウェーブ総合・東京 : NID 0x7fef (32751)\n    緊急警報放送 : SID 0x0400 (1024) / 第1種開始信号 / 地域 0x5a5 (1445)\n"
        ),
        "{out}"
    );
}

#[test]
fn the_network_is_named_only_by_its_table() {
    let identity = Identity {
        transport: 0x4010,
        network: 0x0004,
    };
    let mut info = StreamInfo {
        identity: Some(identity),
        ..StreamInfo::default()
    };
    assert_eq!(network_name(&info), None, "NIT が届くまでは名前を出さない");
    assert_eq!(information::head(&info)[1].1, "NID 0x0004 (4)");
    info.networks.push(NetworkInfo {
        network: 0x0004,
        name: String::from("ＢＳデジタル放送"),
        ..NetworkInfo::default()
    });
    assert_eq!(network_name(&info).as_deref(), Some("ＢＳデジタル放送"));
}

#[test]
fn an_ntp_time_is_written_in_japan_time() {
    assert_eq!(
        information::ntp_jst(0xee7c_1b14_0000_0000),
        "2026-10-16 12:40:04 JST"
    );
    // 秒の最上位ビットが 0 なら 2036 年 2 月 7 日 6:28:16 UTC から数える。
    assert_eq!(information::ntp_jst(1 << 32), "2036-02-07 15:28:17 JST");
}
