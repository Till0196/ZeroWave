// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 何を見るかを決めるフィルター。覚えるのはコンポーネントタグで、識別子ではない（テーブルの更新で
//! ストリームが別の識別子へ移る）。

use std::collections::BTreeMap;

use mediadecoder::es::Codec;

use super::{Kind, Program, Stream};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    scrambled: bool,
    undecodable: bool,
    one_seg: bool,
    soundless: bool,
    carried: std::cmp::Reverse<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Changed {
    pub video: bool,
    pub sound: bool,
}

impl Changed {
    pub fn any(self) -> bool {
        self.video || self.sound
    }
}

#[derive(Debug, Default)]
pub struct Selector {
    wanted: Option<u16>,
    video: Option<u16>,
    sound: Option<u16>,
    video_tag: Option<u16>,
    sound_tag: Option<u16>,
    sounds: Vec<u16>,
}

impl Selector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn wanted(&self) -> Option<u16> {
        self.wanted
    }

    pub fn chosen(&self) -> (Option<u16>, Option<u16>) {
        (self.video, self.sound)
    }

    pub fn video(&self) -> Option<u16> {
        self.video
    }

    pub fn sound(&self) -> Option<u16> {
        self.sound
    }

    pub fn sounds_beside(&self) -> &[u16] {
        &self.sounds
    }

    pub fn hears(&self, id: u16) -> bool {
        Some(id) == self.sound || self.sounds.contains(&id)
    }

    pub fn playing<'a>(&self, known: &'a BTreeMap<u16, Program>) -> Option<&'a Program> {
        if let Some(number) = self.wanted
            && let Some(program) = known.get(&number)
        {
            return Some(program);
        }
        let chosen = self.video.or(self.sound)?;
        known
            .values()
            .find(|program| program.streams.iter().any(|stream| stream.id == chosen))
    }

    pub fn want(&mut self, known: &BTreeMap<u16, Program>, program: u16) -> Option<Changed> {
        self.wanted = Some(program);
        self.play(known, program)
    }

    pub fn play(&mut self, known: &BTreeMap<u16, Program>, program: u16) -> Option<Changed> {
        let found = known.get(&program)?;
        let video = found.picture().map(|stream| stream.id);
        let sound = found.sound().map(|stream| stream.id);
        Some(self.choose(known, video, sound))
    }

    pub fn choose(
        &mut self,
        known: &BTreeMap<u16, Program>,
        video: Option<u16>,
        sound: Option<u16>,
    ) -> Changed {
        let tag_of = |id: u16| {
            known
                .values()
                .flat_map(|program| &program.streams)
                .find(|stream| stream.id == id)
                .and_then(|stream| stream.tag)
        };
        self.video_tag = video.and_then(tag_of);
        self.sound_tag = sound.and_then(tag_of);
        let changed = Changed {
            video: self.video != video,
            sound: self.sound != sound,
        };
        self.video = video;
        self.sound = sound;
        self.sounds = sounds_beside(known, video, sound);
        changed
    }

    /// 選んだ音声をタグで追う（選び直すと、一覧が並び直るたびに主音声へ戻ってしまう）。
    pub fn follow(&mut self, known: &BTreeMap<u16, Program>, program: &Program) -> Changed {
        if self.wanted != Some(program.number) {
            return Changed::default();
        }
        let picture = self
            .video_tag
            .and_then(|tag| program.picture_tagged(tag))
            .or_else(|| program.picture());
        let chosen = self
            .sound_tag
            .and_then(|tag| program.sound_tagged(tag))
            .or_else(|| program.sound());
        let video = picture.map(|stream| stream.id);
        let sound = chosen.map(|stream| stream.id);
        if (self.video, self.sound) == (video, sound) {
            self.video_tag = picture.and_then(|stream| stream.tag);
            self.sound_tag = chosen.and_then(|stream| stream.tag);
            self.sounds = sounds_beside(known, video, sound);
            return Changed::default();
        }
        self.choose(known, video, sound)
    }

    pub fn lost(&mut self, known: &BTreeMap<u16, Program>) -> Changed {
        self.choose(known, None, None)
    }

    pub fn pictures(&self, known: &BTreeMap<u16, Program>) -> Vec<Stream> {
        self.playing(known)
            .map(|program| program.pictures().cloned().collect())
            .unwrap_or_default()
    }

    pub fn play_picture(&mut self, known: &BTreeMap<u16, Program>, id: u16) -> Option<Changed> {
        let found = self
            .pictures(known)
            .into_iter()
            .find(|stream| stream.id == id)?;
        self.video_tag = found.tag;
        Some(self.choose(known, Some(found.id), self.sound))
    }

    pub fn play_sound(
        &mut self,
        known: &BTreeMap<u16, Program>,
        sounds: &[Stream],
        id: u16,
    ) -> Option<Changed> {
        let found = sounds.iter().find(|stream| stream.id == id)?;
        self.sound_tag = found.tag;
        Some(self.choose(known, self.video, Some(found.id)))
    }

    pub fn is_stand_in(&self, known: &BTreeMap<u16, Program>, id: u16) -> bool {
        let Some(program) = self.playing(known) else {
            return false;
        };
        program
            .streams
            .iter()
            .find(|stream| stream.id == id)
            .is_some_and(|stream| program.stand_in(stream))
    }

    /// 順に：デコードできる映像、フルセグ、映像のあるもの、届いた映像の多いもの、番号の小さいもの。
    /// 読めないものは最後。全部そうでも一つ選ぶ。
    pub fn best(
        known: &BTreeMap<u16, Program>,
        scrambled: impl Fn(u16) -> u64,
        arrived: impl Fn(u16) -> u64,
    ) -> Option<u16> {
        let mut best: Option<(Score, u16)> = None;
        for (number, program) in known {
            let video = program.picture();
            let sound = program.sound();
            let decodable = program.find(|kind| match kind {
                Kind::Video(codec) => can_decode(codec),
                _ => false,
            });
            if sound.is_none() && video.is_none() {
                continue;
            }
            let is_scrambled = video
                .into_iter()
                .chain(sound)
                .any(|stream| scrambled(stream.id) > 0);
            let carried = video.or(sound).map_or(0, |stream| arrived(stream.id));
            let score = Score {
                scrambled: is_scrambled,
                undecodable: decodable.is_none(),
                one_seg: program.one_seg(),
                soundless: video.is_none(),
                carried: std::cmp::Reverse(carried),
            };
            if best.is_none_or(|(had, _)| score < had) {
                best = Some((score, *number));
            }
        }
        best.map(|(_, number)| number)
    }
}

/// 全部デコードしておき、切り替えは次にどのストリームを渡すかだけにする。
fn sounds_beside(
    known: &BTreeMap<u16, Program>,
    video: Option<u16>,
    sound: Option<u16>,
) -> Vec<u16> {
    let Some(chosen) = sound.or(video) else {
        return Vec::new();
    };
    known
        .values()
        .find(|program| program.streams.iter().any(|stream| stream.id == chosen))
        .map(|program| program.sounds().map(|stream| stream.id).collect())
        .unwrap_or_default()
}

pub fn can_decode(codec: Codec) -> bool {
    matches!(codec, Codec::Mpeg2 | Codec::H264 | Codec::Hevc)
}
