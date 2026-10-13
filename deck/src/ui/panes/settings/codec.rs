// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「メディアコーデック」。コーデックごとのデコーダーの優先順位。レンダリングに使わない GPU も選べる。
//! 音声は OS のデコーダーを並べるが既定では使わない。ファイルには触ったコーデックだけ書く。

use egui::{FontId, Rect, Stroke, pos2, vec2};
use mediadecoder::es::Codec;
use zwcore::decode::sound::{SoundCodec, SoundPick};
use zwcore::decode::{HwDeinterlace, Offer, Pick, Way};

use super::{Form, skin};
use crate::config::File;
use crate::ui::icons::Icon;

pub(super) const CODECS: [(Codec, &str); 3] = [
    (Codec::Mpeg2, "MPEG-2"),
    (Codec::H264, "H.264"),
    (Codec::Hevc, "H.265"),
];

pub(super) const SOUNDS: [SoundCodec; 2] = [SoundCodec::Aac, SoundCodec::Mpeg];

#[derive(Debug, Clone, Default)]
pub(crate) struct Found {
    pub video: [Vec<Offer>; 3],
    pub sound: [Vec<String>; 2],
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    pick: Pick,
    on: bool,
    offer: Option<Offer>,
}

fn written(file: &File, codec: Codec) -> &Vec<String> {
    match codec {
        Codec::Mpeg2 => &file.video.decoder.mpeg2,
        Codec::H264 => &file.video.decoder.h264,
        Codec::Hevc => &file.video.decoder.hevc,
    }
}

fn written_mut(file: &mut File, codec: Codec) -> &mut Vec<String> {
    match codec {
        Codec::Mpeg2 => &mut file.video.decoder.mpeg2,
        Codec::H264 => &mut file.video.decoder.h264,
        Codec::Hevc => &mut file.video.decoder.hevc,
    }
}

/// GPU と OS のデコーダーは、名前が同じでも別の物。
fn same(pick: &Pick, offer: &Offer) -> bool {
    pick.device == offer.device && (pick.way == Way::System) == !offer.gpu
}

fn rows(written: &[String], offers: &[Offer]) -> Vec<Row> {
    if written.is_empty() {
        // レンダリングの GPU だけが入り、他は並ぶが使わない。
        return offers
            .iter()
            .map(|offer| Row {
                pick: Pick {
                    way: offer.ways[0],
                    device: offer.device.clone(),
                    deinterlace: None,
                },
                on: offer.gpu && offer.draws,
                offer: Some(offer.clone()),
            })
            .collect();
    }
    let mut rows: Vec<Row> = written
        .iter()
        .filter_map(|spelled| {
            let spelled = spelled.trim();
            let on = !spelled.starts_with('!');
            let pick = Pick::parse(spelled.trim_start_matches('!'))?;
            let offer = offers.iter().find(|offer| same(&pick, offer)).cloned();
            Some(Row { pick, on, offer })
        })
        .collect();
    for offer in offers {
        if !rows.iter().any(|row| same(&row.pick, offer)) {
            rows.push(Row {
                pick: Pick {
                    way: offer.ways[0],
                    device: offer.device.clone(),
                    deinterlace: None,
                },
                on: false,
                offer: Some(offer.clone()),
            });
        }
    }
    rows
}

fn spell(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let spelled = row.pick.spelling();
            if row.on {
                spelled
            } else {
                format!("!{spelled}")
            }
        })
        .collect()
}

fn shown(row: &Row) -> String {
    if !row.pick.device.is_empty() {
        return row.pick.device.clone();
    }
    if row.pick.way == Way::System {
        format!("{}（ソフトウェア）", Way::System.name())
    } else {
        String::from("GPU")
    }
}

