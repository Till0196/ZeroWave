// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! お気に入り、最近見たチャンネル、戻る・進む（どれもプロファイルごと）。

use std::time::{Duration, Instant, SystemTime};

use zwcore::guide::Key;

use super::Stage;
use crate::config::favourites::{self, Node};
use crate::window::menu::{Favourite, Station};

const HISTORY: usize = 20;
const CHECK: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub(super) struct History {
    keys: Vec<Key>,
    at: usize,
}

impl History {
    /// 選局が開いた。今の位置と同じ局なら何もしない（戻る・進むで並びを崩さない）。
    pub(super) fn visit(&mut self, key: Key) {
        if self.keys.get(self.at) == Some(&key) {
            return;
        }
        if !self.keys.is_empty() {
            self.keys.truncate(self.at + 1);
        }
        self.keys.push(key);
        if self.keys.len() > HISTORY {
            self.keys.remove(0);
        }
        self.at = self.keys.len() - 1;
    }

    pub(super) fn back(&mut self) -> Option<Key> {
        if self.at == 0 || self.keys.is_empty() {
            return None;
        }
        self.at -= 1;
        Some(self.keys[self.at])
    }

    pub(super) fn forward(&mut self) -> Option<Key> {
        if self.at + 1 >= self.keys.len() {
            return None;
        }
        self.at += 1;
        Some(self.keys[self.at])
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub(super) struct Loaded {
    pub(super) list: favourites::List,
    read: Option<(Option<SystemTime>, String)>,
    checked: Option<Instant>,
    pub(super) serial: u64,
}

impl Stage {
    pub(super) fn watched(&mut self, key: Key) {
        self.reception.reset(Some(key.service), Instant::now());
        self.errors_base = zwcore::filters::Counts::default();
        self.errors_reset_due = self
            .reset_errors_on_tune
            .then(|| Instant::now() + Duration::from_secs(5));
        crate::config::remember_last(key);
        self.recent_serial += 1;
        self.history.visit(key);
    }

    pub(super) fn reset_error_count(&mut self) {
        self.errors_base = self.player.tables().counts;
        self.errors_reset_due = None;
    }

    pub(super) fn settle_errors(&mut self) {
        if self
            .errors_reset_due
            .is_some_and(|due| Instant::now() >= due)
            && !self.player.tables().services.is_empty()
        {
            self.reset_error_count();
        }
        let base = self.errors_base;
        let counts = &mut self.status.counts;
        counts.packets = counts.packets.saturating_sub(base.packets);
        counts.dropped = counts.dropped.saturating_sub(base.dropped);
        counts.errors = counts.errors.saturating_sub(base.errors);
        counts.scrambled = counts.scrambled.saturating_sub(base.scrambled);
    }

    pub(super) fn profile_changed(&mut self) {
        self.history.clear();
        self.favourites.read = None;
        self.recent_serial += 1;
    }

    pub(super) fn refresh_favourites(&mut self) {
        if self
            .favourites
            .checked
            .is_some_and(|at| at.elapsed() < CHECK)
            && self.favourites.read.is_some()
        {
            return;
        }
        self.favourites.checked = Some(Instant::now());
        let profile = crate::config::load_state().profile;
        let mark = (favourites::modified(), profile.clone());
        if self.favourites.read.as_ref() == Some(&mark) {
            return;
        }
        self.favourites.list = favourites::of(&profile);
        self.favourites.read = Some(mark);
        self.favourites.serial += 1;
    }

    pub(super) fn add_favourite(&mut self) {
        let key = if self.player_mode {
            None
        } else {
            self.asked_key.or(self.player.tables().watching)
        };
        let Some(key) = key else {
            self.toast("お気に入りに追加できません。");
            return;
        };
        let name = self
            .guide
            .service(key)
            .map(|service| service.name.clone())
            .or_else(|| self.status.channel.clone())
            .unwrap_or_else(|| format!("サービス {}", key.service));
        let profile = crate::config::load_state().profile;
        match favourites::add(&profile, Node::station(name.clone(), key)) {
            Ok(()) => {
                self.favourites.read = None;
                self.refresh_favourites();
                self.toast(&format!("お気に入りに追加しました。\n{name}"));
            }
            Err(why) => tracing::warn!("お気に入りに追加できない: {why}"),
        }
    }

    pub(super) fn clear_recent(&mut self) {
        crate::config::update_state(|state| {
            let profile = state.profile.clone();
            state.recent.remove(&profile);
        });
        self.recent_serial += 1;
        tracing::info!("最近見たチャンネルの履歴をクリアした");
    }

    pub(super) fn go_back(&mut self) {
        match self.history.back() {
            Some(key) => self.tune(key),
            None => self.toast("該当チャンネルが選局できません。"),
        }
    }

    pub(super) fn go_forward(&mut self) {
        match self.history.forward() {
            Some(key) => self.tune(key),
            None => self.toast("該当チャンネルが選局できません。"),
        }
    }

    pub(super) fn go_previous(&mut self) {
        let state = crate::config::load_state();
        match state
            .recent
            .get(&state.profile)
            .and_then(|recent| recent.get(1))
        {
            Some(key) => self.tune(*key),
            None => self.toast("直前のチャンネルが選局できません。"),
        }
    }

    fn station(&self, key: Key, name: Option<&str>) -> Station {
        let name = name
            .map(str::to_owned)
            .or_else(|| self.guide.service(key).map(|service| service.name.clone()))
            .unwrap_or_else(|| format!("サービス {}", key.service));
        let programme = self
            .guide
            .at(key, self.guide.now_or_machine())
            .map(|programme| {
                let start = programme
                    .start
                    .map(|start| {
                        let (_, _, _, hour, minute, _) = zwcore::guide::civil(start);
                        format!("{hour}:{minute:02} ")
                    })
                    .unwrap_or_default();
                format!(
                    "{start}{}",
                    crate::ui::text::badges::bracketed(&programme.title)
                )
            })
            .unwrap_or_default();
        Station {
            key,
            name,
            programme,
            logo: self.guide.logo_of(key).cloned(),
        }
    }

    pub(super) fn favourite_rows(&self) -> Vec<Favourite> {
        fn rows(stage: &Stage, nodes: &[Node]) -> Vec<Favourite> {
            nodes
                .iter()
                .map(|node| match node.key() {
                    Some(key) => Favourite::Station(stage.station(key, Some(&node.name))),
                    None => Favourite::Folder(node.name.clone(), rows(stage, &node.items)),
                })
                .collect()
        }
        rows(self, &self.favourites.list.items)
    }

    pub(super) fn recent_rows(&self) -> Vec<Station> {
        let state = crate::config::load_state();
        state
            .recent
            .get(&state.profile)
            .into_iter()
            .flatten()
            .map(|key| self.station(*key, None))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(service: u16) -> Key {
        Key {
            network: 4,
            service,
        }
    }

    #[test]
    fn back_and_forward_work_like_a_browser() {
        let mut history = History::default();
        for service in [1, 2, 3] {
            history.visit(key(service));
        }
        assert_eq!(history.back(), Some(key(2)));
        history.visit(key(2));
        assert_eq!(history.back(), Some(key(1)));
        history.visit(key(1));
        assert_eq!(history.back(), None);
        assert_eq!(history.forward(), Some(key(2)));
        history.visit(key(2));
        history.visit(key(9));
        assert_eq!(history.forward(), None);
        assert_eq!(history.back(), Some(key(2)));
        assert_eq!(history.back(), Some(key(1)));
        for service in 10..40 {
            history.visit(key(service));
        }
        assert_eq!(history.keys.len(), HISTORY);
    }
}
