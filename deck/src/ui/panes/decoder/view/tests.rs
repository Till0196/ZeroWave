// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::ui::panes::decoder::{Decoding, show, written};
use egui::Rect;

fn sample() -> Decoding {
    let mut video = zwcore::player::VideoStats::default();
    video.decoder.coded = Some((
        mediadecoder::es::Codec::Mpeg2,
        mediadecoder::es::Format {
            width: 1440,
            height: 1080,
            depth: 8,
            chroma: 1,
            rate: (30000, 1001),
            progressive: false,
            aspect: (16, 9),
            ..Default::default()
        },
    ));
    video.decoder.decoder = Some((String::from("VideoToolbox"), true));
    video.frames = [120, 480, 1200];
    video.structures = [125, 0];
    video.decoded = 1800;
    video.drawn = 1798;
    video.output = Some((1920, 1080));
    video.settings = Some(zwcore::output::video::Settings {
        deinterlace: zwcore::output::video::Deinterlace::Bwdif,
        ..Default::default()
    });
    video.spent.film = true;
    video.spent.locked = true;
    video.spent.matched = 1500;
    video.spent.woven = 300;
    video.spent.combed = 2;
    video.spent.film_changes = 1;
    video.pace = 1.0;
    let mut sound = zwcore::player::SoundStats::default();
    sound.decoder = Some(zwcore::decode::sound::Using {
        codec: zwcore::decode::sound::SoundCodec::Aac,
        name: String::from("ZeroWave AAC Audio Decoder"),
        why: None,
    });
    sound.shape = Some(zwcore::sound::sounding::Shape {
        rate: 48000,
        speakers: vec![Speaker::FrontLeft, Speaker::FrontRight],
        mixdown: mediadecoder::audio::Mixdown::default(),
        out: 2,
        device: 2,
    });
    sound.heard = 2_880_000;
    Decoding {
        video,
        sound,
        painter: Some(String::from("Apple M2 (Metal, AGX)")),
        scaling: Some(zwcore::output::video::Scaling {
            auto_up: false,
            auto_down: false,
            ..zwcore::output::video::Scaling::default()
        }),
        sound_codec: Some(String::from("AAC-LC ADTS")),
        dual_mono: true,
    }
}

fn sparse() -> Decoding {
    use Speaker::*;
    let mut decoding = Decoding::default();
    let mut speakers = vec![
        FrontLeft,
        FrontRight,
        FrontCentre,
        LowFrequency,
        BackLeft,
        BackRight,
        FrontLeftCentre,
        FrontRightCentre,
        BackCentre,
        LowFrequencyRight,
        SideLeft,
        SideRight,
    ];
    speakers.extend([
        TopFrontLeft,
        TopFrontRight,
        TopFrontCentre,
        TopCentre,
        TopBackLeft,
        TopBackRight,
        TopSideLeft,
        TopSideRight,
        TopBackCentre,
        BottomFrontCentre,
        BottomFrontLeft,
        BottomFrontRight,
    ]);
    decoding.sound.shape = Some(zwcore::sound::sounding::Shape {
        rate: 48000,
        speakers,
        mixdown: mediadecoder::audio::Mixdown::default(),
        out: 24,
        device: 24,
    });
    decoding.sound.passthrough = true;
    decoding.sound.dropped = 3;
    decoding.sound_codec = Some(String::from("AAC-LC LOAS/LATM"));
    decoding
}

