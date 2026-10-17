// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! PID タブのテーブルと、PID の内容の名前、全部をコピーするときの文。

use super::*;

pub(super) fn pids(ui: &mut Ui, info: &StreamInfo, chosen: &mut Option<u16>, skin: &skin::Skin) {
    head(ui, skin);
    // 見えている行だけビルドする（行の高さは `table::ROW`）。
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, table::ROW, info.pids.len(), |ui, rows| {
            for line in &info.pids[rows] {
                if line_of(ui, line, *chosen == Some(line.pid), skin) {
                    *chosen = Some(line.pid);
                }
            }
        });
}

fn head(ui: &mut Ui, skin: &skin::Skin) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, table::HEAD_HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, skin.bar);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, skin.edge));
    let font = FontId::proportional(table::HEAD_SIZE);
    let labels = ["PID", "入力", "出力", "ドロップ", "エラー"];
    let mut at = rect.left();
    for (index, label) in labels.iter().enumerate() {
        let column = table::COLUMNS[index];
        let (align, x) = if index == 0 {
            (egui::Align2::LEFT_CENTER, at + table::PAD)
        } else {
            (egui::Align2::RIGHT_CENTER, at + column - table::PAD)
        };
        ui.painter().text(
            pos2(x, rect.center().y),
            align,
            label,
            font.clone(),
            skin.dim,
        );
        at += column;
    }
    ui.painter().text(
        pos2(at + table::PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "内容",
        font,
        skin.dim,
    );
}

fn line_of(ui: &mut Ui, line: &Row, chosen: bool, skin: &skin::Skin) -> bool {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, table::ROW), Sense::hover());
    let response = ui.interact(rect, ui.scope_id().with(("PID", line.pid)), Sense::click());
    let painter = ui.painter();
    let colour = if chosen {
        painter.rect_filled(rect, 0.0, skin.chosen);
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(skin::MARK, rect.height())),
            0.0,
            skin.selected,
        );
        skin.chosen_text
    } else {
        if response.hovered() {
            painter.rect_filled(rect, 0.0, skin.hover);
        }
        skin.text
    };
    let font = FontId::monospace(table::TEXT_SIZE);
    let numbers = [
        zwcore::si::hex_id(line.pid, 4),
        line.counted.arrived.to_string(),
        line.counted.passed.to_string(),
        line.counted.lost.to_string(),
        line.counted.errors.to_string(),
    ];
    let mut at = rect.left();
    for (index, text) in numbers.iter().enumerate() {
        let column = table::COLUMNS[index];
        let (align, x) = if index == 0 {
            (egui::Align2::LEFT_CENTER, at + table::PAD)
        } else {
            (egui::Align2::RIGHT_CENTER, at + column - table::PAD)
        };
        let ink = if !chosen && index >= 3 && text != "0" {
            skin.trouble
        } else {
            colour
        };
        painter.text(pos2(x, rect.center().y), align, text, font.clone(), ink);
        at += column;
    }
    painter.text(
        pos2(at + table::PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &line.what,
        font,
        colour,
    );
    response.clicked()
}

/// NIT のネットワーク名記述子で示されたものだけ。
pub fn network_name(info: &StreamInfo) -> Option<String> {
    let identity = info.identity?;
    info.networks
        .iter()
        .find(|network| network.network == identity.network && !network.name.is_empty())
        .map(|network| network.name.clone())
}

/// 情報タブに出ている文そのまま、空行を挟んで PID タブのテーブルをタブ区切りで。
pub fn written(info: &StreamInfo) -> String {
    let mut out = String::new();
    for (label, value) in super::information::head(info) {
        out.push_str(&format!("{label} {value}\n"));
    }
    write(&mut out, &super::information::tree(info), 0);
    out.push_str("\nPID\t入力\t出力\tドロップ\tエラー\t内容\n");
    for line in &info.pids {
        out.push_str(&format!(
            "0x{:04x}\t{}\t{}\t{}\t{}\t{}\n",
            line.pid,
            line.counted.arrived,
            line.counted.passed,
            line.counted.lost,
            line.counted.errors,
            line.what,
        ));
    }
    out
}

/// 番組で示されたもの（番組表を読まないと分からない）が先。
pub fn what_carries(container: Container, pid: u16, programs: &[(u16, u16, &str)]) -> String {
    if let Some((_, service, declared)) = programs.iter().find(|(known, _, _)| *known == pid) {
        return format!("[{service:04}] {}", what_declared(declared));
    }
    let fixed = match container {
        Container::Ts => fixed_name(pid),
        Container::Tlv => fixed_mmt_name(pid),
    };
    fixed.unwrap_or_default().to_owned()
}

fn fixed_name(pid: u16) -> Option<&'static str> {
    Some(match pid {
        0x0000 => "PAT",
        0x0001 => "CAT",
        0x0002 => "TSDT",
        0x0010 => "NIT",
        0x0011 => "SDT/BAT",
        0x0012 => "H-EIT",
        0x0013 => "RST",
        0x0014 => "TDT/TOT",
        0x0017 => "DCT",
        0x001e => "DIT",
        0x001f => "SIT",
        // ARIB STD-B10 第2部 表 4-1（伝送制御信号の PID）。
        0x0020 => "LIT",
        0x0021 => "ERT",
        0x0022 => "PCAT",
        0x0023 => "SDTT",
        0x0024 => "BIT",
        0x0025 => "NBIT/LDT",
        0x0026 => "M-EIT",
        0x0027 => "L-EIT",
        0x0028 => "SDTT",
        0x0029 => "CDT",
        0x1fff => "NULL",
        _ => return None,
    })
}

/// MMT で規格が場所を決めている packet_id。ARIB STD-B60 表 4-12。
fn fixed_mmt_name(packet_id: u16) -> Option<&'static str> {
    Some(match packet_id {
        0x0000 => "PA (PLT/MPT)",
        0x0001 => "CA",
        0x0002 => "AL-FEC",
        0x8000 => "MH-EIT",
        0x8001 => "MH-AIT",
        0x8002 => "MH-BIT",
        0x8003 => "MH-SDTT",
        0x8004 => "MH-SDT",
        0x8005 => "MH-TOT",
        0x8006 => "MH-CDT",
        0x8007 => "データ伝送",
        0x8008 => "MH-DIT",
        0x8009 => "MH-SIT",
        _ => return None,
    })
}

/// 番組表で示された種別（TS は `stream_type`、MMT はアセットの種別）を言葉に。
pub fn what_declared(declared: &str) -> &'static str {
    if let Some(stream_type) = declared
        .strip_prefix("0x")
        .and_then(|digits| u8::from_str_radix(digits, 16).ok())
    {
        return what_stream(stream_type);
    }
    match declared {
        "hev1" | "hvc1" => "HEVC",
        "avc1" | "avc3" => "H.264",
        "mp4a" => "AAC LATM",
        "stpp" => "字幕 (TTML)",
        "aapp" => "アプリ",
        _ => "—",
    }
}

pub fn what_stream(stream_type: u8) -> &'static str {
    match stream_type {
        0x01 | 0x02 => "MPEG-2 Video",
        0x03 | 0x04 => "MPEG Audio",
        0x0f => "AAC",
        0x11 => "AAC LATM",
        0x1b => "H.264",
        0x24 => "HEVC",
        0x06 => "字幕/データ",
        0x0d => "データカルーセル",
        _ => "—",
    }
}