pub(super) fn codecs(
    form: &mut Form<'_, '_>,
    edited: &mut File,
    found: Option<&Found>,
    tab: &mut usize,
    sound_tab: &mut usize,
) {
    form.note(
        "上から順に使用します。使用できない場合は次のデコーダーに切り替えます。\n\
         GPU デコーダー内蔵のデインターレースを使うと、ZeroWave のデインターレースと逆テレシネは使えません。",
    );
    form.section("映像デコーダー");
    let labels: Vec<&str> = CODECS.iter().map(|(_, label)| *label).collect();
    tabs(form, tab, &labels, "codec.tab");
    let (codec, _) = CODECS[(*tab).min(2)];
    let Some(found) = found else {
        form.note("この PC のデコーダーを調べています…");
        form.ui
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
        return;
    };
    let built = zwcore::decode::has_software(codec);
    let mut zerowave = !crate::config::apply::zerowave_off(written(edited, codec));
    let was = zerowave;
    let mut rows = rows(written(edited, codec), &found.video[(*tab).min(2)]);
    let before = rows.clone();
    table(form, codec, &mut rows, built.then_some(&mut zerowave));
    if rows != before || zerowave != was {
        let mut spelled = spell(&rows);
        if built && !zerowave {
            spelled.push(format!("!{}", crate::config::apply::ZEROWAVE));
        }
        *written_mut(edited, codec) = spelled;
    }

    *form.at += 12.0;
    form.section("音声デコーダー");
    let labels: Vec<&str> = SOUNDS.iter().map(|codec| codec.name()).collect();
    tabs(form, sound_tab, &labels, "codec.sound_tab");
    let sound = SOUNDS[(*sound_tab).min(1)];
    let mut rows = sound_rows(
        sound_written(edited, sound),
        &found.sound[(*sound_tab).min(1)],
    );
    let before = rows.clone();
    sound_table(form, sound, &mut rows);
    if rows != before {
        *sound_written_mut(edited, sound) = sound_spell(&rows);
    }
    *form.at += 6.0;
    let line = form.line(skin::ROW);
    let reset = Rect::from_min_size(
        pos2(line.right() - 96.0, line.top()),
        vec2(96.0, line.height()),
    );
    let any = CODECS
        .iter()
        .any(|(codec, _)| !written(edited, *codec).is_empty())
        || SOUNDS
            .iter()
            .any(|codec| !sound_written(edited, *codec).is_empty());
    if form.button_enabled(reset, "codec.reset", "既定に戻す", any) {
        for (codec, _) in CODECS {
            written_mut(edited, codec).clear();
        }
        for codec in SOUNDS {
            sound_written_mut(edited, codec).clear();
        }
    }
}

/// OS のデコーダーだけで、ZeroWave はテーブルの最後に外せずに置く。
#[derive(Debug, Clone, PartialEq)]
struct SoundRow {
    pick: SoundPick,
    on: bool,
    present: bool,
}

fn sound_written(file: &File, codec: SoundCodec) -> &Vec<String> {
    match codec {
        SoundCodec::Aac => &file.audio.decoder.aac,
        SoundCodec::Mpeg => &file.audio.decoder.mpeg,
    }
}

fn sound_written_mut(file: &mut File, codec: SoundCodec) -> &mut Vec<String> {
    match codec {
        SoundCodec::Aac => &mut file.audio.decoder.aac,
        SoundCodec::Mpeg => &mut file.audio.decoder.mpeg,
    }
}

fn sound_rows(written: &[String], offers: &[String]) -> Vec<SoundRow> {
    let named = |name: &String| SoundPick::System(Some(name.clone()));
    let mut rows: Vec<SoundRow> = written
        .iter()
        .filter_map(|spelled| {
            let spelled = spelled.trim();
            let on = !spelled.starts_with('!');
            let pick = SoundPick::parse(spelled.trim_start_matches('!'))?;
            if pick == SoundPick::ZeroWave {
                return None;
            }
            let present = match &pick {
                SoundPick::System(Some(name)) => offers.contains(name),
                _ => !offers.is_empty(),
            };
            Some(SoundRow { pick, on, present })
        })
        .collect();
    for offer in offers {
        if !rows.iter().any(|row| row.pick == named(offer)) {
            rows.push(SoundRow {
                pick: named(offer),
                on: false,
                present: true,
            });
        }
    }
    rows
}

fn sound_spell(rows: &[SoundRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let spelled = row.pick.spelling();
            if row.on {
                spelled
            } else {
                format!("!{spelled}")
            }
        })
        .collect()
}

