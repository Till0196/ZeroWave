// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The engine over transport streams ffmpeg made (`testdata/make.sh`).

use std::collections::VecDeque;

use zwcore::decode::Prefer;
use zwcore::engine::clock::HZ;
use zwcore::engine::decoding::Decoding;
use zwcore::engine::{Engine, Out};
use zwcore::filters::Container;

#[derive(Default)]
struct Played {
    pictures: Vec<(i64, u32, u32)>,
    sounds: Vec<(i64, usize, usize)>,
    trouble: Vec<String>,
}

mod common;
use common::testdata as read;

fn play(name: &str) -> Played {
    let bytes = read(name);
    let container = Container::of_path(name).expect("a name that says which");
    // The head goes through one engine to find what is there; the stream is then played
    // from the beginning through another.
    let mut looking = Engine::new(container, 48_000);
    looking.push(&bytes[..bytes.len().min(4 << 20)]);
    let mut engine = Engine::new(container, 48_000);
    if let Some(program) = looking.best() {
        engine.want(program);
    }
    let mut played = Played {
        pictures: Vec::new(),
        sounds: Vec::new(),
        trouble: Vec::new(),
    };
    let mut decoding = Decoding::new(Prefer::Software);
    for chunk in bytes.chunks(65_536) {
        engine.push(chunk);
        take(&mut engine, &mut decoding, &mut played);
    }
    finish(&mut decoding, &mut played);
    played
}

/// The same, handing the sound over coded.
fn pass(name: &str) -> Played {
    let bytes = read(name);
    let container = Container::of_path(name).expect("a name that says which");
    let mut looking = Engine::new(container, 48_000);
    looking.push(&bytes);
    let mut engine = Engine::new(container, 48_000);
    engine.set_passthrough(zwcore::sound::Passthrough::On);
    if let Some(program) = looking.best() {
        engine.want(program);
    }
    let mut played = Played {
        pictures: Vec::new(),
        sounds: Vec::new(),
        trouble: Vec::new(),
    };
    let mut decoding = Decoding::new(Prefer::Software);
    for chunk in bytes.chunks(65_536) {
        engine.push(chunk);
        take(&mut engine, &mut decoding, &mut played);
    }
    finish(&mut decoding, &mut played);
    played
}

fn take(engine: &mut Engine, decoding: &mut Decoding, played: &mut Played) {
    let mut outs = VecDeque::new();
    while let Some(out) = engine.next() {
        decoding.push(out, &mut outs);
    }
    keep(outs, played);
}

fn finish(decoding: &mut Decoding, played: &mut Played) {
    let mut outs = VecDeque::new();
    decoding.flush(&mut outs);
    keep(outs, played);
}

fn keep(outs: VecDeque<Out>, played: &mut Played) {
    for out in outs {
        match out {
            Out::Picture { at, picture } => {
                let about = picture.about();
                played
                    .pictures
                    .push((at, about.format.width, about.format.height));
            }
            Out::Sound { at, sound, .. } => {
                played.sounds.push((at, sound.channels.len(), sound.len()));
            }
            Out::Coded { at, frame } => {
                let header = mediadecoder::audio::adts::Header::read(&frame).expect("a header");
                assert_eq!(header.length, frame.len());
                let mut burst = Vec::new();
                let samples = zwcore::sound::iec61937::pack(&frame, &mut burst).expect("a burst");
                assert_eq!(burst.len(), samples * 4);
                played.sounds.push((at, header.blocks, frame.len()));
            }
            Out::Trouble { what, .. } => played.trouble.push(what),
            Out::Switched
            | Out::DataStreams(_)
            | Out::Data { .. }
            | Out::Item { .. }
            | Out::CaptionStreams(_)
            | Out::Caption { .. }
            | Out::ApplicationService(_) => {}
            Out::Access(_) => unreachable!("the decoding stage takes every access unit"),
        }
    }
}

#[test]
fn a_stream_of_mpeg2_gives_pictures_and_sound_that_belong_together() {
    let played = play("mpeg2-interlaced.ts");
    assert!(played.pictures.len() > 30, "{}", played.pictures.len());
    assert!(played.sounds.len() > 30, "{}", played.sounds.len());
    for (_, width, height) in &played.pictures {
        assert_eq!((*width, *height), (352, 240));
    }
    for (_, channels, samples) in &played.sounds {
        assert_eq!(*channels, 2);
        assert_eq!(*samples, 1024);
    }

    let times: Vec<i64> = played.pictures.iter().map(|(at, ..)| *at).collect();
    assert!(
        times.windows(2).all(|pair| pair[0] < pair[1]),
        "出す順になっていない: {times:?}"
    );

    // Sound and pictures share a clock, so they overlap within a few hundred milliseconds.
    let first_picture = times[0];
    let first_sound = played.sounds[0].0;
    let apart = (first_picture - first_sound).abs();
    assert!(apart < HZ, "映像と音声が {apart} 離れている");
}

#[test]
fn a_transport_stream_passes_through_frame_for_frame() {
    let (coded, decoded) = (pass("mpeg2-interlaced.ts"), play("mpeg2-interlaced.ts"));
    assert!(coded.trouble.is_empty(), "{:?}", coded.trouble);
    assert!(!coded.sounds.is_empty());
    assert_eq!(coded.sounds.len(), decoded.sounds.len());
    let coded_times: Vec<i64> = coded.sounds.iter().map(|(at, ..)| *at).collect();
    let decoded_times: Vec<i64> = decoded.sounds.iter().map(|(at, ..)| *at).collect();
    assert_eq!(coded_times, decoded_times);
    for (_, blocks, _) in &coded.sounds {
        assert_eq!(*blocks, 1);
    }
}

