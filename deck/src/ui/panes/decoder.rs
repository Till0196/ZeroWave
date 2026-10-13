// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ストリームの情報の「デコーダ」の欄（`zwcore::player::stats` を集めたもの）。
//! 行はウィンドウにも「コピー」にも使う。

use egui::{Rect, Ui, pos2, vec2};
use mediadecoder::audio::Speaker;
use zwcore::player::{SoundStats, VideoStats};

use super::stream::{Pressed, feet};
use crate::ui::theme::stream as skin;

mod view;

pub const WIDTH: f32 = crate::ui::theme::decoder::WIDTH;
pub const HEIGHT: f32 = crate::ui::theme::decoder::HEIGHT;

/// 映像と音声の枠を並べ（狭ければ縦に並べる）、下にレンダリングの欄。コピーする文は [`written`] の行。
pub fn show(ui: &mut Ui, decoding: &Decoding, skin: &skin::Skin) -> Option<Pressed> {
    let look = view::look_for(skin);
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, look.ground);
    let foot = Rect::from_min_size(
        pos2(full.left(), full.bottom() - skin::BAR_HEIGHT),
        vec2(full.width(), skin::BAR_HEIGHT),
    );
    let mut pressed = feet(ui, foot, skin);
    if ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::C)) {
        pressed = Some(Pressed::Copy);
    }
    let body = Rect::from_min_max(full.min, pos2(full.right(), foot.top()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            view::cards(ui, decoding, &look);
        });
    pressed
}

