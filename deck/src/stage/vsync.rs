// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 提示の時刻を、垂直同期の等間隔の目盛りに揃える。
//! ウィンドウでは DWM が合成するので、サーフェスを取得できた時刻が垂直同期に揃わず揺れる（60 Hz の画面で
//! 13 ms と 27 ms が交互に来るなど）。この時刻で動かす絵（プラグインのレイヤー）は、揺れがそのまま位置の揺れになる。
//! 平均すれば垂直同期ごとに一回提示しているので、一回の提示を一目盛りと数え、周期は取得の時刻から測る
//! （OS が言うリフレッシュレートは整数に丸めてあることがある）。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 測った時刻との差を、一回にこれだけ寄せる。
const PULL: u32 = 16;

/// 周期を測る取得の数。
const MEASURED: usize = 64;

/// 予測よりこれだけ（周期に対して）遅く取得できたら、垂直同期を飛ばしたとみる。
const SKIPPED: f64 = 0.9;

#[derive(Debug, Default)]
pub(super) struct Vsync {
    last: Option<Instant>,
    measured: VecDeque<Instant>,
}

impl Vsync {
    /// `measured` はサーフェスを取得できた時刻、`refresh` は OS が言うリフレッシュレート（ミリヘルツ、
    /// 分からなければ 0）。前に返した時刻より前には戻らない。
    pub(super) fn at(&mut self, measured: Instant, refresh: u32) -> Instant {
        self.measured.push_back(measured);
        if self.measured.len() > MEASURED {
            self.measured.pop_front();
        }
        let at = match (self.last, self.period(refresh)) {
            (Some(last), Some(period)) => {
                let mut predicted = last + period;
                if measured > predicted {
                    let late = (measured - predicted).as_secs_f64() / period.as_secs_f64();
                    if late >= SKIPPED {
                        predicted += period.mul_f64(late.round().max(1.0));
                    }
                }
                if measured >= predicted {
                    predicted + (measured - predicted).min(period) / PULL
                } else if predicted - measured > period * 2 {
                    // 取得が目盛りより大きく遅れている（目盛りが走りすぎた）。測った時刻まで待つ。
                    measured.max(last)
                } else {
                    predicted - (predicted - measured) / PULL
                }
            }
            _ => measured,
        };
        let at = self.last.map_or(at, |last| at.max(last));
        self.last = Some(at);
        at
    }

    /// 取得の間隔の平均。垂直同期を飛ばした間隔は除く（OS が言う周期の 1.75 倍、分からなければ中央値の 1.5 倍を
    /// 超える物。ウィンドウでは周期の 0.8 倍と 1.6 倍が交互に来ることがあり、これは飛ばしではない）。
    fn period(&self, refresh: u32) -> Option<Duration> {
        let told = (refresh != 0).then(|| Duration::from_secs_f64(1000.0 / f64::from(refresh)));
        if self.measured.len() > MEASURED / 4 {
            let mut gaps: Vec<Duration> = self
                .measured
                .iter()
                .zip(self.measured.iter().skip(1))
                .map(|(a, b)| b.saturating_duration_since(*a))
                .collect();
            gaps.sort_unstable();
            let most = told.map_or(gaps[gaps.len() / 2] * 3 / 2, |told| told * 7 / 4);
            let kept: Vec<&Duration> = gaps.iter().filter(|gap| **gap <= most).collect();
            if !kept.is_empty() {
                return Some(kept.iter().copied().sum::<Duration>() / kept.len() as u32);
            }
        }
        told
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uneven_acquires_land_on_an_even_grid() {
        let mut vsync = Vsync::default();
        let start = Instant::now();
        // 60 Hz の画面で、ウィンドウのときに測った取得の間隔（13.3 ms・26.7 ms が混じる）。OS は 59 Hz と言う。
        let gaps = [13_336, 26_757, 13_282, 13_326];
        let mut measured = start;
        let mut times = Vec::new();
        for gap in gaps.iter().cycle().take(400) {
            measured += Duration::from_micros(*gap);
            times.push(vsync.at(measured, 59_000));
        }
        let period = Duration::from_micros(16_675);
        for pair in times[200..].windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                step.abs_diff(period) < Duration::from_micros(500),
                "{step:?}"
            );
        }
        // 垂直同期を三つ飛ばしても目盛りの上にいる。
        let last = *times.last().unwrap();
        let skipped = measured + period * 4;
        let at = vsync.at(skipped, 59_000);
        assert!(at > last + period * 3);
        assert!(at.saturating_duration_since(skipped) < period / 2);
        assert!(skipped.saturating_duration_since(at) < period / 2);
    }

    #[test]
    fn skipped_vsyncs_do_not_stretch_the_period() {
        let mut vsync = Vsync::default();
        let mut measured = Instant::now();
        let period = Duration::from_micros(6_061);
        let mut times = Vec::new();
        for n in 0..600u32 {
            measured += if n % 40 == 39 { period * 2 } else { period };
            times.push(vsync.at(measured, 165_000));
        }
        for pair in times[100..].windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                step.abs_diff(period) < Duration::from_micros(500)
                    || step.abs_diff(period * 2) < Duration::from_micros(500),
                "{step:?}"
            );
        }
    }

    #[test]
    fn it_never_goes_back() {
        let mut vsync = Vsync::default();
        let start = Instant::now();
        let first = vsync.at(start, 60_000);
        assert!(vsync.at(start - Duration::from_millis(5), 60_000) >= first);
    }
}
