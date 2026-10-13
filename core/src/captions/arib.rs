// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! TS の字幕・文字スーパー（ARIB STD-B24）。

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::time::Instant;

use isdb::caption::{Body, Group, Kind, Layout, Presenter};

use super::{Choice, SUPERIMPOSE_FOR};
use crate::sound::sounding::Say;

pub type Placed = Option<(Layout, Instant)>;

#[derive(Debug, Default)]
pub struct State {
    /// コンポーネント・種類・言語ごとのレイアウト（管理データで設定したもの）。
    presenters: BTreeMap<(u16, Kind, u8), Presenter>,
    caption: Placed,
    /// 三分で消す。
    superimpose: Placed,
    dirty: bool,
}

/// ワンセグは文字集合が固定（TR-B14 第三分冊 6.4.1）。
fn presenter_for(kind: Kind, language: u8, one_seg: bool) -> Presenter {
    let mut presenter = Presenter::for_language(kind, language);
    if one_seg {
        presenter.set_one_seg();
    }
    presenter
}

impl State {
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// レイアウトは残る。
    pub fn clear(&mut self) {
        self.caption = None;
        self.superimpose = None;
        self.dirty = true;
    }

    pub fn clear_caption(&mut self) {
        self.caption = None;
        self.dirty = true;
    }

    pub fn clear_superimpose(&mut self) {
        self.superimpose = None;
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        self.presenters.clear();
        self.clear();
    }

    /// 出すかどうか（字幕、文字スーパー）は [`Choice::actual`] で決まる。
    pub fn shown(&self, caption: bool, superimpose: bool) -> (Placed, Placed) {
        (
            self.caption.clone().filter(|_| caption),
            self.superimpose.clone().filter(|_| superimpose),
        )
    }

    pub fn expire(&mut self) {
        if self
            .superimpose
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() > SUPERIMPOSE_FOR)
        {
            self.superimpose = None;
            self.dirty = true;
        }
    }

    pub fn take(
        &mut self,
        pid: u16,
        group: Group,
        one_seg: bool,
        captions: &mut Choice,
        superimposes: &mut Choice,
        sound: Option<&Sender<Say>>,
    ) {
        // 字幕と文字スーパーは言語を別々に選ぶ。
        let choice = match group.kind {
            Kind::Caption => captions,
            Kind::Superimpose => superimposes,
        };
        match &group.body {
            Body::Management(management) => {
                let changed = choice.set_languages(
                    management.languages.iter().map(|l| l.iso).collect(),
                    management.languages.iter().map(|l| l.dmf).collect(),
                );
                if changed {
                    // 出す言語が変わったので、前の言語の文は消す。
                    match group.kind {
                        Kind::Caption => self.caption = None,
                        Kind::Superimpose => self.superimpose = None,
                    }
                    self.dirty = true;
                }
                // 切り替えたとき管理データを待たずに済むよう、ほかの言語も設定しておく。
                let actual = choice.actual().max(1);
                let presenter = self
                    .presenters
                    .entry((pid, group.kind, actual))
                    .or_insert_with(|| presenter_for(group.kind, actual, one_seg));
                presenter.manage(management);
                for ((p, kind, _), presenter) in &mut self.presenters {
                    if *p == pid && *kind == group.kind && presenter.language != actual {
                        presenter.manage(management);
                    }
                }
            }
            Body::Statement(statement) => {
                // 出さないときも第 1 言語でレイアウトしておく（出すようになったときに管理データを待たない）。
                let actual = choice.actual().max(1);
                if group.language() != Some(actual) {
                    return;
                }
                let presenter = self
                    .presenters
                    .entry((pid, group.kind, actual))
                    .or_insert_with(|| presenter_for(group.kind, actual, one_seg));
                let layout = presenter.present(statement);
                if let (Some(number), Some(sound)) = (layout.sound, sound) {
                    let _ = sound.send(Say::Chime(u32::from(number)));
                }
                let slot = match group.kind {
                    Kind::Superimpose => &mut self.superimpose,
                    Kind::Caption => &mut self.caption,
                };
                if layout.texts.is_empty() {
                    if let Some(after) = layout.clear_after_ms {
                        if let Some((had, since)) = slot {
                            let at = since.elapsed().as_millis() as u32 + after;
                            for text in had.texts.iter_mut().filter(|t| t.until_ms.is_none()) {
                                text.until_ms = Some(at);
                            }
                        }
                    } else if layout.clear {
                        *slot = None;
                    }
                } else if layout.clear || slot.is_none() || group.kind == Kind::Superimpose {
                    if group.kind == Kind::Superimpose {
                        tracing::debug!(
                            "文字スーパー: {}",
                            layout
                                .texts
                                .iter()
                                .map(|t| t.text.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    *slot = Some((layout, Instant::now()));
                } else if let Some((had, since)) = slot {
                    // 先の文が TIME CS で消えるまでは、新しい文はその後に出す（重なるので）。
                    let passed = since.elapsed().as_millis() as u32;
                    let after_clear = had
                        .texts
                        .iter()
                        .filter_map(|t| t.until_ms)
                        .max()
                        .unwrap_or(0);
                    // CS が来るまで足し続けるので、出ていない文は捨てる。
                    had.texts
                        .retain(|text| text.until_ms.is_none_or(|until| until > passed));
                    for mut text in layout.texts {
                        text.delay_ms = (text.delay_ms + passed).max(after_clear);
                        text.until_ms = text.until_ms.map(|u| (u + passed).max(after_clear));
                        had.texts.push(text);
                    }
                }
                self.dirty = true;
            }
        }
    }
}
