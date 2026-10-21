// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GPU の性能の計測。測っている間はトーストを出し、測った結果は `state.toml` の `[gpu]` に覚えて次の起動では測らない。

use zwcore::output::video::Deinterlace;
use zwcore::output::video::gpu::Upload;
use zwcore::screen::Measure;

use super::Stage;

const TITLE: &str = "GPU 性能の測定中";

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct Remembered {
    measured: Vec<Entry>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Entry {
    gpu: String,
    width: u32,
    height: u32,
    depth: u8,
    progressive: bool,
    deinterlace: String,
    upload: String,
}

impl Entry {
    fn of(measure: &Measure) -> Self {
        Self {
            gpu: measure.gpu.clone(),
            width: measure.width,
            height: measure.height,
            depth: measure.depth,
            progressive: measure.progressive,
            deinterlace: measure.deinterlace.name().to_owned(),
            upload: match measure.upload {
                Upload::Staged => "staged",
                Upload::Mapped => "mapped",
            }
            .to_owned(),
        }
    }

    fn measure(&self) -> Option<Measure> {
        let deinterlace = [
            Deinterlace::Weave,
            Deinterlace::Bob,
            Deinterlace::Yadif,
            Deinterlace::Bwdif,
        ]
        .into_iter()
        .find(|step| step.name() == self.deinterlace)?;
        let upload = match self.upload.as_str() {
            "staged" => Upload::Staged,
            "mapped" => Upload::Mapped,
            _ => return None,
        };
        Some(Measure {
            gpu: self.gpu.clone(),
            width: self.width,
            height: self.height,
            depth: self.depth,
            progressive: self.progressive,
            deinterlace,
            upload,
        })
    }
}

pub(super) fn recall() {
    let Some(remembered) = crate::config::load_state()
        .gpu
        .and_then(|table| table.try_into::<Remembered>().ok())
    else {
        return;
    };
    zwcore::screen::recall(
        remembered
            .measured
            .iter()
            .filter_map(Entry::measure)
            .collect(),
    );
}

impl Stage {
    /// 測り始めたらトーストを出し、測り終えたら引っ込めて結果を保存する。
    pub(super) fn watch_measuring(&mut self) {
        let now = zwcore::screen::measuring();
        if now == self.measuring {
            return;
        }
        self.measuring = now;
        if now {
            self.notice(zwsdk::ui::Notice {
                title: String::from(TITLE),
                progress: Some(0.0),
                ..zwsdk::ui::Notice::default()
            });
            return;
        }
        self.notice(zwsdk::ui::Notice {
            title: String::from(TITLE),
            seconds: Some(0.0),
            ..zwsdk::ui::Notice::default()
        });
        let remembered = Remembered {
            measured: zwcore::screen::measures().iter().map(Entry::of).collect(),
        };
        let table = toml::Table::try_from(&remembered).ok();
        crate::config::update_state(move |state| state.gpu = table);
    }
}
