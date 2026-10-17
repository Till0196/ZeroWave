// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 下端のステータスバー。値の無い欄は 0 と書かず `<プロファイル>` のような山括弧のまま出す。

use egui::{Color32, FontId, Sense, Stroke, Ui, Vec2};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::status;

#[derive(Debug, Clone, Default)]
pub struct Status {
    /// 録画では無い。
    pub tuner: Option<String>,
    pub profile: Option<String>,
    pub channel: Option<String>,
    pub picture: Option<(u32, u32, f32)>,
    pub volume: u32,
    /// バーは消さない。
    pub muted: bool,
    /// パススルー中で、音量も消音も効かない。
    pub fixed: bool,
    pub sound: Option<String>,
    pub recording: bool,
    pub recorded_for: u64,
    /// 緊急警報放送の対象のサービスの名前（TS は NIT、MMT は MPT の緊急情報記述子）。
    pub emergency: Option<String>,
    pub cnr: Option<f32>,
    /// 数えられないコンテナでは `None` で、欄ごと出さない。
    pub counts: zwcore::filters::Counts,
    pub bitrate: f64,
    /// 信号の欄を赤字にする（E201・E202 の間）。
    pub signal_alarm: bool,
    /// PMT（MMT は MPT）がまだなら分からない。
    pub data: Option<bool>,
    pub clock: String,
}

impl Status {
    /// 空欄だと欄が無いように見えるので、映像が来る前から出す。
    fn picture(&self) -> String {
        match self.picture {
            Some((width, height, zoom)) => {
                format!("{width} x {height} ({:.0} %)", zoom * 100.0)
            }
            None => "0 x 0 (0 %)".to_owned(),
        }
    }

    /// 値の無い信号の質は「--」。
    fn signal_text(&self) -> String {
        match self.cnr {
            Some(cnr) => format!("{cnr:.2} dB / {:.2} Mbps", self.bitrate),
            None => format!("-- dB / {:.2} Mbps", self.bitrate),
        }
    }
}

impl Status {
    fn counts_text(&self) -> String {
        let counts = self.counts;
        format!(
            "D {} / E {} / S {}",
            counts.dropped, counts.errors, counts.scrambled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clicked {
    Channel,
    Sound,
    Volume(u32),
    /// 音量のバーを右クリックした。
    Mute,
    Record,
    RecordMenu,
    Capture,
    CaptureMenu,
    ErrorsMenu,
}

pub fn text_size(pt: u32) -> f32 {
    if pt == 0 {
        status::TEXT_SIZE
    } else {
        status::TEXT_SIZE * pt as f32 / status::REFERENCE_PT
    }
}

pub fn font(size: f32) -> FontId {
    crate::ui::text::font::status(size)
}

/// `wanted`（0 で文字に合わせる）があればその高さに。文字が収まらない高さにはしない。
pub fn height(ctx: &egui::Context, size: f32, wanted: u32) -> f32 {
    let row = ctx.fonts_mut(|fonts| fonts.row_height(&font(size)));
    let fitted = (row + status::V_PAD * 2.0).ceil();
    if wanted == 0 {
        fitted
    } else {
        (wanted as f32).max(row.ceil())
    }
}

/// ステータスバーの項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Profile,
    Channel,
    Picture,
    Volume,
    Sound,
    Record,
    Capture,
    Signal,
    Errors,
    Clock,
}

impl Item {
    pub const ALL: [Self; 10] = [
        Self::Profile,
        Self::Channel,
        Self::Picture,
        Self::Volume,
        Self::Sound,
        Self::Record,
        Self::Capture,
        Self::Signal,
        Self::Errors,
        Self::Clock,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Channel => "channel",
            Self::Picture => "picture",
            Self::Volume => "volume",
            Self::Sound => "sound",
            Self::Record => "record",
            Self::Capture => "capture",
            Self::Signal => "signal",
            Self::Errors => "error",
            Self::Clock => "clock",
        }
    }

    pub fn of(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.key() == key)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Profile => "プロファイル",
            Self::Channel => "チャンネル",
            Self::Picture => "映像の大きさ",
            Self::Volume => "音量",
            Self::Sound => "音声",
            Self::Record => "録画",
            Self::Capture => "キャプチャ",
            Self::Signal => "信号",
            Self::Errors => "エラー（D / E / S）",
            Self::Clock => "時計",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arrange {
    pub items: Vec<(Item, f32)>,
    /// 1 なら入りきらない後ろの項目は出さない。
    pub rows: usize,
}

impl Default for Arrange {
    fn default() -> Self {
        Self {
            items: Item::ALL.iter().map(|item| (*item, 0.0)).collect(),
            rows: 1,
        }
    }
}

impl Arrange {
    pub fn of(ui: &crate::config::file::Ui) -> Self {
        let items = ui
            .status_list()
            .iter()
            .filter(|it| it.shown)
            .filter_map(|it| Some((Item::of(&it.id)?, it.width as f32)))
            .collect();
        let rows = if ui.status_multi_row {
            ui.status_max_rows.max(1) as usize
        } else {
            1
        };
        Self { items, rows }
    }
}

struct Metrics {
    font: FontId,
    clock_font: FontId,
    size: f32,
    /// 文字の高さ一つ分（音量のバーとカメラのアイコンの幅に）。
    em: f32,
    pad: f32,
    clock_pad: f32,
}

impl Metrics {
    fn new(ctx: &egui::Context, size: f32) -> Self {
        let font = font(size);
        let em = measure(ctx, "あ", &font).max(1.0);
        Self {
            font,
            clock_font: FontId::monospace(size),
            size,
            em,
            pad: status::PAD * size / status::TEXT_SIZE,
            clock_pad: status::CLOCK_PAD * size / status::TEXT_SIZE,
        }
    }
}

fn measure(ctx: &egui::Context, text: &str, font: &FontId) -> f32 {
    ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    })
}