pub fn written(decoding: &Decoding) -> String {
    let mut out = String::new();
    for (title, lines) in sections(decoding) {
        out.push_str(title);
        out.push('\n');
        for line in lines {
            out.push_str("  ");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Decoding {
    pub video: VideoStats,
    pub sound: SoundStats,
    pub painter: Option<String>,
    pub scaling: Option<zwcore::output::video::Scaling>,
    pub sound_codec: Option<String>,
    pub dual_mono: bool,
}

pub fn sections(decoding: &Decoding) -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("映像デコーダ", video(&decoding.video)),
        ("レンダリング", render(decoding)),
        ("音声デコーダ", sound(decoding)),
    ]
}

fn render(decoding: &Decoding) -> Vec<String> {
    let mut lines = vec![format!(
        "GPU: {}",
        decoding.painter.as_deref().unwrap_or("—")
    )];
    if let Some(scaling) = decoding.scaling {
        lines.push(format!("拡大: {}", scaling.upscale.label()));
        lines.push(format!("縮小: {}", scaling.downscale.label()));
    }
    lines
}

fn video(video: &VideoStats) -> Vec<String> {
    let mut lines = Vec::new();
    let coded = video.decoder.coded;
    lines.push(format!(
        "コーデック: {}",
        coded.map_or_else(
            || String::from("—"),
            |(codec, format)| { format!("{} {}", codec_name(codec), format_text(&format)) }
        )
    ));
    let (device, mode) = match &video.decoder.decoder {
        Some((name, true)) => (name.as_str(), "ハードウェア"),
        Some((name, false)) => (name.as_str(), "ソフトウェア"),
        None => ("—", "—"),
    };
    lines.push(format!("モード: {mode}"));
    lines.push(format!("デコーダデバイス: {device}"));
    lines.push(format!(
        "出力サイズ: {}",
        video.output.map_or_else(
            || String::from("—"),
            |(width, height)| format!("{width} x {height}")
        )
    ));
    lines.push(format!("I フレーム: {}", video.frames[0]));
    lines.push(format!("P フレーム: {}", video.frames[1]));
    lines.push(format!("B フレーム: {}", video.frames[2]));
    lines.push(format!("スキップ（デコードせずに破棄）: {}", video.skipped));
    lines.push(format!("リピートフィールド: {}", video.repeats));
    if matches!(coded, Some((mediadecoder::es::Codec::Mpeg2, _))) {
        let [frame, field] = video.structures;
        let judged = match zwcore::player::structure_of(frame, field) {
            Some(zwcore::player::CodedStructure::Frame) => "フレームピクチャー",
            Some(zwcore::player::CodedStructure::Field) => "フィールドピクチャー",
            None => "判定中",
        };
        lines.push(format!(
            "ピクチャー構造: {judged}（フレームピクチャー {frame}　フィールドピクチャー {field}）"
        ));
    }
    lines.push(format!(
        "フレーム数: デコード {}　表示 {}　破棄 {}",
        video.decoded, video.drawn, video.late
    ));
    lines.push(format!("デコードエラー: {}", video.troubles));
    lines.push(format!("再生レート: {:.2}", video.pace));
    lines.push(format!(
        "基本 FPS: {}",
        coded.map_or_else(|| String::from("—"), |(_, format)| fps(format.rate))
    ));
    match video.settings {
        Some(settings) => {
            let progressive = coded.is_some_and(|(_, format)| format.progressive);
            lines.push(format!(
                "デインターレース: {}",
                if progressive {
                    String::from("なし（プログレッシブ）")
                } else {
                    settings.deinterlace.name().to_owned()
                }
            ));
            lines.push(format!(
                "逆テレシネ: {}",
                if settings.telecine {
                    "有効"
                } else {
                    "無効"
                }
            ));
            if settings.telecine && !progressive {
                // film と見ればフィールドマッチで組み合わせる。周期を掴めたときだけ 5 枚に 1 枚を抜いて 24 枚に揃える。
                let spent = &video.spent;
                if spent.soft {
                    lines.push(String::from("検出: プログレッシブ"));
                    lines.push(String::from("24p 化: ストリームのタイムスタンプどおり"));
                } else if spent.film {
                    lines.push(String::from("検出: 30p"));
                    lines.push(format!(
                        "24p 化: {}",
                        if spent.locked && spent.whole_frames {
                            "重複検出 (5 フレームに 1 フレームを間引き)"
                        } else if spent.locked {
                            "3:2 プルダウン検出 (5 フレームに 1 フレームを間引き)"
                        } else {
                            "フレーム周期が検出されません (30fps)"
                        }
                    ));
                } else {
                    lines.push(String::from("検出: 60i"));
                }
                lines.push(format!(
                    "フィールドマッチ: {} フレーム　間引き {} フレーム　コーミング {} フレーム　30fps/60i の切り替え {} 回",
                    spent.matched, spent.woven, spent.combed, spent.film_changes
                ));
            }
        }
        None => lines.push(String::from("デインターレース: —")),
    }
    lines
}

fn sound(decoding: &Decoding) -> Vec<String> {
    let sound = &decoding.sound;
    let mut lines = vec![format!(
        "コーデック: {}",
        decoding.sound_codec.as_deref().unwrap_or("—")
    )];
    lines.push(if sound.passthrough {
        String::from("デコーダ: —（パススルー）")
    } else {
        match &sound.decoder {
            Some(using) => match &using.why {
                Some(why) => format!("デコーダ: {}（{why}）", using.name),
                None => format!("デコーダ: {}", using.name),
            },
            None => String::from("デコーダ: —"),
        }
    });
    match &sound.shape {
        Some(shape) => {
            lines.push(format!("サンプリング周波数: {} Hz", shape.rate));
            lines.push(format!(
                "チャンネル: {}（{}）→ 出力 {} ch（デバイス {} ch）",
                layout(&shape.speakers),
                shape
                    .speakers
                    .iter()
                    .map(|speaker| speaker_name(*speaker))
                    .collect::<Vec<_>>()
                    .join(" "),
                shape.out,
                shape.device
            ));
            lines.push(format!("ダウンミックス: {}", downmix(sound, shape)));
            lines.push(format!(
                "ストリームのダウンミックス係数: {}",
                sent(&shape.mixdown)
            ));
        }
        None => lines.push(String::from("再生前")),
    }
    if decoding.dual_mono {
        lines.push(format!(
            "デュアルモノ: {}",
            match sound.dual_mono {
                zwcore::engine::DualMono::Main => "主音声",
                zwcore::engine::DualMono::Sub => "副音声",
                zwcore::engine::DualMono::Both => "主+副",
            }
        ));
    }
    lines.push(format!(
        "パススルー: {}",
        if sound.passthrough {
            "する（ビットストリームのまま出力）"
        } else {
            "しない（デコードして出力）"
        }
    ));
    lines.push(format!(
        "出力サンプル数: {}　欠落 {} サンプル",
        sound.heard, sound.missed
    ));
    lines
}

fn downmix(sound: &SoundStats, shape: &zwcore::sound::sounding::Shape) -> String {
    let from = shape.speakers.len();
    if sound.passthrough {
        return String::from("なし（パススルー）");
    }
    if from == shape.out {
        return match (from, sound.mix.surround) {
            (6, Some(_)) => String::from("なし（チャンネル割り当てを指定）"),
            _ => String::from("なし"),
        };
    }
    let how = match (from, shape.out) {
        (1, _) => "モノラル → ステレオ",
        (6, 2) if sound.mix.downmix.is_some() => "5.1ch → 2ch（手動割り当て）",
        (6, 2) => "5.1ch → 2ch",
        (24, 2) => "22.2ch → 5.1ch → 2ch",
        (24, 6) => "22.2ch → 5.1ch",
        (24, 8) => "22.2ch → 7.1ch",
        _ => "",
    };
    if how.is_empty() {
        format!("{from} ch → {} ch", shape.out)
    } else {
        how.to_owned()
    }
}

/// 放送が送ったダウンミックスの係数。送っていなければ規格の既定。
fn sent(mixdown: &mediadecoder::audio::Mixdown) -> String {
    let mut parts = Vec::new();
    if let Some(centre) = mixdown.centre {
        parts.push(format!("センター {}", db(centre)));
    }
    if let Some(surround) = mixdown.surround {
        parts.push(format!("サラウンド {}", db(surround)));
    }
    if let Some(lfe) = mixdown.lfe {
        parts.push(format!("LFE {}", db(lfe)));
    }
    if mixdown.pseudo_surround {
        parts.push(String::from("疑似サラウンド"));
    }
    if let Some(g) = mixdown.twenty_two {
        parts.push(format!(
            "22.2 g1〜g6 {}",
            g.iter().map(|gain| db(*gain)).collect::<Vec<_>>().join(" ")
        ));
    }
    if parts.is_empty() {
        String::from("なし（規格の既定値）")
    } else {
        parts.join("、")
    }
}

fn db(gain: f32) -> String {
    if gain <= 0.0 {
        String::from("−∞ dB")
    } else {
        format!("{:.1} dB", 20.0 * gain.log10())
    }
}

fn layout(speakers: &[Speaker]) -> String {
    let lfe = speakers
        .iter()
        .filter(|speaker| matches!(speaker, Speaker::LowFrequency | Speaker::LowFrequencyRight))
        .count();
    let rest = speakers.len() - lfe;
    match (rest, lfe) {
        (1, 0) => String::from("モノラル"),
        (2, 0) => String::from("ステレオ"),
        (rest, 0) => format!("{rest} ch"),
        (rest, lfe) => format!("{rest}.{lfe} ch"),
    }
}

/// スピーカー配置の略称（ARIB と ITU-R BS.2051 の略号）。
fn speaker_name(speaker: Speaker) -> &'static str {
    use Speaker::*;
    match speaker {
        FrontCentre => "FC",
        FrontLeft => "FL",
        FrontRight => "FR",
        FrontLeftCentre => "FLc",
        FrontRightCentre => "FRc",
        SideLeft => "SiL",
        SideRight => "SiR",
        BackCentre => "BC",
        BackLeft => "BL",
        BackRight => "BR",
        TopCentre => "TpC",
        TopFrontCentre => "TpFC",
        TopFrontLeft => "TpFL",
        TopFrontRight => "TpFR",
        TopSideLeft => "TpSiL",
        TopSideRight => "TpSiR",
        TopBackCentre => "TpBC",
        TopBackLeft => "TpBL",
        TopBackRight => "TpBR",
        BottomFrontCentre => "BtFC",
        BottomFrontLeft => "BtFL",
        BottomFrontRight => "BtFR",
        LowFrequency => "LFE1",
        LowFrequencyRight => "LFE2",
        Unplaced => "?",
    }
}

