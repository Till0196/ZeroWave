// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードの統計（ストリーム情報の「デコーダ」の欄）。どれも選局ごとに 0 から数える。

use super::*;
use crate::sound::sounding::Shape;

#[derive(Debug, Clone, Default)]
pub struct VideoStats {
    pub decoder: Decoder,
    pub frames: [u64; 3],
    pub repeats: u64,
    pub structures: [u64; 2],
    pub troubles: u64,
    pub skipped: u64,
    pub decoded: u64,
    pub drawn: u64,
    pub late: u64,
    pub output: Option<(u32, u32)>,
    /// 実際に作られた映像出力の設定で、要求した物と違うことがある。
    pub settings: Option<crate::output::video::Settings>,
    pub spent: Spent,
    pub pace: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SoundStats {
    pub shape: Option<Shape>,
    pub heard: u64,
    pub missed: u64,
    pub dropped: u64,
    pub repeated: u64,
    pub passthrough: bool,
    pub mix: crate::sound::mix::Mix,
    pub dual_mono: crate::engine::DualMono,
    pub decoder: Option<crate::decode::sound::Using>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodedStructure {
    Frame,
    Field,
}

/// 判定までに数えるピクチャーの数（フィールドピクチャーは二枚で一つ）。
const STRUCTURE_SAMPLES: u64 = 30;

/// これだけのフィールドピクチャーが来たらフィールドピクチャーの放送とする（四対）。
/// エンコーダーはピクチャーごとに切り替え、フレームピクチャーだけが何分も続くことがあるので、
/// 多いほうではなく出たかどうかで見る。
const FIELD_EVIDENCE: u64 = 8;

pub fn structure_of(frame: u64, field: u64) -> Option<CodedStructure> {
    if frame + field / 2 < STRUCTURE_SAMPLES {
        return None;
    }
    Some(if field >= FIELD_EVIDENCE {
        CodedStructure::Field
    } else {
        CodedStructure::Frame
    })
}

impl Player {
    pub fn structure_pending(&self) -> bool {
        let mpeg2 =
            self.spent_decoding.about.lock().is_ok_and(|about| {
                matches!(about.coded, Some((mediadecoder::es::Codec::Mpeg2, _)))
            });
        mpeg2 && self.coded_structure().is_none()
    }

    pub fn coded_structure(&self) -> Option<CodedStructure> {
        let structures = &self.spent_decoding.structures;
        structure_of(
            structures[0].load(Ordering::Relaxed),
            structures[1].load(Ordering::Relaxed),
        )
    }

    pub(super) fn drain_spent(&mut self) {
        if let Some(video) = self.video.as_mut() {
            let spent = video.spent();
            self.spent_meter.add(&spent);
            self.spent_total.add(&spent);
        }
    }

    pub fn video_stats(&mut self) -> VideoStats {
        self.drain_spent();
        let decodes = &self.spent_decoding;
        VideoStats {
            decoder: decodes
                .about
                .lock()
                .map(|about| about.clone())
                .unwrap_or_default(),
            frames: [
                decodes.frames[0].load(Ordering::Relaxed),
                decodes.frames[1].load(Ordering::Relaxed),
                decodes.frames[2].load(Ordering::Relaxed),
            ],
            repeats: decodes.repeats.load(Ordering::Relaxed),
            structures: [
                decodes.structures[0].load(Ordering::Relaxed),
                decodes.structures[1].load(Ordering::Relaxed),
            ],
            troubles: decodes.troubles.load(Ordering::Relaxed),
            skipped: self.dropped_access.load(Ordering::Relaxed),
            decoded: self.video_decoded.load(Ordering::Relaxed),
            drawn: self.drawn - self.tuned_counts.0,
            late: self.late - self.tuned_counts.1,
            output: self.picture_size(),
            settings: self.video.as_ref().map(Screen::settings),
            spent: self.spent_total,
            pace: self.pace,
        }
    }

    pub fn sound_stats(&self) -> SoundStats {
        let shared = &self.shared;
        SoundStats {
            shape: shared.shape.lock().ok().and_then(|shape| shape.clone()),
            heard: shared.heard.load(Ordering::Relaxed),
            missed: shared.missed.load(Ordering::Relaxed),
            dropped: shared.dropped.load(Ordering::Relaxed),
            repeated: shared.repeated.load(Ordering::Relaxed),
            passthrough: self.passes_sound_through(),
            mix: self.mix(),
            dual_mono: self.dual_mono(),
            decoder: crate::decode::sound::using(),
        }
    }
}

#[cfg(test)]
mod structure_tests {
    use super::*;

    #[test]
    fn the_structure_is_decided_once_enough_are_counted() {
        assert_eq!(structure_of(10, 0), None, "少ない");
        assert_eq!(structure_of(30, 0), Some(CodedStructure::Frame));
        assert_eq!(
            structure_of(0, 60),
            Some(CodedStructure::Field),
            "フィールドは二枚で一つ"
        );
        assert_eq!(
            structure_of(300, 6),
            Some(CodedStructure::Frame),
            "三対では足りない"
        );
    }

    #[rustfmt::skip]
    const SAMPLED: [(u64, u64); 118] = [
        (0, 155), (0, 153), (0, 154), (0, 152), (0, 150), (0, 154), (0, 152), (0, 153),
        (0, 151), (0, 152), (0, 152), (0, 151), (0, 151), (0, 154), (0, 153), (0, 151),
        (0, 153), (0, 150), (0, 153), (0, 150), (0, 153), (0, 151), (0, 153), (0, 153),
        (0, 151), (0, 151), (0, 150), (0, 153), (0, 153), (0, 149), (0, 154), (0, 153),
        (0, 150), (0, 151), (0, 154), (0, 151), (0, 150), (0, 150), (0, 150), (0, 150),
        (0, 154), (0, 152), (0, 153), (0, 149), (0, 154), (0, 150), (0, 151), (0, 151),
        (0, 151), (0, 150), (0, 150), (0, 153), (0, 151), (0, 150), (0, 147), (0, 155),
        (0, 150), (0, 152), (0, 152), (0, 150), (0, 151), (22, 140), (106, 97), (108, 99),
        (72, 114), (94, 105), (82, 107), (8, 149), (0, 147), (0, 158), (0, 151), (0, 153),
        (0, 150), (0, 151), (0, 151), (0, 151), (0, 150), (0, 150), (0, 150), (0, 150),
        (0, 153), (0, 149), (0, 153), (0, 145), (0, 156), (0, 150), (0, 150), (0, 149),
        (0, 153), (0, 154), (0, 150), (0, 150), (0, 149), (0, 154), (0, 150), (12, 144),
        (74, 116), (38, 128), (30, 138), (34, 136), (42, 130), (32, 136), (0, 150),
        (0, 152), (0, 148), (0, 156), (0, 149), (0, 151), (112, 99), (46, 127), (0, 153),
        (0, 152), (0, 155), (22, 139), (0, 150), (0, 150), (92, 104), (2, 152),
    ];

    #[test]
    fn a_broadcast_that_mixes_field_pictures_into_frame_pictures_is_of_field_pictures() {
        let (mut frame, mut field) = (0, 0);
        let mut judged = Vec::new();
        for (fields, frames) in SAMPLED {
            field += fields;
            frame += frames;
            judged.push(structure_of(frame, field));
        }
        assert_eq!(judged[60], Some(CodedStructure::Frame));
        assert!(
            judged[61..]
                .iter()
                .all(|judged| *judged == Some(CodedStructure::Field))
        );
        assert!(field / 2 < frame / 10);
    }
}
