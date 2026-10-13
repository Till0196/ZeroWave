// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルスキャンが何を読めるか。

#![cfg(feature = "coaxd")]

use zwcore::scan::Reading;

/// 何も入れなければ、揃っていないと返す。
#[test]
fn nothing_read_is_not_settled() {
    let reading = Reading::new(false);
    assert!(!reading.settled());
    assert!(reading.found().is_empty());
    assert!(reading.relatives().is_empty());
    assert!(reading.group().is_none());
}
