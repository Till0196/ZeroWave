// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組説明のウィンドウ：ビルドする、出す、視聴中の局に追従させる。

use super::*;

impl Screen {
    /// `from` は開いたウィンドウ（番組表や番組検索なら、それをオーナーにする）。
    pub(super) fn open_detail(
        &mut self,
        key: Key,
        programme: zwcore::guide::Programme,
        from: Option<WindowId>,
    ) {
        let detail = self.detail_of(key, programme, None);
        self.show_detail(detail, true, from);
    }

    pub(super) fn watching_detail(
        &self,
        follow: ui::guide::detail::Follow,
    ) -> Option<ui::guide::detail::Detail> {
        let key = self.watching?;
        let now = self.guide.now_or_machine();
        let programme = match follow {
            ui::guide::detail::Follow::Now => self.guide.at(key, now),
            ui::guide::detail::Follow::Next => self.guide.after(key, now),
        }?
        .clone();
        Some(self.detail_of(key, programme, Some(follow)))
    }

    fn detail_of(
        &self,
        key: Key,
        programme: zwcore::guide::Programme,
        follow: Option<ui::guide::detail::Follow>,
    ) -> ui::guide::detail::Detail {
        ui::guide::detail::of(&self.guide, key, programme, follow)
    }

    /// 番組説明のウィンドウに出す。ウィンドウはいつも一つ。
    pub(super) fn show_detail(
        &mut self,
        detail: ui::guide::detail::Detail,
        focus: bool,
        from: Option<WindowId>,
    ) {
        let owner = self.owner_of(true, from);
        if let Some(sub) = self
            .subs
            .values_mut()
            .find(|sub| matches!(sub.what, Which::Detail(_)))
        {
            sub.what = Which::Detail(Box::new(detail));
            sub.dirty = true;
            if focus {
                let window = Arc::clone(&sub.window);
                self.reown(&window, owner);
                window.focus_window();
            }
            return;
        }
        if let Some(waiting) = self
            .pending
            .iter_mut()
            .find(|waiting| matches!(waiting, Which::Detail(_)))
        {
            *waiting = Which::Detail(Box::new(detail));
            return;
        }
        self.open_from(Which::Detail(Box::new(detail)), from);
    }

    /// メニューから開いた番組説明を、局と番組に付いていかせる。
    pub(super) fn follow_watching(&mut self) {
        let Some(follow) = self.subs.values().find_map(|sub| match &sub.what {
            Which::Detail(detail) => detail.follow,
            _ => None,
        }) else {
            return;
        };
        if let Some(detail) = self.watching_detail(follow) {
            self.show_detail(detail, false, None);
        }
    }
}