fn natural(ctx: &egui::Context, item: Item, status: &Status, m: &Metrics) -> Option<f32> {
    let text = |sample: &str| measure(ctx, sample, &m.font);
    let inner = match item {
        Item::Profile => text(status::PROFILE_SAMPLE),
        Item::Channel => text(status::CHANNEL_SAMPLE),
        Item::Picture => text(status::PICTURE_SAMPLE),
        Item::Volume => status::VOLUME_EMS * m.em,
        Item::Sound => text(status::SOUND_SAMPLE),
        Item::Record => {
            status::RECORD_SAMPLES
                .iter()
                .map(|sample| text(sample))
                .fold(0.0, f32::max)
                + (status::RECORD_ICON_EMS + status::RECORD_GAP_EMS) * m.size
        }
        Item::Capture => status::CAPTURE_EMS * m.em,
        Item::Signal => text(status::QUALITY_SAMPLE),
        Item::Errors => text(status::COUNTS_SAMPLE),
        Item::Clock => {
            return Some(
                measure(ctx, status::CLOCK_SAMPLE, &m.clock_font).max(measure(
                    ctx,
                    &status.clock,
                    &m.clock_font,
                )) + m.clock_pad * 2.0,
            );
        }
    };
    Some(inner + m.pad * 2.0)
}

/// 設定ウィンドウで「自動」の幅を出すため。
pub fn natural_width(ctx: &egui::Context, item: Item, status: &Status, size: f32) -> f32 {
    let m = Metrics::new(ctx, size);
    natural(ctx, item, status, &m).unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    item: Item,
    row: usize,
    left: f32,
    right: f32,
    /// 右端に寄せた時計（並びの最後が時計のとき）。
    pinned: bool,
}

/// 並びの最後が時計なら一行目の右端に寄せる。入りきらなければ次の行へ（`rows` まで）、
/// それも無ければそこから後ろは出さない。
fn place(cells: &[(Item, f32)], left: f32, right: f32, rows: usize) -> Vec<Cell> {
    let mut out = Vec::with_capacity(cells.len());
    let (flow, first_limit) = match cells.split_last() {
        Some(((Item::Clock, width), rest)) => {
            out.push(Cell {
                item: Item::Clock,
                row: 0,
                left: right - width,
                right,
                pinned: true,
            });
            (rest, right - width)
        }
        _ => (cells, right),
    };
    let mut row = 0;
    let mut x = left;
    for (item, width) in flow {
        let limit = if row == 0 { first_limit } else { right };
        if x + width > limit {
            if x > left && row + 1 < rows.max(1) {
                row += 1;
                x = left;
                if x + width > right {
                    break;
                }
            } else {
                break;
            }
        }
        out.push(Cell {
            item: *item,
            row,
            left: x,
            right: x + width,
            pinned: false,
        });
        x += width;
    }
    out
}

