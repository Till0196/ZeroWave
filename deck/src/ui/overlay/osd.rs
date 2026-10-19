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
    /// 部分ごとの濃さ。描くときに [`State::shown`] で入れる。
    pub shown: Shown,
}

/// チャンネルの出し方（`osd.channel`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    LogoAndName,
    Name,
    Number,
    /// 選局しても出さない。
    Off,
}

impl Look {
    pub fn of(said: &str) -> Self {
        match said {
            "name" => Self::Name,
            "number" => Self::Number,
            "off" => Self::Off,
            _ => Self::LogoAndName,
        }
    }
}

/// OSD の設定（`[osd]`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prefs {
    /// チャンネルとメッセージを出す秒（`osd.fade`）。
    pub fade: f32,
    pub look: Look,
    /// 番組情報を出す秒（`osd.event`）。0 なら自動では出さない。
    pub event: u32,
    /// 選局したときと番組が替わったときに番組情報を出すか。
    pub on_channel: bool,
    pub on_event: bool,
}

impl Prefs {
    pub fn of(file: &crate::config::file::Osd) -> Self {
        let on = |flag: &str| file.event_on.iter().any(|said| said == flag);
        Self {
            fade: file.fade.max(1) as f32,
            look: Look::of(&file.channel),
            event: file.event,
            on_channel: on("channel"),
            on_event: on("event"),
        }
    }

    /// 頼まれて番組情報を出すときの秒。自動で出さない設定でも出す。
    fn asked(self) -> f32 {
        if self.event == 0 {
            osd::SHOWN_FOR
        } else {
            self.event as f32
        }
    }
}

impl Default for Prefs {
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

/// 部分ごとの濃さ（0〜1）と、チャンネルの出し方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub channel: f32,
    pub event: f32,
    pub message: f32,
    pub look: Look,
}

impl Default for Shown {
    fn default() -> Self {
        Self {
            channel: 1.0,
            event: 1.0,
            message: 1.0,
            look: Look::LogoAndName,
        }
    }
}

impl Shown {
    pub fn most(self) -> f32 {
        self.channel.max(self.event).max(self.message)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Timer(Option<(Instant, f32)>);

impl Timer {
    fn start(&mut self, hold: f32) {
        self.0 = Some((Instant::now(), hold));
    }

    fn opacity(&mut self) -> f32 {
        let Some((at, hold)) = self.0 else {
            return 0.0;
        };
        let since = at.elapsed().as_secs_f32();
        if since >= hold + osd::FADE {
            self.0 = None;
            return 0.0;
        }
        if since <= hold {
            return 1.0;
        }
        1.0 - (since - hold) / osd::FADE
    }
}

/// チャンネル・番組情報・メッセージは、それぞれの秒で消える。
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    channel: Timer,
    event: Timer,
    message: Timer,
    look: Look,
}

impl State {
    /// キーで頼まれて出す。チャンネルを出さない設定でも出す。
    pub fn show(&mut self, prefs: Prefs) {
        self.look = match prefs.look {
            Look::Off => Look::LogoAndName,
            look => look,
        };
        self.channel.start(prefs.fade);
        self.event.start(prefs.asked());
    }

    /// 選局や番組の切り替わりで出す。設定で出さない物は出さない。
    pub fn show_on(&mut self, prefs: Prefs, why: Why) {
        if why == Why::Channel && prefs.look != Look::Off {
            self.look = prefs.look;
            self.channel.start(prefs.fade);
        }
        let wanted = match why {
            Why::Channel => prefs.on_channel,
            Why::Event => prefs.on_event,
        };
        if wanted && prefs.event > 0 {
            self.event.start(prefs.event as f32);
        }
    }

    /// メッセージ（選局できなかったときなど）を出す。
    pub fn tell(&mut self, prefs: Prefs) {
        self.message.start(prefs.fade);
    }

