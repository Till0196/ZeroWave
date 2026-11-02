// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! JavaScript のプラグイン：`ts/zwjs.wasm` に `zw-stamp` と同じ形でスクリプトとマニフェストを足し、
//! 本体として読み込んで描かせる。

#![cfg(feature = "host")]

use zwsdk::abi;
use zwsdk::host::{Io, Picture, Plugin};

fn leb(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn section(name: &str, content: &[u8], out: &mut Vec<u8>) {
    let mut body = Vec::new();
    leb(name.len(), &mut body);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(content);
    out.push(0);
    leb(body.len(), out);
    out.extend_from_slice(&body);
}

const SCRIPT: &str = r#"
let frames = 0;
globalThis.__zwNew = (config) => { globalThis.size = JSON.parse(config).size; };
globalThis.__zwMessage = () => {};
globalThis.__zwTick = () => {};
globalThis.__zwEvents = () => "[]";
globalThis.__zwFrame = () => {
  frames += 1;
  __zw.fillStyle(1, "rgb(255, 0, 0)");
  __zw.fillRect(1, 0, 0, size[0], size[1]);
  return true;
};
"#;

#[test]
fn a_script_stamped_onto_the_runtime_draws() {
    let runtime = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/ts/zwjs.wasm"))
        .expect("ts/zwjs.wasm（javy/build.sh で作る）");
    let mut wasm = runtime;
    let manifest = format!(
        r#"{{"abi":{},"name":"script-test","version":"0","hooks":["layer"],"layer":{{"above":"captions"}}}}"#,
        abi::VERSION
    );
    section(abi::MANIFEST, manifest.as_bytes(), &mut wasm);
    section(abi::SCRIPT, SCRIPT.as_bytes(), &mut wasm);

    let engine = zwsdk::host::engine().expect("engine");
    let place = std::env::temp_dir().join("zw-script-test.wasm");
    let plugin = Plugin::from_bytes(&engine, &wasm, &place).expect("読み込める");
    assert_eq!(plugin.name(), "script-test");
    let logged = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = std::sync::Arc::clone(&logged);
    let config = zwsdk::layer::Config {
        size: (64, 36),
        ..Default::default()
    };
    let mut layer = plugin
        .layer(
            Box::new(move |_, text| seen.lock().unwrap().push(text.to_owned())),
            Io::default(),
            zwsdk::draw::FontContext::default(),
            &config,
        )
        .expect("起動する");
    let painted = layer.frame((64, 36), false).expect("描ける");
    let Some(Some((frame, Picture::Rgba(rgba)))) = painted else {
        panic!("描いていない: {:?}", logged.lock().unwrap());
    };
    assert_eq!((frame.width, frame.height), (64, 36));
    assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
}
