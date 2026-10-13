// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルスキャンのテスト。

use super::walk::{heading, pin_stream, satellite_places, service_lines, split_found};

#[test]
fn a_satellite_transponder_is_walked_for_its_other_streams() {
    let target = Target {
        name: String::from("ISDB-S BS15"),
        tlv: false,
        words: vec![
            (String::from("DELIVERY_SYSTEM"), String::from("ISDBS")),
            (String::from("FREQUENCY"), String::from("1318460")),
        ],
    };
    let found = vec![Found {
        network: 4,
        transport: 0x40f1,
        ..Found::default()
    }];
    let sampled = Sampled {
        listed: vec![
            (4, 0x4010),
            (4, 0x40f1),
            (4, 0x40f2),
            (4, 0x48f3),
            (6, 0x60f1),
        ],
        ..Sampled::default()
    };
    let mut widened = std::collections::BTreeSet::new();
    let more = satellite_places(&target, &found, &sampled, &[], &mut widened);
    let named: Vec<&str> = more
        .iter()
        .map(|more| more.words.last().unwrap().1.as_str())
        .collect();
    assert_eq!(
        named,
        ["16626", "18675"],
        "0x40f2 と 0x48f3 だけ（他のトランスポンダと CS は見ない）"
    );
    assert_eq!(
        pin_stream(&target, &found, &sampled).last().unwrap(),
        &(String::from("STREAM_ID"), String::from("16625"))
    );
    assert!(satellite_places(&target, &found, &sampled, &[], &mut widened).is_empty());
    assert!(satellite_places(&more[0], &found, &sampled, &[], &mut Default::default()).is_empty());
}

#[test]
fn a_driver_that_names_places_walks_every_place() {
    let target = Target {
        name: String::from("ISDB-S BS15"),
        tlv: false,
        words: vec![
            (String::from("DELIVERY_SYSTEM"), String::from("ISDBS")),
            (String::from("FREQUENCY"), String::from("1318460")),
        ],
    };
    let found = vec![Found {
        network: 4,
        transport: 0x40f1,
        ..Found::default()
    }];
    let sampled = Sampled {
        listed: vec![(4, 0x40f1), (4, 0x40f2)],
        relative_streams: Some(8),
        ..Sampled::default()
    };
    let more = satellite_places(&target, &found, &sampled, &[], &mut Default::default());
    assert_eq!(more.len(), 8);
    assert_eq!(pin_stream(&target, &found, &sampled), target.words);
}
use super::*;

#[test]
fn the_account_reads_like_bondriver_coax() {
    let sampled = Sampled {
        cnr: Some(27.5),
        mbps: Some(17.27),
        ..Sampled::default()
    };
    assert_eq!(
        heading("UHF 27ch", "受信", &sampled),
        "UHF 27ch: 受信　CNR 27.5 dB　17.3 Mbps"
    );
    let found = Found {
        told: vec![
            Told {
                id: 1024,
                name: String::from("ゼロウェーブテレビ１"),
                provider: String::from("ゼロウェーブ"),
                kind: 0x01,
                scrambled: false,
            },
            Told {
                id: 101,
                name: String::new(),
                kind: 0xc0,
                scrambled: true,
                ..Told::default()
            },
        ],
        ..Found::default()
    };
    assert_eq!(
        service_lines(&found),
        [
            "        SID 0x0400 (1024) ゼロウェーブテレビ１　ゼロウェーブ　(TV)",
            "        SID 0x0065 (101) ???　(データ/ワンセグ, 有料)",
        ]
    );
}

#[test]
fn a_split_frame_is_read_from_its_header() {
    let sampled = Sampled {
        found: Vec::new(),
        relatives: vec![
            isdb::tsmf::Relative {
                number: 1,
                stream_id: 0x4031,
                original_network_id: 4,
                tlv: true,
                slots: 26,
            },
            isdb::tsmf::Relative {
                number: 2,
                stream_id: 0x4032,
                original_network_id: 4,
                tlv: true,
                slots: 26,
            },
        ],
        group: Some(isdb::tsmf::Group {
            id: 8,
            carriers: 2,
            sequence: 1,
        }),
        ..Sampled::default()
    };
    let found = split_found(&sampled);
    assert_eq!(found.len(), 2, "TSMF に含まれる 2 つの TS とも書く");
    assert_eq!(found[0].network, 4);
    assert_eq!(found[0].transport, 0x4031);
    assert_eq!(found[0].slot, Some((true, 1)), "TLV の相対TS 1");
    assert!(found[0].services.is_empty(), "サービス名は結合してから");
}

#[test]
fn one_carriers_frame_is_written_from_its_header_with_names_read() {
    let sampled = Sampled {
        found: vec![Found {
            network: 4,
            transport: 0x4031,
            network_name: String::from("ゼロウェーブケーブル"),
            slot: Some((false, 1)),
            ..Found::default()
        }],
        relatives: vec![
            isdb::tsmf::Relative {
                number: 1,
                stream_id: 0x4031,
                original_network_id: 4,
                tlv: false,
                slots: 26,
            },
            isdb::tsmf::Relative {
                number: 10,
                stream_id: 45168,
                original_network_id: 11,
                tlv: true,
                slots: 26,
            },
        ],
        group: Some(isdb::tsmf::Group {
            id: 28,
            carriers: 1,
            sequence: 1,
        }),
        ..Sampled::default()
    };
    let found = split_found(&sampled);
    assert_eq!(found.len(), 2, "TSMF に含まれる 2 つの TS とも");
    assert_eq!(
        found[0].network_name, "ゼロウェーブケーブル",
        "読めた名前は添える"
    );
    assert_eq!(found[0].slot, Some((false, 1)));
    assert_eq!((found[1].network, found[1].transport), (11, 45168));
    assert_eq!(
        found[1].slot,
        Some((true, 10)),
        "TLV の相対TS 10（ヘッダのフラグ）"
    );
}

#[test]
fn a_carrier_without_a_frame_is_read_from_its_tables() {
    let sampled = Sampled {
        found: vec![Found {
            network: 0x7fe6,
            transport: 0x7fe6,
            ..Found::default()
        }],
        ..Sampled::default()
    };
    let found = split_found(&sampled);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].transport, 0x7fe6);
}

#[test]
fn a_lock_is_judged_by_the_tuner_not_by_packets() {
    let quiet = coax::daemon::Seen::default();
    assert_eq!(quiet.packets, 0, "開く前はパケットが届いていない");
    assert_eq!(quiet.locked, None, "ドライバーがまだ報告していない");
    let locked = coax::daemon::Seen {
        locked: Some(true),
        ..coax::daemon::Seen::default()
    };
    assert!(locked.locked == Some(true) && locked.packets == 0);
    let waiting = coax::daemon::Seen {
        locked: Some(false),
        ..coax::daemon::Seen::default()
    };
    assert_ne!(waiting.locked, Some(true));
}

#[test]
fn a_line_that_cannot_be_scanned_says_why_in_the_settings_words() {
    let refused = |plan: &str, bands: &[&str]| {
        targets(&Spec {
            plan: plan.to_owned(),
            bands: bands.iter().map(|one| (*one).to_owned()).collect(),
            ..Spec::default()
        })
        .expect_err("断る")
    };
    assert_eq!(refused("dvbt2", &[]), "[[line]] の plan「dvbt2」は不明です");
    assert_eq!(
        refused("isdbt", &["shf"]),
        "[[line]] の bands「shf」は不明です"
    );
    assert_eq!(
        refused("isdbs3", &["cs"]),
        "ISDB-S3 は 110°CS の右旋をスキャンできません（bands の「cs」）"
    );
}
