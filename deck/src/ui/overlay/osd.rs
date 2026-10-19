// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像の上に重なる、いまの番組の情報。選局したときと番組が変わったときに出て、数秒で消える。

use std::time::Instant;

use egui::{Color32, FontId, Rect, Ui, pos2, vec2};

use crate::ui::theme::osd;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Osd {
    /// 名前が無ければ空で、一行目は時刻だけ。
    pub title: crate::ui::text::badges::Titled,
    pub clock: String,
    pub remote_key: Option<u8>,
    pub badge: String,
    pub badge_colour: Color32,
    pub logo: Option<std::sync::Arc<zwcore::guide::Logo>>,
    pub band: String,
    pub channel: String,
    pub airtime: String,
    pub info: String,
    /// メッセージ（選局できなかったときなど）。あれば題名の代わりに出る。
    pub said: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    shown_at: Option<Instant>,
    /// 薄れ始めるまでの秒。
    hold: f32,
}

/// 番組情報を自動で出すとき。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Auto {
    /// 出す秒。0 なら自動では出さない。
    pub seconds: u32,
    pub on_channel: bool,
    pub on_event: bool,
}

impl Auto {
    pub fn of(file: &crate::config::file::Osd) -> Self {
        let on = |flag: &str| file.event_on.iter().any(|said| said == flag);
        Self {
            seconds: file.event,
            on_channel: on("channel"),
            on_event: on("event"),
        }
    }

    /// 頼まれて出すときの秒。自動で出さない設定でも出す。
    pub fn asked(self) -> f32 {
        if self.seconds == 0 {
            osd::SHOWN_FOR
        } else {
            self.seconds as f32
        }
    }
}

impl Default for Auto {
    fn default() -> Self {
        Self::of(&crate::config::file::Osd::default())
    }
}

/// 自動で出すきっかけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Channel,
    Event,
}

impl State {
    /// 頼まれて出す（キー・メッセージ）。
    pub fn show(&mut self, auto: Auto) {
        self.show_for(auto.asked());
    }

    /// 選局や番組の切り替わりで出す。設定で出さないなら何もしない。
    pub fn show_on(&mut self, auto: Auto, why: Why) {
        let wanted = match why {
            Why::Channel => auto.on_channel,
            Why::Event => auto.on_event,
        };
        if wanted && auto.seconds > 0 {
            self.show_for(auto.seconds as f32);
        }
    }

    fn show_for(&mut self, seconds: f32) {
        self.shown_at = Some(Instant::now());
        self.hold = seconds;
    }

    /// 局の OSD が録画に切り替えた後まで残らないように。
    pub fn hide(&mut self) {
        self.shown_at = None;
    }

    pub fn opacity(&mut self) -> Option<f32> {
        let since = self.shown_at?.elapsed().as_secs_f32();
        if since >= self.hold + osd::FADE {
            self.shown_at = None;
            return None;
        }
        if since <= self.hold {
            return Some(1.0);
        }
        Some(1.0 - (since - self.hold) / osd::FADE)
    }

    pub fn is_shown(&self) -> bool {
        self.shown_at.is_some()
    }
}