/// 音声を切り替えても、映像は始め直さない（素材は音声を三本持つ）。
#[test]
fn changing_the_sound_leaves_the_picture_alone() {
    let name = "three-sounds.ts";
    let bytes = read(name);
    let container = Container::of_path(name).expect("a name that says which");
    let mut looking = Engine::new(container, 48_000);
    looking.push(&bytes);
    let mut engine = Engine::new(container, 48_000);
    engine.want(looking.best().expect("the programme"));

    let mut units_before = 0u64;
    let mut switched_at = None;
    let mut switched_to = None;
    let mut units_after = 0u64;
    let mut restarted_after = 0u64;
    let mut sound_after_switch = 0u64;
    // 番組の音声は選んでいないものも全部デコードされて出てくる（即時の切り替えの前提）。
    let mut sounds_before = std::collections::BTreeSet::new();
    for (n, chunk) in bytes.chunks(4_096).enumerate() {
        engine.push(chunk);
        while let Some(out) = engine.next() {
            match out {
                Out::Access(unit) => {
                    if switched_at.is_some() {
                        units_after += 1;
                        if unit.first {
                            restarted_after += 1;
                        }
                    } else {
                        units_before += 1;
                    }
                }
                Out::Sound { stream, .. } => {
                    if switched_at.is_some() {
                        if Some(stream) == switched_to {
                            sound_after_switch += 1;
                        }
                    } else {
                        sounds_before.insert(stream);
                    }
                }
                _ => {}
            }
        }
        if switched_at.is_none() && units_before >= 20 {
            let (_, chosen) = engine.chosen();
            let next = engine.next_sound().expect("the programme has three sounds");
            assert_ne!(Some(next.id), chosen, "it stepped to another sound");
            assert!(
                sounds_before.contains(&next.id),
                "the sound switched to was not being decoded beside the chosen one: {sounds_before:?}"
            );
            switched_at = Some(n);
            switched_to = Some(next.id);
        }
        // 切り替えてから 64 KiB。ランダムアクセスポイントを待っていたら、ここには一枚も来ない。
        if switched_at.is_some_and(|at| n >= at + 16) {
            break;
        }
    }
    assert!(switched_at.is_some(), "the picture never began");
    assert!(
        units_after > 0,
        "the picture stopped at the change of sound (waiting for an entry point)"
    );
    assert_eq!(
        restarted_after, 0,
        "the decoder was told to start afresh at a change of sound"
    );
    assert!(sound_after_switch > 0, "the new sound never came");
    assert!(
        sounds_before.len() >= 3,
        "every sound of the programme is decoded, not only the chosen one: {sounds_before:?}"
    );
}

/// Changing the programme part way through a stream: the engine says it has switched,
/// and both pictures and sound come from the new programme.
#[test]
fn switching_programme_part_way_gives_pictures_and_sound_of_the_new_one() {
    let name = "two-programmes.ts";
    let bytes = read(name);
    let container = Container::of_path(name).expect("a name that says which");
    let mut looking = Engine::new(container, 48_000);
    looking.push(&bytes);
    let first = looking.best().expect("再生できる番組");
    let clear = |stream: Option<&zwcore::filters::Stream>| {
        stream.is_some_and(|stream| {
            looking
                .counted(stream.id)
                .is_some_and(|counted| counted.scrambled == 0)
        })
    };
    let second = looking
        .programs()
        .values()
        .filter(|program| {
            program.number != first && clear(program.picture()) && clear(program.sound())
        })
        .map(|program| program.number)
        .next()
        .expect("デコードできる番組がもう一つ");

    let mut engine = Engine::new(container, 48_000);
    engine.want(first);
    let mut decoding = Decoding::new(Prefer::Software);
    let mut switched = false;
    let mut switched_said = 0;
    let (mut before, mut after) = (Played::default(), Played::default());
    let chunks: Vec<&[u8]> = bytes.chunks(4_096).collect();
    for (n, chunk) in chunks.iter().enumerate() {
        if !switched && n >= chunks.len() / 2 {
            switched = true;
            engine.want(second);
        }
        engine.push(chunk);
        let mut outs = VecDeque::new();
        while let Some(out) = engine.next() {
            if matches!(out, Out::Switched) {
                switched_said += 1;
            }
            decoding.push(out, &mut outs);
        }
        keep(outs, if switched { &mut after } else { &mut before });
    }
    finish(&mut decoding, &mut after);
    assert_eq!(switched_said, 1, "Switched が {switched_said} 回");
    assert!(
        !before.pictures.is_empty() && !before.sounds.is_empty(),
        "切り替える前: 映像 {} 音声 {} 問題あり {:?}（{first} → {second}）",
        before.pictures.len(),
        before.sounds.len(),
        before.trouble
    );
    assert!(
        !after.pictures.is_empty(),
        "切り替えたあとの映像が出ない（問題あり {:?}）",
        after.trouble
    );
    assert!(!after.sounds.is_empty(), "切り替えたあとの音声が出ない");
}
