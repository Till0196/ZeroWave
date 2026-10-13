// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `--sound-check`：同じ録画の音声を ZeroWave と OS のデコーダーでデコードして突き合わせる。

use std::io::Read;

use mediadecoder::audio::Sound;
use zwcore::decode::sound::{SoundPick, SoundPlan, set_sound_plan};
use zwcore::engine::{Engine, Out};
use zwcore::filters::Container;

const SETTLE: usize = 4 * 1024;

/// ずれを探す幅（サンプル）。AAC のエンコーダー遅延は 1024 か 2048 で、HE-AAC の倍まで。
const LAG_MOST: usize = 4096;

struct Heard {
    rate: u32,
    speakers: Vec<mediadecoder::audio::Speaker>,
    channels: Vec<Vec<f32>>,
    runs: usize,
    /// デコーダーが返したエラーの数と、最初の物。
    troubles: usize,
    first_trouble: Option<String>,
}

fn hear(path: &str, plan: SoundPlan, most: usize) -> Result<Heard, String> {
    set_sound_plan(plan);
    let container = Container::of_path(path).ok_or("コンテナが名前から決まりません")?;
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut engine = Engine::new(container, 48_000);
    let mut chunk = vec![0u8; 1 << 16];
    let mut wanted = false;
    let mut heard: Option<Heard> = None;
    let mut troubles = 0;
    let mut first_trouble = None;
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        engine.push(&chunk[..read]);
        if !wanted
            && let Some(number) = engine
                .programs()
                .values()
                .find(|program| {
                    program
                        .streams
                        .iter()
                        .any(|stream| matches!(stream.kind, zwcore::filters::Kind::Audio(_)))
                })
                .map(|program| program.number)
        {
            engine.want(number);
            wanted = true;
        }
        while let Some(out) = engine.next() {
            match out {
                Out::Sound { sound, .. } => add(&mut heard, &sound),
                Out::Trouble { what, .. } => {
                    troubles += 1;
                    first_trouble.get_or_insert(what);
                }
                _ => {}
            }
        }
        if heard.as_ref().is_some_and(|heard| heard.runs >= most) {
            break;
        }
    }
    let mut heard = heard.ok_or_else(|| String::from("音が出ません"))?;
    heard.troubles = troubles;
    heard.first_trouble = first_trouble;
    Ok(heard)
}

fn add(heard: &mut Option<Heard>, sound: &Sound) {
    let heard = heard.get_or_insert_with(|| Heard {
        rate: sound.rate,
        speakers: sound.speakers.clone(),
        channels: vec![Vec::new(); sound.channels.len()],
        runs: 0,
        troubles: 0,
        first_trouble: None,
    });
    if heard.channels.len() != sound.channels.len() {
        return;
    }
    for (into, from) in heard.channels.iter_mut().zip(&sound.channels) {
        into.extend_from_slice(from);
    }
    heard.runs += 1;
}

const LAG_WINDOW: usize = 48_000;

fn error_at(a: &[f32], b: &[f32], lag: usize) -> f64 {
    let length = a.len().min(b.len().saturating_sub(lag)).min(LAG_WINDOW);
    if length == 0 {
        return f64::INFINITY;
    }
    let sum: f64 = a[..length]
        .iter()
        .zip(&b[lag..lag + length])
        .map(|(x, y)| f64::from(x - y).powi(2))
        .sum();
    sum / length as f64
}

fn snr(a: &[f32], b: &[f32], lag_a: usize, lag_b: usize) -> f64 {
    let length = a
        .len()
        .saturating_sub(lag_a)
        .min(b.len().saturating_sub(lag_b));
    let (a, b) = (&a[lag_a..lag_a + length], &b[lag_b..lag_b + length]);
    let signal: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum();
    let noise: f64 = a.iter().zip(b).map(|(x, y)| f64::from(x - y).powi(2)).sum();
    if noise == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (signal / noise).log10()
}

