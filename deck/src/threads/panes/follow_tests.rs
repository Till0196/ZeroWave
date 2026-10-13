// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 保存で使っているプロファイルの名前を追うことのテスト。

use super::follow_active;
use crate::config::file::Profile;

fn named(names: &[&str]) -> Vec<Profile> {
    names
        .iter()
        .map(|name| Profile {
            name: (*name).to_owned(),
            ..Profile::default()
        })
        .collect()
}

#[test]
fn saving_keeps_the_profile_in_use() {
    let before = named(&["家", "寝室"]);
    assert_eq!(
        follow_active("家", &before, &named(&["家", "寝室", "新しい"])),
        None
    );
    assert_eq!(
        follow_active("", &before, &named(&["家"])),
        None,
        "all は all"
    );
    assert_eq!(
        follow_active("寝室", &before, &named(&["家", "寝室だけ"])),
        Some(String::from("寝室だけ")),
        "名前を変えたら追う"
    );
    assert_eq!(
        follow_active("寝室", &before, &named(&["家"])),
        Some(String::new()),
        "消したら all"
    );
}

#[test]
fn a_renamed_profile_is_found_where_it_was() {
    use super::renamed_profiles;
    let before = named(&["家", "寝室"]);
    assert_eq!(
        renamed_profiles(&before, &named(&["家", "二階"])),
        [(String::from("寝室"), String::from("二階"))]
    );
    assert!(renamed_profiles(&before, &named(&["家", "寝室"])).is_empty());
    assert!(
        renamed_profiles(&before, &named(&["家"])).is_empty(),
        "消したのは変更ではない"
    );
    assert!(
        renamed_profiles(&before, &named(&["寝室", "家"])).is_empty(),
        "並べ替えは変更ではない"
    );
}