pub fn show(ui: &mut Ui, full: Rect, osd: &Osd, opacity: f32, hdr: bool) {
    let dp = scale_for(full.width());
    let rect = Rect::from_min_size(full.min, vec2(full.width(), osd::HEIGHT * dp));
    let shaded = Rect::from_min_size(
        full.min,
        vec2(full.width(), rect.height() * osd::SHADE_DEPTH),
    );
    let strength = if hdr { osd::SHADE_HDR } else { osd::SHADE_SDR };
    shade(
        &ui.painter().with_clip_rect(shaded),
        shaded,
        opacity * strength,
        osd::SHADE,
        hdr,
    );
    let painter = ui.painter().with_clip_rect(rect);
    let fade = |colour: Color32| colour.gamma_multiply(opacity);
    let left = rect.left() + osd::PAD_X * dp;
    let right = rect.right() - osd::PAD_X * dp;

    let clock = painter.layout_no_wrap(
        osd.clock.clone(),
        FontId::monospace(osd::CLOCK_SIZE * dp),
        fade(osd::CLOCK),
    );
    let title_top = rect.top() + osd::TITLE_TOP * dp;
    let title_area = osd::TITLE_AREA * dp;
    painter.galley(
        pos2(
            right - clock.size().x,
            title_top + (title_area - clock.size().y) / 2.0,
        ),
        clock,
        fade(osd::CLOCK),
    );
    let (titled, colour) = match &osd.said {
        Some(said) => (crate::ui::text::badges::Titled::plain(said), osd::TROUBLE),
        None => (osd.title.clone(), osd::TITLE),
    };
    let title_width = (right - left - 60.0 * dp).max(0.0);
    // タイトルは高さ 38dp のバーの中で縦に中央。
    crate::ui::text::badges::draw_line(
        &painter,
        pos2(left, title_top + title_area / 2.0),
        title_width,
        &titled,
        crate::ui::text::font::osd(osd::TITLE_SIZE * dp),
        fade(colour),
        crate::ui::text::badges::Look::Outlined,
        opacity,
    );

    let line = Rect::from_min_size(
        pos2(left, rect.top() + osd::LINE_TOP * dp),
        vec2(right - left, osd::LINE_HEIGHT * dp),
    );
    let middle = line.center().y;
    let mut at = line.left();
    if let Some(key) = osd.remote_key {
        crate::ui::key_circle(
            &painter,
            pos2(at + osd::KEY_BADGE * dp / 2.0, middle),
            osd::KEY_BADGE * dp / 2.0,
            fade(Color32::WHITE),
            None,
            key,
            osd::KEY_SIZE * dp,
            fade(osd::KEY_TEXT),
        );
        at += (osd::KEY_BADGE + osd::KEY_GAP) * dp;
    }
    if !osd.badge.is_empty() {
        let text_height = painter
            .layout_no_wrap(
                "あ".to_owned(),
                crate::ui::text::font::osd(osd::BAND_SIZE * dp),
                Color32::WHITE,
            )
            .size()
            .y
            + painter
                .layout_no_wrap(
                    "あ".to_owned(),
                    crate::ui::text::font::osd(osd::CHANNEL_SIZE * dp),
                    Color32::WHITE,
                )
                .size()
                .y;
        let logo = crate::ui::logo_pill(
            &painter,
            pos2(at, middle),
            osd.logo.as_ref(),
            &osd.badge,
            fade(osd.badge_colour),
            fade(Color32::WHITE),
            osd::LOGO_SIZE * dp,
            text_height,
            osd::LOGO_PAD_X * dp,
            osd::LOGO_MIN_WIDTH * dp,
        );
        at = logo.right() + osd::LOGO_GAP * dp;
    }
    let info = painter.layout_no_wrap(
        osd.info.clone(),
        crate::ui::text::font::osd(osd::INFO_SIZE * dp),
        fade(osd::INFO),
    );
    let info_left = line.right() - info.size().x;
    painter.galley(
        pos2(info_left, middle - info.size().y / 2.0),
        info,
        fade(osd::INFO),
    );
    // 番組の時間は左端から幅の `AIRTIME_AT` が基本だが、局名は切らず、長ければ時間が右へ寄る。
    let airtime_left = rect.left() + rect.width() * osd::AIRTIME_AT;
    let room = (info_left - osd::LOGO_GAP * dp * 2.0 - at).max(0.0);
    let band = painter.layout_no_wrap(
        osd.band.clone(),
        crate::ui::text::font::osd(osd::BAND_SIZE * dp),
        fade(osd::BAND),
    );
    let channel = crate::ui::one_line(
        &painter,
        &osd.channel,
        crate::ui::text::font::osd(osd::CHANNEL_SIZE * dp),
        fade(osd::CHANNEL),
        room,
        0.0,
    );
    let channel_width = channel.size().x;
    let channel_height = channel.size().y;
    if osd.band.is_empty() {
        painter.galley(
            pos2(at, middle - channel.size().y / 2.0),
            channel,
            fade(osd::CHANNEL),
        );
    } else {
        let top = middle - (band.size().y + channel.size().y) / 2.0;
        painter.galley(pos2(at, top), band.clone(), fade(osd::BAND));
        painter.galley(pos2(at, top + band.size().y), channel, fade(osd::CHANNEL));
    }
    if !osd.airtime.is_empty() {
        let airtime = painter.layout_no_wrap(
            osd.airtime.clone(),
            crate::ui::text::font::osd(osd::AIRTIME_SIZE * dp),
            fade(osd::AIRTIME),
        );
        let top = if osd.band.is_empty() {
            middle - airtime.size().y / 2.0
        } else {
            middle - (band.size().y + channel_height) / 2.0
        };
        let x = airtime_left.max(at + band.size().x.max(channel_width) + osd::AIRTIME_GAP * dp);
        if x + airtime.size().x <= info_left - osd::LOGO_GAP * dp {
            painter.galley(pos2(x, top), airtime, fade(osd::AIRTIME));
        }
    }
}

