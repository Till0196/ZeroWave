// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! チャンネルスキャン。どの周波数を選局するかと、1 つの周波数を読んで何が含まれていたかまで。周波数は地上とケーブルが
//! ヘルツ、衛星がキロヘルツ（混ぜると決してロックしないチューナーに見える）。

use std::collections::BTreeMap;

use crate::filters::feed::{Feed, Sink};
use crate::filters::packet::Section;
use crate::guide::{Guide, Service};
use crate::si::Info;

pub use coax::plan::Band;

pub use coax::plan::satellite_frequency;
pub use coax::scan::{Spec, Target};

mod reading;
mod sample;
#[cfg(test)]
mod tests;
mod walk;

pub use reading::{Found, Reading, Told};
pub use sample::{
    Missed, Options, Sampled, Step, sample, serving_tuner, tuner_matching, wait_for_lock,
};
pub use walk::{Hit, walk};

/// 断る理由は設定ファイルの項目名（`[[line]]` のキー）で返す。
pub fn targets(spec: &Spec) -> Result<Vec<Target>, String> {
    coax::scan::targets(spec).map_err(|why| match why {
        coax::scan::Error::NotCarried { .. } => why.to_string(),
        coax::scan::Error::Plan(_) | coax::scan::Error::Band(_) => format!("[[line]] の {why}"),
    })
}
