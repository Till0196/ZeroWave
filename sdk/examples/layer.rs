// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レイヤーのプラグインを本体なしで動かし、最後の画面を PNG に書く：
//!
//! ```sh
//! cargo run -p zwsdk --features host --example layer -- <プラグイン> <フレーム数> [出力.png]
//!     [--cfg 名前=値]... [--msg JSON]... [--clock live|<UNIX 秒>] [--bg RRGGBB]
//! ```
//!
//! フレームはマニフェストの `fps`（無ければ 20）で実時間に合わせて回す（通信するプラグインのため）。
//! `--cfg` はプラグインの設定（`net=true` など）、`--msg` は最初に送る `zwsdk::layer::Message` の JSON。
//! `--clock live` は放送中、`--clock <UNIX 秒>` はその時刻からの録画として、毎秒 `Clock` を送る。
//! `--bg` は PNG の背景の色（無ければ透明のまま）。プラグインのログと要求（`Event`）は標準エラーに出る。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use zwsdk::host::{Io, Level, Picture, Plugin};
use zwsdk::layer::{Config, Message};

const SIZE: (u32, u32) = (1920, 1080);

enum Clock {
    Live,
    Recorded(i64),
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut cfg = Vec::new();
    let mut messages = Vec::new();
    let mut clock = None;
    let mut background = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cfg" => {
                let pair = args.next().expect("--cfg 名前=値");
                let (name, value) = pair.split_once('=').expect("--cfg 名前=値");
                cfg.push((name.to_owned(), value.to_owned()));
            }
            "--msg" => {
                let json = args.next().expect("--msg JSON");
                let message: Message = serde_json::from_str(&json).expect("Message の JSON");
                messages.push(message);
            }
            "--clock" => {
                let at = args.next().expect("--clock live|UNIX 秒");
                clock = Some(if at == "live" {
                    Clock::Live
                } else {
                    Clock::Recorded(at.parse().expect("UNIX 秒"))
                });
            }
            "--bg" => {
                let hex = args.next().expect("--bg RRGGBB");
                let rgb =
                    u32::from_str_radix(hex.trim_start_matches('#'), 16).expect("--bg RRGGBB");
                background = Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
            }
            _ => positional.push(arg),
        }
    }
    let [path, frames, rest @ ..] = positional.as_slice() else {
        eprintln!(
            "layer <プラグイン> <フレーム数> [出力.png] [--cfg 名前=値]... [--msg JSON]... [--clock live|UNIX 秒]"
        );
        std::process::exit(2);
    };
    let frames: u32 = frames.parse().expect("フレーム数");
    let out = rest.first().map(PathBuf::from);

    let engine = zwsdk::host::engine().expect("エンジン");
    let plugin = Plugin::load(&engine, std::path::Path::new(path)).expect("プラグイン");
    let fps = plugin.layer_manifest().fps.unwrap_or(20).max(1);
    let mut io = Io::default();
    // 宣言の既定値（`net` など）は本体と同じく設定の一覧から入れる。
    for setting in &plugin.manifest().config {
        io.defaults
            .insert(setting.key.clone(), setting.default.clone());
    }
    for (name, value) in cfg {
        io.cfg.insert(name, value);
    }
    let faces = zwsdk::fonts::Faces::find();
    let fonts = zwsdk::draw::font_context(&faces);
    if let Some(sans) = faces.sans {
        io.fonts.insert(String::from("sans"), sans);
    }
    let logger: zwsdk::host::Logger = Box::new(|level, text| {
        let mark = if level == Level::Warning { "!" } else { " " };
        eprintln!("{mark} {text}");
    });
    let config = Config {
        nvram: Vec::new(),
        size: SIZE,
    };
    let mut layer = plugin.layer(logger, io, fonts, &config).expect("レイヤー");
    layer
        .message(
            &Message::Area {
                width: SIZE.0,
                height: SIZE.1,
            },
            &[],
        )
        .expect("Area");
    for message in &messages {
        layer.message(message, &[]).expect("Message");
    }

    let began = Instant::now();
    let unix_now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    };
    let mut sent_clock = None;
    let mut last = None;
    // 絵の並び（GPU で重ねる形）で受け取ったフレームの数。
    let mut as_sprites = 0u32;
    // プラグインの時間（tick・frame・events）と、canvas のレンダリングを含む frame の時間。
    let mut busy = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    for n in 0..frames {
        let second = began.elapsed().as_secs();
        if let Some(clock) = &clock
            && sent_clock != Some(second)
        {
            sent_clock = Some(second);
            let (unix, recorded) = match clock {
                Clock::Live => (unix_now(), false),
                Clock::Recorded(at) => (at + second as i64, true),
            };
            layer
                .message(&Message::Clock { unix, recorded }, &[])
                .expect("Clock");
        }
        let started = Instant::now();
        layer.tick().expect("tick");
        // 本体と同じく、canvas を貼るだけの画面は絵の並びで受け取る。最後のフレームは PNG に書くので RGBA で。
        let sprites = n + 1 < frames;
        match layer.frame(SIZE, sprites).expect("frame") {
            Some(Some((frame, Picture::Rgba(rgba)))) => {
                last = Some((frame.width, frame.height, rgba))
            }
            Some(Some((_, Picture::Sprites(_)))) => as_sprites += 1,
            _ => {}
        }
        let (events, _) = layer.events().expect("events");
        let took = started.elapsed();
        busy += took;
        slowest = slowest.max(took);
        for event in events {
            eprintln!("> {event:?}");
        }
        let due = began + Duration::from_secs_f64(f64::from(n + 1) / f64::from(fps));
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    eprintln!(
        "{frames} フレーム（{fps} fps）: 平均 {:.1} ms、最長 {:.1} ms、絵の並び {as_sprites} 回",
        busy.as_secs_f64() * 1000.0 / f64::from(frames.max(1)),
        slowest.as_secs_f64() * 1000.0
    );
    if let (Some(out), Some((width, height, mut rgba))) = (out, last) {
        if let Some(bg) = background {
            // 乗算済みでない RGBA を背景の上に重ねる。
            for px in rgba.as_chunks_mut::<4>().0 {
                let alpha = u32::from(px[3]);
                for (c, b) in px.iter_mut().zip(bg) {
                    *c = ((u32::from(*c) * alpha + u32::from(b) * (255 - alpha)) / 255) as u8;
                }
                px[3] = 255;
            }
        }
        std::fs::write(&out, png(width, height, &rgba)).expect("PNG を書く");
        eprintln!("{} に書いた（{width}×{height}）", out.display());
    }
}

/// 圧縮しない PNG（zlib の stored ブロック）。
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(0xffff).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}