/// 狭いウィンドウでは縮め（[`osd::DP_LEAST`] まで）、広いウィンドウでは幅の比の半分だけ伸びる
/// （比例させると 4K で大きすぎる）。
pub fn scale_for(width: f32) -> f32 {
    let ratio = width / osd::REFERENCE_WIDTH;
    if ratio >= 1.0 {
        1.0 + (ratio - 1.0) * osd::GROWTH
    } else {
        ratio.max(osd::DP_LEAST)
    }
}

/// 画面右下のトースト。
pub fn toast(
    ui: &mut Ui,
    full: Rect,
    toast: &crate::draw::scene::Toasted,
    opacity: f32,
    hdr: bool,
) {
    let dp = scale_for(full.width());
    let painter = ui.painter().with_clip_rect(full);
    let fade = |colour: Color32| colour.gamma_multiply(opacity);
    let em = osd::TOAST_SIZE * dp;
    let small = em * osd::TOAST_DETAIL;
    let title_colour = match toast.tone {
        zwsdk::ui::Tone::Warn => osd::TOAST_WARN,
        zwsdk::ui::Tone::Bad => osd::TOAST_BAD,
        zwsdk::ui::Tone::Good => osd::TOAST_GOOD,
        _ => osd::TOAST_TEXT,
    };
    // 長い文は画面の幅に合わせて折り返す。
    let (pad_x, pad_y) = (osd::TOAST_PAD_X * dp, osd::TOAST_PAD_Y * dp);
    let margin = osd::TOAST_MARGIN * dp;
    let most = (full.width() * osd::TOAST_WIDEST).min(full.width() - margin * 2.0 - pad_x * 2.0);
    let title_font = crate::ui::text::font::osd(em);
    let detail_font = crate::ui::text::font::osd(small);
    let titles: Vec<_> = wrapped(ui, &toast.title, &title_font, most)
        .into_iter()
        .map(|row| painter.layout_no_wrap(row, title_font.clone(), fade(title_colour)))
        .collect();
    let details: Vec<_> = toast
        .details
        .iter()
        .flat_map(|line| wrapped(ui, line, &detail_font, most))
        .map(|row| painter.layout_no_wrap(row, detail_font.clone(), fade(osd::TOAST_DETAIL_TEXT)))
        .collect();
    // 高さは文字の枠（1 em）から。galley の箱はヒラギノで 1.5 em になる（`ui::seat_em`）。
    let line = small * 1.45;
    let title_line = em * 1.35;
    let width = titles
        .iter()
        .chain(&details)
        .map(|galley| galley.size().x)
        .fold(0.0, f32::max);
    let title_height = em + (titles.len().max(1) - 1) as f32 * title_line;
    let size = vec2(
        width + pad_x * 2.0,
        title_height + details.len() as f32 * line + pad_y * 2.0,
    );
    let pill = Rect::from_min_size(
        pos2(
            full.right() - margin - size.x,
            full.bottom() - margin - size.y,
        ),
        size,
    );
    painter.rect_filled(pill, 0.0, ground(osd::TOAST_BACK, opacity, hdr));
    let mut top = pill.top() + pad_y;
    for galley in titles {
        let y = crate::ui::seat_em(&galley, top, em);
        painter.galley(pos2(pill.left() + pad_x, y), galley, fade(title_colour));
        top += title_line;
    }
    let mut top = pill.top() + pad_y + title_height;
    for galley in details {
        let y = crate::ui::seat_em(&galley, top + (line - small) / 2.0, small);
        painter.galley(
            pos2(pill.left() + pad_x, y),
            galley,
            fade(osd::TOAST_DETAIL_TEXT),
        );
        top += line;
    }
    if let Some(progress) = toast.progress {
        let height = (2.0 * dp).max(2.0);
        let track = Rect::from_min_max(pos2(pill.left(), pill.bottom() - height), pill.max);
        painter.rect_filled(track, 0.0, fade(osd::TOAST_TRACK));
        let done = Rect::from_min_size(
            track.min,
            vec2(track.width() * progress.clamp(0.0, 1.0), height),
        );
        painter.rect_filled(done, 0.0, fade(osd::TOAST_TEXT));
    }
}

