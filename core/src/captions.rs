// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 字幕・文字スーパー。TS（[`arib`]）と MMT/TLV（[`ttml`]）が同じ形で並ぶ。
//! 組み立てるのは `data` のスレッド、レンダリングは画面の側で、共有するのは [`snapshot`] だけ。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::engine::CaptionStream;
use crate::sound::sounding::Say;

pub mod arib;
pub mod ttml;

pub const SUPERIMPOSE_FOR: Duration = Duration::from_secs(180);

/// 管理データの言語ごとの表示モード（ARIB STD-B24 表 9-5 の 2 ビット）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 視聴者の操作に関係なく表示する（強制表示）。
    Automatic,
    /// 表示しない。
    Hidden,
    /// 視聴者の選択で表示する。特定条件（11）も同じに扱う。
    Selectable,
}

impl Mode {
    /// DMF の上位 2 ビット（受信時）。録画の再生も放送と同じに扱うので、下位 2 ビット（記録再生時）は見ない。
    fn of(dmf: u8) -> Self {
        match (dmf >> 2) & 3 {
            0 => Self::Automatic,
            1 => Self::Hidden,
            _ => Self::Selectable,
        }
    }
}

#[derive(Debug, Default)]
pub struct Choice {
    /// 出したい言語（0 表示しない、1 第 1、2 第 2）。
    wanted: u8,
    /// 「表示しない」のときも強制表示（自動表示）の言語は出す。
    forced: bool,
    languages: Vec<[u8; 3]>,
    /// `languages` と同じ並びの DMF。無ければどれも選択表示とみなす。
    modes: Vec<u8>,
    dirty: bool,
}

impl Choice {
    /// 実際に出す言語（0 なら出さない）。第 1・第 2 言語のときは選んだ言語、それが無いか自動非表示なら
    /// もう一方。自動表示の言語が別にあればそちらを出す。「表示しない」のときは、`forced` なら自動表示の
    /// 言語だけ。管理データがまだ無ければ、選んでいれば第 1 言語。
    pub fn actual(&self) -> u8 {
        let count = self.languages.len().min(2) as u8;
        if count == 0 {
            return self.wanted.min(1);
        }
        let mode = |number: u8| -> Option<Mode> {
            (number >= 1 && number <= count).then(|| {
                self.modes
                    .get(usize::from(number) - 1)
                    .map_or(Mode::Selectable, |dmf| Mode::of(*dmf))
            })
        };
        let automatic = (1..=count).find(|number| mode(*number) == Some(Mode::Automatic));
        if self.wanted == 0 {
            return if self.forced {
                automatic.unwrap_or(0)
            } else {
                0
            };
        }
        let wanted = self.wanted.min(2);
        if mode(wanted) == Some(Mode::Automatic) {
            return wanted;
        }
        if let Some(number) = automatic {
            return number;
        }
        [wanted, 3 - wanted]
            .into_iter()
            .find(|number| mode(*number) == Some(Mode::Selectable))
            .unwrap_or(0)
    }

    /// 実際に出す言語が変われば真。
    pub fn want(&mut self, wanted: u8) -> bool {
        let wanted = wanted.min(2);
        self.change(|choice| choice.wanted = wanted)
    }

    pub fn force(&mut self, forced: bool) -> bool {
        self.change(|choice| choice.forced = forced)
    }

    /// 言語と、その DMF（無ければ空）。実際に出す言語が変われば真。
    pub fn set_languages(&mut self, languages: Vec<[u8; 3]>, modes: Vec<u8>) -> bool {
        self.change(|choice| {
            choice.languages = languages;
            choice.modes = modes;
        })
    }

    fn change(&mut self, edit: impl FnOnce(&mut Self)) -> bool {
        let before = (
            self.actual(),
            self.wanted,
            self.forced,
            self.languages.clone(),
            self.modes.clone(),
        );
        edit(self);
        let after = (
            self.actual(),
            self.wanted,
            self.forced,
            self.languages.clone(),
            self.modes.clone(),
        );
        if after != before {
            self.dirty = true;
        }
        after.0 != before.0
    }

    /// 希望は残る。
    pub fn reset(&mut self) {
        self.languages.clear();
        self.modes.clear();
        self.dirty = true;
    }

    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

#[derive(Debug, Default)]
pub struct Tracker {
    pub choice: Choice,
    /// 文字スーパーは字幕と別に選ぶ。
    pub superimposing: Choice,
    pub arib: arib::State,
    pub ttml: ttml::State,
}

impl Tracker {
    /// TS なら真（MMT は通信のアプリにも渡す）。
    pub fn take(
        &mut self,
        pid: u16,
        data: &[u8],
        streams: &[CaptionStream],
        sound: Option<&Sender<Say>>,
    ) -> bool {
        if let Some(group) = isdb::caption::pes(data) {
            let one_seg = streams.iter().any(|s| s.pid == pid && s.one_seg);
            self.arib.take(
                pid,
                group,
                one_seg,
                &mut self.choice,
                &mut self.superimposing,
                sound,
            );
            return true;
        }
        if let Some(stream) = streams.iter().find(|c| c.pid == pid) {
            self.ttml.take(
                stream,
                data,
                streams,
                &mut self.choice,
                &mut self.superimposing,
                sound,
            );
        }
        false
    }

    pub fn want(&mut self, wanted: u8) -> u8 {
        if self.choice.want(wanted) {
            self.arib.clear_caption();
        }
        self.choice.actual()
    }

