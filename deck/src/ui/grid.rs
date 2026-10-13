// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組表とミニ番組表が共有するもの：時刻を縦の位置に変換するグリッドと、番組のセル一つの表示。

use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2};
use zwcore::guide::{Moment, Programme, civil};

use super::theme::epg;

#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub from: Moment,
    pub hours: u32,
    pub hour: f32,
}

impl Grid {
    pub fn at_moment(&self, moment: Moment, top: f32) -> Option<f32> {
        let since = moment - self.from;
        if since < 0 || since > i64::from(self.hours) * 3600 {
            return None;
        }
        Some(top + self.hour * since as f32 / 3600.0)
    }

    pub fn until(&self) -> Moment {
        self.from + i64::from(self.hours) * 3600
    }

    /// 外は切り、終了時刻の無い番組は下端まで。一分に満たないものは置かない。
    pub fn place(&self, body: Rect, start: Moment, ends: Option<Moment>) -> Option<Rect> {
        let last = self.until();
        let ends = ends.unwrap_or(last);
        let top = self.at_moment(start.max(self.from), body.top())?;
        let bottom = self.at_moment(ends.min(last), body.top())?;
        if bottom - top < 2.0 {
            return None;
        }
        Some(Rect::from_min_max(
            pos2(body.left(), top),
            pos2(body.right(), bottom),
        ))
    }
}

/// セルの見た目のうち、番組表とミニ番組表で違うところ。
#[derive(Debug, Clone, Copy)]
pub struct Style<'a> {
    pub skin: &'a epg::Skin,
    pub edge: Color32,
    pub minute: Color32,
    pub title: Color32,
    pub detail: Color32,
    /// 枠線を付ける辺。番組表は上と右、ミニ番組表は左。
    pub edges: Edges,
    pub pad: f32,
    pub minute_size: f32,
    pub title_size: f32,
    pub title_leading: f32,
    /// 説明を出すか（文字の大きさと行送り）。ミニ番組表は出さない。
    pub detail_text: Option<(f32, f32)>,
    pub top_pad: f32,
    pub tag: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Edges {
    pub top: bool,
    pub right: bool,
    pub left: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Primary,
    Secondary,
    /// 二度押し。
    Double,
}