fn sound_table(form: &mut Form<'_, '_>, codec: SoundCodec, rows: &mut [SoundRow]) {
    let id = codec as u8;
    let head = form.line(22.0);
    let at = columns(head);
    for (x, text) in [(at.name, "デコーダー"), (at.tag, "処理")] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    let mut moved: Option<(usize, usize)> = None;
    let count = rows.len();
    for (index, row) in rows.iter_mut().enumerate() {
        let line = form.line(34.0);
        form.ui.painter().hline(
            line.x_range(),
            line.top() - 3.0,
            Stroke::new(1.0, form.look.edge),
        );
        let at = columns(line);
        let middle = line.center().y;
        form.ui.painter().text(
            pos2(at.number, middle),
            egui::Align2::LEFT_CENTER,
            (index + 1).to_string(),
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
        let tick = Rect::from_center_size(pos2(at.tick + 7.0, middle), vec2(22.0, 22.0));
        if form.tick(tick, ("codec.sound_on", id, index), row.on) {
            row.on = !row.on;
        }
        let name = match &row.pick {
            SoundPick::System(Some(name)) => name.clone(),
            _ => String::from("OS のデコーダー"),
        };
        let name = if row.present {
            name
        } else {
            format!("{name}（見つかりません）")
        };
        form.ui.painter().text(
            pos2(at.name, middle),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(skin::TEXT_SIZE),
            if row.on && row.present {
                form.look.text
            } else {
                form.look.group
            },
        );
        tag(form, pos2(at.tag, middle), "CPU", false);
        if form.button_enabled(
            at.up,
            ("codec.sound_up", id, index),
            Icon::ArrowUp,
            index > 0,
        ) {
            moved = Some((index, index - 1));
        }
        if form.button_enabled(
            at.down,
            ("codec.sound_down", id, index),
            Icon::ArrowDown,
            index + 1 < count,
        ) {
            moved = Some((index, index + 1));
        }
    }
    if let Some((from, to)) = moved {
        rows.swap(from, to);
    }
    let line = form.line(34.0);
    form.ui.painter().hline(
        line.x_range(),
        line.top() - 3.0,
        Stroke::new(1.0, form.look.edge),
    );
    let at = columns(line);
    let middle = line.center().y;
    form.ui.painter().text(
        pos2(at.number, middle),
        egui::Align2::LEFT_CENTER,
        "—",
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        form.look.label,
    );
    let tick = Rect::from_center_size(pos2(at.tick + 7.0, middle), vec2(22.0, 22.0));
    let _ = form.tick(tick, ("codec.sound_zerowave", id), true);
    let colour = form.look.text;
    form.fitted(
        pos2(at.name, middle),
        zwcore::decode::sound::zerowave_name(codec),
        at.tag - 6.0 - at.name,
        FontId::proportional(skin::TEXT_SIZE),
        colour,
    );
    tag(form, pos2(at.tag, middle), "CPU", false);
}

fn tabs(form: &mut Form<'_, '_>, tab: &mut usize, labels: &[&str], id: &'static str) {
    let row = form.line(30.0);
    form.ui.painter().hline(
        row.x_range(),
        row.bottom(),
        Stroke::new(1.0, form.look.edge),
    );
    let mut x = row.left();
    for (index, label) in labels.iter().enumerate() {
        let galley = form.ui.painter().layout_no_wrap(
            (*label).to_owned(),
            FontId::proportional(skin::TEXT_SIZE),
            form.look.text,
        );
        let width = galley.size().x + 32.0;
        let rect = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
        let chosen = index == *tab;
        let hit = form
            .ui
            .interact(rect, form.ui.id().with((id, index)), egui::Sense::click());
        if hit.clicked() {
            *tab = index;
        }
        form.ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            FontId::proportional(skin::TEXT_SIZE),
            if chosen {
                form.look.chosen_text
            } else {
                form.look.item
            },
        );
        if chosen {
            form.ui.painter().rect_filled(
                Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max),
                0.0,
                form.look.chosen_mark,
            );
        }
        x += width + 4.0;
    }
}

struct Columns {
    number: f32,
    tick: f32,
    name: f32,
    way: Rect,
    deinterlace: Rect,
    tag: f32,
    up: Rect,
    down: Rect,
}

fn columns(row: Rect) -> Columns {
    let down = Rect::from_min_size(
        pos2(row.right() - 28.0, row.center().y - 12.0),
        vec2(26.0, 24.0),
    );
    let up = down.translate(vec2(-30.0, 0.0));
    let tag = up.left() - 48.0;
    let deinterlace = Rect::from_min_size(
        pos2(tag - 108.0, row.center().y - skin::SELECT_HEIGHT / 2.0),
        vec2(98.0, skin::SELECT_HEIGHT),
    );
    let way = Rect::from_min_size(
        pos2(deinterlace.left() - 114.0, deinterlace.top()),
        vec2(108.0, skin::SELECT_HEIGHT),
    );
    Columns {
        number: row.left() + 8.0,
        tick: row.left() + 30.0,
        name: row.left() + 56.0,
        way,
        deinterlace,
        tag,
        up,
        down,
    }
}

