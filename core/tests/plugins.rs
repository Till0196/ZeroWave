// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 動いている最中にプラグインを足せること（`Loaded::plus`）。素材は `ZW_PLUGIN_WASM`。

#![cfg(feature = "plugins")]

use std::path::Path;

use zwcore::plugins::Loaded;

fn wasm() -> Option<String> {
    let path = std::env::var("ZW_PLUGIN_WASM").ok()?;
    if !Path::new(&path).is_file() {
        eprintln!("ZW_PLUGIN_WASM の {path} が無いので飛ばす");
        return None;
    }
    Some(path)
}

/// 一つ足すと載り、二度目は差し替えになる（増えない）。
#[test]
fn a_plugin_is_added_while_running_and_replaced_the_second_time() {
    let Some(path) = wasm() else {
        eprintln!("ZW_PLUGIN_WASM が無いので飛ばす（ビルドした .wasm のパスを入れる）");
        return;
    };
    let folder = std::env::temp_dir().join(format!("zerowave-plus-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&folder);
    let loaded = Loaded::load(&folder, &mut |line| eprintln!("{line}"));
    assert!(loaded.is_empty(), "最初は何も読み込んでいないはず");

    let now = loaded
        .plus(Path::new(&path), &mut |line| eprintln!("{line}"))
        .expect("一つ入る");
    assert_eq!(now.all().len(), 1, "足した一つ");
    let name = now.all()[0].name().to_owned();
    assert!(!name.is_empty(), "名前がある");

    // 二度目は差し替え（同じ名前の物が二つ載ると、同じバイト列が二度通る）。
    let again = now
        .plus(Path::new(&path), &mut |_| {})
        .expect("差し替えられる");
    assert_eq!(again.all().len(), 1, "増えない");

    if again.all()[0].has("stream") {
        assert!(!again.stream_chain().is_empty(), "チェーンに入る");
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn something_that_is_not_a_plugin_does_not_go_in() {
    let folder = std::env::temp_dir().join(format!("zerowave-plus-bad-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&folder);
    let not_wasm = folder.join("notes.txt");
    let _ = std::fs::write(&not_wasm, "これは wasm ではありません");
    let loaded = Loaded::load(&folder, &mut |_| {});
    assert!(loaded.plus(&not_wasm, &mut |_| {}).is_err());
    let _ = std::fs::remove_dir_all(&folder);
}