fn rows_of(cells: &[Cell]) -> usize {
    cells.iter().map(|cell| cell.row + 1).max().unwrap_or(1)
}

fn lay_out(
    ctx: &egui::Context,
    status: &Status,
    m: &Metrics,
    arrange: &Arrange,
    left: f32,
    right: f32,
) -> Vec<Cell> {
    let widths: Vec<(Item, f32)> = arrange
        .items
        .iter()
        .filter_map(|(item, width)| {
            let natural = natural(ctx, *item, status, m)?;
            Some((*item, if *width > 0.0 { *width } else { natural }))
        })
        .collect();
    place(&widths, left, right, arrange.rows)
}

pub fn bar_height(
    ctx: &egui::Context,
    status: &Status,
    arrange: &Arrange,
    size: f32,
    wanted: u32,
    width: f32,
) -> f32 {
    let one = height(ctx, size, wanted);
    if arrange.rows <= 1 {
        return one;
    }
    let m = Metrics::new(ctx, size);
    one * rows_of(&lay_out(ctx, status, &m, arrange, 0.0, width)) as f32
}

/// 欄の幅は見本の文字列をそのフォントで測った幅か、決めた幅。
pub fn show(
    ui: &mut Ui,
    status: &Status,
    skin: &status::Skin,
    size: f32,
    arrange: &Arrange,
) -> Option<Clicked> {
    let bar = ui.max_rect();
    let ctx = ui.ctx().clone();
    let m = Metrics::new(&ctx, size);
    let cells = lay_out(&ctx, status, &m, arrange, bar.left(), bar.right());
    let rows = rows_of(&cells);
    let row_height = bar.height() / rows as f32;
    for row in 1..rows {
        ui.painter().hline(
            bar.x_range(),
            bar.top() + row_height * row as f32,
            Stroke::new(1.0, skin.divider),
        );
    }
    let mut clicked = None;
    for cell in cells {
        let top = bar.top() + row_height * cell.row as f32;
        let rect = egui::Rect::from_min_max(
            egui::pos2(cell.left, top),
            egui::pos2(cell.right, top + row_height),
        );
        if let Some(said) = draw(ui, cell.item, rect, status, skin, &m, cell.pinned, true) {
            clicked = Some(said);
        }
    }
    clicked
}

pub fn preview(
    ui: &mut Ui,
    item: Item,
    rect: egui::Rect,
    status: &Status,
    skin: &status::Skin,
    size: f32,
) {
    let m = Metrics::new(ui.ctx(), size);
    draw(ui, item, rect, status, skin, &m, false, false);
}