fn codec_name(codec: mediadecoder::es::Codec) -> &'static str {
    match codec {
        mediadecoder::es::Codec::Mpeg2 => "MPEG-2",
        mediadecoder::es::Codec::H264 => "H.264",
        mediadecoder::es::Codec::Hevc => "HEVC",
    }
}

fn format_text(format: &mediadecoder::es::Format) -> String {
    let chroma = chroma_name(format.chroma);
    format!(
        "{} x {} {} {} bit {chroma} {}:{}",
        format.width,
        format.height,
        if format.progressive {
            "プログレッシブ"
        } else {
            "インターレース"
        },
        format.depth,
        format.aspect.0,
        format.aspect.1
    )
}

fn chroma_name(chroma: u8) -> &'static str {
    match chroma {
        1 => "4:2:0",
        2 => "4:2:2",
        3 => "4:4:4",
        _ => "?",
    }
}

fn fps((frames, per): (u32, u32)) -> String {
    if per == 0 {
        String::from("—")
    } else {
        format!("{:.2}", f64::from(frames) / f64::from(per))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_is_named_by_its_low_frequency_channels() {
        use Speaker::*;
        assert_eq!(layout(&[FrontLeft, FrontRight]), "ステレオ");
        assert_eq!(
            layout(&[
                FrontCentre,
                FrontLeft,
                FrontRight,
                BackLeft,
                BackRight,
                LowFrequency
            ]),
            "5.1 ch"
        );
        let mut twenty_two = vec![FrontCentre; 22];
        twenty_two.extend([LowFrequency, LowFrequencyRight]);
        assert_eq!(layout(&twenty_two), "22.2 ch");
    }

    #[test]
    fn the_coefficients_the_broadcast_did_not_send_are_the_standard() {
        let none = mediadecoder::audio::Mixdown::default();
        assert_eq!(sent(&none), "なし（規格の既定値）");
        let sent_centre = mediadecoder::audio::Mixdown {
            centre: Some(std::f32::consts::FRAC_1_SQRT_2),
            ..none
        };
        assert_eq!(sent(&sent_centre), "センター -3.0 dB");
        assert_eq!(fps((30000, 1001)), "29.97");
    }
}