/// コピーの文は見た目を変える前と同じ行の並び。
#[test]
fn the_copied_text_is_the_rows_as_before() {
    let expected = "\
映像デコーダ
  コーデック: MPEG-2 1440 x 1080 インターレース 8 bit 4:2:0 16:9
  モード: ハードウェア
  デコーダデバイス: VideoToolbox
  出力サイズ: 1920 x 1080
  I フレーム: 120
  P フレーム: 480
  B フレーム: 1200
  スキップ（デコードせずに破棄）: 0
  リピートフィールド: 0
  ピクチャー構造: フレームピクチャー（フレームピクチャー 125　フィールドピクチャー 0）
  フレーム数: デコード 1800　表示 1798　破棄 0
  デコードエラー: 0
  再生レート: 1.00
  基本 FPS: 29.97
  デインターレース: bwdif
  逆テレシネ: 有効
  検出: 30p
  24p 化: 3:2 プルダウン検出 (5 フレームに 1 フレームを間引き)
  フィールドマッチ: 1500 フレーム　間引き 300 フレーム　コーミング 2 フレーム　30fps/60i の切り替え 1 回
レンダリング
  GPU: Apple M2 (Metal, AGX)
  拡大: Lanczos3
  縮小: Mitchell
音声デコーダ
  コーデック: AAC-LC ADTS
  デコーダ: ZeroWave AAC Audio Decoder
  サンプリング周波数: 48000 Hz
  チャンネル: ステレオ（FL FR）→ 出力 2 ch（デバイス 2 ch）
  ダウンミックス: なし
  ストリームのダウンミックス係数: なし（規格の既定値）
  デュアルモノ: 主音声
  パススルー: しない（デコードして出力）
  出力サンプル数: 2880000　欠落 0 サンプル
";
    assert_eq!(written(&sample()), expected);
}

/// ウィジェットが右クリックでコピーする行は、どれも全部のコピーの中にある行。
#[test]
fn every_piece_copies_a_row_of_the_whole() {
    for decoding in [sample(), sparse()] {
        let video = crate::ui::panes::decoder::video(&decoding.video);
        let sound = crate::ui::panes::decoder::sound(&decoding);
        for key in [
            "コーデック",
            "モード",
            "デコーダデバイス",
            "出力サイズ",
            "基本 FPS",
            "再生レート",
            "フレーム数",
            "デコードエラー",
            "I フレーム",
            "P フレーム",
            "B フレーム",
            "スキップ（デコードせずに破棄）",
            "リピートフィールド",
            "デインターレース",
        ] {
            assert!(!line(&video, key).is_empty(), "映像の「{key}」");
        }
        for key in [
            "コーデック",
            "デコーダ",
            "サンプリング周波数",
            "チャンネル",
            "ダウンミックス",
            "ストリームのダウンミックス係数",
            "パススルー",
            "出力サンプル数",
        ] {
            assert!(!line(&sound, key).is_empty(), "音声の「{key}」");
        }
    }
    let video = crate::ui::panes::decoder::video(&sample().video);
    assert_eq!(value(&video, "デインターレース"), "bwdif");
    assert!(line(&video, "フィールドマッチ").starts_with("フィールドマッチ: 1500"));
    assert_eq!(line(&video, "デコーダ"), "");
    let sound = crate::ui::panes::decoder::sound(&sparse());
    assert_eq!(line(&sound, "パススルーの破棄・重複フレーム"), "");
}

#[test]
fn the_gpu_splits_into_its_name_and_tags() {
    assert_eq!(
        split_gpu(Some("Apple M2 (Metal, AGX)")),
        ("Apple M2", vec!["Metal", "AGX"])
    );
    assert_eq!(
        split_gpu(Some("Intel(R) UHD Graphics 630 (Vulkan, )")),
        ("Intel(R) UHD Graphics 630", vec!["Vulkan"])
    );
    assert_eq!(split_gpu(Some("GLES 2.0")), ("GLES 2.0", Vec::new()));
    assert_eq!(split_gpu(None), ("—", Vec::new()));
}

#[test]
fn the_speakers_light_where_they_sound() {
    use Speaker::*;
    let lit = |speakers: &[Speaker]| {
        let (_, _, cells) = placed(speakers);
        cells
            .into_iter()
            .filter(|(.., on)| *on)
            .map(|(name, ..)| name)
            .collect::<Vec<_>>()
    };
    assert_eq!(lit(&[FrontLeft, FrontRight]), ["FL", "FR"]);
    assert_eq!(lit(&[FrontCentre]), ["FC"]);
    assert_eq!(
        lit(&[
            FrontCentre,
            FrontLeft,
            FrontRight,
            BackLeft,
            BackRight,
            LowFrequency
        ]),
        ["FL", "FC", "FR", "LFE", "BL", "BR"]
    );
    let shape = sparse().sound.shape.expect("形");
    let (columns, rows, cells) = placed(&shape.speakers);
    assert_eq!((columns, rows), (5, 7));
    assert_eq!(cells.iter().filter(|(.., on)| *on).count(), 24);
    let mut places: Vec<_> = cells.iter().map(|(_, c, r, _)| (*c, *r)).collect();
    places.sort_unstable();
    places.dedup();
    assert_eq!(places.len(), 24);
}