fn table(form: &mut Form<'_, '_>, codec: Codec, rows: &mut [Row], zerowave: Option<&mut bool>) {
    let head = form.line(22.0);
    let at = columns(head);
    for (x, text) in [
        (at.name, "デコーダー"),
        (at.way.left(), "方式"),
        (at.deinterlace.left(), "デインターレース"),
        (at.tag, "処理"),
    ] {
        form.ui.painter().text(
            pos2(x, head.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
    }
    let mut moved: Option<(usize, usize)> = None;
    let count = rows.len();
    for (index, row) in rows.iter_mut().enumerate() {
        let line = form.line(34.0);
        form.ui.painter().hline(
            line.x_range(),
            line.top() - 3.0,
            Stroke::new(1.0, form.look.edge),
        );
        let at = columns(line);
        let present = row.offer.is_some();
        let colour = if row.on && present {
            form.look.text
        } else {
            form.look.group
        };
        let middle = line.center().y;
        form.ui.painter().text(
            pos2(at.number, middle),
            egui::Align2::LEFT_CENTER,
            (index + 1).to_string(),
            FontId::proportional(skin::TEXT_SIZE - 1.0),
            form.look.label,
        );
        let tick = Rect::from_center_size(pos2(at.tick + 7.0, middle), vec2(22.0, 22.0));
        if form.tick(tick, ("codec.on", codec as u8, index), row.on) {
            row.on = !row.on;
        }
        let name = if present {
            shown(row)
        } else {
            format!("{}（見つかりません）", shown(row))
        };
        form.fitted(
            pos2(at.name, middle),
            &name,
            at.way.left() - 6.0 - at.name,
            FontId::proportional(skin::TEXT_SIZE),
            colour,
        );
        let ways = row
            .offer
            .as_ref()
            .map_or_else(|| vec![row.pick.way], |offer| offer.ways.clone());
        if ways.len() > 1 {
            let choices: Vec<(&str, &str)> = ways
                .iter()
                .map(|way| (way.spelling(), way.name()))
                .collect();
            let mut spelled = row.pick.way.spelling().to_owned();
            form.select_at(
                at.way,
                ("codec.way", codec as u8, index),
                &mut spelled,
                &choices,
            );
            if let Some(way) = Way::from_spelling(&spelled) {
                row.pick.way = way;
            }
            if row
                .pick
                .deinterlace
                .is_some_and(|mode| !row.pick.way.deinterlacers().contains(&mode))
            {
                row.pick.deinterlace = None;
            }
        } else {
            form.ui.painter().with_clip_rect(at.way).text(
                pos2(at.way.left() + 9.0, middle),
                egui::Align2::LEFT_CENTER,
                row.pick.way.name(),
                FontId::proportional(skin::TEXT_SIZE),
                form.look.label,
            );
        }
        // デインターレースの既定は「しない」（ドライバーの物では品質を担保できない）。
        let modes = row.pick.way.deinterlacers();
        if modes.is_empty() {
            form.ui.painter().text(
                pos2(at.deinterlace.left() + 9.0, middle),
                egui::Align2::LEFT_CENTER,
                "—",
                FontId::proportional(skin::TEXT_SIZE),
                form.look.label,
            );
        } else {
            let mut choices: Vec<(&str, &str)> = vec![("off", "しない")];
            choices.extend(modes.iter().map(|mode| (mode.spelling(), mode.name())));
            let mut spelled = row
                .pick
                .deinterlace
                .map_or("off", HwDeinterlace::spelling)
                .to_owned();
            form.select_at(
                at.deinterlace,
                ("codec.deinterlace", codec as u8, index),
                &mut spelled,
                &choices,
            );
            row.pick.deinterlace = HwDeinterlace::from_spelling(&spelled);
        }
        let gpu = row
            .offer
            .as_ref()
            .map_or(row.pick.way != Way::System, |offer| offer.gpu);
        tag(
            form,
            pos2(at.tag, middle),
            if gpu { "GPU" } else { "CPU" },
            gpu,
        );
        if form.button_enabled(
            at.up,
            ("codec.up", codec as u8, index),
            Icon::ArrowUp,
            index > 0,
        ) {
            moved = Some((index, index - 1));
        }
        if form.button_enabled(
            at.down,
            ("codec.down", codec as u8, index),
            Icon::ArrowDown,
            index + 1 < count,
        ) {
            moved = Some((index, index + 1));
        }
    }
    if let Some((from, to)) = moved {
        rows.swap(from, to);
    }
    let Some(zerowave) = zerowave else {
        return;
    };
    let line = form.line(34.0);
    form.ui.painter().hline(
        line.x_range(),
        line.top() - 3.0,
        Stroke::new(1.0, form.look.edge),
    );
    let at = columns(line);
    let middle = line.center().y;
    form.ui.painter().text(
        pos2(at.number, middle),
        egui::Align2::LEFT_CENTER,
        "—",
        FontId::proportional(skin::TEXT_SIZE - 1.0),
        form.look.label,
    );
    let tick = Rect::from_center_size(pos2(at.tick + 7.0, middle), vec2(22.0, 22.0));
    let fixed = count == 0;
    let on = fixed || *zerowave;
    if form.tick(tick, ("codec.zerowave", codec as u8), on) && !fixed {
        *zerowave = !*zerowave;
    }
    let colour = if on { form.look.text } else { form.look.label };
    form.fitted(
        pos2(at.name, middle),
        zwcore::decode::zerowave_name(codec),
        at.tag - 6.0 - at.name,
        FontId::proportional(skin::TEXT_SIZE),
        colour,
    );
    tag(form, pos2(at.tag, middle), "CPU", false);
}

fn tag(form: &mut Form<'_, '_>, at: egui::Pos2, text: &str, gpu: bool) {
    let colour = if gpu {
        form.look.chosen_text
    } else {
        form.look.label
    };
    let painter = form.ui.painter();
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(10.0), colour);
    let rect = Rect::from_min_size(pos2(at.x, at.y - 8.0), vec2(galley.size().x + 12.0, 16.0));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, colour.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(name: &str, draws: bool) -> Offer {
        Offer {
            device: name.to_owned(),
            ways: vec![Way::Auto, Way::D3d12, Way::D3d11],
            gpu: true,
            draws,
        }
    }

    fn mft(name: &str) -> Offer {
        Offer {
            device: name.to_owned(),
            ways: vec![Way::System],
            gpu: false,
            draws: false,
        }
    }

    #[test]
    fn nothing_written_is_the_drawing_gpu_alone() {
        let offers = [gpu("RTX", true), gpu("Radeon", false), mft("MFT")];
        let rows = rows(&[], &offers);
        let on: Vec<bool> = rows.iter().map(|row| row.on).collect();
        assert_eq!(on, [true, false, false]);
        assert_eq!(rows[0].pick.way, Way::Auto);
    }

    #[test]
    fn written_order_wins_and_new_devices_come_last() {
        let offers = [gpu("RTX", true), gpu("Radeon", false), mft("MFT")];
        let written = [String::from("d3d11:Radeon"), String::from("!auto:RTX")];
        let rows = rows(&written, &offers);
        assert_eq!(spell(&rows), ["d3d11:Radeon", "!auto:RTX", "!system:MFT"]);
    }

    #[test]
    fn a_deinterlacer_rides_on_its_row() {
        let offers = [Offer {
            device: String::from("RTX"),
            ways: vec![Way::Auto, Way::Nvdec],
            gpu: true,
            draws: true,
        }];
        let rows = rows(&[String::from("nvdec:RTX:adaptive")], &offers);
        assert_eq!(rows[0].pick.deinterlace, Some(HwDeinterlace::Adaptive));
        assert_eq!(spell(&rows), ["nvdec:RTX:adaptive"]);
    }

    #[test]
    fn sound_rows_start_off_and_keep_their_names() {
        let offers = [String::from("Microsoft AAC Audio Decoder MFT")];
        let mut rows = sound_rows(&[], &offers);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].on);
        rows[0].on = true;
        assert_eq!(
            sound_spell(&rows),
            ["system:Microsoft AAC Audio Decoder MFT"]
        );
        let rows = sound_rows(&[String::from("zerowave")], &offers);
        assert_eq!(
            sound_spell(&rows),
            ["!system:Microsoft AAC Audio Decoder MFT"]
        );
    }

    #[test]
    fn zerowave_left_out_is_not_a_row() {
        let written = [String::from("auto:RTX"), String::from("!zerowave")];
        let rows = rows(&written, &[gpu("RTX", true)]);
        assert_eq!(spell(&rows), ["auto:RTX"]);
        assert!(crate::config::apply::zerowave_off(&written));
        assert!(!crate::config::apply::zerowave_off(&written[..1]));
    }

    #[test]
    fn a_missing_device_stays() {
        let rows = rows(&[String::from("d3d12:外した GPU")], &[gpu("RTX", true)]);
        assert!(rows[0].offer.is_none());
        assert_eq!(spell(&rows)[0], "d3d12:外した GPU");
    }
}