fn wrapped(ui: &Ui, text: &str, font: &egui::FontId, most: f32) -> Vec<String> {
    ui.fonts_mut(|fonts| {
        let mut rows = Vec::new();
        let mut row = String::new();
        let mut width = 0.0;
        let mut space: Option<usize> = None;
        for letter in text.chars() {
            let advance = fonts.glyph_width(font, letter);
            if width + advance > most && !row.is_empty() {
                match space.take() {
                    Some(at) if at > 0 => {
                        let rest = row.split_off(at);
                        rows.push(row.trim_end().to_owned());
                        row = rest.trim_start().to_owned();
                    }
                    _ => rows.push(std::mem::take(&mut row)),
                }
                width = row.chars().map(|c| fonts.glyph_width(font, c)).sum();
            }
            if letter == ' ' {
                space = Some(row.len() + 1);
            }
            row.push(letter);
            width += advance;
        }
        if !row.is_empty() || rows.is_empty() {
            rows.push(row);
        }
        rows
    })
}

/// HDR のサーフェスはリニアで混ぜ、SDR のサーフェスはガンマの空間で混ぜる（`Sheet::gamma`）。濃さはリニアで混ぜたときに
/// 揃え、SDR では中くらいの明るさの映像の上で同じ暗さになる不透明度に直す。
pub fn dim(alpha: f32, hdr: bool) -> f32 {
    use zwcore::output::video::colour::{to_encoded, to_linear};
    let alpha = alpha.clamp(0.0, 1.0);
    if hdr {
        return alpha;
    }
    const MIDDLE: f32 = 0.5;
    let left = (1.0 - alpha) * to_linear(MIDDLE);
    (1.0 - to_encoded(left) / MIDDLE).clamp(0.0, 1.0)
}

/// [`dim`] でサーフェスの混ぜ方に合わせ、薄れを掛ける。色は乗算済みのまま渡す。
pub fn ground(colour: Color32, opacity: f32, hdr: bool) -> Color32 {
    let alpha = f32::from(colour.a()) / 255.0;
    if alpha <= 0.0 {
        return Color32::TRANSPARENT;
    }
    let kept = dim(alpha * opacity, hdr) / alpha;
    colour.gamma_multiply(kept.min(1.0 / alpha))
}

/// 一枚の三角形のメッシュで、頂点に色を持たせる。半透明の色の矩形を重ねると、重なった一画素と
/// egui の縁のぼかしで縞になる。
pub fn shade(painter: &egui::Painter, rect: Rect, opacity: f32, stops: &[(f32, f32)], hdr: bool) {
    let black = |alpha: f32| Color32::from_black_alpha((dim(alpha * opacity, hdr) * 255.0) as u8);
    let mut mesh = egui::Mesh::default();
    let rows: Vec<(f32, Color32)> = stops
        .iter()
        .map(|&(at, alpha)| (rect.top() + rect.height() * at, black(alpha)))
        .collect();
    for (y, colour) in &rows {
        mesh.colored_vertex(pos2(rect.left(), *y), *colour);
        mesh.colored_vertex(pos2(rect.right(), *y), *colour);
    }
    for row in 0..rows.len() as u32 - 1 {
        let (top, bottom) = (row * 2, row * 2 + 2);
        mesh.add_triangle(top, top + 1, bottom);
        mesh.add_triangle(top + 1, bottom + 1, bottom);
    }
    painter.add(egui::Shape::mesh(mesh));
}

