// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! wasm の境界の名前（文字列）が一か所に揃っていること。`#[link]` はリテラルしか取れないので、guest 側と
//! `javy/src/lib.rs` の文字列を読んで `zwsdk::abi` と照らす。

use std::collections::BTreeSet;

use zwsdk::abi;

fn import_modules_in(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = source;
    while let Some(at) = rest.find("wasm_import_module = \"") {
        let after = &rest[at + "wasm_import_module = \"".len()..];
        let end = after.find('"').expect("閉じる引用符");
        found.insert(after[..end].to_string());
        rest = &after[end..];
    }
    found
}

fn known_imports() -> BTreeSet<String> {
    [
        abi::import::BUF,
        abi::import::OUTPUT,
        abi::import::LOG,
        abi::import::CLOCK,
        abi::import::CFG,
        abi::import::FONT,
        abi::import::NET,
        abi::import::PROC,
        abi::import::TUNER,
        abi::import::SOCK,
        abi::import::FILES,
        abi::import::DRAW,
        abi::import::LAYER,
        abi::import::UI,
        abi::import::CONTROL,
        abi::import::VIDEO,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[test]
fn the_guest_imports_only_named_namespaces() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src/plug");
    let mut used = BTreeSet::new();
    for file in ["abi.rs", "host.rs"] {
        let source = std::fs::read_to_string(format!("{root}/{file}")).expect(file);
        used.extend(import_modules_in(&source));
    }
    assert!(!used.is_empty());
    let known = known_imports();
    let unknown: Vec<_> = used.difference(&known).collect();
    assert!(unknown.is_empty(), "abi.rs に無い名前空間: {unknown:?}");
}

#[test]
fn javy_imports_only_named_namespaces() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/javy/src/lib.rs");
    let source = std::fs::read_to_string(path).expect("javy/src/lib.rs");
    let used = import_modules_in(&source);
    assert_eq!(
        used,
        [abi::import::DRAW, abi::import::LAYER, abi::import::BUF]
            .into_iter()
            .map(String::from)
            .collect()
    );
    for name in [
        abi::export::LAYER_NEW,
        abi::export::LAYER_MESSAGE,
        abi::export::LAYER_TICK,
        abi::export::LAYER_FRAME,
        abi::export::LAYER_EVENTS,
        abi::export::LAYER_DROP,
    ] {
        assert!(source.contains(&format!("fn {name}(")), "{name}");
    }
}

#[test]
fn the_stamp_writes_the_same_manifest_version() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ts/bin/zw-stamp.mjs");
    let source = std::fs::read_to_string(path).expect("zw-stamp.mjs");
    assert!(
        source.contains(&format!("abi: {}", abi::VERSION)),
        "zw-stamp.mjs の abi"
    );
    assert!(
        source.contains(&format!("\"{}\"", abi::MANIFEST)),
        "zw-stamp.mjs のセクション名"
    );
}

#[test]
fn export_stream_names_match() {
    // `export_stream!` はマクロなので、展開した文字列をここで照合する。
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/plug/mod.rs");
    let source = std::fs::read_to_string(path).expect("plug/mod.rs");
    for name in [
        abi::export::STREAM_NEW,
        abi::export::STREAM_START,
        abi::export::STREAM_INPUT,
        abi::export::STREAM_STOP,
        abi::export::STREAM_DROP,
    ] {
        assert!(source.contains(&format!("fn {name}(")), "{name}");
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/plug/abi.rs");
    let source = std::fs::read_to_string(path).expect("plug/abi.rs");
    for name in [abi::ALLOC, abi::FREE, abi::MANIFEST] {
        assert!(source.contains(&format!("fn {name}(")), "{name}");
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/plug/layer.rs");
    let source = std::fs::read_to_string(path).expect("plug/layer.rs");
    for name in [
        abi::export::LAYER_NEW,
        abi::export::LAYER_MESSAGE,
        abi::export::LAYER_TICK,
        abi::export::LAYER_FRAME,
        abi::export::LAYER_EVENTS,
        abi::export::LAYER_DROP,
    ] {
        assert!(source.contains(&format!("fn {name}(")), "{name}");
    }
}
