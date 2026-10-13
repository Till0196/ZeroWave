// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像フィルターの現在の状態を deck に伝える。

use super::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// 別のスレッドでビルドしている（その間は本体のデインタレースでレンダリングする）。
    pub building: bool,
    /// 適用中のフィルター（デインタレースのスロットの物が先）。
    pub stages: Vec<StageReport>,
    pub paused: Vec<Paused>,
    /// ビルドできなかったフィルター（名前、理由）。
    pub failed: Vec<(String, String)>,
    pub budget: f64,
    pub rate: f64,
    pub device: String,
    pub host: Vec<String>,
    pub latency_ms: f64,
    /// 本体の補間が遅らせて出す長さ（ミリ秒、[`Hooks::held_ms`]）。
    pub held_ms: f64,
    pub lag: Option<super::host::Lag>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StageReport {
    pub name: String,
    pub deinterlace: bool,
    pub passes: usize,
    pub input: [u32; 2],
    pub output: [u32; 2],
    pub ms: Option<f64>,
    pub max: Option<f64>,
    /// パスごとの直近の平均（GPU が時刻を測れるときだけ）。
    pub pass_times: Vec<(String, f64)>,
    /// 上限を越えているが、`keep` で外さずに掛けている。
    pub over: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paused {
    pub name: String,
    pub deinterlace: bool,
    pub ms: f64,
    pub budget: f64,
    pub pass_times: Vec<(String, f64)>,
}

impl Hooks {
    pub fn report(&self) -> Report {
        let outside = self.host.outside_ms();
        let stage = |stage: &Stage, deinterlace: bool| {
            let extra = if stage.has_host() { outside } else { 0.0 };
            let measured = stage
                .timing
                .as_ref()
                .and_then(timing::Timing::summary)
                .map(|(mean, max, each)| (mean + extra, max + extra, each));
            StageReport {
                name: stage.name.clone(),
                deinterlace,
                passes: stage.passes.len(),
                input: stage.input_size,
                output: stage.graph.output,
                ms: measured.as_ref().map(|(mean, _, _)| *mean).or(stage.ms),
                max: measured.as_ref().map(|(_, max, _)| *max).or(stage.ms),
                pass_times: measured
                    .map(|(_, _, each)| {
                        stage
                            .passes
                            .iter()
                            .filter(|pass| pass.slot.is_some())
                            .map(|pass| pass.name.clone())
                            .zip(each)
                            .collect()
                    })
                    .unwrap_or_default(),
                over: stage.over,
            }
        };
        Report {
            building: self.building.is_some(),
            stages: self
                .deinterlace
                .iter()
                .map(|s| stage(s, true))
                .chain(self.picture.iter().map(|s| stage(s, false)))
                .collect(),
            paused: self.paused.clone(),
            failed: self.failed.clone(),
            budget: self.budget(),
            rate: self
                .key
                .as_ref()
                .filter(|key| key.rate[1] > 0)
                .map_or(0.0, |key| f64::from(key.rate[0]) / f64::from(key.rate[1])),
            device: self.device_name.clone(),
            host: self.host_jobs.clone(),
            latency_ms: self.latency_ms(),
            held_ms: self.held_ms(),
            lag: self.host.lag(),
        }
    }
}