pub fn cell(
    ui: &mut Ui,
    rect: Rect,
    programme: &Programme,
    now: Option<Moment>,
    style: &Style<'_>,
) -> Option<Click> {
    let skin = style.skin;
    let response = ui.interact(
        rect,
        ui.id()
            .with((style.tag, programme.service, programme.start)),
        Sense::click(),
    );
    let painter = ui.painter().with_clip_rect(rect);
    // 上端がジャンルの色、下端は共通の青灰。ポインターを重ねたセルは白に寄せ、文字を暗く、白い枠。
    let focused = response.hovered();
    let genre = crate::ui::guide::epg::genre_colour(programme.genre(), skin);
    let (top, bottom) = if focused && skin.focus_lift > 0 {
        (
            super::theme::lightened(genre, skin.focus_lift),
            super::theme::lightened(skin.genre_end, skin.focus_lift),
        )
    } else {
        (genre, skin.genre_end)
    };
    super::graded(&painter, rect, top, bottom);
    let edge = Stroke::new(1.0, style.edge);
    if style.edges.top {
        painter.hline(rect.x_range(), rect.top(), edge);
    }
    if style.edges.right {
        painter.vline(rect.right(), rect.y_range(), edge);
    }
    if style.edges.left {
        painter.vline(rect.left(), rect.y_range(), edge);
    }
    let (mut minute_colour, mut title_colour, mut detail_colour) = if focused {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(epg::FOCUS_WIDTH, skin.focus_edge),
            egui::StrokeKind::Inside,
        );
        (skin.focus_text, skin.focus_text, skin.focus_detail)
    } else {
        (style.minute, style.title, style.detail)
    };
    if now.is_some_and(|now| programme.ends().is_some_and(|ends| ends <= now)) {
        painter.rect_filled(rect, 0.0, skin.genre_end.gamma_multiply(epg::ENDED_VEIL));
        minute_colour = minute_colour.gamma_multiply(epg::ENDED_INK);
        title_colour = title_colour.gamma_multiply(epg::ENDED_INK);
        detail_colour = detail_colour.gamma_multiply(epg::ENDED_INK);
    }
    let response = response.on_hover_text(crate::ui::text::badges::bracketed(&programme.title));

    let left = rect.left() + style.pad;
    let width = rect.width() - style.pad * 2.0;
    let mut at = rect.top() + style.top_pad;
    let titled = crate::ui::text::badges::titled(&programme.title, &programme.detail);
    let minute_of = |start: Moment| {
        let (_, _, _, _, minute, _) = civil(start);
        format!("{minute:02}")
    };
    // 短い番組は分と題名を一行に。枠の高さに合わせて文字を小さくする。
    let inner = rect.height() - 4.0;
    if inner < style.minute_size + 1.0 + style.title_leading {
        let size = (inner - 1.0).clamp(epg::TITLE_LEAST, style.title_size);
        let middle = rect.top() + rect.height() / 2.0;
        let mut x = left;
        if let Some(start) = programme.start {
            let minute = painter.layout_no_wrap(
                minute_of(start),
                FontId::monospace(size.min(style.minute_size)),
                minute_colour,
            );
            painter.galley(
                pos2(x, super::seat_em(&minute, middle - size / 2.0, size)),
                minute.clone(),
                minute_colour,
            );
            x += minute.size().x + 4.0;
        }
        crate::ui::text::badges::draw_line(
            &painter,
            pos2(x, middle),
            (rect.right() - style.pad - x).max(0.0),
            &titled,
            crate::ui::text::font::guide(size),
            title_colour,
            crate::ui::text::badges::Look::Coloured,
            1.0,
        );
        return click_of(&response);
    }
    if let Some(start) = programme.start {
        painter.text(
            pos2(left, at),
            egui::Align2::LEFT_TOP,
            minute_of(start),
            FontId::monospace(style.minute_size),
            minute_colour,
        );
        at += style.minute_size + 1.0;
    }
    let room = rect.bottom() - at - 2.0;
    if room <= 0.0 {
        return click_of(&response);
    }
    let used = crate::ui::text::badges::draw_wrapped(
        &painter,
        pos2(left, at),
        width,
        &titled,
        crate::ui::text::font::guide(style.title_size),
        title_colour,
        style.title_leading,
    );
    at += used.min(room);

    if let Some((size, leading)) = style.detail_text
        && at + leading < rect.bottom()
        && !programme.detail.is_empty()
    {
        let detail = super::laid(
            &painter,
            &crate::ui::text::badges::cleaned(&programme.detail),
            crate::ui::text::font::guide(size),
            detail_colour,
            width,
            leading,
        );
        painter.galley(pos2(left, at), detail, detail_colour);
    }
    click_of(&response)
}

fn click_of(response: &egui::Response) -> Option<Click> {
    // 二度目の押しは `clicked` も真なので、先に見る。
    if response.double_clicked() {
        Some(Click::Double)
    } else if response.clicked() {
        Some(Click::Primary)
    } else if response.secondary_clicked() {
        Some(Click::Secondary)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use egui::vec2;

    use super::*;

    #[test]
    fn programmes_are_placed_by_the_hours_shown() {
        let grid = Grid {
            from: 0,
            hours: 4,
            hour: 100.0,
        };
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(130.0, 400.0));
        assert!(grid.place(body, -7200, Some(-3600)).is_none());
        assert!(grid.place(body, 4 * 3600, Some(5 * 3600)).is_none());
        let crossing = grid
            .place(body, -1800, Some(1800))
            .expect("またぐものが置けない");
        assert_eq!(crossing.top(), 0.0);
        assert!((crossing.height() - 50.0).abs() < 0.1);
        let open = grid
            .place(body, 3 * 3600, None)
            .expect("終了時刻の無いものが置けない");
        assert!((open.bottom() - 400.0).abs() < 0.1);
        assert!(grid.place(body, 0, Some(20)).is_none());
    }
}