/// 押せる欄は `live` のときだけ受ける。右の区切りは、右端に寄せた時計の他は全部に引く。
#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut Ui,
    item: Item,
    rect: egui::Rect,
    status: &Status,
    skin: &status::Skin,
    m: &Metrics,
    pinned: bool,
    live: bool,
) -> Option<Clicked> {
    let font = &m.font;
    let inner = rect.shrink2(Vec2::new(m.pad, 0.0));
    let sense = match item {
        Item::Channel | Item::Sound | Item::Record | Item::Capture | Item::Errors => {
            Some(Sense::click())
        }
        Item::Volume => Some(Sense::click_and_drag()),
        _ => None,
    };
    let name = match item {
        Item::Channel => "局の欄",
        Item::Volume => "音量の欄",
        Item::Sound => "音声の欄",
        Item::Record => "録画の欄",
        Item::Capture => "キャプチャの欄",
        Item::Errors => "エラーの欄",
        _ => "",
    };
    let response = sense.filter(|_| live).map(|sense| {
        let response = ui.interact(rect, ui.scope_id().with(name), sense);
        if response.hovered() {
            ui.painter().rect_filled(rect, 0.0, skin.divider);
        }
        response
    });
    let painter = ui.painter().with_clip_rect(inner);
    let mut clicked = None;
    match item {
        Item::Profile => answered(
            &painter,
            inner,
            font,
            skin,
            status.profile.as_deref(),
            "<プロファイル>",
        ),
        Item::Channel => {
            answered(
                &painter,
                inner,
                font,
                skin,
                status.channel.as_deref(),
                "<チャンネル>",
            );
            if response.as_ref().is_some_and(|it| it.clicked()) {
                clicked = Some(Clicked::Channel);
            }
        }
        Item::Picture => text_in(&painter, inner, font, &status.picture(), skin.text),
        Item::Volume => {
            volume_bar(&painter, inner, status, skin);
            if let Some(response) = response {
                if response.secondary_clicked() {
                    clicked = Some(Clicked::Mute);
                } else if response.is_pointer_button_down_on()
                    && ui.input(|input| input.pointer.primary_down())
                    && let Some(at) = response.interact_pointer_pos()
                {
                    let wanted = volume_at(volume_track(inner), at.x);
                    // ドラッグしている間は毎フレーム来るので、変わったときだけ通知する。
                    if wanted != status.volume || status.muted {
                        clicked = Some(Clicked::Volume(wanted));
                    }
                }
                let said = if status.fixed {
                    String::from("パススルー中は音量を変えられません")
                } else if status.muted {
                    format!("消音（{}%）", status.volume)
                } else {
                    format!("音量 {}%", status.volume)
                };
                response.on_hover_text(said);
            }
        }
        Item::Sound => {
            answered(
                &painter,
                inner,
                font,
                skin,
                status.sound.as_deref(),
                "<音声>",
            );
            if response.as_ref().is_some_and(|it| it.clicked()) {
                clicked = Some(Clicked::Sound);
            }
        }
        Item::Record => {
            let side = status::RECORD_ICON_EMS * m.size;
            let mark = egui::Rect::from_center_size(
                egui::pos2(inner.left() + side / 2.0, inner.center().y),
                Vec2::splat(side),
            );
            let rest = egui::Rect::from_min_max(
                egui::pos2(mark.right() + status::RECORD_GAP_EMS * m.size, inner.top()),
                inner.max,
            );
            if status.recording {
                icons::paint(&painter, mark, Icon::Record, skin.dot_recording);
                let seconds = status.recorded_for;
                let text = format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                );
                text_in(&painter, rest, font, &text, skin.text);
            } else {
                icons::paint(&painter, mark, Icon::Stop, skin.text);
                text_in(&painter, rest, font, "<録画>", skin.text);
            }
            if let Some(response) = &response {
                if response.clicked() {
                    clicked = Some(Clicked::Record);
                } else if response.secondary_clicked() {
                    clicked = Some(Clicked::RecordMenu);
                }
            }
        }
        Item::Capture => {
            let side = inner
                .height()
                .min(inner.width())
                .min(status::CAPTURE_ICON_EMS * m.size);
            icons::paint(
                &painter,
                egui::Rect::from_center_size(inner.center(), Vec2::splat(side)),
                Icon::Camera,
                skin.text,
            );
            if let Some(response) = &response {
                if response.clicked() {
                    clicked = Some(Clicked::Capture);
                } else if response.secondary_clicked() {
                    clicked = Some(Clicked::CaptureMenu);
                }
            }
        }
        Item::Signal => {
            let colour = if status.signal_alarm {
                skin.alarm
            } else {
                skin.text
            };
            text_in(&painter, inner, font, &status.signal_text(), colour);
        }
        Item::Errors => {
            let colour = if status.counts.clean() {
                skin.text
            } else {
                skin.trouble
            };
            text_in(&painter, inner, font, &status.counts_text(), colour);
            if response.is_some_and(|response| response.secondary_clicked()) {
                clicked = Some(Clicked::ErrorsMenu);
            }
        }
        Item::Clock => {
            let painter = ui.painter().with_clip_rect(rect);
            let galley =
                painter.layout_no_wrap(status.clock.clone(), m.clock_font.clone(), skin.clock);
            let y = crate::ui::seat_em(&galley, rect.center().y - m.size / 2.0, m.size);
            let width = galley.size().x;
            painter.galley(
                egui::pos2(rect.right() - m.clock_pad - width, y),
                galley,
                skin.clock,
            );
        }
    }
    if !pinned {
        ui.painter()
            .vline(rect.right(), rect.y_range(), Stroke::new(1.0, skin.divider));
    }
    clicked
}