pub fn sound_check(path: &str, most: usize) -> bool {
    let own = match hear(path, SoundPlan::default(), most) {
        Ok(heard) => heard,
        Err(why) => {
            println!("{path}: ZeroWave で音が取れません（{why}）");
            return false;
        }
    };
    let os = match hear(
        path,
        SoundPlan {
            aac: vec![SoundPick::System(None)],
            mpeg: vec![SoundPick::System(None)],
        },
        most,
    ) {
        Ok(heard) => heard,
        Err(why) => {
            println!("{path}: OS のデコーダーで音声をデコードできません（{why}）");
            return false;
        }
    };
    println!(
        "ZeroWave  {} Hz {} ch {:?}  {} 回 {} サンプル",
        own.rate,
        own.channels.len(),
        own.speakers,
        own.runs,
        own.channels.first().map_or(0, Vec::len)
    );
    println!(
        "OS        {} Hz {} ch {:?}  {} 回 {} サンプル",
        os.rate,
        os.channels.len(),
        os.speakers,
        os.runs,
        os.channels.first().map_or(0, Vec::len)
    );
    for (who, heard) in [("ZeroWave", &own), ("OS", &os)] {
        if let Some(first) = &heard.first_trouble {
            println!("{who} のエラー {} 回（最初は {first}）", heard.troubles);
        }
    }
    if own.rate != os.rate || own.channels.len() != os.channels.len() {
        println!("形式が違います");
        return false;
    }
    let (a, b) = (&own.channels[0], &os.channels[0]);
    let mut best = (0usize, 0usize, f64::INFINITY);
    for lag in 0..=LAG_MOST {
        let ahead = error_at(b, a, lag);
        if ahead < best.2 {
            best = (lag, 0, ahead);
        }
        let behind = error_at(a, b, lag);
        if behind < best.2 {
            best = (0, lag, behind);
        }
    }
    let (lag_own, lag_os, _) = best;
    println!("ずれ  ZeroWave を {lag_own}、OS を {lag_os} サンプル進めて比べる");
    // 立ち上がりは除く（最初のフレームの扱いはデコーダーごとに違う）。
    let mut worst = f64::INFINITY;
    for (index, (a, b)) in own.channels.iter().zip(&os.channels).enumerate() {
        let whole = snr(a, b, lag_own, lag_os);
        let settled = snr(a, b, lag_own + SETTLE, lag_os + SETTLE);
        worst = worst.min(settled);
        println!(
            "  {index} {:?}  全体 {whole:.1} dB、先頭の {SETTLE} サンプルを除いて {settled:.1} dB",
            own.speakers.get(index)
        );
    }
    let (a, b) = (&own.channels[0], &os.channels[0]);
    let length = a
        .len()
        .saturating_sub(lag_own)
        .min(b.len().saturating_sub(lag_os));
    let mut bad = Vec::new();
    let blocks = length / 1024;
    for block in 0..blocks {
        let at = block * 1024;
        let x = &a[lag_own + at..lag_own + at + 1024];
        let y = &b[lag_os + at..lag_os + at + 1024];
        let signal: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
        let noise: f64 = x.iter().zip(y).map(|(p, q)| f64::from(p - q).powi(2)).sum();
        if noise > 0.0 && 10.0 * (signal / noise).log10() < 60.0 {
            bad.push((
                block,
                10.0 * (signal / noise.max(1e-30)).log10(),
                signal / 1024.0,
            ));
        }
    }
    println!("外れた区間 {} / {blocks}", bad.len());
    for (block, decibels, power) in bad.iter().take(12) {
        println!("  #{block}  {decibels:.1} dB（信号の平均 {power:.2e}）");
    }
    let agreed = worst >= 60.0;
    println!(
        "{}",
        if agreed {
            "合っています"
        } else {
            "合っていません"
        }
    );
    agreed
}