pub fn info_line(
    codec: Option<&str>,
    picture: Option<(u32, u32, bool)>,
    sound: Option<&str>,
    rate: Option<u32>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(codec) = codec {
        parts.push(codec.to_owned());
    }
    if let Some((width, height, progressive)) = picture {
        parts.push(format!(
            "{width}×{height}{}",
            if progressive { "p" } else { "i" }
        ));
    }
    let mut heard = Vec::new();
    if let Some(sound) = sound {
        heard.push(sound.to_owned());
    }
    if let Some(rate) = rate {
        heard.push(format!("{}kHz", rate / 1000));
    }
    if !heard.is_empty() {
        parts.push("♪".to_owned());
        parts.push(heard.join(" "));
    }
    parts.join("　")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_info_line_reads_like_the_design() {
        assert_eq!(
            info_line(
                Some("MPEG-2"),
                Some((1440, 1080, false)),
                Some("AAC-LC ADTS ステレオ"),
                Some(48_000)
            ),
            "MPEG-2　1440×1080i　♪　AAC-LC ADTS ステレオ 48kHz"
        );
    }

    #[test]
    fn what_is_not_known_is_left_out() {
        assert_eq!(info_line(None, None, None, None), "");
        assert_eq!(info_line(Some("H.264"), None, None, None), "H.264");
    }

    #[test]
    fn a_message_takes_the_place_of_the_title() {
        let osd = Osd {
            title: crate::ui::text::badges::Titled::plain("番組名"),
            said: Some(String::from("選局できない")),
            ..Osd::default()
        };
        let titled = match &osd.said {
            Some(said) => crate::ui::text::badges::Titled::plain(said),
            None => osd.title.clone(),
        };
        assert_eq!(titled.name, "選局できない");
    }

    #[test]
    fn the_osd_is_shown_then_gone() {
        let mut state = State::default();
        assert!(state.opacity().is_none());
        state.show(Auto::default());
        assert_eq!(state.opacity(), Some(1.0));
        state.shown_at =
            Some(Instant::now() - std::time::Duration::from_secs_f32(5.0 + osd::FADE + 1.0));
        assert!(state.opacity().is_none());
        assert!(!state.is_shown());
    }

    /// 表示時間は osd.event の秒。
    #[test]
    fn the_osd_stays_for_the_seconds_set() {
        let auto = Auto {
            seconds: 20,
            on_channel: true,
            on_event: true,
        };
        let mut state = State::default();
        state.show_on(auto, Why::Event);
        state.shown_at = Some(Instant::now() - std::time::Duration::from_secs(10));
        assert_eq!(state.opacity(), Some(1.0));
        state.shown_at = Some(Instant::now() - std::time::Duration::from_secs(21));
        assert!(state.opacity().is_none());
    }

    #[test]
    fn the_osd_is_shown_only_when_asked_for() {
        let channel_only = Auto {
            seconds: 5,
            on_channel: true,
            on_event: false,
        };
        let mut state = State::default();
        state.show_on(channel_only, Why::Event);
        assert!(!state.is_shown(), "番組変更時に表示しない");
        state.show_on(channel_only, Why::Channel);
        assert!(state.is_shown());

        let never = Auto {
            seconds: 0,
            ..channel_only
        };
        let mut state = State::default();
        state.show_on(never, Why::Channel);
        assert!(!state.is_shown(), "0 秒なら自動では出さない");
        state.show(never);
        assert!(state.is_shown(), "キーで頼めば出す");
    }
}

#[cfg(test)]
mod ground_tests {
    use super::dim;

    /// SDR ではリニアで混ぜたときと同じ暗さに直す。端は動かさない。
    #[test]
    fn sdr_grounds_are_made_as_dark_as_light_would_make_them() {
        assert_eq!(dim(0.72, true), 0.72);
        assert!(dim(0.0, false).abs() < 1e-6);
        assert!((dim(1.0, false) - 1.0).abs() < 1e-6);
        let sdr = dim(0.72, false);
        assert!((0.4..0.5).contains(&sdr), "{sdr}");
    }
}