#[test]
fn a_count_reads_red_only_while_it_rises() {
    let ctx = egui::Context::default();
    let mut seen = Vec::new();
    for (time, count) in [(0.0, 5), (1.0, 5), (2.0, 6), (4.0, 6), (6.0, 6), (7.0, 0)] {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| seen.push(rising(ui, "試し", count)));
        output.textures_delta.clear();
    }
    // 増えてから 3 秒は赤、選局で減れば数え直し。
    assert_eq!(seen, [false, false, true, true, false, false]);
}

fn drawn(width: f32, decoding: &Decoding, theme: &skin::Skin) -> (Vec<Rect>, [Rect; 3], Rect) {
    let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 2400.0));
    let ctx = egui::Context::default();
    crate::ui::text::font::install(&ctx);
    let input = || egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    let mut placed = [Rect::NOTHING; 3];
    let mut output = ctx.run_ui(input(), |_| {});
    for _ in 0..2 {
        output.textures_delta.clear();
        output = ctx.run_ui(input(), |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
            placed = cards(&mut child, decoding, &look_for(theme));
        });
    }
    output.textures_delta.clear();
    let mut whole = ctx.run_ui(input(), |ui| {
        let _ = show(ui, decoding, theme);
    });
    whole.textures_delta.clear();
    let texts = output
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
        .collect();
    (texts, placed, screen)
}

/// 文字はウィンドウからはみ出さず互いに重ならない。広ければ映像と音声を左右に、狭ければ縦に。
#[test]
fn nothing_overlaps_at_any_width() {
    for theme in [skin::DARK, skin::LIGHT] {
        for decoding in [sample(), sparse(), Decoding::default()] {
            for width in [320.0, 420.0, 560.0, 680.0, 900.0, 1280.0] {
                let (texts, [video, sound, render], screen) = drawn(width, &decoding, &theme);
                assert!(texts.len() >= 10, "{width}: {}", texts.len());
                for rect in &texts {
                    assert!(
                        rect.left() >= screen.left() - 0.5 && rect.right() <= screen.right() + 0.5,
                        "{width}: {rect:?}"
                    );
                }
                for (nth, one) in texts.iter().enumerate() {
                    for other in &texts[nth + 1..] {
                        let overlap = one.intersect(*other);
                        assert!(
                            !overlap.is_positive() || overlap.area() < 1.0,
                            "{width}: {one:?} と {other:?} が重なる"
                        );
                    }
                }
                for card in [video, sound, render] {
                    assert!(card.is_positive());
                    assert!(card.right() <= screen.right() + 0.5, "{width}: {card:?}");
                }
                assert!(!video.intersects(sound), "{width}");
                assert!(
                    render.top() >= video.bottom().max(sound.bottom()),
                    "{width}"
                );
                if width >= 900.0 {
                    assert!(video.right() <= sound.left(), "{width}: 左右に並ばない");
                } else if width <= 560.0 {
                    assert!(video.bottom() <= sound.top(), "{width}: 縦に並ばない");
                }
            }
        }
    }
}

#[test]
fn the_skin_follows_the_ground_it_is_given() {
    assert_eq!(look_for(&skin::DARK).ground, look::DARK.ground);
    assert_eq!(look_for(&skin::LIGHT).ground, look::LIGHT.ground);
}

/// ピクチャー構造は判定と枚数を別の行に。コピーする元の行は一行のまま。
#[test]
fn the_picture_structure_reads_in_two_rows() {
    let video = crate::ui::panes::decoder::video(&sample().video);
    let said = value(&video, "ピクチャー構造");
    assert_eq!(
        structure_rows(said),
        "フレームピクチャー\nフレームピクチャー 125　フィールドピクチャー 0"
    );
    assert!(!line(&video, "ピクチャー構造").contains('\n'));
    assert_eq!(structure_rows("判定中"), "判定中");
}

#[test]
fn the_sound_tiles_are_short() {
    assert!(MISSED.chars().count() <= 2, "{MISSED}");
}