fn text_in(painter: &egui::Painter, rect: egui::Rect, font: &FontId, text: &str, colour: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = painter.layout_job(job);
    let em = font.size;
    let y = crate::ui::seat_em(&galley, rect.center().y - em / 2.0, em);
    painter.galley(egui::pos2(rect.left(), y), galley, colour);
}

fn answered(
    painter: &egui::Painter,
    rect: egui::Rect,
    font: &FontId,
    skin: &status::Skin,
    value: Option<&str>,
    stand_in: &str,
) {
    match value {
        Some(value) => text_in(painter, rect, font, value, skin.value),
        None => text_in(painter, rect, font, stand_in, skin.text),
    }
}

fn volume_track(inner: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        inner.center(),
        Vec2::new(inner.width(), status::VOLUME_HEIGHT),
    )
}

/// ドラッグしたまま欄の外へ出ても端で止まる。
fn volume_at(track: egui::Rect, x: f32) -> u32 {
    let at = ((x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
    (at * zwcore::sound::volume::MAX as f32).round() as u32
}

fn volume_bar(painter: &egui::Painter, inner: egui::Rect, status: &Status, skin: &status::Skin) {
    let track = volume_track(inner);
    painter.rect_filled(track, 0.0, skin.volume_track);
    let share =
        status.volume.min(zwcore::sound::volume::MAX) as f32 / zwcore::sound::volume::MAX as f32;
    let filled =
        egui::Rect::from_min_size(track.min, Vec2::new(track.width() * share, track.height()));
    let colour = if status.muted || status.fixed {
        skin.dim
    } else {
        skin.volume_filled
    };
    painter.rect_filled(filled, 0.0, colour);
    painter.rect_stroke(
        track,
        0.0,
        Stroke::new(1.0, skin.volume_edge),
        egui::StrokeKind::Inside,
    );
}

pub fn empty_scene(ui: &mut Ui, rect: egui::Rect) {
    use crate::ui::theme::scene;

    // egui はグラデーションを持たないので細い矩形を重ねる（168度で縦に近いので縦に切る）。
    let painter = ui.painter();
    let steps = 48;
    for step in 0..steps {
        let at = step as f32 / steps as f32;
        let band = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + rect.height() * at),
            Vec2::new(rect.width(), rect.height() / steps as f32 + 1.0),
        );
        painter.rect_filled(band, 0.0, mix(scene::TOP, scene::BOTTOM, at));
    }
    let job = egui::text::LayoutJob::single_section(
        "ZeroWave".to_owned(),
        egui::TextFormat {
            font_id: FontId::proportional(scene::MARK_SIZE),
            color: scene::MARK,
            extra_letter_spacing: scene::MARK_SPACING,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, scene::MARK);
}

fn mix(from: Color32, to: Color32, at: f32) -> Color32 {
    let blend = |from: u8, to: u8| (f32::from(from) + (f32::from(to) - f32::from(from)) * at) as u8;
    Color32::from_rgb(
        blend(from.r(), to.r()),
        blend(from.g(), to.g()),
        blend(from.b(), to.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counts_read_d_e_s() {
        let status = Status {
            counts: zwcore::filters::Counts {
                packets: 1000,
                dropped: 2,
                errors: 3,
                scrambled: 4,
            },
            ..Status::default()
        };
        assert_eq!(status.counts_text(), "D 2 / E 3 / S 4");
    }

    #[test]
    fn the_volume_bar_reads_its_position_and_stops_at_its_ends() {
        let track = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), Vec2::new(100.0, 8.0));
        assert_eq!(volume_at(track, 10.0), 0);
        assert_eq!(volume_at(track, 60.0), 50);
        assert_eq!(volume_at(track, 110.0), 100);
        assert_eq!(volume_at(track, -40.0), 0);
        assert_eq!(volume_at(track, 400.0), 100);
    }

    #[test]
    fn a_signal_nobody_measured_is_not_written_as_zero() {
        let mut status = Status::default();
        assert!(status.signal_text().starts_with("-- dB"));
        status.cnr = Some(28.5);
        assert!(status.signal_text().starts_with("28.50 dB"));
    }

    /// 文字の大きさを変えるとバーの高さも変わる。狭いウィンドウでは時計と重ならない。
    #[test]
    fn a_narrow_bar_drops_its_last_cells() {
        let ctx = egui::Context::default();
        crate::ui::text::font::install(&ctx);
        let status = Status {
            clock: String::from("12:34"),
            ..Status::default()
        };
        let skin = crate::ui::theme::status::DARK;
        let texts = |width: f32, size: f32| {
            let bar = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(width, 30.0));
            let input = || egui::RawInput {
                screen_rect: Some(bar),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input(), |_| {});
            for _ in 0..2 {
                output.textures_delta.clear();
                output = ctx.run_ui(input(), |ui| {
                    show(ui, &status, &skin, size, &Arrange::default());
                });
            }
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter(|clipped| matches!(clipped.shape, egui::Shape::Text(_)))
                .map(|clipped| {
                    clipped
                        .shape
                        .visual_bounding_rect()
                        .intersect(clipped.clip_rect)
                })
                .filter(|rect| rect.is_positive())
                .collect::<Vec<_>>()
        };
        let wide = texts(2000.0, text_size(0));
        let narrow = texts(420.0, text_size(0));
        assert!(narrow.len() < wide.len(), "{} {}", narrow.len(), wide.len());
        let clock = narrow
            .iter()
            .copied()
            .max_by(|a, b| a.right().total_cmp(&b.right()))
            .unwrap();
        assert!(
            narrow
                .iter()
                .filter(|rect| **rect != clock)
                .all(|rect| rect.right() <= clock.left())
        );
        assert!(height(&ctx, text_size(18), 0) > height(&ctx, text_size(0), 0));
        assert_eq!(height(&ctx, text_size(0), 40), 40.0);
        assert!(
            height(&ctx, text_size(0), 4) >= height(&ctx, text_size(0), 0) - status::V_PAD * 2.0
        );
    }

    fn widths(list: &[(Item, f32)]) -> Vec<(Item, f32)> {
        list.to_vec()
    }

    /// 既定の並び：左から詰め、時計は右端に寄せ、入りきらない後ろは出さない。
    #[test]
    fn the_default_order_packs_left_and_pins_the_clock_right() {
        let cells = widths(&[
            (Item::Profile, 50.0),
            (Item::Channel, 60.0),
            (Item::Volume, 40.0),
            (Item::Clock, 30.0),
        ]);
        let placed = place(&cells, 0.0, 400.0, 1);
        let clock = placed.iter().find(|it| it.item == Item::Clock).unwrap();
        assert!(clock.pinned);
        assert_eq!((clock.left, clock.right, clock.row), (370.0, 400.0, 0));
        let flow: Vec<(Item, f32, f32)> = placed
            .iter()
            .filter(|it| !it.pinned)
            .map(|it| (it.item, it.left, it.right))
            .collect();
        assert_eq!(
            flow,
            [
                (Item::Profile, 0.0, 50.0),
                (Item::Channel, 50.0, 110.0),
                (Item::Volume, 110.0, 150.0)
            ]
        );
        let narrow = place(&cells, 0.0, 130.0, 1);
        let shown: Vec<Item> = narrow.iter().map(|it| it.item).collect();
        assert_eq!(shown, [Item::Clock, Item::Profile]);
        assert_eq!(rows_of(&narrow), 1);
    }

    /// 入りきらない項目は次の行の左から。最大の行数で止める。
    #[test]
    fn overflowing_items_go_to_the_next_row_up_to_the_most_rows() {
        let cells = widths(&[
            (Item::Profile, 50.0),
            (Item::Channel, 60.0),
            (Item::Volume, 40.0),
            (Item::Sound, 70.0),
            (Item::Clock, 30.0),
        ]);
        let placed = place(&cells, 0.0, 130.0, 2);
        let at = |item: Item| placed.iter().find(|it| it.item == item).copied();
        assert_eq!(at(Item::Profile).unwrap().row, 0);
        let channel = at(Item::Channel).unwrap();
        assert_eq!((channel.row, channel.left), (1, 0.0));
        assert_eq!(at(Item::Volume).unwrap().row, 1);
        assert!(at(Item::Sound).is_none());
        assert_eq!(rows_of(&placed), 2);
        let three = place(&cells, 0.0, 130.0, 3);
        assert_eq!(
            three.iter().find(|it| it.item == Item::Sound).unwrap().row,
            2
        );
        assert_eq!(rows_of(&place(&cells, 0.0, 1000.0, 3)), 1);
    }

    #[test]
    fn a_clock_in_the_middle_is_an_ordinary_cell() {
        let cells = widths(&[(Item::Clock, 30.0), (Item::Profile, 50.0)]);
        let placed = place(&cells, 10.0, 400.0, 1);
        assert_eq!(placed[0].item, Item::Clock);
        assert!(!placed[0].pinned);
        assert_eq!((placed[0].left, placed[1].left), (10.0, 40.0));
    }

    /// 設定に書いていなければ、全部の項目を既定の並び、自動の幅、一行。
    #[test]
    fn the_default_arrangement_is_the_old_bar() {
        assert_eq!(
            Arrange::of(&crate::config::file::Ui::default()),
            Arrange::default()
        );
        let keys: Vec<&str> = Item::ALL.iter().map(|it| it.key()).collect();
        assert_eq!(keys, crate::config::file::STATUS_ITEMS);
        let mut ui = crate::config::file::Ui::default();
        let mut list = ui.status_list();
        list[0].shown = false;
        list[8].width = 150;
        list.swap(8, 9);
        ui.set_status_list(list);
        ui.status_multi_row = true;
        ui.status_max_rows = 3;
        let arrange = Arrange::of(&ui);
        assert_eq!(arrange.rows, 3);
        assert_eq!(arrange.items.len(), 9);
        assert_eq!(arrange.items[0].0, Item::Channel);
        assert_eq!(arrange.items[7], (Item::Clock, 0.0));
        assert_eq!(arrange.items[8], (Item::Errors, 150.0));
    }

    #[test]
    fn a_set_width_is_kept() {
        let ctx = egui::Context::default();
        crate::ui::text::font::install(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let m = Metrics::new(&ctx, text_size(0));
        let status = Status::default();
        let arrange = Arrange {
            items: vec![(Item::Profile, 123.0), (Item::Errors, 150.0)],
            rows: 1,
        };
        let cells = lay_out(&ctx, &status, &m, &arrange, 0.0, 2000.0);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].right, 123.0);
        assert_eq!(cells[1].right - cells[1].left, 150.0);
    }

    /// 二行にできて二行要る幅なら、バーの高さは一行の二倍。
    #[test]
    fn a_two_row_bar_is_twice_as_tall() {
        let ctx = egui::Context::default();
        crate::ui::text::font::install(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let size = text_size(0);
        let status = Status {
            clock: String::from("12:34"),
            ..Status::default()
        };
        let one = height(&ctx, size, 0);
        assert_eq!(
            bar_height(&ctx, &status, &Arrange::default(), size, 0, 420.0),
            one
        );
        let two = Arrange {
            rows: 2,
            ..Arrange::default()
        };
        assert_eq!(bar_height(&ctx, &status, &two, size, 0, 4000.0), one);
        assert_eq!(bar_height(&ctx, &status, &two, size, 0, 700.0), one * 2.0);
        assert_eq!(bar_height(&ctx, &status, &two, size, 30, 700.0), 60.0);
    }
}