    /// 文字スーパーの言語（0 表示しない、1 第 1、2 第 2）。
    pub fn want_superimpose(&mut self, wanted: u8) {
        if self.superimposing.want(wanted) {
            self.clear_superimpose();
        }
    }

    /// 「表示しない」のときも強制表示のものを出すか（字幕、文字スーパー）。
    pub fn force(&mut self, caption: bool, superimpose: bool) {
        if self.choice.force(caption) {
            self.arib.clear_caption();
        }
        if self.superimposing.force(superimpose) {
            self.clear_superimpose();
        }
    }

    fn clear_superimpose(&mut self) {
        self.arib.clear_superimpose();
        self.ttml.clear_superimpose();
    }

    pub fn clear(&mut self) {
        self.arib.clear();
        self.ttml.clear();
    }

    pub fn reset(&mut self) {
        self.choice.reset();
        self.superimposing.reset();
        self.arib.reset();
        self.ttml.reset();
    }

    pub fn snapshot(&mut self) -> Option<Captions> {
        self.arib.expire();
        self.ttml.expire();
        let dirty = self.choice.take_dirty()
            | self.superimposing.take_dirty()
            | self.arib.take_dirty()
            | self.ttml.take_dirty();
        if !dirty {
            return None;
        }
        let language = self.choice.actual();
        let superimposing = self.superimposing.actual() != 0;
        let (caption, superimpose) = self.arib.shown(language != 0, superimposing);
        let (ttml_caption, ttml_superimpose) = self.ttml.shown(language != 0, superimposing);
        Some(Captions {
            caption,
            superimpose,
            ttml_caption,
            ttml_superimpose,
            language,
            languages: self.choice.languages.clone(),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Captions {
    pub caption: arib::Placed,
    /// 緊急地震速報はこれで来る。
    pub superimpose: arib::Placed,
    /// レンダリング側が文書の時刻のとおりに出す。
    pub ttml_caption: Option<Arc<ttml::Shown>>,
    pub ttml_superimpose: Option<Arc<ttml::Shown>>,
    pub language: u8,
    pub languages: Vec<[u8; 3]>,
}

pub type LatestCaptions = Arc<Mutex<Option<Arc<Captions>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    /// DMF の上位 2 ビット（受信時）。下位 2 ビットはわざと逆にして、見ていないことも確かめる。
    const AUTO: u8 = 0b0010;
    const HIDDEN: u8 = 0b0110;
    const SELECT: u8 = 0b1000;
    const SPECIAL: u8 = 0b1101;

    fn actual(modes: &[u8], wanted: u8, forced: bool) -> u8 {
        let mut choice = Choice::default();
        let languages = [*b"jpn", *b"eng"][..modes.len()].to_vec();
        choice.set_languages(languages, modes.to_vec());
        choice.want(wanted);
        choice.force(forced);
        choice.actual()
    }

    #[test]
    fn the_language_shown_follows_the_choice_and_the_display_mode() {
        // （DMF、メニュー、強制表示のチェック、出す言語）
        let table: &[(&[u8], u8, bool, u8)] = &[
            // 管理データがまだ無い：選んでいれば第 1 言語。
            (&[], 1, true, 1),
            (&[], 2, true, 1),
            (&[], 0, true, 0),
            // どちらも選択表示：選んだとおり。
            (&[SELECT, SELECT], 1, true, 1),
            (&[SELECT, SELECT], 2, true, 2),
            (&[SELECT, SELECT], 0, true, 0),
            (&[SELECT, SELECT], 0, false, 0),
            // 一言語だけなら第 2 言語を選んでも第 1 言語。
            (&[SELECT], 2, false, 1),
            // 自動表示の言語が別にあればそちらを出す。
            (&[AUTO, SELECT], 2, false, 1),
            (&[SELECT, AUTO], 1, false, 2),
            (&[AUTO, SELECT], 1, false, 1),
            // 表示しない：強制表示のチェックがあれば自動表示の言語だけ。
            (&[AUTO, SELECT], 0, true, 1),
            (&[SELECT, AUTO], 0, true, 2),
            (&[AUTO, SELECT], 0, false, 0),
            // 選んだ言語が自動非表示か無ければ、もう一方。
            (&[SELECT, HIDDEN], 2, false, 1),
            (&[HIDDEN, SELECT], 1, false, 2),
            // 自動非表示はどの場合も出さない。
            (&[HIDDEN, HIDDEN], 1, true, 0),
            (&[HIDDEN], 0, true, 0),
            // 特定条件は選択表示と同じ。
            (&[SPECIAL], 1, false, 1),
            (&[SPECIAL], 0, true, 0),
        ];
        for &(modes, wanted, forced, shown) in table {
            assert_eq!(
                actual(modes, wanted, forced),
                shown,
                "DMF {modes:?}、メニュー {wanted}、強制表示 {forced}"
            );
        }
    }

    #[test]
    fn a_change_of_display_mode_tells_that_the_language_shown_changed() {
        let mut choice = Choice::default();
        choice.want(0);
        choice.force(true);
        assert!(!choice.set_languages(vec![*b"jpn"], vec![SELECT]));
        assert!(choice.set_languages(vec![*b"jpn"], vec![AUTO]));
        assert_eq!(choice.actual(), 1);
        assert!(choice.force(false));
        assert_eq!(choice.actual(), 0);
    }
}
