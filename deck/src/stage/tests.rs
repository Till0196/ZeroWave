// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

#[test]
fn a_later_source_of_the_profile_is_kept() {
    let file = crate::config::File::parse(
        r#"
[[source]]
kind = "mirakurun"
label = "home"
url = "http://192.0.2.1:40772"

[[source]]
kind = "mirakurun"
label = "sub"
url = "http://192.0.2.2:40772"

[[source]]
kind = "mirakurun"
label = "other"
url = "http://192.0.2.3:40772"

[[profile]]
name = "two"
sources = ["home", "sub"]
"#,
    )
    .expect("設定");
    assert!(super::uses_source(
        &file,
        "two",
        Some("http://192.0.2.1:40772")
    ));
    assert!(super::uses_source(
        &file,
        "two",
        Some("http://192.0.2.2:40772")
    ));
    assert!(!super::uses_source(
        &file,
        "two",
        Some("http://192.0.2.3:40772")
    ));
    assert!(!super::uses_source(&file, "two", None));
}

/// 再生スレッドの状態を別のスレッドへ移せること（でないとメニューを開いた間に再生が止まる）。
#[test]
fn the_stage_can_leave_the_event_loop_thread() {
    fn is_send<T: Send>() {}
    is_send::<super::Stage>();
    is_send::<super::Boot>();
    is_send::<crate::threads::link::ToStage>();
    is_send::<crate::threads::link::FromStage>();
}

#[test]
fn a_repeated_toast_counts_up() {
    let toast = |shown: &str| super::Toast {
        said: String::from("A へ選局できない: 空きが無い"),
        shown: crate::draw::scene::Toasted {
            title: shown.to_owned(),
            ..Default::default()
        },
        at: std::time::Instant::now(),
        serial: 1,
        hold: super::TOAST_FOR,
    };
    let said = "A へ選局できない: 空きが無い";
    assert_eq!(super::repeat_of(&toast(said), said), 2);
    assert_eq!(
        super::repeat_of(&toast("A へ選局できない: 空きが無い（2 回目）"), said),
        3
    );
    assert_eq!(super::repeat_of(&toast(said), "別の一文"), 1);
}

#[test]
fn a_toast_splits_into_title_and_details() {
    let toast = super::Toast::of("見出し。\n一行目\n\n二行目", 1, 1);
    assert_eq!(toast.shown.title, "見出し。");
    assert_eq!(
        toast.shown.details,
        ["一行目", "二行目"],
        "空の行は出さない"
    );
    assert_eq!(toast.hold, super::TOAST_FOR + 2.0 * super::TOAST_PER_LINE);
    let again = super::Toast::of("見出し。\n別の詳細", 2, 2);
    assert_eq!(again.shown.title, "見出し。（2 回目）");
    assert_eq!(again.shown.details, ["別の詳細"]);
    assert_eq!(super::repeat_of(&again, "見出し。\nまた別の詳細"), 3);
    assert_eq!(
        super::said_with("見出し。", &""),
        "見出し。",
        "理由が空なら行を足さない"
    );
    assert_eq!(super::id_text(0x0400), "0x0400 (1024)");
}

#[test]
fn a_named_service_needs_every_number_said() {
    use super::{Named, Pick};
    let channel = |network: u16, transport: u16, service: u16| zwcore::input::source::Channel {
        name: String::new(),
        key: Some(zwcore::guide::Key { network, service }),
        transport: Some(transport),
        order: None,
        how: String::new(),
        container: zwcore::filters::Container::Ts,
    };
    let channels = [
        channel(4, 0x4010, 101),
        channel(4, 0x4031, 231),
        channel(6, 0x6020, 231),
    ];
    let found = |named: Named| {
        Pick::Named(named)
            .from(&channels)
            .map(|channel| channel.transport)
    };
    let sid = Named {
        service: Some(231),
        ..Named::default()
    };
    assert_eq!(found(sid), Some(Some(0x4031)));
    assert_eq!(
        found(Named {
            network: Some(6),
            ..sid
        }),
        Some(Some(0x6020))
    );
    assert_eq!(
        found(Named {
            transport: Some(0x4010),
            ..Named::default()
        }),
        Some(Some(0x4010))
    );
    assert_eq!(
        found(Named {
            transport: Some(0x4010),
            ..sid
        }),
        None
    );
}
