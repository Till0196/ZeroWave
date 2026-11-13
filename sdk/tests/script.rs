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

/// HTTP は応答の記録を、WebSocket は届いたメッセージを返し、送った物を覚える。
mod fake {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use zwsdk::net::conn::{Arrived, Open, Outgoing};
    use zwsdk::net::{Conn, Failure, Network, Reply, Session};

    pub type Seen = Arc<Mutex<Vec<String>>>;

    #[derive(Debug)]
    pub struct Net(pub Seen);

    impl Network for Net {
        fn has_ip(&self) -> bool {
            true
        }
        fn resolve(&self, _: &str) -> Option<String> {
            None
        }
        fn reach(&self, _: &str, _: u16, _: Duration, _: bool) -> Option<(String, u32)> {
            None
        }
        fn session(&self) -> Box<dyn Session> {
            Box::new(Net(Arc::clone(&self.0)))
        }
    }

    impl Session for Net {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &[(String, String)],
            _: Option<&[u8]>,
            _: bool,
        ) -> Result<Reply, Failure> {
            Err(Failure::Connect)
        }
        fn add_root_certificate(&self, _: &[u8]) -> bool {
            false
        }
        fn open(&self, open: Open) -> Box<dyn Conn> {
            let headers: Vec<String> = open
                .headers
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            self.0.lock().unwrap().push(format!(
                "open {} {} {} {}",
                open.method,
                open.url,
                headers.join(","),
                String::from_utf8_lossy(open.body.as_deref().unwrap_or(b"-"))
            ));
            let arrived = if open.protocols.is_some() {
                vec![
                    Arrived::Head {
                        status: 101,
                        headers: Vec::new(),
                    },
                    Arrived::Text(String::from("hello")),
                ]
            } else {
                vec![
                    Arrived::Head {
                        status: 200,
                        headers: vec![(String::from("content-type"), String::from("text/plain"))],
                    },
                    Arrived::Data(b"ab".to_vec()),
                    Arrived::Data(b"c".to_vec()),
                    Arrived::End {
                        code: 0,
                        reason: String::new(),
                    },
                ]
            };
            Box::new(Script(arrived.into(), Arc::clone(&self.0)))
        }
    }

    #[derive(Debug)]
    struct Script(std::collections::VecDeque<Arrived>, Seen);

    impl Conn for Script {
        fn next(&mut self) -> Option<Arrived> {
            self.0.pop_front()
        }
        fn send(&mut self, outgoing: Outgoing) -> bool {
            let said = match outgoing {
                Outgoing::Text(bytes) => format!("text {}", String::from_utf8_lossy(&bytes)),
                Outgoing::Binary(bytes) => format!("binary {bytes:?}"),
                Outgoing::Close(code, why) => {
                    format!("close {code} {}", String::from_utf8_lossy(&why))
                }
            };
            self.1.lock().unwrap().push(said);
            true
        }
    }
}

const NET_SCRIPT: &str = r#"
const said = [];
const conns = {};
globalThis.__zwNew = () => {
  conns.http = __zw.netOpen("POST", "http://example.jp/a", "x-a: 1\n", new Uint8Array([104, 105]));
  conns.ws = __zw.netWebSocket("ws://example.jp/s", "chat", "");
};
globalThis.__zwMessage = () => {};
globalThis.__zwTick = () => {
  for (const name of Object.keys(conns)) {
    const bytes = __zw.netPoll(conns[name]);
    if (!bytes) continue;
    let at = 0;
    while (at < bytes.length) {
      const len = (bytes[at + 1] << 24) | (bytes[at + 2] << 16) | (bytes[at + 3] << 8) | bytes[at + 4];
      const body = bytes.subarray(at + 5, at + 5 + len);
      said.push(`${name} ${bytes[at]} ${new TextDecoder().decode(body.subarray(bytes[at] === 1 ? 2 : 0))}`);
      at += 5 + len;
    }
    if (name === "ws") {
      __zw.netSend(conns.ws, 1, new TextEncoder().encode("hi"));
      __zw.netSend(conns.ws, 8, new Uint8Array([3, 232, 98]));
    }
    __zw.netClose(conns[name]);
    delete conns[name];
  }
};
globalThis.__zwFrame = () => false;
globalThis.__zwEvents = () => {
  const out = JSON.stringify(said.map((text) => ({ Log: text })));
  said.length = 0;
  return out;
};
"#;

#[test]
fn a_script_talks_through_the_host_network() {
    let runtime = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/ts/zwjs.wasm"))
        .expect("ts/zwjs.wasm（javy/build.sh で作る）");
    let mut wasm = runtime;
    let manifest = format!(
        r#"{{"abi":{},"name":"script-net","version":"0","hooks":["layer"],"layer":{{"above":"captions"}},"config":[{{"key":"net","kind":"bool","default":"true"}}]}}"#,
        abi::VERSION
    );
    section(abi::MANIFEST, manifest.as_bytes(), &mut wasm);
    section(abi::SCRIPT, NET_SCRIPT.as_bytes(), &mut wasm);

    let seen = fake::Seen::default();
    zwsdk::net::install(std::sync::Arc::new(fake::Net(std::sync::Arc::clone(&seen))));
    let engine = zwsdk::host::engine().expect("engine");
    let place = std::env::temp_dir().join("zw-script-net.wasm");
    let plugin = Plugin::from_bytes(&engine, &wasm, &place).expect("読み込める");
    let mut io = Io::default();
    io.defaults
        .insert(String::from("net"), String::from("true"));
    let mut layer = plugin
        .layer(
            Box::new(|_, _| {}),
            io,
            zwsdk::draw::FontContext::default(),
            &zwsdk::layer::Config::default(),
        )
        .expect("起動する");
    layer.tick().expect("tick");
    let (events, _) = layer.events().expect("events");
    let logs: Vec<String> = events
        .into_iter()
        .filter_map(|event| match event {
            zwsdk::layer::Event::Log(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        logs,
        [
            "http 1 content-type: text/plain",
            "http 2 ab",
            "http 2 c",
            "http 4 ",
            "ws 1 ",
            "ws 3 hello",
        ]
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "open POST http://example.jp/a x-a=1 hi",
            "open GET ws://example.jp/s  -",
            "text hi",
            "close 1000 b",
        ]
    );
}