    /// 局の OSD が録画に切り替えた後まで残らないように。
    pub fn hide(&mut self) {
        *self = Self {
            look: self.look,
            ..Self::default()
        };
    }

    pub fn shown(&mut self) -> Option<Shown> {
        let shown = Shown {
            channel: self.channel.opacity(),
            event: self.event.opacity(),
            message: self.message.opacity(),
            look: self.look,
        };
        (shown.most() > 0.0).then_some(shown)
    }

    pub fn is_shown(&self) -> bool {
        self.channel.0.is_some() || self.event.0.is_some() || self.message.0.is_some()
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
    // 部分の濃さは、いちばん濃い部分を `opacity` として比で掛ける。
    let most = osd.shown.most();
    let part = |of: f32| {
        if most > 0.0 { opacity * of / most } else { 0.0 }
    };
    let channel_opacity = part(osd.shown.channel);
    let in_channel = |colour: Color32| colour.gamma_multiply(channel_opacity);
    let look = osd.shown.look;
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
    let (titled, colour, title_opacity) = match &osd.said {
        Some(said) => (
            crate::ui::text::badges::Titled::plain(said),
            osd::TROUBLE,
            part(osd.shown.message.max(osd.shown.event)),
        ),
        None => (osd.title.clone(), osd::TITLE, part(osd.shown.event)),
    };
    let title_width = (right - left - 60.0 * dp).max(0.0);
    // タイトルは高さ 38dp のバーの中で縦に中央。
    crate::ui::text::badges::draw_line(
        &painter,
        pos2(left, title_top + title_area / 2.0),
        title_width,
        &titled,
        crate::ui::text::font::osd(osd::TITLE_SIZE * dp),
        colour.gamma_multiply(title_opacity),
        crate::ui::text::badges::Look::Outlined,
        title_opacity,
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
            in_channel(Color32::WHITE),
            None,
            key,
            osd::KEY_SIZE * dp,
            in_channel(osd::KEY_TEXT),
        );
        at += (osd::KEY_BADGE + osd::KEY_GAP) * dp;
    }
    // 番号だけなら、番号の無い局だけ名前を出す。
    let named = look != Look::Number || osd.remote_key.is_none();
    if look == Look::LogoAndName && !osd.badge.is_empty() {
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
            in_channel(osd.badge_colour),
            in_channel(Color32::WHITE),
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
    let (band_text, channel_text) = if named {
        (osd.band.as_str(), osd.channel.as_str())
    } else {
        ("", "")
    };
    let band = painter.layout_no_wrap(
        band_text.to_owned(),
        crate::ui::text::font::osd(osd::BAND_SIZE * dp),
        in_channel(osd::BAND),
    );
    let channel = crate::ui::one_line(
        &painter,
        channel_text,
        crate::ui::text::font::osd(osd::CHANNEL_SIZE * dp),
        in_channel(osd::CHANNEL),
        room,
        0.0,
    );
    let channel_width = channel.size().x;
    let channel_height = channel.size().y;
    if band_text.is_empty() {
        painter.galley(
            pos2(at, middle - channel.size().y / 2.0),
            channel,
            in_channel(osd::CHANNEL),
        );
    } else {
        let top = middle - (band.size().y + channel.size().y) / 2.0;
        painter.galley(pos2(at, top), band.clone(), in_channel(osd::BAND));
        painter.galley(
            pos2(at, top + band.size().y),
            channel,
            in_channel(osd::CHANNEL),
        );
    }
    if !osd.airtime.is_empty() {
        let in_event = |colour: Color32| colour.gamma_multiply(part(osd.shown.event));
        let airtime = painter.layout_no_wrap(
            osd.airtime.clone(),
            crate::ui::text::font::osd(osd::AIRTIME_SIZE * dp),
            in_event(osd::AIRTIME),
        );
        let top = if band_text.is_empty() {
            middle - airtime.size().y / 2.0
        } else {
            middle - (band.size().y + channel_height) / 2.0
        };
        let x = airtime_left.max(at + band.size().x.max(channel_width) + osd::AIRTIME_GAP * dp);
        if x + airtime.size().x <= info_left - osd::LOGO_GAP * dp {
            painter.galley(pos2(x, top), airtime, in_event(osd::AIRTIME));
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
        assert!(state.shown().is_none());
        state.show(Prefs::default());
        assert_eq!(state.shown().map(Shown::most), Some(1.0));
        let long_ago = Instant::now() - std::time::Duration::from_secs(60);
        state.channel.0 = state.channel.0.map(|(_, hold)| (long_ago, hold));
        state.event.0 = state.event.0.map(|(_, hold)| (long_ago, hold));
        assert!(state.shown().is_none());
        assert!(!state.is_shown());
    }

    fn prefs() -> Prefs {
        Prefs {
            fade: 3.0,
            look: Look::LogoAndName,
            event: 20,
            on_channel: true,
            on_event: true,
        }
    }

    fn ago(state: &mut State, seconds: u64) {
        let at = Instant::now() - std::time::Duration::from_secs(seconds);
        for timer in [&mut state.channel, &mut state.event, &mut state.message] {
            timer.0 = timer.0.map(|(_, hold)| (at, hold));
        }
    }

    /// チャンネルとメッセージは osd.fade、番組情報は osd.event の秒だけ出る。
    #[test]
    fn each_part_stays_for_its_own_seconds() {
        let mut state = State::default();
        state.show_on(prefs(), Why::Channel);
        ago(&mut state, 10);
        let shown = state.shown().expect("番組情報は残る");
        assert_eq!(shown.channel, 0.0);
        assert_eq!(shown.event, 1.0);
        ago(&mut state, 21);
        assert!(state.shown().is_none());

        let mut state = State::default();
        state.tell(prefs());
        let shown = state.shown().expect("メッセージ");
        assert_eq!((shown.message, shown.channel, shown.event), (1.0, 0.0, 0.0));
        ago(&mut state, 4);
        assert!(state.shown().is_none());
    }

    #[test]
    fn event_information_is_shown_only_when_asked_for() {
        let channel_only = Prefs {
            on_event: false,
            ..prefs()
        };
        let mut state = State::default();
        state.show_on(channel_only, Why::Event);
        assert!(!state.is_shown(), "番組変更時に表示しない");
        state.show_on(channel_only, Why::Channel);
        assert_eq!(state.shown().map(|shown| shown.event), Some(1.0));

        let never = Prefs {
            event: 0,
            ..channel_only
        };
        let mut state = State::default();
        state.show_on(never, Why::Channel);
        assert_eq!(
            state.shown().map(|shown| shown.event),
            Some(0.0),
            "0 秒なら自動では出さない"
        );
        state.show(never);
        assert_eq!(
            state.shown().map(|shown| shown.event),
            Some(1.0),
            "キーで頼めば出す"
        );
    }

    /// osd.channel の綴りと、選局したときの出し方。
    #[test]
    fn the_channel_is_shown_as_set() {
        assert_eq!(Look::of("logo+name"), Look::LogoAndName);
        assert_eq!(Look::of("name"), Look::Name);
        assert_eq!(Look::of("number"), Look::Number);
        assert_eq!(Look::of("off"), Look::Off);

        let off = Prefs {
            look: Look::Off,
            on_channel: false,
            ..prefs()
        };
        let mut state = State::default();
        state.show_on(off, Why::Channel);
        assert!(!state.is_shown(), "表示しない");
        state.show(off);
        assert_eq!(
            state.shown().map(|shown| (shown.channel, shown.look)),
            Some((1.0, Look::LogoAndName)),
            "キーで頼めば出す"
        );

        let number = Prefs {
            look: Look::Number,
            ..prefs()
        };
        let mut state = State::default();
        state.show_on(number, Why::Channel);
        assert_eq!(state.shown().map(|shown| shown.look), Some(Look::Number));
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
