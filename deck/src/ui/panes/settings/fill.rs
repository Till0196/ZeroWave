// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ページの残りの高さで一部だけをスクロールする所。上に置いた物は止まったままで、下に固定する物も置ける。
//! 中身が短ければ中身の高さまで縮める。

use egui::{Rect, pos2};

use super::form::Form;
use super::{scroll_bar, scrolled};

/// ページの下の余白と、スクロールする所の最小の高さ。
const BOTTOM: f32 = 8.0;
const LEAST: f32 = 40.0;
/// スクロールする所と下に固定する物の間。
const GAP: f32 = 8.0;
/// スクロールバーを掴める幅（`bar_track`）。中身の右端からこれだけ右までをバーの範囲にすると、バーは中身のすぐ右に出る。
pub(super) const TRACK: f32 = 12.0;

/// 前の回に測った物。最初の回は無いので、ページの下までを取る。
#[derive(Debug, Clone, Copy, Default)]
struct Kept {
    scroll: f32,
    content: f32,
    foot: f32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Fill {
    id: egui::Id,
    rect: Rect,
}

impl Fill {
    /// 今の位置から始める。`x` は左右の範囲。
    pub(super) fn begin(
        form: &Form<'_, '_>,
        salt: impl std::hash::Hash + std::fmt::Debug,
        x: (f32, f32),
    ) -> Self {
        let id = form.ui.scope_id().with(("fill", salt));
        let kept: Option<Kept> = form.ui.data(|data| data.get_temp(id));
        let top = *form.at;
        let most = form.body.bottom() - BOTTOM - kept.map_or(0.0, |kept| kept.foot);
        let bottom = kept
            .map_or(most, |kept| most.min(top + kept.content))
            .max(top + LEAST);
        Self {
            id,
            rect: Rect::from_min_max(pos2(x.0, top), pos2(x.1, bottom)),
        }
    }

    /// スクロールする所。
    pub(super) fn rect(&self) -> Rect {
        self.rect
    }

    /// 中身を `lead` だけ下げて描く。`span` はホイールとスクロールバーの左右の範囲で、バーはその右端に出す。
    pub(super) fn scroll<R>(
        &self,
        form: &mut Form<'_, '_>,
        span: (f32, f32),
        lead: f32,
        draw: impl FnOnce(&mut Form<'_, '_>) -> R,
    ) -> R {
        let mut kept = self.kept(form);
        let wheel = Rect::from_min_max(
            pos2(span.0, self.rect.top()),
            pos2(span.1, self.rect.bottom()),
        );
        kept.scroll = scrolled(form.ui, wheel, kept.scroll, kept.content);
        let mut at = self.rect.top() + lead - kept.scroll;
        let said = form.clipped(self.rect, &mut at, draw);
        let content = at + kept.scroll - self.rect.top();
        scroll_bar(form.ui, form.look, wheel, kept.scroll, content);
        let grew = (content - kept.content).abs() > 0.5;
        kept.content = content;
        self.keep(form, kept, grew);
        *form.at = self.rect.bottom();
        said
    }

    /// 下に固定する物。高さは描いてみるまで分からないので、前の回の高さで場所を取り、変わったら描き直す。
    pub(super) fn foot<R>(
        &self,
        form: &mut Form<'_, '_>,
        draw: impl FnOnce(&mut Form<'_, '_>) -> R,
    ) -> R {
        let mut kept = self.kept(form);
        let start = self.rect.bottom() + GAP;
        let mut at = start;
        let body = Rect::from_min_max(
            pos2(self.rect.left(), start),
            pos2(self.rect.right(), form.body.bottom()),
        );
        let said = form.within(body, &mut at, draw);
        let foot = at - self.rect.bottom();
        let grew = (foot - kept.foot).abs() > 0.5;
        kept.foot = foot;
        self.keep(form, kept, grew);
        *form.at = at;
        said
    }

    fn kept(&self, form: &Form<'_, '_>) -> Kept {
        form.ui
            .data(|data| data.get_temp(self.id))
            .unwrap_or_default()
    }

    fn keep(&self, form: &Form<'_, '_>, kept: Kept, grew: bool) {
        form.ui.data_mut(|data| data.insert_temp(self.id, kept));
        if grew {
            form.ui.ctx().request_repaint();
        }
    }
}
