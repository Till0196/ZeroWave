// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 音声と映像の一覧を表示用の名前にする。

use super::*;
use crate::guide::Described;

pub(super) fn say_sounds(engine: &Engine) {
    let sounds = engine.sounds();
    if sounds.len() < 2 && !engine.sound_is_dual_mono() {
        return;
    }
    let at = engine.sound_at();
    let said: Vec<String> = sounds
        .iter()
        .enumerate()
        .map(|(n, stream)| {
            let mark = if Some(n) == at { " ←" } else { "" };
            format!("{}: {}{mark}", n + 1, describe(stream))
        })
        .collect();
    tracing::info!("音声  {}", said.join("   "));
    if engine.sound_is_dual_mono() {
        tracing::info!("  デュアルモノ（{}）", engine.dual_mono().name());
    }
}

pub(super) fn say_pictures(engine: &Engine) {
    let pictures = engine.pictures();
    if pictures.len() < 2 {
        return;
    }
    let at = engine.picture_at();
    let said: Vec<String> = pictures
        .iter()
        .enumerate()
        .map(|(n, stream)| {
            let mark = if Some(n) == at { " ←" } else { "" };
            let name = if engine.is_stand_in(stream.id) {
                " 降雨対応".to_owned()
            } else {
                stream
                    .name
                    .as_ref()
                    .map_or(String::new(), |name| format!(" {name}"))
            };
            format!("{}: 0x{:04x}{name}{mark}", n + 1, stream.id)
        })
        .collect();
    tracing::info!("映像  {}", said.join("   "));
}

/// 「映像1」「映像2」…と番号で、降雨対応の映像には「 (降雨対応放送)」を付ける。
/// 記述子の文字列は選択メニューの名前に向かないので使わない。
pub(super) fn picture_names(engine: &Engine) -> Vec<String> {
    engine
        .pictures()
        .iter()
        .enumerate()
        .map(|(n, stream)| {
            if engine.is_stand_in(stream.id) {
                format!("映像{} (降雨対応放送)", n + 1)
            } else {
                format!("映像{}", n + 1)
            }
        })
        .collect()
}

pub(super) fn sound_choices(engine: &Engine) -> (Vec<super::SoundChoice>, Option<usize>) {
    use super::SoundChoice;
    let sounds = engine.sounds();
    let playing = engine.sound_at();
    let mode = engine.dual_mono();
    let mut choices = Vec::new();
    let mut at = None;
    for (n, stream) in sounds.iter().enumerate() {
        let component = stream.component.as_ref();
        let dual = component.is_some_and(|c| c.dual_mono())
            || (Some(n) == playing && engine.sound_is_dual_mono());
        let numbered = || format!("音声{}", n + 1);
        let first = component
            .map(|c| {
                c.called().map_or_else(
                    || crate::guide::language_name(&c.language).to_owned(),
                    str::to_owned,
                )
            })
            .filter(|s| !s.is_empty())
            .or_else(|| component.and_then(|c| c.assist().name().map(str::to_owned)))
            .unwrap_or_else(numbered);
        if dual {
            let second = component
                .and_then(|c| c.second.as_deref())
                .map(|iso| crate::guide::language_name(iso).to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(numbered);
            let labels = [
                (DualMono::Main, format!("{first}（主）")),
                (DualMono::Sub, format!("{second}（副）")),
                (DualMono::Both, format!("{first}（主）＋{second}（副）")),
            ];
            for (which, label) in labels {
                if Some(n) == playing && which == mode {
                    at = Some(choices.len());
                }
                choices.push(SoundChoice {
                    label,
                    stream: n,
                    mode: Some(which),
                });
            }
        } else {
            let mut label = first;
            if let Some(component) = component {
                let arrangement = component.arrangement();
                if !arrangement.is_empty() {
                    label.push(' ');
                    label.push_str(&arrangement);
                }
                label = component.with_assist(label);
            }
            if Some(n) == playing {
                at = Some(choices.len());
            }
            choices.push(SoundChoice {
                label,
                stream: n,
                mode: None,
            });
        }
    }
    (choices, at)
}

pub(super) fn describe(stream: &Stream) -> String {
    let Some(component) = &stream.component else {
        return format!("0x{:04x}", stream.id);
    };
    let arrangement = match component.arrangement() {
        said if said.is_empty() => String::from("？"),
        said => said,
    };
    let called = component.with_assist(component.who());
    format!("0x{:04x} {called} {arrangement}", stream.id)
}
